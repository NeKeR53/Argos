//! Registre des agents : l'arbre Assistant → Agents → Workers, persisté en SQLite, avec
//! les boards des Agents (`board`). Il applique les règles de la hiérarchie (D1, D13, D17,
//! D20) et ne décide rien d'autre.

mod board;
mod surfaces;
mod triggers;

pub use board::{Position, TaskUpdate};
pub use surfaces::MAX_HTML;
pub use triggers::{MIN_EVERY_SECONDS, NewTrigger};

use argos_core::{AgentId, AgentRecord, AgentState, Charter, EngineKind, Level};
use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("base du registre : {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("agent inconnu : {0}")]
    NotFound(AgentId),
    #[error("hiérarchie violée : {0}")]
    Hierarchy(String),
    #[error("un Agent s'appelle déjà « {0} »")]
    DuplicateName(String),
    #[error("charte invalide : {0}")]
    InvalidCharter(String),
    #[error("schéma du registre en version {found}, attendu {expected}")]
    SchemaVersion { found: i64, expected: i64 },
    #[error("mission inconnue : {0}")]
    MissionNotFound(String),
    #[error("tâche inconnue : {0}")]
    TaskNotFound(String),
    #[error("board : {0}")]
    Board(String),
    #[error("surface inconnue : {0}")]
    SurfaceNotFound(String),
    #[error("surface : {0}")]
    Surface(String),
    #[error("déclencheur inconnu : {0}")]
    TriggerNotFound(String),
    #[error("déclencheur : {0}")]
    Trigger(String),
}

pub type Result<T> = std::result::Result<T, RegistryError>;

/// Un agent à créer sous un parent existant.
#[derive(Debug, Clone)]
pub struct NewAgent {
    pub parent: AgentId,
    pub level: Level,
    pub engine: EngineKind,
    pub model: Option<String>,
    /// Profil de la politique de modèles dont viennent moteur et modèle.
    pub profile: Option<String>,
    /// Nœud d'un Agent (D32) ; ignoré pour un Worker, qui suit son Agent.
    pub node: Option<String>,
    pub workdir: PathBuf,
    pub charter: Option<Charter>,
}

/// Bilan de la réhydratation au démarrage du daemon.
#[derive(Debug, Default, PartialEq)]
pub struct Rehydration {
    /// Assistant et Agents qui tournaient : parkés, à reprendre par leur session-id.
    pub parked: Vec<AgentId>,
    /// Workers qui tournaient : retirés. Leur Agent relance leurs tâches (D10).
    pub lost_workers: Vec<AgentId>,
}

pub struct Registry {
    db: Connection,
}

/// Migrations successives ; la version du schéma est leur nombre (`PRAGMA user_version`).
const MIGRATIONS: [&str; 7] = [
    AGENTS_SCHEMA,
    board::SCHEMA,
    PROFILE_COLUMN,
    surfaces::SCHEMA,
    triggers::SCHEMA,
    triggers::ARMED_COLUMN,
    NODE_COLUMN,
];

/// v3 : profil de la politique de modèles (T8).
const PROFILE_COLUMN: &str = "ALTER TABLE agents ADD COLUMN profile TEXT;";
const NODE_COLUMN: &str = "ALTER TABLE agents ADD COLUMN node TEXT;";
const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

