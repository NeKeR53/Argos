//! Déclencheurs (D29–D31) : sources d'événements qui stimulent un agent. Gardés en
//! base ; le moteur d'événements (`crate::events`) les relit et les fait tourner.

use super::{Registry, RegistryError, Result, now, parsed};
use argos_core::{AgentId, AgentState, Level, Trigger, TriggerKind};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::Path;
use uuid::Uuid;

pub(super) const SCHEMA: &str = "
CREATE TABLE triggers (
    id         TEXT PRIMARY KEY,
    owner      TEXT NOT NULL REFERENCES agents(id),
    target     TEXT NOT NULL REFERENCES agents(id),
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('every', 'cron', 'switch', 'watch', 'webhook')),
    spec       TEXT NOT NULL,
    prompt     TEXT NOT NULL,
    secret     TEXT,
    enabled    INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_fired INTEGER
);
CREATE INDEX triggers_by_owner ON triggers(owner);
CREATE INDEX triggers_by_target ON triggers(target);
";

/// Intervalle minimal d'un déclencheur périodique : au-delà, on brûle des tokens pour rien.
pub const MIN_EVERY_SECONDS: u64 = 60;

const COLUMNS: &str = "id, owner, target, name, kind, spec, prompt, secret, enabled, created_at, last_fired, armed_at";

/// v6 : instant de (ré)activation, référence des périodiques après une suspension.
pub(super) const ARMED_COLUMN: &str =
    "ALTER TABLE triggers ADD COLUMN armed_at INTEGER; UPDATE triggers SET armed_at = created_at;";

/// Un déclencheur à créer.
#[derive(Debug, Clone)]
pub struct NewTrigger {
    pub owner: AgentId,
    pub target: AgentId,
    pub name: String,
    pub kind: TriggerKind,
    pub spec: String,
    pub prompt: String,
}

impl Registry {
    /// Crée un déclencheur actif, après validation de son paramètre.
    pub fn add_trigger(&self, new: NewTrigger) -> Result<Trigger> {
        for agent in [&new.owner, &new.target] {
            let record = self.require(agent)?;
            if record.level == Level::Worker {
                return Err(RegistryError::Trigger(
                    "un Worker ne crée ni ne reçoit de déclencheur".into(),
                ));
            }
            if record.state == AgentState::Retired {
                return Err(RegistryError::Hierarchy(format!("{agent} est retiré")));
            }
        }
        let name = new.name.trim();
        let prompt = new.prompt.trim();
        if name.is_empty() || prompt.is_empty() {
            return Err(RegistryError::Trigger(
                "un déclencheur a un nom et une consigne".into(),
            ));
        }
        let spec = checked_spec(new.kind, new.spec.trim())?;
        let id = format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let secret =
            (new.kind == TriggerKind::Webhook).then(|| format!("{}", Uuid::new_v4().simple()));
        self.db.execute(
            &format!(
                "INSERT INTO triggers ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, NULL, ?9)"
            ),
            params![
                id,
                new.owner.as_str(),
                new.target.as_str(),
                name,
                new.kind.as_str(),
                spec,
                prompt,
                secret,
                now() as i64
            ],
        )?;
        self.trigger(&id)
    }

    pub fn trigger(&self, id: &str) -> Result<Trigger> {
        self.db
            .query_row(
                &format!("SELECT {COLUMNS} FROM triggers WHERE id = ?1"),
                [id],
                trigger_row,
            )
            .optional()?
            .ok_or_else(|| RegistryError::TriggerNotFound(id.into()))
    }

    /// Les déclencheurs dont l'agent est propriétaire ou cible.
    pub fn triggers_of(&self, agent: &AgentId) -> Result<Vec<Trigger>> {
        self.query_triggers(
            "WHERE owner = ?1 OR target = ?1 ORDER BY created_at, rowid",
            [agent.as_str()],
        )
    }

    /// Tous les déclencheurs actifs, pour le moteur d'événements.
    pub fn enabled_triggers(&self) -> Result<Vec<Trigger>> {
        self.query_triggers("WHERE enabled = 1 ORDER BY created_at, rowid", [])
    }

    /// Active ou suspend. Réactiver réarme : un périodique repart de maintenant, au lieu
    /// de rattraper la période passée suspendu.
    pub fn set_trigger_enabled(&self, id: &str, enabled: bool) -> Result<Trigger> {
        let trigger = self.trigger(id)?;
        if enabled && !trigger.enabled {
            self.db.execute(
                "UPDATE triggers SET enabled = 1, armed_at = ?2 WHERE id = ?1",
                params![id, now() as i64],
            )?;
        } else {
            self.db.execute(
                "UPDATE triggers SET enabled = ?2 WHERE id = ?1",
                params![id, enabled],
            )?;
        }
        self.trigger(id)
    }

    pub fn mark_trigger_fired(&self, id: &str, at: u64) -> Result<()> {
        self.db.execute(
            "UPDATE triggers SET last_fired = ?2 WHERE id = ?1",
            params![id, at as i64],
        )?;
        Ok(())
    }

    pub fn delete_trigger(&self, id: &str) -> Result<Trigger> {
        let trigger = self.trigger(id)?;
        self.db
            .execute("DELETE FROM triggers WHERE id = ?1", [id])?;
        Ok(trigger)
    }

