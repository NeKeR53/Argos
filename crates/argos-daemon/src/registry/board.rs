//! Boards des Agents : missions confiées par l'Assistant et tâches qui les découpent
//! (D17–D19). L'ordre de priorité est explicite et les transitions sont tenues par le
//! daemon : un Worker ne part que sur une tâche du board.

use super::{Registry, RegistryError, Result, now, parsed};
use argos_core::{AgentId, AgentState, Board, Level, Mission, MissionState, Task, TaskState};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

pub(super) const SCHEMA: &str = "
CREATE TABLE missions (
    id         TEXT PRIMARY KEY,
    agent      TEXT NOT NULL REFERENCES agents(id),
    text       TEXT NOT NULL,
    state      TEXT NOT NULL CHECK (state IN ('open', 'done', 'cancelled')),
    priority   INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE tasks (
    id         TEXT PRIMARY KEY,
    agent      TEXT NOT NULL REFERENCES agents(id),
    mission    TEXT REFERENCES missions(id),
    title      TEXT NOT NULL,
    detail     TEXT,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'in_progress', 'blocked', 'done', 'cancelled', 'interrupted')),
    priority   INTEGER NOT NULL,
    worker     TEXT REFERENCES agents(id),
    result     TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX missions_by_agent ON missions(agent);
CREATE INDEX tasks_by_agent ON tasks(agent);
CREATE INDEX tasks_by_worker ON tasks(worker);
";

const MISSION_COLUMNS: &str = "id, agent, text, state, created_at";
const TASK_COLUMNS: [&str; 10] = [
    "id",
    "agent",
    "mission",
    "title",
    "detail",
    "state",
    "worker",
    "result",
    "created_at",
    "updated_at",
];

/// Place dans un ordre de priorité.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Top,
    Bottom,
    /// Rang, à partir de 1.
    At(usize),
}

impl Position {
    /// `top`, `bottom` ou un rang (`1` = en tête).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "top" => Some(Position::Top),
            "bottom" => Some(Position::Bottom),
            rank => rank
                .parse::<usize>()
                .ok()
                .filter(|rank| *rank >= 1)
                .map(Position::At),
        }
    }
}

/// Modification d'une tâche ; les champs absents restent tels quels.
#[derive(Debug, Clone, Default)]
pub struct TaskUpdate {
    pub state: Option<TaskState>,
    pub title: Option<String>,
    pub detail: Option<String>,
    pub result: Option<String>,
}

impl Registry {
    /// Confie une mission à un Agent ; elle passe après ses missions en cours.
    pub fn add_mission(&self, agent: &AgentId, text: &str) -> Result<Mission> {
        self.require_board(agent)?;
        let text = non_empty(text, "une mission a un texte")?;
        let mission = Mission {
            id: short_id("m"),
            agent: agent.clone(),
            text,
            state: MissionState::Open,
            created_at: now(),
        };
        self.db.execute(
            "INSERT INTO missions (id, agent, text, state, priority, created_at)
             VALUES (?1, ?2, ?3, 'open', ?4, ?5)",
            params![
                mission.id,
                agent.as_str(),
                mission.text,
                self.next_priority("missions", agent)?,
                mission.created_at as i64
            ],
        )?;
        Ok(mission)
    }

    pub fn mission(&self, id: &str) -> Result<Mission> {
        self.db
            .query_row(
                &format!("SELECT {MISSION_COLUMNS} FROM missions WHERE id = ?1"),
                [id],
                mission_row,
            )
            .optional()?
            .ok_or_else(|| RegistryError::MissionNotFound(id.into()))
    }