const AGENTS_SCHEMA: &str = "
CREATE TABLE agents (
    id         TEXT PRIMARY KEY,
    level      TEXT NOT NULL CHECK (level IN ('assistant', 'agent', 'worker')),
    parent     TEXT REFERENCES agents(id),
    engine     TEXT NOT NULL,
    model      TEXT,
    workdir    TEXT NOT NULL,
    name       TEXT,
    scope      TEXT,
    state      TEXT NOT NULL CHECK (state IN ('active', 'parked', 'retired')),
    session_id TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX agents_by_parent ON agents(parent);
-- Un seul Assistant, jamais remplacé.
CREATE UNIQUE INDEX one_assistant ON agents(level) WHERE level = 'assistant';
-- Deux Agents vivants ne portent pas le même nom.
CREATE UNIQUE INDEX agent_names ON agents(lower(name)) WHERE level = 'agent' AND state != 'retired';
";

const COLUMNS: &str = "id, level, parent, engine, model, workdir, name, scope, state, session_id, created_at, profile, node";

impl Registry {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.pragma_update(None, "foreign_keys", true)?;
        db.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        let found: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !(0..=SCHEMA_VERSION).contains(&found) {
            return Err(RegistryError::SchemaVersion {
                found,
                expected: SCHEMA_VERSION,
            });
        }
        if found < SCHEMA_VERSION {
            let tx = db.unchecked_transaction()?;
            for migration in &MIGRATIONS[found as usize..] {
                tx.execute_batch(migration)?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }
        Ok(Self { db })
    }

    /// L'Assistant, créé au premier appel avec le moteur et le modèle choisis par
    /// l'utilisateur (D21). S'il existe déjà, il est rendu tel quel.
    pub fn ensure_assistant(
        &self,
        engine: EngineKind,
        model: Option<String>,
        workdir: PathBuf,
    ) -> Result<AgentRecord> {
        if let Some(assistant) = self.assistant()? {
            return Ok(assistant);
        }
        let record = AgentRecord {
            id: new_id(),
            level: Level::Assistant,
            parent: None,
            engine,
            model,
            profile: None,
            node: None,
            workdir,
            charter: None,
            state: AgentState::Parked,
            session_id: None,
            created_at: now(),
        };
        self.insert(&record)?;
        Ok(record)
    }

    pub fn assistant(&self) -> Result<Option<AgentRecord>> {
        self.query_one("WHERE level = 'assistant'", [])
    }

    /// Crée un Agent sous l'Assistant ou un Worker sous un Agent. Un nouvel agent
    /// commence parké, sans session : il n'a encore jamais tourné.
    pub fn create(&self, new: NewAgent) -> Result<AgentRecord> {
        let parent = self.require(&new.parent)?;
        if parent.state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(format!(
                "{} est retiré",
                parent.id
            )));
        }
        if parent.level.child() != Some(new.level) {
            return Err(RegistryError::Hierarchy(format!(
                "un {} ne peut pas avoir un {} pour enfant",
                parent.level, new.level
            )));
        }
        let charter = match (new.level, new.charter) {
            (Level::Agent, Some(charter)) => Some(validated(charter)?),
            (Level::Agent, None) => {
                return Err(RegistryError::InvalidCharter(
                    "un Agent a forcément une charte".into(),
                ));
            }
            (_, Some(_)) => {
                return Err(RegistryError::InvalidCharter(
                    "seuls les Agents ont une charte".into(),
                ));
            }
            (_, None) => None,
        };
        if let Some(charter) = &charter
            && self.agent_named(&charter.name)?.is_some()
        {
            return Err(RegistryError::DuplicateName(charter.name.clone()));
        }
        let record = AgentRecord {
            id: new_id(),
            level: new.level,
            parent: Some(parent.id),
            engine: new.engine,
            model: new.model,
            profile: new.profile,
            node: match new.level {
                Level::Worker => parent.node.clone(),
                _ => new.node,
            },
            workdir: new.workdir,
            charter,
            state: AgentState::Parked,
            session_id: None,
            created_at: now(),
        };
        self.insert(&record)?;
        Ok(record)
    }

    pub fn get(&self, id: &AgentId) -> Result<Option<AgentRecord>> {
        self.query_one("WHERE id = ?1", [id.as_str()])
    }

    /// L'Agent vivant qui porte ce nom, sans tenir compte de la casse.
    pub fn agent_named(&self, name: &str) -> Result<Option<AgentRecord>> {
        self.query_one(
            "WHERE level = 'agent' AND state != 'retired' AND lower(name) = lower(?1)",
            [name.trim()],
        )
    }

    /// Enfants non retirés, du plus ancien au plus récent.
    pub fn children(&self, id: &AgentId) -> Result<Vec<AgentRecord>> {
        self.query(
            "WHERE parent = ?1 AND state != 'retired' ORDER BY created_at, rowid",
            [id.as_str()],
        )
    }

    /// Tous les agents non retirés : l'arbre courant.
    pub fn tree(&self) -> Result<Vec<AgentRecord>> {
        self.query("WHERE state != 'retired' ORDER BY created_at, rowid", [])
    }

    /// Descendants non retirés : ce qu'un stop sur cet agent doit atteindre (D14).
    pub fn descendants(&self, id: &AgentId) -> Result<Vec<AgentId>> {
        let mut statement = self.db.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT id FROM agents WHERE parent = ?1 AND state != 'retired'
                 UNION ALL
                 SELECT a.id FROM agents a JOIN sub ON a.parent = sub.id WHERE a.state != 'retired'
             )
             SELECT id FROM sub",
        )?;
        let ids = statement.query_map([id.as_str()], |row| row.get::<_, String>(0))?;
        Ok(ids
            .map(|id| id.map(AgentId::new))
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Passe un agent en actif ou en parké. Le retrait passe par [`Registry::retire`].
    pub fn set_state(&self, id: &AgentId, state: AgentState) -> Result<()> {
        if state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(
                "un retrait passe par retire, qui atteint aussi les enfants".into(),
            ));
        }
        self.update_living(id, "state = ?2", state.as_str())
    }

    /// Enregistre la session du moteur, pour reprendre l'agent après un parking.
    pub fn set_session(&self, id: &AgentId, session_id: &str) -> Result<()> {
        self.update_living(id, "session_id = ?2", session_id)
    }

    /// Change le moteur et le modèle d'un agent (repli de la politique de modèles).
    pub fn set_engine_model(
        &self,
        id: &AgentId,
        engine: EngineKind,
        model: Option<&str>,
    ) -> Result<()> {
        if self.require(id)?.state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(format!("{id} est retiré")));
        }
        self.db.execute(
            "UPDATE agents SET engine = ?2, model = ?3 WHERE id = ?1",
            params![id.as_str(), engine.as_str(), model],
        )?;
        Ok(())
    }

    /// Oublie la session du moteur : le prochain réveil en ouvre une neuve (compaction).
    pub fn clear_session(&self, id: &AgentId) -> Result<()> {
        self.require(id)?;
        self.db.execute(
            "UPDATE agents SET session_id = NULL WHERE id = ?1",
            [id.as_str()],
        )?;
        Ok(())
    }

    /// Retire un agent et tous ses descendants ; rend les identifiants retirés.
    pub fn retire(&self, id: &AgentId) -> Result<Vec<AgentId>> {
        let agent = self.require(id)?;
        if agent.level == Level::Assistant {
            return Err(RegistryError::Hierarchy(
                "l'Assistant ne se retire pas".into(),
            ));
        }
        if agent.state == AgentState::Retired {
            return Ok(vec![]);
        }
        let mut retired = vec![agent.id];
        retired.extend(self.descendants(id)?);
        let tx = self.db.unchecked_transaction()?;
        for id in &retired {
            tx.execute(
                "UPDATE agents SET state = 'retired' WHERE id = ?1",
                [id.as_str()],
            )?;
        }
        // Un Worker retiré en pleine tâche la laisse interrompue, à relancer.
        board::interrupt_tasks_of(&tx, &retired)?;
        triggers::drop_triggers_of(&tx, &retired)?;
        tx.commit()?;
        Ok(retired)
    }

    /// Au démarrage, aucun process n'a survécu : les agents qui tournaient sont parkés,
    /// les Workers qui tournaient sont retirés et leurs tâches interrompues (D10).
    pub fn rehydrate(&self) -> Result<Rehydration> {
        let tx = self.db.unchecked_transaction()?;
        let ids = |filter: &str| -> Result<Vec<AgentId>> {
            let mut statement = tx.prepare(&format!(
                "SELECT id FROM agents WHERE state = 'active' AND {filter} ORDER BY rowid"
            ))?;
            let ids = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(ids
                .map(|id| id.map(AgentId::new))
                .collect::<rusqlite::Result<_>>()?)
        };
        let report = Rehydration {
            parked: ids("level != 'worker'")?,
            lost_workers: ids("level = 'worker'")?,
        };
        tx.execute(
            "UPDATE agents SET state = 'parked' WHERE state = 'active' AND level != 'worker'",
            [],
        )?;
        tx.execute(
            "UPDATE agents SET state = 'retired' WHERE state = 'active' AND level = 'worker'",
            [],
        )?;
        board::interrupt_tasks_of(&tx, &report.lost_workers)?;
        tx.commit()?;
        Ok(report)
    }

    fn require(&self, id: &AgentId) -> Result<AgentRecord> {
        self.get(id)?
            .ok_or_else(|| RegistryError::NotFound(id.clone()))
    }

    /// Met à jour une colonne d'un agent non retiré : le retrait est définitif.
    fn update_living(&self, id: &AgentId, assignment: &str, value: &str) -> Result<()> {
        let agent = self.require(id)?;
        if agent.state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(format!("{id} est retiré")));
        }
        self.db.execute(
            &format!("UPDATE agents SET {assignment} WHERE id = ?1"),
            params![id.as_str(), value],
        )?;
        Ok(())
    }

    fn insert(&self, record: &AgentRecord) -> Result<()> {
        let charter = record.charter.as_ref();
        self.db.execute(
            &format!(
                "INSERT INTO agents ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ),
            params![
                record.id.as_str(),
                record.level.as_str(),
                record.parent.as_ref().map(AgentId::as_str),
                record.engine.as_str(),
                record.model,
                record.workdir.to_string_lossy(),
                charter.map(|charter| charter.name.as_str()),
                charter.map(|charter| charter.scope.as_str()),
                record.state.as_str(),
                record.session_id,
                record.created_at as i64,
                record.profile,
                record.node,
            ],
        )?;
        Ok(())
    }

    fn query(&self, filter: &str, params: impl rusqlite::Params) -> Result<Vec<AgentRecord>> {
        let mut statement = self
            .db
            .prepare(&format!("SELECT {COLUMNS} FROM agents {filter}"))?;
        let records = statement.query_map(params, record)?;
        Ok(records.collect::<rusqlite::Result<_>>()?)
    }

    fn query_one(
        &self,
        filter: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<AgentRecord>> {
        Ok(self.query(filter, params)?.into_iter().next())
    }
}

fn record(row: &Row) -> rusqlite::Result<AgentRecord> {
    let name: Option<String> = row.get(6)?;
    let scope: Option<String> = row.get(7)?;
    Ok(AgentRecord {
        id: AgentId::new(row.get::<_, String>(0)?),
        level: parsed(row, 1)?,
        parent: row.get::<_, Option<String>>(2)?.map(AgentId::new),
        engine: parsed(row, 3)?,
        model: row.get(4)?,
        workdir: PathBuf::from(row.get::<_, String>(5)?),
        charter: name.zip(scope).map(|(name, scope)| Charter { name, scope }),
        state: parsed(row, 8)?,
        session_id: row.get(9)?,
        created_at: row.get::<_, i64>(10)? as u64,
        profile: row.get(11)?,
        node: row.get(12)?,
    })
}

fn parsed<T>(row: &Row, index: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let text: String = row.get(index)?;
    text.parse().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn validated(charter: Charter) -> Result<Charter> {
    let charter = Charter {
        name: charter.name.trim().to_string(),
        scope: charter.scope.trim().to_string(),
    };
    if charter.name.is_empty() || charter.scope.is_empty() {
        return Err(RegistryError::InvalidCharter(
            "le nom et le périmètre sont obligatoires".into(),
        ));
    }
    Ok(charter)
}

fn new_id() -> AgentId {
    AgentId::new(Uuid::new_v4().to_string())
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

    fn registry() -> (Registry, AgentRecord) {
        let registry = Registry::open_in_memory().unwrap();
        let assistant = registry
            .ensure_assistant(EngineKind::Claude, Some("opus".into()), "/argos".into())
            .unwrap();
        (registry, assistant)
    }

    fn charter(name: &str) -> Option<Charter> {
        Some(Charter {
            name: name.into(),
            scope: format!("tout ce qui concerne {name}"),
        })
    }

    fn agent(registry: &Registry, assistant: &AgentRecord, name: &str) -> AgentRecord {
        registry
            .create(NewAgent {
                profile: None,
                node: None,
                parent: assistant.id.clone(),
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: None,
                workdir: format!("/projets/{name}").into(),
                charter: charter(name),
            })
            .unwrap()
    }

    fn worker(registry: &Registry, parent: &AgentRecord) -> AgentRecord {
        registry.create(new_worker(parent)).unwrap()
    }

    fn new_worker(parent: &AgentRecord) -> NewAgent {
        NewAgent {
            profile: None,
            node: None,
            parent: parent.id.clone(),
            level: Level::Worker,
            engine: EngineKind::Codex,
            model: Some("gpt-6-luna".into()),
            workdir: parent.workdir.clone(),
            charter: None,
        }
    }

    #[test]
    fn the_assistant_is_created_once() {
        let (registry, assistant) = registry();
        let again = registry
            .ensure_assistant(EngineKind::Codex, None, "/ailleurs".into())
            .unwrap();
        assert_eq!(again, assistant);
        assert_eq!(assistant.level, Level::Assistant);
        assert_eq!(assistant.parent, None);
        assert_eq!(assistant.state, AgentState::Parked);
    }

    #[test]
    fn agents_go_under_the_assistant_and_workers_under_agents() {
        let (registry, assistant) = registry();
        let domotique = agent(&registry, &assistant, "Domotique");
        let worker = worker(&registry, &domotique);

        assert_eq!(
            registry.get(&domotique.id).unwrap(),
            Some(domotique.clone())
        );
        assert_eq!(
            registry.children(&assistant.id).unwrap(),
            vec![domotique.clone()]
        );
        assert_eq!(
            registry.children(&domotique.id).unwrap(),
            vec![worker.clone()]
        );
        assert_eq!(worker.charter, None);
        assert_eq!(worker.session_id, None);
    }

    #[test]
    fn the_hierarchy_cannot_be_skipped_or_extended() {
        let (registry, assistant) = registry();
        let domotique = agent(&registry, &assistant, "Domotique");
        let worker = worker(&registry, &domotique);

        let mut under_assistant = new_worker(&domotique);
        under_assistant.parent = assistant.id.clone();
        let mut under_agent = new_worker(&domotique);
        under_agent.level = Level::Agent;
        under_agent.charter = charter("Imbriqué");

        for invalid in [under_assistant, under_agent, new_worker(&worker)] {
            assert!(matches!(
                registry.create(invalid),
                Err(RegistryError::Hierarchy(_))
            ));
        }
        let mut second_assistant = new_worker(&domotique);
        second_assistant.level = Level::Assistant;
        assert!(matches!(
            registry.create(second_assistant),
            Err(RegistryError::Hierarchy(_))
        ));
    }

    #[test]
    fn an_unknown_or_retired_parent_is_refused() {
        let (registry, assistant) = registry();
        let mut orphan = new_worker(&assistant);
        orphan.parent = AgentId::new("inconnu");
        assert!(matches!(
            registry.create(orphan),
            Err(RegistryError::NotFound(_))
        ));

        let domotique = agent(&registry, &assistant, "Domotique");
        registry.retire(&domotique.id).unwrap();
        assert!(matches!(
            registry.create(new_worker(&domotique)),
            Err(RegistryError::Hierarchy(_))
        ));
    }

    #[test]
    fn only_agents_have_a_charter_and_it_must_be_filled() {
        let (registry, assistant) = registry();
        let domotique = agent(&registry, &assistant, "Domotique");

        let mut worker_with_charter = new_worker(&domotique);
        worker_with_charter.charter = charter("Intrus");
        let mut agent_without = new_worker(&assistant);
        agent_without.level = Level::Agent;
        let mut blank = agent_without.clone();
        blank.charter = Some(Charter {
            name: "  ".into(),
            scope: "quelque chose".into(),
        });

        for invalid in [worker_with_charter, agent_without, blank] {
            assert!(matches!(
                registry.create(invalid),
                Err(RegistryError::InvalidCharter(_))
            ));
        }
    }

    #[test]
    fn living_agent_names_are_unique_regardless_of_case() {
        let (registry, assistant) = registry();
        let first = agent(&registry, &assistant, "Domotique");
        let mut clash = new_worker(&assistant);
        clash.level = Level::Agent;
        clash.charter = charter(" domotique ");
        assert!(matches!(
            registry.create(clash.clone()),
            Err(RegistryError::DuplicateName(_))
        ));
        assert_eq!(
            registry.agent_named("DOMOTIQUE").unwrap(),
            Some(first.clone())
        );

        registry.retire(&first.id).unwrap();
        let reborn = registry.create(clash).unwrap();
        assert_eq!(reborn.charter.unwrap().name, "domotique");
    }

    #[test]
    fn retiring_an_agent_retires_its_workers_but_not_the_assistant() {
        let (registry, assistant) = registry();
        let domotique = agent(&registry, &assistant, "Domotique");
        let compta = agent(&registry, &assistant, "Compta");
        let w1 = worker(&registry, &domotique);
        let w2 = worker(&registry, &domotique);

        assert_eq!(
            registry.descendants(&assistant.id).unwrap().len(),
            4,
            "deux Agents et deux Workers"
        );
        assert_eq!(
            registry.retire(&domotique.id).unwrap(),
            vec![domotique.id.clone(), w1.id.clone(), w2.id.clone()]
        );
        assert_eq!(
            registry.get(&w1.id).unwrap().unwrap().state,
            AgentState::Retired
        );
        assert_eq!(registry.tree().unwrap(), vec![assistant.clone(), compta]);
        assert_eq!(registry.retire(&domotique.id).unwrap(), vec![]);
        assert!(matches!(
            registry.retire(&assistant.id),
            Err(RegistryError::Hierarchy(_))
        ));
    }

    #[test]
    fn state_and_session_change_until_retirement() {
        let (registry, assistant) = registry();
        let domotique = agent(&registry, &assistant, "Domotique");

        registry
            .set_state(&domotique.id, AgentState::Active)
            .unwrap();
        registry.set_session(&domotique.id, "session-1").unwrap();
        let stored = registry.get(&domotique.id).unwrap().unwrap();
        assert_eq!(
            (stored.state, stored.session_id.as_deref()),
            (AgentState::Active, Some("session-1"))
        );

        assert!(matches!(
            registry.set_state(&domotique.id, AgentState::Retired),
            Err(RegistryError::Hierarchy(_))
        ));
        registry.retire(&domotique.id).unwrap();
        assert!(matches!(
            registry.set_state(&domotique.id, AgentState::Active),
            Err(RegistryError::Hierarchy(_))
        ));
        assert!(matches!(
            registry.set_session(&AgentId::new("inconnu"), "x"),
            Err(RegistryError::NotFound(_))
        ));
    }

    #[test]
    fn rehydration_parks_agents_and_drops_running_workers() {
        let (registry, assistant) = registry();
        let domotique = agent(&registry, &assistant, "Domotique");
        let compta = agent(&registry, &assistant, "Compta");
        let running = worker(&registry, &domotique);
        let parked = worker(&registry, &domotique);
        for id in [&assistant.id, &domotique.id, &running.id] {
            registry.set_state(id, AgentState::Active).unwrap();
        }

        let report = registry.rehydrate().unwrap();

        assert_eq!(
            report.parked,
            vec![assistant.id.clone(), domotique.id.clone()]
        );
        assert_eq!(report.lost_workers, vec![running.id.clone()]);
        let state = |id: &AgentId| registry.get(id).unwrap().unwrap().state;
        assert_eq!(state(&assistant.id), AgentState::Parked);
        assert_eq!(state(&domotique.id), AgentState::Parked);
        assert_eq!(state(&compta.id), AgentState::Parked);
        assert_eq!(state(&running.id), AgentState::Retired);
        assert_eq!(state(&parked.id), AgentState::Parked);
        assert_eq!(registry.rehydrate().unwrap(), Rehydration::default());
    }

    #[test]
    fn the_registry_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("argos.db");
        let (assistant, domotique) = {
            let registry = Registry::open(&path).unwrap();
            let assistant = registry
                .ensure_assistant(EngineKind::Claude, None, "/argos".into())
                .unwrap();
            let domotique = agent(&registry, &assistant, "Domotique");
            registry.set_session(&domotique.id, "session-1").unwrap();
            (assistant, domotique)
        };

        let reopened = Registry::open(&path).unwrap();
        assert_eq!(reopened.assistant().unwrap(), Some(assistant));
        let stored = reopened.get(&domotique.id).unwrap().unwrap();
        assert_eq!(stored.session_id.as_deref(), Some("session-1"));
        assert_eq!(stored.charter, domotique.charter);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("argos.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(
            Registry::open(&path),
            Err(RegistryError::SchemaVersion { found: 99, .. })
        ));
    }
}
