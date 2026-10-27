//! Définitions des fonctions publiées. L'Agent seul publie ou retire les siennes.

use super::{Registry, RegistryError, Result};
use argos_core::{AgentId, AgentState, FunctionInfo, Level, PublishedFunction};
use rusqlite::{OptionalExtension, params};

pub(super) const SCHEMA: &str = "
CREATE TABLE functions (
    agent TEXT NOT NULL REFERENCES agents(id),
    name TEXT NOT NULL,
    definition TEXT NOT NULL,
    PRIMARY KEY (agent, name)
);
";

impl Registry {
    pub fn publish_function(&self, function: &PublishedFunction) -> Result<()> {
        self.function_owner(&function.agent)?;
        crate::functions::validate_definition(function).map_err(RegistryError::Function)?;
        let definition = serde_json::to_string(function)
            .map_err(|_| RegistryError::Function("définition illisible".into()))?;
        self.db.execute(
            "INSERT INTO functions (agent, name, definition) VALUES (?1, ?2, ?3)
             ON CONFLICT(agent, name) DO UPDATE SET definition = excluded.definition",
            params![function.agent.as_str(), function.name, definition],
        )?;
        Ok(())
    }

    pub fn function(&self, agent: &AgentId, name: &str) -> Result<PublishedFunction> {
        self.function_owner(agent)?;
        let definition: Option<String> = self
            .db
            .query_row(
                "SELECT definition FROM functions WHERE agent = ?1 AND name = ?2",
                params![agent.as_str(), name],
                |row| row.get(0),
            )
            .optional()?;
        let definition = definition
            .ok_or_else(|| RegistryError::Function(format!("fonction inconnue : {name}")))?;
        serde_json::from_str(&definition)
            .map_err(|_| RegistryError::Function("définition stockée illisible".into()))
    }

    pub fn functions(&self, agent: &AgentId) -> Result<Vec<FunctionInfo>> {
        self.function_owner(agent)?;
        let mut statement = self
            .db
            .prepare("SELECT definition FROM functions WHERE agent = ?1 ORDER BY name")?;
        let definitions = statement
            .query_map([agent.as_str()], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        definitions
            .iter()
            .map(|definition| {
                serde_json::from_str::<PublishedFunction>(definition)
                    .map(|function| function.info())
                    .map_err(|_| RegistryError::Function("définition stockée illisible".into()))
            })
            .collect()
    }

    pub fn delete_function(&self, agent: &AgentId, name: &str) -> Result<()> {
        self.function_owner(agent)?;
        if self.db.execute(
            "DELETE FROM functions WHERE agent = ?1 AND name = ?2",
            params![agent.as_str(), name],
        )? == 0
        {
            return Err(RegistryError::Function(format!(
                "fonction inconnue : {name}"
            )));
        }
        Ok(())
    }

    fn function_owner(&self, agent: &AgentId) -> Result<()> {
        let owner = self.require(agent)?;
        if owner.level != Level::Agent || owner.state == AgentState::Retired {
            return Err(RegistryError::Function(
                "le propriétaire doit être un Agent non retiré".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::NewAgent;
    use argos_core::{Charter, EngineKind, FunctionRequest};
    use serde_json::json;

    fn setup(registry: &Registry) -> (AgentId, AgentId) {
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
                    name: "Domotique".into(),
                    scope: "la maison".into(),
                }),
            })
            .unwrap();
        (assistant.id, agent.id)
    }

    fn definition(agent: &AgentId) -> PublishedFunction {
        PublishedFunction {
            agent: agent.clone(),
            name: "set_light".into(),
            description: "Allume une lumière.".into(),
            parameters: json!({"type":"object", "properties":{"state":{"type":"boolean"}},
                "required":["state"], "additionalProperties":false}),
            request: FunctionRequest {
                method: "POST".into(),
                url: "http://localhost:8123/api/light".into(),
                headers: [(
                    "Authorization".into(),
                    "Bearer {{env:ARGOS_TEST_SECRET}}".into(),
                )]
                .into(),
                body: Some(json!({"state":"{{state}}"})),
                timeout_ms: 10_000,
            },
        }
    }

    #[test]
    fn functions_survive_restart_and_discovery_hides_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("argos.db");
        let definition = {
            let registry = Registry::open(&path).unwrap();
            let (_, agent) = setup(&registry);
            let mut function = definition(&agent);
            registry.publish_function(&function).unwrap();
            function.description = "Change une lumière.".into();
            registry.publish_function(&function).unwrap();
            function
        };
        let registry = Registry::open(&path).unwrap();
        assert_eq!(
            registry
                .function(&definition.agent, &definition.name)
                .unwrap(),
            definition
        );
        let infos = registry.functions(&definition.agent).unwrap();
        assert_eq!(infos, vec![definition.info()]);
        assert!(
            !serde_json::to_string(&infos)
                .unwrap()
                .contains("Authorization")
        );
        registry
            .delete_function(&definition.agent, &definition.name)
            .unwrap();
        assert!(registry.functions(&definition.agent).unwrap().is_empty());
    }

    #[test]
    fn assistants_workers_and_retired_agents_cannot_publish() {
        let registry = Registry::open_in_memory().unwrap();
        let (assistant, agent) = setup(&registry);
        assert!(registry.publish_function(&definition(&assistant)).is_err());
        let worker = registry
            .create(NewAgent {
                parent: agent.clone(),
                level: Level::Worker,
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                node: None,
                workdir: "/p".into(),
                charter: None,
            })
            .unwrap();
        assert!(registry.publish_function(&definition(&worker.id)).is_err());
        registry.publish_function(&definition(&agent)).unwrap();
        registry.retire(&agent).unwrap();
        assert!(registry.publish_function(&definition(&agent)).is_err());
        assert!(registry.function(&agent, "set_light").is_err());
        assert!(registry.delete_function(&agent, "set_light").is_err());
    }
}
