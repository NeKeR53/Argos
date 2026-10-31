//! Synchros de workspace déclarées (D44) : un Agent placé sur un nœud et un dossier du
//! hub. L'état commun de chaque synchro vit à côté, dans `$ARGOS_HOME/sync/<id>/`.

use super::{Registry, RegistryError, Result, now};
use argos_core::AgentId;
use rusqlite::{OptionalExtension, Row, params};
use std::path::PathBuf;
use uuid::Uuid;

pub(super) const SCHEMA: &str = "
CREATE TABLE syncs (
    id         TEXT PRIMARY KEY,
    agent      TEXT NOT NULL REFERENCES agents(id),
    local      TEXT NOT NULL UNIQUE,
    excludes   TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
";

const COLUMNS: &str = "id, agent, local, excludes, created_at";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRecord {
    pub id: String,
    pub agent: AgentId,
    /// Copie locale, chemin absolu sur le hub.
    pub local: PathBuf,
    /// Exclusions propres à la synchro, en syntaxe `.gitignore`.
    pub excludes: Vec<String>,
    pub created_at: u64,
}

impl Registry {
    pub fn add_sync(
        &self,
        agent: &AgentId,
        local: &std::path::Path,
        excludes: &[String],
    ) -> Result<SyncRecord> {
        self.require(agent)?;
        let record = SyncRecord {
            id: format!("s-{}", &Uuid::new_v4().simple().to_string()[..8]),
            agent: agent.clone(),
            local: local.to_path_buf(),
            excludes: excludes.to_vec(),
            created_at: now(),
        };
        self.db.execute(
            &format!("INSERT INTO syncs ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5)"),
            params![
                record.id,
                agent.as_str(),
                local.to_string_lossy(),
                serde_json::to_string(excludes).expect("liste sérialisable"),
                record.created_at as i64
            ],
        )?;
        Ok(record)
    }

    pub fn syncs(&self) -> Result<Vec<SyncRecord>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM syncs ORDER BY created_at, rowid"
        ))?;
        let all = statement
            .query_map([], sync_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(all)
    }

    pub fn sync(&self, id: &str) -> Result<SyncRecord> {
        self.db
            .query_row(
                &format!("SELECT {COLUMNS} FROM syncs WHERE id = ?1"),
                [id],
                sync_row,
            )
            .optional()?
            .ok_or_else(|| RegistryError::Sync(format!("synchro inconnue : {id}")))
    }

    pub fn has_sync(&self, agent: &AgentId) -> Result<bool> {
        Ok(self
            .db
            .query_row(
                "SELECT 1 FROM syncs WHERE agent = ?1 LIMIT 1",
                [agent.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn remove_sync(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM syncs WHERE id = ?1", [id])?;
        Ok(())
    }
}

fn sync_row(row: &Row) -> rusqlite::Result<SyncRecord> {
    let excludes: String = row.get(3)?;
    Ok(SyncRecord {
        id: row.get(0)?,
        agent: AgentId::new(row.get::<_, String>(1)?),
        local: PathBuf::from(row.get::<_, String>(2)?),
        excludes: serde_json::from_str(&excludes).unwrap_or_default(),
        created_at: row.get::<_, i64>(4)? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{AgentUpdate, NewAgent};
    use argos_core::{Charter, EngineKind, Level};

    #[test]
    fn a_synced_agent_keeps_its_workspace_until_the_sync_is_closed() {
        let registry = Registry::open_in_memory().unwrap();
        let assistant = registry
            .ensure_assistant(EngineKind::Claude, None, "/tmp/a".into())
            .unwrap();
        let agent = registry
            .create(NewAgent {
                level: Level::Agent,
                parent: assistant.id.clone(),
                charter: Some(Charter {
                    name: "Dev".into(),
                    scope: "projet".into(),
                }),
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                workdir: "/srv/projet".into(),
                node: Some("pi".into()),
            })
            .unwrap();
        let sync = registry
            .add_sync(&agent.id, "/Users/moi/projet".as_ref(), &["dist/".into()])
            .unwrap();
        assert_eq!(registry.syncs().unwrap(), std::slice::from_ref(&sync));
        assert!(
            registry
                .add_sync(&agent.id, "/Users/moi/projet".as_ref(), &[])
                .is_err(),
            "une copie locale par synchro"
        );
        let update = AgentUpdate {
            workdir: Some("/srv/autre".into()),
            ..AgentUpdate::default()
        };
        let refused = registry.configure_agent(&agent.id, &update).unwrap_err();
        assert!(refused.to_string().contains("synchro"));
        registry.remove_sync(&sync.id).unwrap();
        registry.configure_agent(&agent.id, &update).unwrap();
    }
}
