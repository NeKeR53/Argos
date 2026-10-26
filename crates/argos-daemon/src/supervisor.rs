//! Superviseur : relie le registre aux sessions vivantes et applique le plan de contrôle
//! (D14) — réveil, interruption, parking, stop en cascade, retrait, panic stop. Tout y est
//! déterministe : il exécute des ordres, il ne décide rien.
//!
//! Règle de cohérence : celui qui retire une session de son emplacement met le registre à
//! jour. Une session morte en retard ne peut donc pas écraser l'état d'une session plus
//! récente du même agent.

use crate::contracts;
use crate::engine::{
    EngineError, EngineEvent, EventStream, McpServer, Session, SessionControl, SessionSpec,
    prepare_codex_home,
};
use crate::memory::Memory;
use crate::policy::ModelPolicy;
use crate::registry::{NewAgent, Registry, RegistryError};
use argos_core::protocol::{AgentStatus, DaemonEvent, Origin};
use argos_core::{AgentId, AgentRecord, AgentState, EngineKind, Level, Usage};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, broadcast, watch};

/// Délai laissé à un process pour sortir après un parking ou un kill.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Remplissage du contexte qui déclenche la compaction contrôlée (D24).
const COMPACT_AT: f64 = 0.7;
/// Garde-fou du dialogue latéral (D40) : messages au plus par paire d'Agents, par fenêtre.
const LATERAL_LIMIT: usize = 20;
const LATERAL_WINDOW: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct SupervisorSettings {
    /// `$ARGOS_HOME` : les `CODEX_HOME` isolés y sont créés.
    pub home: PathBuf,
    /// `~/.codex` de l'utilisateur, d'où vient l'authentification Codex.
    pub user_codex_home: PathBuf,
    /// Binaires à lancer à la place de `claude` / `codex` (tests).
    pub programs: HashMap<EngineKind, PathBuf>,
    /// Adresse du serveur MCP d'Argos (`http://127.0.0.1:7879`). Sans elle, les agents
    /// tournent sans les outils d'Argos.
    pub mcp_url: Option<String>,
    /// Politique de modèles : profils, replis, plafonds (D22).
    pub policy: ModelPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("préparation de l'agent : {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} est retiré")]
    Retired(AgentId),
    #[error("{0} est en cours d'arrêt")]
    Stopping(AgentId),
    #[error("plafond atteint : {0}")]
    Cap(String),
}

pub type Result<T> = std::result::Result<T, SupervisorError>;

#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Mutex<Registry>,
    /// Un verrou par agent : réveil, parking et stop d'un même agent ne se chevauchent
    /// pas, sans bloquer les autres agents.
    slots: Mutex<HashMap<AgentId, Arc<AsyncMutex<Option<Live>>>>>,
    /// Agents dont le process tourne, avec leur drapeau « tour en cours ». Lisible sans
    /// attendre le verrou d'un agent (qui peut être pris par un démarrage lent).
    running: Mutex<HashMap<AgentId, Arc<AtomicBool>>>,
    /// Agents en cours de stop : aucun réveil tant que le stop n'est pas fini (sinon le
    /// rapport tardif d'un Worker relancerait l'Agent qu'on arrête).
    halted: Mutex<HashSet<AgentId>>,
    /// Jetons MCP des sessions en cours → agent appelant.
    tokens: Mutex<HashMap<String, AgentId>>,
    /// Agents à qui l'on a demandé de vider leur contexte dans leur mémoire.
    compacting: Mutex<HashSet<AgentId>>,
    /// Dernier message remis à chaque agent, à relancer après un repli de modèle.
    last_input: Mutex<HashMap<AgentId, String>>,
    /// Instants des derniers messages latéraux, par paire d'Agents (D40).
    lateral: Mutex<HashMap<(AgentId, AgentId), VecDeque<Instant>>>,
    memory: Memory,
    events: broadcast::Sender<DaemonEvent>,
    generations: AtomicU64,
    settings: SupervisorSettings,
}

/// La session en cours d'un agent.
struct Live {
    generation: u64,
    control: SessionControl,
    exited: watch::Receiver<bool>,
}

