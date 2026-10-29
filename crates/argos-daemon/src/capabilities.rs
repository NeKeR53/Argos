//! Capacités explicites d'un Agent, partagées avec ses Workers.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<ExternalMcp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalMcp {
    pub name: String,
    #[serde(flatten)]
    pub transport: Transport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Transport {
    Http {
        url: String,
    },
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Capabilities {
    pub fn validate(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        for server in &self.mcp {
            if server.name == "argos"
                || server.name.is_empty()
                || !server
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                || !names.insert(&server.name)
            {
                return Err(format!("nom MCP invalide ou en double : {}", server.name));
            }
            match &server.transport {
                Transport::Http { url }
                    if !url.starts_with("http://") && !url.starts_with("https://") =>
                {
                    return Err("URL MCP HTTP(S) attendue".into());
                }
                Transport::Stdio { command, .. } if command.trim().is_empty() => {
                    return Err("commande MCP vide".into());
                }
                _ => {}
            }
        }
        let mut paths = BTreeSet::new();
        for path in &self.skills {
            if !Path::new(path).is_absolute()
                || Path::new(path).file_name().and_then(|s| s.to_str()) != Some("SKILL.md")
                || !paths.insert(path)
            {
                return Err(format!(
                    "chemin de skill invalide ou en double : {path} (chemin absolu vers SKILL.md attendu)"
                ));
            }
        }
        Ok(())
    }

    /// Lu sur le nœud d'exécution, pour refuser une configuration non disponible.
    pub fn skill_instructions(&self) -> std::io::Result<String> {
        let mut result = String::new();
        for path in &self.skills {
            // Lire maintenant évite de déclarer disponibles des fichiers absents.
            let content = std::fs::read_to_string(path)?;
            result.push_str(&format!("\n## Skill disponible : {path}\nLis ce fichier et applique ses instructions lorsque pertinentes. Voici son contenu :\n{content}\n"));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_validate_and_roundtrip_both_transports() {
        let value = serde_json::json!({"skills":["/tmp/example/SKILL.md"],"mcp":[
            {"name":"web","type":"http","url":"https://example.test/mcp"},
            {"name":"local","type":"stdio","command":"server","args":["--serve"]}
        ]});
        let config: Capabilities = serde_json::from_value(value.clone()).unwrap();
        config.validate().unwrap();
        assert_eq!(serde_json::to_value(config).unwrap(), value);
    }

    #[test]
    fn reject_reserved_or_duplicate_servers_and_relative_skills() {
        let mut config = Capabilities::default();
        config.skills.push("foo/SKILL.md".into());
        assert!(config.validate().is_err());
        config.skills.clear();
        config.mcp.push(ExternalMcp {
            name: "argos".into(),
            transport: Transport::Http {
                url: "http://localhost".into(),
            },
        });
        assert!(config.validate().is_err());
    }
}
