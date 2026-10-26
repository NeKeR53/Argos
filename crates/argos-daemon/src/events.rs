//! Moteur d'événements (D29–D31) : fait tourner les déclencheurs du nœud. Chaque seconde,
//! il relit les déclencheurs actifs, déclenche les périodiques échus et ouvre ou ferme les
//! surveillances de dossiers selon la base. Un déclenchement remet la consigne à l'agent
//! cible, au nom d'Argos ; un agent parké est réveillé.

use crate::supervisor::Supervisor;
use argos_core::protocol::Origin;
use argos_core::{Trigger, TriggerKind};
use chrono::{Local, TimeZone};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Un même fichier signalé plusieurs fois par le système ne déclenche qu'une fois.
const WATCH_DEBOUNCE: Duration = Duration::from_secs(3);

/// Déclenche : note l'instant, puis remet la consigne à la cible dans sa propre tâche
/// (un réveil peut prendre du temps et ne doit pas retarder les autres déclencheurs).
pub fn fire(supervisor: &Supervisor, trigger: &Trigger, payload: Option<String>) {
    let _ = supervisor.registry().mark_trigger_fired(&trigger.id, now());
    let mut text = format!(
        "[Déclencheur « {} » ({}, {})]\n{}",
        trigger.name, trigger.id, trigger.kind, trigger.prompt
    );
    if let Some(payload) = payload {
        text.push_str("\n\n");
        text.push_str(&payload);
    }
    let supervisor = supervisor.clone();
    let target = trigger.target.clone();
    let name = trigger.name.clone();
    tokio::spawn(async move {
        if let Err(error) = supervisor.deliver(&target, Origin::Argos, text).await {
            eprintln!("déclencheur « {name} » : remise impossible à {target} : {error}");
        }
    });
}

pub struct Events {
    supervisor: Supervisor,
    /// Surveillances ouvertes : déclencheur → (dossier, surveillance).
    watchers: Mutex<HashMap<String, (PathBuf, notify::RecommendedWatcher)>>,
    /// Fichiers vus récemment, pour ne pas déclencher deux fois.
    seen: Mutex<HashMap<(String, PathBuf), Instant>>,
    files: mpsc::UnboundedSender<(String, PathBuf)>,
}

impl Events {
    /// Prépare le moteur et lance la tâche qui reçoit les nouveaux fichiers.
    pub fn new(supervisor: Supervisor) -> Arc<Self> {
        let (files, mut incoming) = mpsc::unbounded_channel();
        let events = Arc::new(Self {
            supervisor,
            watchers: Mutex::default(),
            seen: Mutex::default(),
            files,
        });
        let consumer = Arc::clone(&events);
        tokio::spawn(async move {
            while let Some((trigger, path)) = incoming.recv().await {
                consumer.new_file(&trigger, path);
            }
        });
        events
    }

    /// Fait tourner le moteur, une passe par seconde.
    pub async fn run(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            self.tick(now());
        }
    }

    /// Une passe à l'instant `now` (secondes Unix) : périodiques échus, surveillances.
    pub fn tick(&self, now: u64) {
        let triggers = match self.supervisor.registry().enabled_triggers() {
            Ok(triggers) => triggers,
            Err(error) => {
                eprintln!("moteur d'événements : {error}");
                return;
            }
        };
        for trigger in &triggers {
            if is_due(trigger, now) {
                fire(&self.supervisor, trigger, None);
            }
        }
        self.reconcile_watchers(&triggers);
    }

    /// Ouvre les surveillances qui manquent, ferme celles qui n'ont plus lieu d'être.
    fn reconcile_watchers(&self, triggers: &[Trigger]) {
        let wanted: HashMap<&str, &str> = triggers
            .iter()
            .filter(|trigger| trigger.kind == TriggerKind::Watch)
            .map(|trigger| (trigger.id.as_str(), trigger.spec.as_str()))
            .collect();
        let mut watchers = self.watchers.lock().expect("verrou des surveillances");
        watchers
            .retain(|id, (path, _)| wanted.get(id.as_str()) == Some(&path.to_str().unwrap_or("")));
        for (id, path) in wanted {
            if watchers.contains_key(id) {
                continue;
            }
            match self.watch(id, Path::new(path)) {
                Ok(watcher) => {
                    watchers.insert(id.to_string(), (PathBuf::from(path), watcher));
                }
                Err(error) => eprintln!("surveillance de {path} impossible : {error}"),
            }
        }
    }

    fn watch(&self, trigger: &str, path: &Path) -> notify::Result<notify::RecommendedWatcher> {
        let files = self.files.clone();
        let trigger = trigger.to_string();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                // Un fichier créé, ou déposé par déplacement dans le dossier.
                let arrived = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
                );
                if !arrived {
                    return;
                }
                for path in event.paths.into_iter().filter(|path| path.is_file()) {
                    let _ = files.send((trigger.clone(), path));
                }
            })?;
        watcher.watch(path, RecursiveMode::NonRecursive)?;
        Ok(watcher)
    }

    fn new_file(&self, trigger: &str, path: PathBuf) {
        {
            let mut seen = self.seen.lock().expect("verrou des fichiers vus");
            seen.retain(|_, at| at.elapsed() < WATCH_DEBOUNCE);
            if seen
                .insert((trigger.to_string(), path.clone()), Instant::now())
                .is_some()
            {
                return;
            }
        }
        let Ok(found) = self.supervisor.registry().trigger(trigger) else {
            return;
        };
        if found.enabled {
            let payload = format!("Nouveau fichier : {}", path.display());
            fire(&self.supervisor, &found, Some(payload));
        }
    }
}