impl Supervisor {
    pub fn new(registry: Registry, settings: SupervisorSettings) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                registry: Mutex::new(registry),
                slots: Mutex::default(),
                running: Mutex::default(),
                halted: Mutex::default(),
                tokens: Mutex::default(),
                compacting: Mutex::default(),
                last_input: Mutex::default(),
                lateral: Mutex::default(),
                memory: Memory::new(settings.home.join("memory")),
                events,
                generations: AtomicU64::new(0),
                settings,
            }),
        }
    }

    /// Les mémoires de l'Assistant et des Agents.
    pub fn memory(&self) -> &Memory {
        &self.inner.memory
    }

    /// Compte un message latéral entre deux Agents (D40) ; `false` si la paire a dépassé
    /// son quota sur la fenêtre, pour couper un échange qui tournerait sans fin.
    pub fn allow_lateral(&self, a: &AgentId, b: &AgentId) -> bool {
        let pair = if a <= b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        };
        let mut lateral = locked(&self.inner.lateral);
        let recent = lateral.entry(pair).or_default();
        let now = Instant::now();
        while recent
            .front()
            .is_some_and(|sent| now.duration_since(*sent) > LATERAL_WINDOW)
        {
            recent.pop_front();
        }
        if recent.len() >= LATERAL_LIMIT {
            return false;
        }
        recent.push_back(now);
        true
    }

    pub fn policy(&self) -> &ModelPolicy {
        &self.inner.settings.policy
    }

    /// Flux de tous les événements des agents.
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.inner.events.subscribe()
    }

    /// Accès direct au registre. Ne pas garder le verrou pendant un `await`.
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        locked(&self.inner.registry)
    }

    pub fn tree(&self) -> Result<Vec<AgentStatus>> {
        let agents = self.registry().tree()?;
        Ok(agents
            .into_iter()
            .map(|record| self.status(record))
            .collect())
    }

    pub fn inspect(&self, id: &AgentId) -> Result<AgentStatus> {
        Ok(self.status(self.record(id)?))
    }

    pub fn record(&self, id: &AgentId) -> Result<AgentRecord> {
        self.registry()
            .get(id)?
            .ok_or_else(|| RegistryError::NotFound(id.clone()).into())
    }

    /// Enfants non retirés d'un agent, avec leur état d'exécution.
    pub fn children(&self, id: &AgentId) -> Result<Vec<AgentStatus>> {
        let children = self.registry().children(id)?;
        Ok(children
            .into_iter()
            .map(|record| self.status(record))
            .collect())
    }

    /// Crée un agent dans le registre (il commence parké) et le signale.
    pub fn create(&self, new: NewAgent) -> Result<AgentRecord> {
        let record = self.registry().create(new)?;
        self.publish_status(&record.id);
        Ok(record)
    }

    /// Dossier où ranger le répertoire de travail d'un nouvel Agent.
    pub fn agents_dir(&self) -> PathBuf {
        self.inner.settings.home.join("agents")
    }

    /// Émet un jeton MCP pour `id`, valable jusqu'à la fin de sa session en cours.
    pub fn mcp_token(&self, id: &AgentId) -> String {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        locked(&self.inner.tokens).insert(token.clone(), id.clone());
        token
    }

    /// L'agent auquel ce jeton MCP a été donné.
    pub fn agent_for_token(&self, token: &str) -> Option<AgentId> {
        locked(&self.inner.tokens).get(token).cloned()
    }

    /// Message à un agent. Un agent parké est d'abord réveillé : sa session reprend.
    pub async fn send(&self, id: &AgentId, text: impl Into<String>) -> Result<()> {
        self.deliver(id, Origin::User, text).await
    }

    /// Remet un message à un agent en disant qui l'envoie (un autre agent, Argos) ; un
    /// agent parké est d'abord réveillé.
    pub async fn deliver(&self, id: &AgentId, from: Origin, text: impl Into<String>) -> Result<()> {
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        if live.is_none() {
            *live = Some(self.wake(id).await?);
        }
        if let Some(busy) = locked(&self.inner.running).get(id) {
            busy.store(true, Ordering::SeqCst);
        }
        let text = text.into();
        live.as_ref()
            .expect("session démarrée juste au-dessus")
            .control
            .send(text.clone())?;
        locked(&self.inner.last_input).insert(id.clone(), text.clone());
        self.publish(DaemonEvent::Input {
            agent: id.clone(),
            from,
            text,
        });
        self.publish_status(id);
        Ok(())
    }

    /// Interrompt le tour en cours, s'il y en a un.
    pub async fn interrupt(&self, id: &AgentId) -> Result<()> {
        self.record(id)?;
        let slot = self.slot(id);
        let live = slot.lock().await;
        if let Some(session) = live.as_ref() {
            session.control.interrupt()?;
        }
        Ok(())
    }

    /// Arrêt propre ; rend la main une fois le process sorti.
    pub async fn park(&self, id: &AgentId) -> Result<()> {
        self.record(id)?;
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        if let Some(session) = live.take() {
            let _ = session.control.park();
            if !exited(session.exited.clone()).await {
                session.control.kill();
                exited(session.exited).await;
            }
            self.settle(id);
        }
        Ok(())
    }

    /// Arrêt dur de l'agent et de tout son sous-arbre ; rend la main une fois les process
    /// morts. Rend les agents dont un process a été arrêté.
    pub async fn stop(&self, id: &AgentId) -> Result<Vec<AgentId>> {
        self.record(id)?;
        // Le parent d'abord : arrêté, il ne peut plus relancer de Worker.
        let mut targets = vec![id.clone()];
        targets.extend(self.registry().descendants(id)?);
        locked(&self.inner.halted).extend(targets.iter().cloned());
        let mut stopped = vec![];
        for target in &targets {
            if self.kill(target).await {
                stopped.push(target.clone());
            }
        }
        let mut halted = locked(&self.inner.halted);
        for target in &targets {
            halted.remove(target);
        }
        Ok(stopped)
    }

    /// Retrait définitif de l'agent et de son sous-arbre, process arrêtés d'abord.
    pub async fn retire(&self, id: &AgentId) -> Result<Vec<AgentId>> {
        self.stop(id).await?;
        let retired = self.registry().retire(id)?;
        for id in &retired {
            self.publish_status(id);
        }
        Ok(retired)
    }

    /// Arrêt dur de tout l'arbre (D16).
    pub async fn panic_stop(&self) -> Result<Vec<AgentId>> {
        let Some(assistant) = self.registry().assistant()? else {
            return Ok(vec![]);
        };
        self.stop(&assistant.id).await
    }

    /// Arrêt du daemon : parke toutes les sessions en cours.
    pub async fn shutdown(&self) {
        let ids: Vec<AgentId> = locked(&self.inner.running).keys().cloned().collect();
        futures_util::future::join_all(ids.iter().map(|id| self.park(id))).await;
    }

    /// Lance ou reprend la session d'un agent. Appelé avec le verrou de l'agent tenu.
    async fn wake(&self, id: &AgentId) -> Result<Live> {
        let record = self.record(id)?;
        if record.state == AgentState::Retired {
            return Err(SupervisorError::Retired(id.clone()));
        }
        {
            let halted = locked(&self.inner.halted);
            if halted.contains(id) || record.parent.as_ref().is_some_and(|p| halted.contains(p)) {
                return Err(SupervisorError::Stopping(id.clone()));
            }
        }
        self.check_parallel_cap(&record)?;
        let spawned = match self.spec_for(&record) {
            Ok(spec) => Session::spawn(spec).await.map_err(SupervisorError::from),
            Err(error) => Err(error),
        };
        let (control, events) = match spawned {
            Ok(session) => session.split(),
            Err(error) => {
                self.revoke_tokens(id);
                return Err(error);
            }
        };
        let registered = {
            let registry = self.registry();
            registry
                .set_session(id, control.session_id())
                .and_then(|()| registry.set_state(id, AgentState::Active))
        };
        if let Err(error) = registered {
            control.kill();
            self.revoke_tokens(id);
            return Err(error.into());
        }

        let generation = self.inner.generations.fetch_add(1, Ordering::SeqCst) + 1;
        let busy = Arc::new(AtomicBool::new(false));
        locked(&self.inner.running).insert(id.clone(), Arc::clone(&busy));
        let (exit_tx, exit_rx) = watch::channel(false);
        tokio::spawn(
            self.clone()
                .pump(id.clone(), generation, events, busy, exit_tx),
        );
        self.publish_status(id);
        Ok(Live {
            generation,
            control,
            exited: exit_rx,
        })
    }

    /// Relaie les événements d'une session jusqu'à sa fin.
    async fn pump(
        self,
        id: AgentId,
        generation: u64,
        mut events: EventStream,
        busy: Arc<AtomicBool>,
        exit: watch::Sender<bool>,
    ) {
        while let Some(event) = events.next_event().await {
            let agent = id.clone();
            match event {
                EngineEvent::AgentMessage { text } => {
                    self.publish(DaemonEvent::Message { agent, text })
                }
                EngineEvent::ToolActivity { name } => {
                    self.publish(DaemonEvent::Tool { agent, name })
                }
                EngineEvent::RateLimited { message } => self.start_fall_back(agent, message),
                EngineEvent::TurnCompleted { outcome, usage } => {
                    busy.store(false, Ordering::SeqCst);
                    self.publish(DaemonEvent::TurnCompleted {
                        agent,
                        outcome,
                        usage,
                    });
                    self.publish_status(&id);
                    self.watch_context(&id, usage);
                }
                EngineEvent::Exited { code, stderr_tail } => {
                    // D'abord prévenir qui attend cette sortie (parking, stop) : il tient
                    // le verrou de l'agent.
                    let _ = exit.send(true);
                    self.publish(DaemonEvent::Exited { agent, code });
                    let slot = self.slot(&id);
                    let mut live = slot.lock().await;
                    // Personne n'a repris la session : c'est une sortie imprévue (panne).
                    if live
                        .as_ref()
                        .is_some_and(|session| session.generation == generation)
                    {
                        *live = None;
                        drop(live);
                        eprintln!(
                            "agent {id} : process sorti de lui-même (code {code:?}) : {}",
                            stderr_tail.join(" | ")
                        );
                        self.settle(&id);
                    }
                }
            }
        }
    }

    /// Tue la session d'un agent s'il en a une ; rend `true` dans ce cas.
    async fn kill(&self, id: &AgentId) -> bool {
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        let Some(session) = live.take() else {
            return false;
        };
        session.control.kill();
        exited(session.exited).await;
        self.settle(id);
        true
    }

    /// La session d'un agent a disparu : il redevient parké (sauf s'il est retiré).
    fn settle(&self, id: &AgentId) {
        locked(&self.inner.running).remove(id);
        self.revoke_tokens(id);
        // Échoue seulement pour un agent retiré, qui doit le rester.
        let _ = self.registry().set_state(id, AgentState::Parked);
        self.publish_status(id);
    }

    fn spec_for(&self, record: &AgentRecord) -> Result<SessionSpec> {
        let settings = &self.inner.settings;
        std::fs::create_dir_all(&record.workdir)?;
        let mut spec = SessionSpec::new(record.engine, &record.workdir);
        spec.model = record.model.clone();
        spec.resume = record.session_id.clone();
        // L'Assistant et les Agents orchestrent : seuls les Workers modifient (D17, D18).
        spec.read_only = record.level != Level::Worker;
        spec.program = settings.programs.get(&record.engine).cloned();
        if let Some(base) = &settings.mcp_url {
            spec.mcp_servers = vec![McpServer {
                name: "argos".into(),
                url: format!("{base}/mcp/{}", self.mcp_token(&record.id)),
            }];
        }
        if record.engine == EngineKind::Codex {
            let home = settings.home.join("codex").join(record.id.as_str());
            prepare_codex_home(&home, &settings.user_codex_home)?;
            spec.codex_home = Some(home);
        }
        // Le contrat du niveau d'abord, puis la mémoire pour ceux qui en ont une.
        let mut instructions = contracts::contract(record, self.policy());
        if record.level.has_memory() {
            let memory = &self.inner.memory;
            memory.ensure(&record.id)?;
            memory.write_state(&record.id, &self.render_state(record)?)?;
            instructions.push('\n');
            instructions.push_str(&memory.digest(&record.id)?);
        }
        spec.instructions = Some(instructions);
        Ok(spec)
    }

    /// L'état structuré d'un agent, réécrit dans sa mémoire à chaque réveil (D23).
    fn render_state(&self, record: &AgentRecord) -> Result<String> {
        let registry = self.registry();
        let mut state = String::from("# État (tenu par Argos, réécrit à chaque réveil)\n\n");
        match &record.charter {
            Some(charter) => {
                let _ = writeln!(state, "- Rôle : Agent « {} »", charter.name);
                let _ = writeln!(state, "- Périmètre : {}", charter.scope);
            }
            None => {
                let _ = writeln!(state, "- Rôle : {}", record.level);
            }
        }
        let _ = writeln!(
            state,
            "- Moteur : {} ({})\n- Répertoire de travail : {}",
            record.engine,
            record.model.as_deref().unwrap_or("modèle par défaut"),
            record.workdir.display()
        );
        let children = registry.children(&record.id)?;
        let title = match record.level {
            Level::Assistant => "Tes Agents",
            _ => "Tes Workers",
        };
        let _ = writeln!(state, "\n## {title}");
        if children.is_empty() {
            state.push_str("- aucun\n");
        }
        for child in &children {
            match &child.charter {
                Some(charter) => {
                    let _ = writeln!(
                        state,
                        "- « {} » ({}) — {} — {}",
                        charter.name, child.id, charter.scope, child.state
                    );
                }
                None => {
                    let task = registry.task_of_worker(&child.id)?;
                    let task = task.map_or("sans tâche".into(), |task| {
                        format!("tâche {} [{}] {}", task.id, task.state, task.title)
                    });
                    let _ = writeln!(state, "- {} — {} — {task}", child.id, child.state);
                }
            }
        }
        if record.level == Level::Agent {
            let board = registry.board(&record.id)?;
            state.push_str("\n## Ton board\n");
            if board.missions.is_empty() && board.tasks.is_empty() {
                state.push_str("- vide\n");
            }
            for mission in &board.missions {
                let _ = writeln!(state, "- Mission {} : {}", mission.id, mission.text);
            }
            for task in &board.tasks {
                let _ = writeln!(state, "  - {} [{}] {}", task.id, task.state, task.title);
            }
        }
        Ok(state)
    }

    /// Compaction contrôlée (D24) : passé le seuil, l'agent est prié d'écrire dans sa
    /// mémoire ce qui doit survivre ; le tour suivant fini, sa session repart à neuf.
    fn watch_context(&self, id: &AgentId, usage: Option<Usage>) {
        let flushing = locked(&self.inner.compacting).remove(id);
        if flushing {
            tokio::spawn(self.clone().renew_session(id.clone()));
            return;
        }
        let Some(fill) = usage.and_then(|usage| usage.context_fill()) else {
            return;
        };
        let has_memory = self
            .record(id)
            .is_ok_and(|record| record.level.has_memory());
        if fill < COMPACT_AT || !has_memory {
            return;
        }
        locked(&self.inner.compacting).insert(id.clone());
        let prompt = format!(
            "[Argos] Ton contexte est rempli à {:.0} %. Avant qu'il soit vidé, mets ta mémoire à jour avec memory_write : index.md (l'essentiel), journal.md (ce qui s'est passé), notes/ (le détail utile). Réponds ensuite seulement « prêt » : ta prochaine session repartira de ta mémoire.",
            fill * 100.0
        );
        let supervisor = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            if supervisor
                .deliver(&id, Origin::Argos, prompt)
                .await
                .is_err()
            {
                locked(&supervisor.inner.compacting).remove(&id);
            }
        });
    }

    /// Parke l'agent et oublie sa session : le prochain réveil repart de sa mémoire.
    async fn renew_session(self, id: AgentId) {
        if self.park(&id).await.is_ok() && self.registry().clear_session(&id).is_ok() {
            let _ = self.inner.memory.log(
                &id,
                "contexte compacté : la session suivante repart de la mémoire",
            );
            self.publish_status(&id);
        }
    }

    /// Plafond de sessions simultanées du modèle de cet agent (D22).
    fn check_parallel_cap(&self, record: &AgentRecord) -> Result<()> {
        let Some(model) = &record.model else {
            return Ok(());
        };
        let Some(cap) = self.policy().max_parallel(model) else {
            return Ok(());
        };
        let running: Vec<AgentId> = locked(&self.inner.running).keys().cloned().collect();
        let registry = self.registry();
        let same_model = running
            .iter()
            .filter(|other| **other != record.id)
            .filter_map(|other| registry.get(other).ok().flatten())
            .filter(|other| other.model.as_ref() == Some(model))
            .count();
        if same_model >= cap {
            return Err(SupervisorError::Cap(format!(
                "{cap} session(s) {model} déjà en cours"
            )));
        }
        Ok(())
    }

    /// Lance le repli dans sa propre tâche. Fonction synchrone : le type de la tâche de
    /// relais ne dépend ainsi pas du repli, qui réveille l'agent et relance un relais.
    fn start_fall_back(&self, id: AgentId, reason: String) {
        tokio::spawn(self.clone().fall_back(id, reason));
    }

    /// Repli de la politique de modèles (D22) : quota épuisé, l'agent passe au maillon
    /// suivant de la chaîne de son profil et reprend ce qu'il faisait. Changer de moteur
    /// ouvre une session neuve : un Agent repart de sa mémoire, un Worker de sa tâche.
    async fn fall_back(self, id: AgentId, reason: String) {
        let Ok(record) = self.record(&id) else {
            return;
        };
        let next = record.profile.as_deref().and_then(|profile| {
            self.policy().fallback(
                profile,
                record.level,
                record.engine,
                record.model.as_deref(),
            )
        });
        let Some(next) = next else {
            self.notice(
                &id,
                format!("{reason} ; aucun repli prévu : l'agent attend"),
            );
            return;
        };
        let _ = self.park(&id).await;
        let switched = {
            let registry = self.registry();
            registry
                .set_engine_model(&id, next.engine, Some(&next.model))
                .and_then(|()| {
                    if next.engine == record.engine {
                        Ok(())
                    } else {
                        registry.clear_session(&id)
                    }
                })
        };
        if switched.is_err() {
            return;
        }
        let change = format!(
            "{reason} : bascule de {} {} vers {} {}",
            record.engine,
            record.model.as_deref().unwrap_or("?"),
            next.engine,
            next.model
        );
        let _ = self.inner.memory.log(&id, &change);
        self.notice(&id, change);
        self.publish_status(&id);

        let task = self.registry().task_of_worker(&id).ok().flatten();
        let resume = match task {
            Some(task) if record.level == Level::Worker && next.engine != record.engine => {
                Some(format!(
                    "[Argos] Reprise sur un autre moteur après un quota épuisé.\n[Tâche {}]\n{}\n\n{}",
                    task.id,
                    task.title,
                    task.detail.unwrap_or_default()
                ))
            }
            _ => locked(&self.inner.last_input).get(&id).cloned(),
        };
        if let Some(text) = resume
            && let Err(error) = self.deliver(&id, Origin::Argos, text).await
        {
            self.notice(&id, format!("reprise impossible après le repli : {error}"));
        }
    }

    /// Information courte pour l'utilisateur, à propos d'un agent.
    fn notice(&self, id: &AgentId, text: String) {
        self.publish(DaemonEvent::Notice {
            agent: Some(id.clone()),
            text,
        });
    }

    fn revoke_tokens(&self, id: &AgentId) {
        locked(&self.inner.tokens).retain(|_, holder| holder != id);
    }

    fn slot(&self, id: &AgentId) -> Arc<AsyncMutex<Option<Live>>> {
        Arc::clone(locked(&self.inner.slots).entry(id.clone()).or_default())
    }

    fn status(&self, record: AgentRecord) -> AgentStatus {
        let busy = locked(&self.inner.running).get(&record.id).cloned();
        AgentStatus {
            running: busy.is_some(),
            busy: busy.is_some_and(|busy| busy.load(Ordering::SeqCst)),
            record,
        }
    }

    fn publish(&self, event: DaemonEvent) {
        // Sans abonné, l'événement est simplement perdu.
        let _ = self.inner.events.send(event);
    }

    fn publish_status(&self, id: &AgentId) {
        if let Ok(agent) = self.inspect(id) {
            self.publish(DaemonEvent::AgentUpdated { agent });
        }
    }
}

/// Attend la sortie signalée par la tâche de la session ; `false` après le délai.
async fn exited(mut signal: watch::Receiver<bool>) -> bool {
    // Un émetteur disparu signifie aussi que la session est finie.
    tokio::time::timeout(EXIT_TIMEOUT, signal.wait_for(|exited| *exited))
        .await
        .is_ok()
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("verrou empoisonné")
}