    /// Clôt ou annule une mission ouverte. Annuler annule aussi ses tâches non
    /// terminées ; rend alors les Workers à arrêter (ceux des tâches en cours).
    pub fn set_mission_state(&self, id: &str, state: MissionState) -> Result<Vec<AgentId>> {
        let mission = self.mission(id)?;
        if mission.state != MissionState::Open {
            return Err(RegistryError::Board(format!(
                "la mission {id} est déjà {}",
                mission.state
            )));
        }
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "UPDATE missions SET state = ?2 WHERE id = ?1",
            params![id, state.as_str()],
        )?;
        let mut workers = vec![];
        if state == MissionState::Cancelled {
            let mut statement = tx.prepare(
                "SELECT worker FROM tasks
                 WHERE mission = ?1 AND state = 'in_progress' AND worker IS NOT NULL",
            )?;
            workers = statement
                .query_map([id], |row| row.get::<_, String>(0).map(AgentId::new))?
                .collect::<rusqlite::Result<_>>()?;
            drop(statement);
            tx.execute(
                "UPDATE tasks SET state = 'cancelled', updated_at = ?2
                 WHERE mission = ?1 AND state NOT IN ('done', 'cancelled')",
                params![id, now() as i64],
            )?;
        }
        tx.commit()?;
        Ok(workers)
    }

    pub fn reposition_mission(&self, id: &str, position: Position) -> Result<()> {
        let mission = self.mission(id)?;
        self.reposition("missions", &mission.agent, id, position)
    }

    /// Inscrit une tâche à faire sur le board d'un Agent, rattachée ou non à une mission.
    pub fn add_task(
        &self,
        agent: &AgentId,
        title: &str,
        detail: Option<&str>,
        mission: Option<&str>,
    ) -> Result<Task> {
        self.require_board(agent)?;
        if let Some(mission) = mission {
            let mission = self.mission(mission)?;
            if mission.agent != *agent {
                return Err(RegistryError::Board(format!(
                    "la mission {} n'est pas sur ton board",
                    mission.id
                )));
            }
            if mission.state != MissionState::Open {
                return Err(RegistryError::Board(format!(
                    "la mission {} est {}",
                    mission.id, mission.state
                )));
            }
        }
        let id = short_id("t");
        let stamp = now() as i64;
        self.db.execute(
            "INSERT INTO tasks (id, agent, mission, title, detail, state, priority, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?7)",
            params![
                id,
                agent.as_str(),
                mission,
                non_empty(title, "une tâche a un titre")?,
                detail.map(str::trim).filter(|detail| !detail.is_empty()),
                self.next_priority("tasks", agent)?,
                stamp
            ],
        )?;
        self.task(&id)
    }

    pub fn task(&self, id: &str) -> Result<Task> {
        self.db
            .query_row(
                &format!(
                    "SELECT {} FROM tasks WHERE id = ?1",
                    TASK_COLUMNS.join(", ")
                ),
                [id],
                task_row,
            )
            .optional()?
            .ok_or_else(|| RegistryError::TaskNotFound(id.into()))
    }

    pub fn update_task(&self, id: &str, update: TaskUpdate) -> Result<Task> {
        let task = self.task(id)?;
        let title = match update.title {
            Some(title) => non_empty(&title, "une tâche a un titre")?,
            None => task.title,
        };
        self.db.execute(
            "UPDATE tasks SET state = ?2, title = ?3, detail = ?4, result = ?5, updated_at = ?6
             WHERE id = ?1",
            params![
                id,
                update.state.unwrap_or(task.state).as_str(),
                title,
                update.detail.or(task.detail),
                update.result.or(task.result),
                now() as i64
            ],
        )?;
        self.task(id)
    }

    /// Confie une tâche à un Worker. Elle doit être à faire, bloquée ou interrompue.
    pub fn assign_task(&self, id: &str, worker: &AgentId) -> Result<Task> {
        let task = self.task(id)?;
        if !matches!(
            task.state,
            TaskState::Pending | TaskState::Blocked | TaskState::Interrupted
        ) {
            return Err(RegistryError::Board(format!(
                "la tâche {id} est {}",
                task.state
            )));
        }
        self.db.execute(
            "UPDATE tasks SET state = 'in_progress', worker = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, worker.as_str(), now() as i64],
        )?;
        self.task(id)
    }

    /// La tâche confiée en dernier à ce Worker.
    pub fn task_of_worker(&self, worker: &AgentId) -> Result<Option<Task>> {
        Ok(self
            .db
            .query_row(
                &format!(
                    "SELECT {} FROM tasks WHERE worker = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT 1",
                    TASK_COLUMNS.join(", ")
                ),
                [worker.as_str()],
                task_row,
            )
            .optional()?)
    }

    pub fn reposition_task(&self, id: &str, position: Position) -> Result<()> {
        let task = self.task(id)?;
        self.reposition("tasks", &task.agent, id, position)
    }

    /// Le board d'un Agent, dans l'ordre de priorité.
    pub fn board(&self, agent: &AgentId) -> Result<Board> {
        self.require_board(agent)?;
        let mut statement = self.db.prepare(&format!(
            "SELECT {MISSION_COLUMNS} FROM missions
             WHERE agent = ?1 AND state = 'open' ORDER BY priority, rowid"
        ))?;
        let missions = statement
            .query_map([agent.as_str()], mission_row)?
            .collect::<rusqlite::Result<_>>()?;
        let columns: Vec<String> = TASK_COLUMNS.iter().map(|c| format!("t.{c}")).collect();
        let mut statement = self.db.prepare(&format!(
            "SELECT {} FROM tasks t LEFT JOIN missions m ON t.mission = m.id
             WHERE t.agent = ?1
               AND (m.state = 'open' OR (t.mission IS NULL AND t.state NOT IN ('done', 'cancelled')))
             ORDER BY m.priority IS NULL, m.priority, t.priority, t.rowid",
            columns.join(", ")
        ))?;
        let tasks = statement
            .query_map([agent.as_str()], task_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Board {
            agent: agent.clone(),
            missions,
            tasks,
        })
    }

    /// Seuls les Agents vivants ont un board.
    fn require_board(&self, agent: &AgentId) -> Result<()> {
        let record = self.require(agent)?;
        if record.level != Level::Agent {
            return Err(RegistryError::Board(format!(
                "seuls les Agents ont un board, pas un {}",
                record.level
            )));
        }
        if record.state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(format!("{agent} est retiré")));
        }
        Ok(())
    }

    fn next_priority(&self, table: &str, agent: &AgentId) -> Result<i64> {
        Ok(self.db.query_row(
            &format!("SELECT COALESCE(MAX(priority), 0) + 1 FROM {table} WHERE agent = ?1"),
            [agent.as_str()],
            |row| row.get(0),
        )?)
    }

    /// Réécrit l'ordre de `table` pour cet Agent, avec `id` à la position voulue.
    fn reposition(&self, table: &str, agent: &AgentId, id: &str, position: Position) -> Result<()> {
        let mut statement = self.db.prepare(&format!(
            "SELECT id FROM {table} WHERE agent = ?1 ORDER BY priority, rowid"
        ))?;
        let mut ids: Vec<String> = statement
            .query_map([agent.as_str()], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        drop(statement);
        let current = ids
            .iter()
            .position(|other| other == id)
            .ok_or_else(|| RegistryError::Board(format!("{id} introuvable sur le board")))?;
        let moved = ids.remove(current);
        let index = match position {
            Position::Top => 0,
            Position::Bottom => ids.len(),
            Position::At(rank) => (rank - 1).min(ids.len()),
        };
        ids.insert(index, moved);
        let tx = self.db.unchecked_transaction()?;
        for (rank, id) in ids.iter().enumerate() {
            tx.execute(
                &format!("UPDATE {table} SET priority = ?2 WHERE id = ?1"),
                params![id, rank as i64 + 1],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Passe en `interrupted` les tâches en cours de ces Workers (crash, retrait).
pub(super) fn interrupt_tasks_of(db: &Connection, workers: &[AgentId]) -> Result<()> {
    for worker in workers {
        db.execute(
            "UPDATE tasks SET state = 'interrupted', updated_at = ?2
             WHERE worker = ?1 AND state = 'in_progress'",
            params![worker.as_str(), now() as i64],
        )?;
    }
    Ok(())
}

fn mission_row(row: &Row) -> rusqlite::Result<Mission> {
    Ok(Mission {
        id: row.get(0)?,
        agent: AgentId::new(row.get::<_, String>(1)?),
        text: row.get(2)?,
        state: parsed(row, 3)?,
        created_at: row.get::<_, i64>(4)? as u64,
    })
}

fn task_row(row: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        agent: AgentId::new(row.get::<_, String>(1)?),
        mission: row.get(2)?,
        title: row.get(3)?,
        detail: row.get(4)?,
        state: parsed(row, 5)?,
        worker: row.get::<_, Option<String>>(6)?.map(AgentId::new),
        result: row.get(7)?,
        created_at: row.get::<_, i64>(8)? as u64,
        updated_at: row.get::<_, i64>(9)? as u64,
    })
}

fn non_empty(text: &str, refusal: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(RegistryError::Board(refusal.into()));
    }
    Ok(text.into())
}

/// Identifiant court et lisible (`m-1a2b3c4d`, `t-…`) : les agents le recopient.
fn short_id(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

#[cfg(test)]
mod tests {
    use super::super::NewAgent;
    use super::*;
    use argos_core::{AgentRecord, Charter, EngineKind};

    struct Setup {
        registry: Registry,
        assistant: AgentRecord,
        agent: AgentRecord,
    }

    fn setup() -> Setup {
        let registry = Registry::open_in_memory().unwrap();
        let assistant = registry
            .ensure_assistant(EngineKind::Claude, None, "/argos".into())
            .unwrap();
        let agent = new_agent(&registry, &assistant, "Domotique");
        Setup {
            registry,
            assistant,
            agent,
        }
    }

    fn new_agent(registry: &Registry, assistant: &AgentRecord, name: &str) -> AgentRecord {
        registry
            .create(NewAgent {
                profile: None,
                node: None,
                parent: assistant.id.clone(),
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: None,
                workdir: "/projets".into(),
                charter: Some(Charter {
                    name: name.into(),
                    scope: "un périmètre".into(),
                }),
            })
            .unwrap()
    }

    fn worker(registry: &Registry, agent: &AgentRecord) -> AgentRecord {
        registry
            .create(NewAgent {
                profile: None,
                node: None,
                parent: agent.id.clone(),
                level: Level::Worker,
                engine: EngineKind::Codex,
                model: None,
                workdir: "/projets".into(),
                charter: None,
            })
            .unwrap()
    }

    fn titles(board: &Board) -> Vec<&str> {
        board.tasks.iter().map(|task| task.title.as_str()).collect()
    }

    #[test]
    fn missions_and_tasks_follow_their_priority() {
        let s = setup();
        let first = s.registry.add_mission(&s.agent.id, "éclairage").unwrap();
        let second = s.registry.add_mission(&s.agent.id, "chauffage").unwrap();
        s.registry
            .add_task(&s.agent.id, "ampoules", None, Some(&first.id))
            .unwrap();
        s.registry
            .add_task(&s.agent.id, "radiateurs", None, Some(&second.id))
            .unwrap();
        s.registry
            .add_task(&s.agent.id, "ménage", None, None)
            .unwrap();
        assert_eq!(
            titles(&s.registry.board(&s.agent.id).unwrap()),
            vec!["ampoules", "radiateurs", "ménage"]
        );

        // Réprioriser une mission réordonne ses tâches avec elle.
        s.registry
            .reposition_mission(&second.id, Position::Top)
            .unwrap();
        let board = s.registry.board(&s.agent.id).unwrap();
        assert_eq!(board.missions[0].id, second.id);
        assert_eq!(titles(&board), vec!["radiateurs", "ampoules", "ménage"]);
    }

    #[test]
    fn tasks_can_be_reordered_within_the_board() {
        let s = setup();
        for title in ["a", "b", "c"] {
            s.registry.add_task(&s.agent.id, title, None, None).unwrap();
        }
        let c = s.registry.board(&s.agent.id).unwrap().tasks[2].id.clone();
        s.registry.reposition_task(&c, Position::At(1)).unwrap();
        assert_eq!(
            titles(&s.registry.board(&s.agent.id).unwrap()),
            vec!["c", "a", "b"]
        );
        s.registry.reposition_task(&c, Position::Bottom).unwrap();
        assert_eq!(
            titles(&s.registry.board(&s.agent.id).unwrap()),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn only_agents_have_a_board_and_missions_stay_on_their_own() {
        let s = setup();
        let compta = new_agent(&s.registry, &s.assistant, "Compta");
        let theirs = s.registry.add_mission(&compta.id, "factures").unwrap();
        assert!(matches!(
            s.registry
                .add_task(&s.agent.id, "x", None, Some(&theirs.id)),
            Err(RegistryError::Board(_))
        ));
        assert!(matches!(
            s.registry.add_mission(&s.assistant.id, "x"),
            Err(RegistryError::Board(_))
        ));
        assert!(matches!(
            s.registry.add_task(&s.agent.id, "  ", None, None),
            Err(RegistryError::Board(_))
        ));
    }

    #[test]
    fn a_task_goes_to_a_worker_only_when_there_is_something_to_do() {
        let s = setup();
        let w = worker(&s.registry, &s.agent);
        let task = s
            .registry
            .add_task(&s.agent.id, "ampoules", Some("3 pièces"), None)
            .unwrap();
        let assigned = s.registry.assign_task(&task.id, &w.id).unwrap();
        assert_eq!(
            (assigned.state, assigned.worker.as_ref()),
            (TaskState::InProgress, Some(&w.id))
        );
        assert_eq!(
            s.registry.task_of_worker(&w.id).unwrap().unwrap().id,
            task.id
        );

        assert!(matches!(
            s.registry.assign_task(&task.id, &w.id),
            Err(RegistryError::Board(_))
        ));
        let done = s
            .registry
            .update_task(
                &task.id,
                TaskUpdate {
                    state: Some(TaskState::Done),
                    result: Some("3 ampoules changées".into()),
                    ..TaskUpdate::default()
                },
            )
            .unwrap();
        assert_eq!(done.result.as_deref(), Some("3 ampoules changées"));
        assert_eq!(done.detail.as_deref(), Some("3 pièces"));
        assert!(matches!(
            s.registry.assign_task(&task.id, &w.id),
            Err(RegistryError::Board(_))
        ));
    }

    #[test]
    fn cancelling_a_mission_cancels_its_open_tasks_and_names_its_workers() {
        let s = setup();
        let mission = s.registry.add_mission(&s.agent.id, "éclairage").unwrap();
        let running = s
            .registry
            .add_task(&s.agent.id, "salon", None, Some(&mission.id))
            .unwrap();
        let pending = s
            .registry
            .add_task(&s.agent.id, "cuisine", None, Some(&mission.id))
            .unwrap();
        let w = worker(&s.registry, &s.agent);
        s.registry.assign_task(&running.id, &w.id).unwrap();

        let workers = s
            .registry
            .set_mission_state(&mission.id, MissionState::Cancelled)
            .unwrap();

        assert_eq!(workers, vec![w.id.clone()]);
        for id in [&running.id, &pending.id] {
            assert_eq!(s.registry.task(id).unwrap().state, TaskState::Cancelled);
        }
        assert!(s.registry.board(&s.agent.id).unwrap().missions.is_empty());
        assert!(matches!(
            s.registry
                .set_mission_state(&mission.id, MissionState::Done),
            Err(RegistryError::Board(_))
        ));
    }

    #[test]
    fn a_vanished_worker_leaves_its_task_interrupted() {
        let s = setup();
        let retired = worker(&s.registry, &s.agent);
        let crashed = worker(&s.registry, &s.agent);
        let first = s.registry.add_task(&s.agent.id, "un", None, None).unwrap();
        let second = s
            .registry
            .add_task(&s.agent.id, "deux", None, None)
            .unwrap();
        s.registry.assign_task(&first.id, &retired.id).unwrap();
        s.registry.assign_task(&second.id, &crashed.id).unwrap();

        s.registry.retire(&retired.id).unwrap();
        s.registry
            .set_state(&crashed.id, AgentState::Active)
            .unwrap();
        s.registry.rehydrate().unwrap();

        for id in [&first.id, &second.id] {
            assert_eq!(s.registry.task(id).unwrap().state, TaskState::Interrupted);
        }
        // Une tâche interrompue repart avec un nouveau Worker.
        let fresh = worker(&s.registry, &s.agent);
        assert_eq!(
            s.registry.assign_task(&first.id, &fresh.id).unwrap().state,
            TaskState::InProgress
        );
    }

    #[test]
    fn a_version_one_database_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("argos.db");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(super::super::AGENTS_SCHEMA).unwrap();
            db.pragma_update(None, "user_version", 1).unwrap();
        }
        let registry = Registry::open(&path).unwrap();
        let assistant = registry
            .ensure_assistant(EngineKind::Claude, None, "/a".into())
            .unwrap();
        let agent = new_agent(&registry, &assistant, "Domotique");
        assert!(registry.add_mission(&agent.id, "éclairage").is_ok());
    }

    #[test]
    fn positions_parse_from_text() {
        assert_eq!(Position::parse("top"), Some(Position::Top));
        assert_eq!(Position::parse(" 2 "), Some(Position::At(2)));
        assert_eq!(Position::parse("0"), None);
        assert_eq!(Position::parse("milieu"), None);
    }
}
