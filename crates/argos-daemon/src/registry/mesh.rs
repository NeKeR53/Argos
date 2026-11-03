//! Le registre dans un maillage de nœuds pairs (D48). Chaque nœud détient ses agents
//! (`node` vide) et garde une copie d'annuaire de ceux des autres (`node` = leur
//! détenteur). S'y ajoutent la file des messages qui attendent un pair injoignable, la
//! passation des fiches d'un ancien hub, et le paquet de tout ce que possède un agent qui
//! change de nœud.

use super::{Registry, RegistryError, Result, now, record};
use argos_core::{AgentId, AgentRecord, Level};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rusqlite::types::Value as Sql;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// v13 : file des messages pour un pair injoignable, et passation des fiches qu'un ancien
/// hub tenait pour des agents placés sur un nœud (D48, T5).
pub(super) const SCHEMA: &str = "
CREATE TABLE outbox (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    peer       TEXT NOT NULL,
    message    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX outbox_by_peer ON outbox(peer, seq);
ALTER TABLE agents ADD COLUMN handover INTEGER NOT NULL DEFAULT 0;
UPDATE agents SET handover = 1 WHERE node IS NOT NULL;
";

/// Ce que possède un agent, table par table : une ligne est une liste de colonnes et de
/// valeurs, pour voyager telle quelle d'un registre à l'autre. Les synchros (D44) n'en
/// sont pas : elles appartiennent au nœud de la copie locale.
const OWNED: [(&str, &str); 7] = [
    ("missions", "agent"),
    ("tasks", "agent"),
    ("triggers", "owner"),
    ("functions", "agent"),
    ("surfaces", "agent"),
    ("escalations", "agent"),
    ("agent_capabilities", "agent"),
];

/// Une ligne d'une table, colonne par colonne.
pub type Row = Vec<(String, Cell)>;

/// Une valeur SQLite sérialisable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(String),
}

impl From<Sql> for Cell {
    fn from(value: Sql) -> Self {
        match value {
            Sql::Null => Cell::Null,
            Sql::Integer(n) => Cell::Integer(n),
            Sql::Real(x) => Cell::Real(x),
            Sql::Text(s) => Cell::Text(s),
            Sql::Blob(b) => Cell::Blob(BASE64.encode(b)),
        }
    }
}

impl From<&Cell> for Sql {
    fn from(cell: &Cell) -> Self {
        match cell {
            Cell::Null => Sql::Null,
            Cell::Integer(n) => Sql::Integer(*n),
            Cell::Real(x) => Sql::Real(*x),
            Cell::Text(s) => Sql::Text(s.clone()),
            Cell::Blob(b) => Sql::Blob(BASE64.decode(b).unwrap_or_default()),
        }
    }
}

/// Les fiches d'un agent et de ses Workers, et les lignes de tout ce qu'ils possèdent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rows {
    pub agents: Vec<AgentRecord>,
    pub tables: Vec<(String, Vec<Row>)>,
}

impl Registry {
    /// Les agents détenus par ce nœud, retirés compris, parents d'abord : ce qu'il annonce
    /// à ses pairs.
    pub fn owned(&self) -> Result<Vec<AgentRecord>> {
        self.query(
            "WHERE node IS NULL ORDER BY CASE level WHEN 'assistant' THEN 0 WHEN 'agent' THEN 1 ELSE 2 END, created_at, rowid",
            [],
        )
    }