/// Un déclencheur périodique est-il échu à `now` ? La référence est son dernier
/// déclenchement ou sa dernière (ré)activation : après une absence il ne part qu'une
/// fois, et un déclencheur réactivé attend sa prochaine échéance.
pub fn is_due(trigger: &Trigger, now: u64) -> bool {
    let since = trigger.reference();
    match trigger.kind {
        TriggerKind::Every => trigger
            .spec
            .parse::<u64>()
            .is_ok_and(|every| now >= since + every),
        TriggerKind::Cron => next_cron(&trigger.spec, since).is_some_and(|next| now >= next),
        _ => false,
    }
}

/// Prochaine échéance cron strictement après `after` (secondes Unix), heure locale.
fn next_cron(expression: &str, after: u64) -> Option<u64> {
    let cron = expression.parse::<croner::Cron>().ok()?;
    let reference = Local.timestamp_opt(after as i64, 0).single()?;
    let next = cron.find_next_occurrence(&reference, false).ok()?;
    u64::try_from(next.timestamp()).ok()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argos_core::AgentId;

    fn trigger(kind: TriggerKind, spec: &str, created_at: u64, last_fired: Option<u64>) -> Trigger {
        Trigger {
            id: "tr-1".into(),
            owner: AgentId::new("a"),
            target: AgentId::new("a"),
            name: "relevé".into(),
            kind,
            spec: spec.into(),
            prompt: "Relève.".into(),
            secret: None,
            enabled: true,
            created_at,
            armed_at: created_at,
            last_fired,
        }
    }

    #[test]
    fn an_interval_falls_due_from_its_last_firing() {
        let every = trigger(TriggerKind::Every, "600", 1_000, None);
        assert!(!is_due(&every, 1_599));
        assert!(is_due(&every, 1_600));
        let fired = trigger(TriggerKind::Every, "600", 1_000, Some(1_600));
        assert!(!is_due(&fired, 1_700));
        // Après une longue absence : échu, mais une seule fois (la référence avance).
        assert!(is_due(&fired, 100_000));
    }

    #[test]
    fn a_reactivated_cron_waits_for_its_next_occurrence() {
        // Créé il y a longtemps, jamais déclenché, réactivé à `now` : rien ne part
        // avant la prochaine minute pleine.
        let mut cron = trigger(TriggerKind::Cron, "* * * * *", 1_000, None);
        cron.armed_at = 2_000_000;
        assert!(!is_due(&cron, 2_000_000));
        assert!(is_due(&cron, 2_000_061));
    }

    #[test]
    fn a_cron_falls_due_at_its_next_occurrence() {
        // Toutes les minutes : échu une minute au plus après la référence.
        let minutely = trigger(TriggerKind::Cron, "* * * * *", 1_000_000, None);
        assert!(!is_due(&minutely, 1_000_000));
        assert!(is_due(&minutely, 1_000_061));
        let never = trigger(TriggerKind::Cron, "pas une expression", 1_000_000, None);
        assert!(!is_due(&never, u64::MAX / 2));
    }

    #[test]
    fn manual_and_external_triggers_never_fall_due_by_time() {
        for kind in [
            TriggerKind::Switch,
            TriggerKind::Webhook,
            TriggerKind::Watch,
        ] {
            assert!(!is_due(&trigger(kind, "", 0, None), u64::MAX / 2));
        }
    }
}
