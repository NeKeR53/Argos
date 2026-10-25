//! Types partagés entre le daemon et la GUI. Vocabulaire : docs/GLOSSARY.md.

use serde::{Deserialize, Serialize};

/// Version du protocole entre la GUI et le daemon.
pub const PROTOCOL_VERSION: u32 = 1;

/// Niveau d'un agent dans la hiérarchie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Assistant,
    Agent,
    Worker,
}

impl Level {
    /// Le niveau que celui-ci contrôle : chaque niveau ne pilote que celui juste en dessous.
    pub fn child(self) -> Option<Level> {
        match self {
            Level::Assistant => Some(Level::Agent),
            Level::Agent => Some(Level::Worker),
            Level::Worker => None,
        }
    }

    /// Seuls l'Assistant et les Agents ont une mémoire persistante.
    pub fn has_memory(self) -> bool {
        self != Level::Worker
    }
}

/// Le CLI qui fait tourner un agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Claude,
    Codex,
}

impl EngineKind {
    /// Nom du binaire à lancer.
    pub fn binary(self) -> &'static str {
        match self {
            EngineKind::Claude => "claude",
            EngineKind::Codex => "codex",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_controls_only_the_next_one() {
        assert_eq!(Level::Assistant.child(), Some(Level::Agent));
        assert_eq!(Level::Agent.child(), Some(Level::Worker));
        assert_eq!(Level::Worker.child(), None);
    }

    #[test]
    fn workers_have_no_memory() {
        assert!(Level::Assistant.has_memory());
        assert!(Level::Agent.has_memory());
        assert!(!Level::Worker.has_memory());
    }

    #[test]
    fn serializes_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&Level::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(
            serde_json::to_string(&EngineKind::Codex).unwrap(),
            "\"codex\""
        );
    }
}