    /// Met à jour la copie d'annuaire d'un agent détenu par `owner`. Un agent détenu ici
    /// n'est jamais remplacé par l'annonce d'un pair : il change de détenteur par un
    /// déplacement. Rend `false` si l'annonce est ignorée.
    pub fn upsert_replica(&self, record: &AgentRecord, owner: &str) -> Result<bool> {
        if let Some(local) = self.get(&record.id)?
            && local.node.is_none()
        {
            return Ok(false);
        }
        if record.level == Level::Assistant
            && let Some(assistant) = self.assistant()?
            && assistant.id != record.id
        {
            return Err(RegistryError::Hierarchy(format!(
                "le nœud {owner} annonce un autre Assistant ({})",
                record.id
            )));
        }
        let charter = record.charter.as_ref();
        self.db.execute(
            "INSERT INTO agents (id, level, parent, engine, model, workdir, name, scope, state, session_id, created_at, profile, node)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET level = excluded.level, parent = excluded.parent,
                 engine = excluded.engine, model = excluded.model, workdir = excluded.workdir,
                 name = excluded.name, scope = excluded.scope, state = excluded.state,
                 session_id = NULL, profile = excluded.profile, node = excluded.node, handover = 0",
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
                record.created_at as i64,
                record.profile,
                owner,
            ],
        )?;
        Ok(true)
    }

    /// Fiches qu'un ancien hub tient encore pour `node` : à lui transmettre (T5).
    pub fn handovers(&self, node: &str) -> Result<Vec<AgentId>> {
        Ok(self
            .query(
                "WHERE handover = 1 AND node = ?1 AND level = 'agent' ORDER BY created_at, rowid",
                [node],
            )?
            .into_iter()
            .map(|record| record.id)
            .collect())
    }

    /// Marque une fiche comme tenue pour un nœud par un ancien hub (migration v13, tests).
    pub fn mark_handover(&self, id: &AgentId) -> Result<()> {
        self.db.execute(
            "UPDATE agents SET handover = 1 WHERE id = ?1",
            [id.as_str()],
        )?;
        Ok(())
    }

    pub fn handed_over(&self, id: &AgentId) -> Result<()> {
        self.db.execute(
            "UPDATE agents SET handover = 0 WHERE id = ?1 OR parent = ?1",
            [id.as_str()],
        )?;
        Ok(())
    }

    /// Tout ce que possèdent un agent et ses Workers, pour un déplacement.
    pub fn export_rows(&self, id: &AgentId) -> Result<Rows> {
        let agent = self.require(id)?;
        let mut agents = vec![agent];
        agents.extend(self.query(
            "WHERE parent = ?1 AND level = 'worker' ORDER BY created_at, rowid",
            [id.as_str()],
        )?);
        let ids: Vec<String> = agents.iter().map(|a| a.id.as_str().to_string()).collect();
        let mut tables = vec![];
        for (table, column) in OWNED {
            let mut rows = vec![];
            for owner in &ids {
                let mut statement = self
                    .db
                    .prepare(&format!("SELECT * FROM {table} WHERE {column} = ?1"))?;
                let names: Vec<String> = statement.column_names().map(String::from).collect();
                let found = statement.query_map([owner], |row| {
                    (0..names.len())
                        .map(|i| Ok((names[i].clone(), Cell::from(row.get::<_, Sql>(i)?))))
                        .collect::<rusqlite::Result<Row>>()
                })?;
                rows.extend(found.collect::<rusqlite::Result<Vec<Row>>>()?);
            }
            tables.push((table.to_string(), rows));
        }
        Ok(Rows { agents, tables })
    }

    /// Installe un agent qui arrive : ses fiches deviennent celles d'agents détenus ici,
    /// ses lignes sont reprises telles quelles. Une ligne qui vise un agent inconnu ici
    /// (un déclencheur vers un agent d'un nœud jamais vu) est laissée de côté.
    pub fn import_rows(&self, rows: &Rows, workdir: &std::path::Path) -> Result<Vec<String>> {
        let tx = self.db.unchecked_transaction()?;
        let mut skipped = vec![];
        for (index, agent) in rows.agents.iter().enumerate() {
            let workdir = if index == 0 {
                workdir.to_path_buf()
            } else {
                agent.workdir.clone()
            };
            let charter = agent.charter.as_ref();
            // Un Worker retiré garde le dossier de son Agent.
            tx.execute(
                "INSERT INTO agents (id, level, parent, engine, model, workdir, name, scope, state, session_id, created_at, profile, node)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, NULL)
                 ON CONFLICT(id) DO UPDATE SET level = excluded.level, parent = excluded.parent,
                     engine = excluded.engine, model = excluded.model, workdir = excluded.workdir,
                     name = excluded.name, scope = excluded.scope, state = excluded.state,
                     session_id = NULL, profile = excluded.profile, node = NULL, handover = 0",
                params![
                    agent.id.as_str(),
                    agent.level.as_str(),
                    agent.parent.as_ref().map(AgentId::as_str),
                    agent.engine.as_str(),
                    agent.model,
                    workdir.to_string_lossy(),
                    charter.map(|charter| charter.name.as_str()),
                    charter.map(|charter| charter.scope.as_str()),
                    agent.state.as_str(),
                    agent.created_at as i64,
                    agent.profile,
                ],
            )?;
        }
        for (table, lines) in &rows.tables {
            if !OWNED.iter().any(|(known, _)| known == table) {
                continue;
            }
            for line in lines {
                let columns: Vec<&str> = line.iter().map(|(name, _)| name.as_str()).collect();
                let marks: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
                let values: Vec<Sql> = line.iter().map(|(_, cell)| Sql::from(cell)).collect();
                tx.execute("SAVEPOINT line", [])?;
                let inserted = tx.execute(
                    &format!(
                        "INSERT OR REPLACE INTO {table} ({}) VALUES ({})",
                        columns.join(", "),
                        marks.join(", ")
                    ),
                    rusqlite::params_from_iter(values.iter()),
                );
                match inserted {
                    Ok(_) => tx.execute("RELEASE line", [])?,
                    Err(error) => {
                        tx.execute("ROLLBACK TO line", [])?;
                        tx.execute("RELEASE line", [])?;
                        skipped.push(format!("{table} : {error}"));
                        0
                    }
                };
            }
        }
        tx.commit()?;
        Ok(skipped)
    }

    /// Après un déplacement : l'agent et ses Workers deviennent des copies d'annuaire de
    /// leur nouveau détenteur, et ce nœud oublie tout ce qu'ils possédaient.
    pub fn release(&self, id: &AgentId, owner: &str) -> Result<()> {
        let rows = self.export_rows(id)?;
        let tx = self.db.unchecked_transaction()?;
        // Dans l'ordre inverse des références : les tâches avant les missions.
        for (table, column) in OWNED.iter().rev() {
            for agent in &rows.agents {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE {column} = ?1"),
                    [agent.id.as_str()],
                )?;
            }
        }
        for agent in &rows.agents {
            tx.execute(
                "UPDATE agents SET node = ?2, session_id = NULL, handover = 0 WHERE id = ?1",
                params![agent.id.as_str(), owner],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Met un message en file pour un pair injoignable ; rend son numéro.
    pub fn enqueue(&self, peer: &str, message: &Value) -> Result<i64> {
        self.db.execute(
            "INSERT INTO outbox (peer, message, created_at) VALUES (?1, ?2, ?3)",
            params![peer, message.to_string(), now() as i64],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Le plus ancien message en attente pour ce pair.
    pub fn next_queued(&self, peer: &str) -> Result<Option<(i64, Value)>> {
        let found: Option<(i64, String)> = self
            .db
            .query_row(
                "SELECT seq, message FROM outbox WHERE peer = ?1 ORDER BY seq LIMIT 1",
                [peer],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(found.and_then(|(seq, text)| Some((seq, serde_json::from_str(&text).ok()?))))
    }

    pub fn dequeue(&self, seq: i64) -> Result<()> {
        self.db
            .execute("DELETE FROM outbox WHERE seq = ?1", [seq])?;
        Ok(())
    }

    /// Messages en attente, par pair.
    pub fn queued(&self) -> Result<Vec<(String, usize)>> {
        let mut statement = self
            .db
            .prepare("SELECT peer, COUNT(*) FROM outbox GROUP BY peer ORDER BY peer")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Une fiche telle qu'enregistrée, pour l'annoncer.
    pub fn announced(&self, id: &AgentId) -> Result<Option<AgentRecord>> {
        Ok(self
            .db
            .query_row(
                &format!("SELECT {} FROM agents WHERE id = ?1", super::COLUMNS),
                [id.as_str()],
                record,
            )
            .optional()?)
    }
}
