//! Surfaces des agents (D26–D28) : pages HTML produites par l'Assistant et les Agents,
//! gardées en base avec leur code. La GUI les héberge et les affiche ; le daemon n'en
//! sert jamais le contenu au réseau.

use super::{Registry, RegistryError, Result, now, parsed};
use argos_core::{AgentId, AgentState, Level, SurfaceDoc, SurfaceKind, SurfaceMeta};
use rusqlite::{OptionalExtension, Row, params};
use uuid::Uuid;

pub(super) const SCHEMA: &str = "
CREATE TABLE surfaces (
    id         TEXT PRIMARY KEY,
    agent      TEXT NOT NULL REFERENCES agents(id),
    kind       TEXT NOT NULL CHECK (kind IN ('cockpit', 'artifact')),
    title      TEXT NOT NULL,
    html       TEXT NOT NULL,
    version    INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX surfaces_by_agent ON surfaces(agent);
";

/// Taille maximale du code d'une surface.
pub const MAX_HTML: usize = 2 * 1024 * 1024;

const META: &str = "id, agent, kind, title, version, created_at, updated_at";

impl Registry {
    /// Crée une surface pour l'Assistant ou un Agent vivant.
    pub fn add_surface(
        &self,
        agent: &AgentId,
        kind: SurfaceKind,
        title: &str,
        html: &str,
    ) -> Result<SurfaceMeta> {
        let owner = self.require(agent)?;
        if owner.level == Level::Worker {
            return Err(RegistryError::Surface(
                "un Worker ne crée pas de surface : il ne parle pas à l'utilisateur".into(),
            ));
        }
        if owner.state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(format!("{agent} est retiré")));
        }
        let title = checked_title(title)?;
        checked_html(html)?;
        let id = format!("s-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let stamp = now() as i64;
        self.db.execute(
            "INSERT INTO surfaces (id, agent, kind, title, html, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
            params![id, agent.as_str(), kind.as_str(), title, html, stamp],
        )?;
        self.surface_meta(&id)
    }

    pub fn surface(&self, id: &str) -> Result<SurfaceDoc> {
        let meta = self.surface_meta(id)?;
        let html = self
            .db
            .query_row("SELECT html FROM surfaces WHERE id = ?1", [id], |row| {
                row.get(0)
            })?;
        Ok(SurfaceDoc { meta, html })
    }

    pub fn surface_meta(&self, id: &str) -> Result<SurfaceMeta> {
        self.db
            .query_row(
                &format!("SELECT {META} FROM surfaces WHERE id = ?1"),
                [id],
                meta_row,
            )
            .optional()?
            .ok_or_else(|| RegistryError::SurfaceNotFound(id.into()))
    }

    /// Les surfaces d'un agent, de la plus ancienne à la plus récente.
    pub fn surfaces(&self, agent: &AgentId) -> Result<Vec<SurfaceMeta>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {META} FROM surfaces WHERE agent = ?1 ORDER BY created_at, rowid"
        ))?;
        let surfaces = statement
            .query_map([agent.as_str()], meta_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(surfaces)
    }

    /// Nouveau code et/ou nouveau titre. Un nouveau code incrémente la version.
    pub fn update_surface(
        &self,
        id: &str,
        html: Option<&str>,
        title: Option<&str>,
    ) -> Result<SurfaceMeta> {
        let meta = self.surface_meta(id)?;
        let title = match title {
            Some(title) => checked_title(title)?,
            None => meta.title,
        };
        if let Some(html) = html {
            checked_html(html)?;
            self.db.execute(
                "UPDATE surfaces SET html = ?2, title = ?3, version = version + 1, updated_at = ?4
                 WHERE id = ?1",
                params![id, html, title, now() as i64],
            )?;
        } else {
            self.db.execute(
                "UPDATE surfaces SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, title, now() as i64],
            )?;
        }
        self.surface_meta(id)
    }

    /// Supprime une surface ; rend ce qu'elle était, pour le signaler.
    pub fn delete_surface(&self, id: &str) -> Result<SurfaceMeta> {
        let meta = self.surface_meta(id)?;
        self.db
            .execute("DELETE FROM surfaces WHERE id = ?1", [id])?;
        Ok(meta)
    }
}

fn meta_row(row: &Row) -> rusqlite::Result<SurfaceMeta> {
    Ok(SurfaceMeta {
        id: row.get(0)?,
        agent: AgentId::new(row.get::<_, String>(1)?),
        kind: parsed(row, 2)?,
        title: row.get(3)?,
        version: row.get::<_, i64>(4)? as u64,
        created_at: row.get::<_, i64>(5)? as u64,
        updated_at: row.get::<_, i64>(6)? as u64,
    })
}

fn checked_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(RegistryError::Surface("une surface a un titre".into()));
    }
    Ok(title.into())
}

fn checked_html(html: &str) -> Result<()> {
    if html.trim().is_empty() {
        return Err(RegistryError::Surface(
            "le code de la surface est vide".into(),
        ));
    }
    if html.len() > MAX_HTML {
        return Err(RegistryError::Surface(format!(
            "code trop gros ({} octets, {MAX_HTML} au plus)",
            html.len()
        )));
    }
    Ok(())
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
                node: None,
                workdir: "/p".into(),
                charter: Some(Charter {
                    name: "Lecteur".into(),
                    scope: "les PDF".into(),
                }),
            })
            .unwrap();
        let worker = registry
            .create(NewAgent {
                parent: agent.id.clone(),
                level: Level::Worker,
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                node: None,
                workdir: "/p".into(),
                charter: None,
            })
            .unwrap();
        (registry, agent.id, worker.id)
    }

    #[test]
    fn a_surface_keeps_its_code_and_versions() {
        let (registry, agent, _) = setup();
        let created = registry
            .add_surface(&agent, SurfaceKind::Cockpit, "Lecteur PDF", "<p>v1</p>")
            .unwrap();
        assert_eq!((created.version, created.kind), (1, SurfaceKind::Cockpit));
        assert!(created.id.starts_with("s-"));

        let retitled = registry
            .update_surface(&created.id, None, Some("Lecteur"))
            .unwrap();
        assert_eq!((retitled.version, retitled.title.as_str()), (1, "Lecteur"));
        let updated = registry
            .update_surface(&created.id, Some("<p>v2</p>"), None)
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(registry.surface(&created.id).unwrap().html, "<p>v2</p>");
        assert_eq!(registry.surfaces(&agent).unwrap(), vec![updated.clone()]);

        assert_eq!(registry.delete_surface(&created.id).unwrap().id, created.id);
        assert!(matches!(
            registry.surface(&created.id),
            Err(RegistryError::SurfaceNotFound(_))
        ));
    }

    #[test]
    fn workers_have_no_surface_and_bad_code_is_refused() {
        let (registry, agent, worker) = setup();
        let refused = [
            registry.add_surface(&worker, SurfaceKind::Artifact, "x", "<p>x</p>"),
            registry.add_surface(&agent, SurfaceKind::Artifact, " ", "<p>x</p>"),
            registry.add_surface(&agent, SurfaceKind::Artifact, "x", "  "),
            registry.add_surface(
                &agent,
                SurfaceKind::Artifact,
                "x",
                &"a".repeat(MAX_HTML + 1),
            ),
        ];
        for result in refused {
            assert!(
                matches!(result, Err(RegistryError::Surface(_))),
                "{result:?}"
            );
        }
    }
}