    fn query_triggers(&self, filter: &str, params: impl rusqlite::Params) -> Result<Vec<Trigger>> {
        let mut statement = self
            .db
            .prepare(&format!("SELECT {COLUMNS} FROM triggers {filter}"))?;
        let triggers = statement
            .query_map(params, trigger_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(triggers)
    }
}

/// Supprime les déclencheurs dont le propriétaire ou la cible est retiré.
pub(super) fn drop_triggers_of(db: &Connection, agents: &[AgentId]) -> Result<()> {
    for agent in agents {
        db.execute(
            "DELETE FROM triggers WHERE owner = ?1 OR target = ?1",
            [agent.as_str()],
        )?;
    }
    Ok(())
}

fn checked_spec(kind: TriggerKind, spec: &str) -> Result<String> {
    let refused = |reason: String| Err(RegistryError::Trigger(reason));
    match kind {
        TriggerKind::Every => match spec.parse::<u64>() {
            Ok(seconds) if seconds >= MIN_EVERY_SECONDS => Ok(seconds.to_string()),
            Ok(_) => refused(format!(
                "intervalle d'au moins {MIN_EVERY_SECONDS} secondes"
            )),
            Err(_) => refused(format!("intervalle illisible : « {spec} »")),
        },
        TriggerKind::Cron => match spec.parse::<croner::Cron>() {
            Ok(_) => Ok(spec.into()),
            Err(error) => refused(format!("expression cron illisible « {spec} » : {error}")),
        },
        TriggerKind::Watch => {
            let path = Path::new(spec);
            if !path.is_absolute() || !path.is_dir() {
                return refused(format!(
                    "« {spec} » n'est pas un dossier existant (chemin absolu)"
                ));
            }
            Ok(spec.into())
        }
        TriggerKind::Switch | TriggerKind::Webhook => Ok(String::new()),
    }
}

fn trigger_row(row: &Row) -> rusqlite::Result<Trigger> {
    Ok(Trigger {
        id: row.get(0)?,
        owner: AgentId::new(row.get::<_, String>(1)?),
        target: AgentId::new(row.get::<_, String>(2)?),
        name: row.get(3)?,
        kind: parsed(row, 4)?,
        spec: row.get(5)?,
        prompt: row.get(6)?,
        secret: row.get(7)?,
        enabled: row.get(8)?,
        created_at: row.get::<_, i64>(9)? as u64,
        last_fired: row.get::<_, Option<i64>>(10)?.map(|at| at as u64),
        armed_at: row.get::<_, i64>(11)? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::super::NewAgent;
    use super::*;
    use argos_core::{Charter, EngineKind};

    fn setup() -> (Registry, AgentId, AgentId) {
        let registry = Registry::open_in_memory().unwrap();
        let assistant = registry
            .ensure_assistant(EngineKind::Claude, None, "/a".into())
            .unwrap();
        let agent = registry
            .create(NewAgent {
                parent: assistant.id.clone(),
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                workdir: "/p".into(),
                charter: Some(Charter {
                    name: "Domotique".into(),
                    scope: "la maison".into(),
                }),
            })
            .unwrap();
        (registry, assistant.id, agent.id)
    }

    fn new(owner: &AgentId, target: &AgentId, kind: TriggerKind, spec: &str) -> NewTrigger {
        NewTrigger {
            owner: owner.clone(),
            target: target.clone(),
            name: "relevé".into(),
            kind,
            spec: spec.into(),
            prompt: "Relève les capteurs.".into(),
        }
    }

    #[test]
    fn each_kind_checks_its_parameter() {
        let (registry, assistant, agent) = setup();
        let dir = tempfile::tempdir().unwrap();
        let valid = [
            (TriggerKind::Every, "900"),
            (TriggerKind::Cron, "*/15 * * * *"),
            (TriggerKind::Watch, dir.path().to_str().unwrap()),
            (TriggerKind::Switch, ""),
            (TriggerKind::Webhook, ""),
        ];
        for (kind, spec) in valid {
            let trigger = registry
                .add_trigger(new(&assistant, &agent, kind, spec))
                .unwrap();
            assert!(trigger.enabled);
            assert_eq!(trigger.secret.is_some(), kind == TriggerKind::Webhook);
        }
        let invalid = [
            (TriggerKind::Every, "30"),
            (TriggerKind::Every, "souvent"),
            (TriggerKind::Cron, "toutes les heures"),
            (TriggerKind::Watch, "relatif/dossier"),
            (TriggerKind::Watch, "/nexiste/pas"),
        ];
        for (kind, spec) in invalid {
            assert!(
                matches!(
                    registry.add_trigger(new(&assistant, &agent, kind, spec)),
                    Err(RegistryError::Trigger(_))
                ),
                "{kind} {spec}"
            );
        }
    }

    #[test]
    fn triggers_follow_their_agents() {
        let (registry, assistant, agent) = setup();
        let trigger = registry
            .add_trigger(new(&assistant, &agent, TriggerKind::Every, "600"))
            .unwrap();
        assert_eq!(registry.triggers_of(&agent).unwrap(), vec![trigger.clone()]);
        assert_eq!(
            registry.triggers_of(&assistant).unwrap(),
            vec![trigger.clone()]
        );

        let paused = registry.set_trigger_enabled(&trigger.id, false).unwrap();
        assert!(!paused.enabled);
        assert!(registry.enabled_triggers().unwrap().is_empty());
        registry.set_trigger_enabled(&trigger.id, true).unwrap();
        registry.mark_trigger_fired(&trigger.id, 42).unwrap();
        assert_eq!(registry.trigger(&trigger.id).unwrap().last_fired, Some(42));

        // Réactiver réarme à maintenant.
        registry.set_trigger_enabled(&trigger.id, false).unwrap();
        let rearmed = registry.set_trigger_enabled(&trigger.id, true).unwrap();
        assert!(rearmed.armed_at >= trigger.armed_at);
        assert_eq!(rearmed.reference(), rearmed.armed_at.max(42));

        // Retirer la cible supprime ses déclencheurs.
        registry.retire(&agent).unwrap();
        assert!(matches!(
            registry.trigger(&trigger.id),
            Err(RegistryError::TriggerNotFound(_))
        ));
    }
}
