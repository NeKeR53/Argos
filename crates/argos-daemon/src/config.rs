//! Configuration du daemon (`$ARGOS_HOME/config.toml`) et jeton d'accès à l'API.

use crate::engine::Access;
use crate::node::transport::NodeConfig;
use crate::policy::{DEFAULT_MODELS, ModelPolicy};
use argos_core::EngineKind;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use uuid::Uuid;

/// Écrite telle quelle au premier démarrage, suivie de la politique de modèles par
/// défaut : c'est aussi la documentation du fichier.
const DEFAULT_CONFIG: &str = r#"# Configuration du daemon Argos.

[assistant]
# Moteur et modèle de l'Assistant, choisis par l'utilisateur (D21).
# Pris en compte à la création de l'Assistant ; ensuite, le modèle de chaque agent se
# change dans la console (menu du modèle, dans son en-tête).
engine = "claude"            # "claude" ou "codex"
model = "claude-opus-5-5"

[api]
# Adresse de l'API WebSocket. Locale seulement : les autres nœuds passent par SSH (D32).
listen = "127.0.0.1:7878"

[mcp]
# Adresse du serveur MCP par lequel les agents pilotent leurs enfants. Locale seulement.
listen = "127.0.0.1:7879"

[workers]
# Droits des Workers : "full" (ceux de l'utilisateur : commandes, réseau, installations,
# sans approbation) ou "workspace" (écrire dans leur répertoire, sans réseau).
# L'Assistant et les Agents n'exécutent jamais rien eux-mêmes.
access = "full"

# Autres nœuds (D32, D33) : le hub s'y connecte, l'Assistant peut y placer des Agents.
# Sur le nœud, lancer `argos-daemon node` (voir son node.toml) ; son jeton est dans
# $ARGOS_HOME/node-token.
# [nodes.pi]
# ssh = "pi"                       # hôte de ~/.ssh/config ; ou url = "ws://127.0.0.1:7880"
# remote = "127.0.0.1:7880"        # écoute du nœud, vue depuis cet hôte
# token_file = "/chemin/vers/le/jeton"
"#;

/// Configuration d'un daemon lancé en mode nœud (`$ARGOS_HOME/node.toml`).
const DEFAULT_NODE_CONFIG: &str = r#"# Configuration d'Argos en mode nœud (`argos-daemon node`).

[node]
# Nom du nœud, celui que le hub lui donne dans [nodes.<nom>].
name = "nœud"
# Écoute de la connexion du hub. Locale seulement : le hub passe par un tunnel SSH (D32).
listen = "127.0.0.1:7880"
# Mandataire MCP local des agents du nœud.
mcp_listen = "127.0.0.1:7881"
"#;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub assistant: AssistantConfig,
    pub api: ApiConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    /// Politique de modèles (D22) ; celle par défaut si la section manque.
    #[serde(default)]
    pub models: ModelPolicy,
    /// Les autres nœuds, par nom.
    #[serde(default)]
    pub nodes: BTreeMap<String, NodeConfig>,
    #[serde(default)]
    pub workers: WorkersConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkersConfig {
    /// `full` : les droits de l'utilisateur (installer, réseau…) ; `workspace` : écrire
    /// dans leur répertoire seulement.
    pub access: Access,
}

impl Default for WorkersConfig {
    fn default() -> Self {
        Self {
            access: Access::Full,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeModeConfig {
    pub node: NodeSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeSection {
    pub name: String,
    pub listen: SocketAddr,
    pub mcp_listen: SocketAddr,
}

impl NodeModeConfig {
    /// Lit `node.toml` ; au premier démarrage, écrit d'abord celui par défaut.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            fs::write(path, DEFAULT_NODE_CONFIG)?;
        }
        Ok(toml::from_str(&fs::read_to_string(path)?)?)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpConfig {
    pub listen: SocketAddr,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 7879)),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantConfig {
    pub engine: EngineKind,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub listen: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("config.toml illisible : {0}")]
    Parse(#[from] toml::de::Error),
    #[error("politique de modèles invalide : {0}")]
    Policy(String),
    #[error("{0}")]
    Node(String),
}

impl Config {
    /// Lit la configuration ; au premier démarrage, écrit d'abord celle par défaut.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            fs::write(path, format!("{DEFAULT_CONFIG}{DEFAULT_MODELS}"))?;
        }
        let config: Config = toml::from_str(&fs::read_to_string(path)?)?;
        config.models.check().map_err(ConfigError::Policy)?;
        for (name, node) in &config.nodes {
            node.check(name).map_err(ConfigError::Node)?;
        }
        Ok(config)
    }
}

/// Jeton exigé par l'API, lu dans `path` ou créé (lisible par l'utilisateur seul).
pub fn load_or_create_token(path: &Path) -> io::Result<String> {
    if let Ok(token) = fs::read_to_string(path) {
        return Ok(token.trim().to_string());
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{token}")?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_default_config_is_written_once_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_create(&path).unwrap();
        assert_eq!(config.models, ModelPolicy::default());
        assert_eq!(config.assistant.engine, EngineKind::Claude);
        assert_eq!(config.assistant.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(config.api.listen, "127.0.0.1:7878".parse().unwrap());
        assert!(config.nodes.is_empty());
        assert_eq!(config.workers.access, Access::Full);

        fs::write(
            &path,
            "[assistant]\nengine = \"codex\"\n[api]\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap();
        assert_eq!(config.mcp.listen, "127.0.0.1:7879".parse().unwrap());
        let edited = Config::load_or_create(&path).unwrap();
        assert_eq!(edited.assistant.engine, EngineKind::Codex);
        assert_eq!(edited.assistant.model, None);
        // Une config d'avant T4, sans [mcp], reste valide.
        assert_eq!(edited.mcp.listen, "127.0.0.1:7879".parse().unwrap());
    }

    #[test]
    fn nodes_are_declared_by_name_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let base = "[assistant]\nengine = \"claude\"\n[api]\nlisten = \"127.0.0.1:9000\"\n";
        fs::write(
            &path,
            format!("{base}[nodes.pi]\nssh = \"pi\"\ntoken = \"t\"\n[nodes.vps]\nurl = \"ws://127.0.0.1:7880\"\ntoken = \"u\"\n"),
        )
        .unwrap();
        let config = Config::load_or_create(&path).unwrap();
        assert_eq!(config.nodes.keys().collect::<Vec<_>>(), ["pi", "vps"]);
        assert_eq!(config.nodes["pi"].ssh.as_deref(), Some("pi"));

        fs::write(&path, format!("{base}[nodes.pi]\nssh = \"pi\"\n")).unwrap();
        assert!(matches!(
            Config::load_or_create(&path),
            Err(ConfigError::Node(_))
        ));
    }

    #[test]
    fn the_default_node_config_parses() {
        let dir = tempfile::tempdir().unwrap();
        let config = NodeModeConfig::load_or_create(&dir.path().join("node.toml")).unwrap();
        assert_eq!(config.node.listen, "127.0.0.1:7880".parse().unwrap());
        assert_eq!(config.node.mcp_listen, "127.0.0.1:7881".parse().unwrap());
    }

    #[test]
    fn a_broken_config_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[assistant]\nengine = \"gemini\"\n").unwrap();
        assert!(matches!(
            Config::load_or_create(&path),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn the_token_is_private_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");

        let token = load_or_create_token(&path).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(load_or_create_token(&path).unwrap(), token);
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
