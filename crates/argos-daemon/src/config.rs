//! Configuration du daemon (`$ARGOS_HOME/config.toml`) et jeton d'accès à l'API.

use crate::engine::Access;
use crate::node::transport::NodeConfig;
use crate::policy::{DEFAULT_MODELS, ModelPolicy};
use argos_core::EngineKind;
use serde::Deserialize;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::fs::{self};
use std::io::{self, Write};
use std::net::SocketAddr;
#[cfg(unix)]
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

[surfaces]
# Serveur HTTP des surfaces : chacune s'ouvre aussi dans un navigateur, dans le même bac
# à sable que dans la console, depuis ce Mac ou un autre appareil du réseau local.
# Depuis ce Mac (127.0.0.1), sans clé. Un autre appareil s'associe une fois en scannant
# le QR code de http://127.0.0.1:7890/, ou en tapant le code affiché dessous ; il garde
# son propre secret en cookie. Les liens des agents ne portent aucune clé. Cette même
# page liste les appareils, révoque chacun et renouvelle le code.
# "127.0.0.1:7890" pour ce Mac seulement.
listen = "0.0.0.0:7890"
# Adresse des liens donnés par les agents ; par défaut http://<nom Bonjour>.local:<port>.
# public_url = "http://192.168.1.20:7890"

[workers]
# Ces droits s’appliquent aussi aux Agents, qui exécutent directement par défaut.
# Droits des Workers : "full" (ceux de l'utilisateur : commandes, réseau, installations,
# sans approbation) ou "workspace" (écrire dans leur répertoire, sans réseau).
# Seul l'Assistant reste en lecture seule.
access = "full"

[board]
# Préemption (D14) : quand une mission passe en tête du board d'un Agent, ses Workers
# occupés à une tâche moins prioritaire sont mis au repos, puis reprennent d'eux-mêmes
# quand cette mission est close ou annulée.
preempt = false

# Nœuds pairs (D48) : chaque nœud détient ses agents et se relie aux autres. Ce nœud :
# [node]
# name = "mac"                     # son nom dans le maillage (par défaut : nom de la machine)
# listen = "127.0.0.1:7880"        # écoute des pairs ; locale, ils passent par SSH
# assistant = true                 # peut créer l'Assistant s'il n'en connaît aucun
# Les pairs qu'il joint lui-même ; un lien ouvert sert dans les deux sens. Le jeton d'un
# pair est dans son $ARGOS_HOME/node-token.
# [peers.pi]
# ssh = "pi"                       # hôte de ~/.ssh/config ; ou url = "ws://127.0.0.1:7880"
# remote = "127.0.0.1:7880"        # écoute du pair, vue depuis cet hôte
# token_file = "/chemin/vers/le/jeton"

# Canaux de discussion (D45) : parler à l'Assistant hors de la console, par exemple
# depuis son téléphone. Un canal par section ; l'Assistant choisit où il répond.
# Telegram (D43) : créer le bot avec @BotFather, mettre son jeton dans
# $ARGOS_HOME/<id>-token (ou la variable ARGOS_<ID>_TOKEN), puis écrire au bot : il répond
# avec l'identifiant à reporter dans chat_id. Les vocaux utilisent ffmpeg, whisper-cli et
# la section [voice].
# [channels.telegram]
# type = "telegram"
# chat_id = 123456789
# escalations = true               # signaler les escalades, avec leurs réponses en boutons
# surfaces = true                  # envoyer le lien de chaque nouvelle surface (appareil associé)
# voice_replies = true             # false : toujours répondre en texte, même à un vocal
# max_per_hour = 30                # envois de l'Assistant au-delà desquels le daemon refuse
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
    #[serde(default)]
    pub surfaces: SurfacesConfig,
    /// Politique de modèles (D22) ; celle par défaut si la section manque.
    #[serde(default)]
    pub models: ModelPolicy,
    /// Ce nœud dans le maillage (D48).
    #[serde(default)]
    pub node: Option<NodeIdentity>,
    /// Les pairs que ce nœud joint lui-même, par nom (`[peers]`, anciennement `[nodes]`).
    #[serde(default, alias = "nodes")]
    pub peers: BTreeMap<String, NodeConfig>,
    #[serde(default)]
    pub workers: WorkersConfig,
    #[serde(default)]
    pub board: BoardConfig,
    /// Canaux de discussion (D45), par identifiant.
    #[serde(default)]
    pub channels: BTreeMap<String, crate::channels::ChannelConfig>,
    /// Ancienne section `[telegram]`, lue comme le canal `telegram` : voir [`Self::channels`].
    #[serde(default)]
    pub telegram: Option<crate::telegram::TelegramConfig>,
    /// Section `[voice]`, partagée avec la GUI : les canaux qui parlent y lisent le modèle
    /// whisper et la voix de synthèse.
    #[serde(default)]
    pub voice: Option<crate::telegram::SpeechConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BoardConfig {
    /// Préemption des Workers moins prioritaires (D14), désactivée par défaut.
    #[serde(default)]
    pub preempt: bool,
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

/// Section `[node]` de `config.toml` : ce nœud dans le maillage (D48).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeIdentity {
    pub name: Option<String>,
    pub listen: Option<SocketAddr>,
    /// Ce nœud peut créer l'Assistant s'il n'en connaît aucun. Par défaut : oui, sauf
    /// pour un ancien nœud d'exécution (qui a un `node.toml`).
    pub assistant: Option<bool>,
}

/// Ce que le daemon retient de son identité, `config.toml` d'abord, puis `node.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub name: String,
    pub listen: SocketAddr,
    pub assistant: bool,
}

impl Config {
    pub fn identity(&self, home: &Path) -> Identity {
        let legacy = fs::read_to_string(home.join("node.toml"))
            .ok()
            .and_then(|text| toml::from_str::<NodeModeConfig>(&text).ok())
            .map(|config| config.node);
        let section = self.node.clone().unwrap_or_default();
        Identity {
            name: section
                .name
                .or_else(|| legacy.as_ref().map(|node| node.name.clone()))
                .unwrap_or_else(machine_name),
            listen: section
                .listen
                .or_else(|| legacy.as_ref().map(|node| node.listen))
                .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 7880))),
            assistant: section.assistant.unwrap_or(legacy.is_none()),
        }
    }
}

/// Nom court de la machine, en minuscules : le nom d'un nœud par défaut.
fn machine_name() -> String {
    let read = |program: &str, args: &[&str]| {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        let name = String::from_utf8(output.stdout).ok()?.trim().to_lowercase();
        (output.status.success() && !name.is_empty()).then_some(name)
    };
    read("scutil", &["--get", "LocalHostName"])
        .or_else(|| read("hostname", &["-s"]))
        .unwrap_or_else(|| "nœud".into())
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeModeConfig {
    pub node: NodeSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeSection {
    pub name: String,
    pub listen: SocketAddr,
    /// Mandataire MCP de l'ancien mode nœud ; ignoré depuis D48.
    #[serde(default)]
    pub mcp_listen: Option<SocketAddr>,
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
pub struct SurfacesConfig {
    pub listen: SocketAddr,
    #[serde(default)]
    pub public_url: Option<String>,
}

impl Default for SurfacesConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 7890)),
            public_url: None,
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
    #[error("config.toml : {0}")]
    Edit(String),
}

impl Config {
    /// Lit la configuration ; au premier démarrage, écrit d'abord celle par défaut.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            fs::write(path, format!("{DEFAULT_CONFIG}{DEFAULT_MODELS}"))?;
        }
        let mut config: Config = toml::from_str(&fs::read_to_string(path)?)?;
        if let Some(telegram) = config.telegram.take() {
            eprintln!(
                "config.toml : la section [telegram] est lue comme [channels.telegram] \
                 (type = \"telegram\") ; renomme-la, elle sera retirée"
            );
            config
                .channels
                .entry("telegram".into())
                .or_insert_with(|| crate::channels::ChannelConfig::telegram(telegram));
        }
        config.models.check().map_err(ConfigError::Policy)?;
        for (name, node) in &config.peers {
            node.check(name).map_err(ConfigError::Node)?;
        }
        Ok(config)
    }
}

/// Ce que `argos-daemon config init` a fait du fichier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Init {
    Created,
    /// Migré ; l'ancien fichier est gardé sous ce nom.
    Migrated {
        changes: Vec<String>,
        backup: std::path::PathBuf,
    },
    UpToDate,
}

/// Crée la configuration par défaut, ou migre une configuration existante vers le format
/// de cette version (D46) en gardant ses commentaires. L'installeur et la mise à jour
/// l'appellent avec le nouveau binaire ; le fichier migré doit encore se lire.
pub fn init(path: &Path) -> Result<Init, ConfigError> {
    if !path.exists() {
        Config::load_or_create(path)?;
        return Ok(Init::Created);
    }
    let text = fs::read_to_string(path)?;
    let Some((migrated, changes)) = migrate(&text)? else {
        Config::load_or_create(path)?;
        return Ok(Init::UpToDate);
    };
    let config: Config = toml::from_str(&migrated)?;
    config.models.check().map_err(ConfigError::Policy)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".avant-{}", env!("CARGO_PKG_VERSION")));
    let backup = path.with_file_name(name);
    fs::copy(path, &backup)?;
    fs::write(path, migrated)?;
    Ok(Init::Migrated { changes, backup })
}

/// Migrations du format, chacune reconnue à ce qu'elle trouve dans le fichier : elles
/// s'appliquent quel que soit l'écart de versions, et deux passes ne changent rien.
pub fn migrate(text: &str) -> Result<Option<(String, Vec<String>)>, ConfigError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| ConfigError::Edit(error.to_string()))?;
    let mut changes = Vec::new();

    // 0.3 : l'ancienne section [telegram] devient le canal [channels.telegram] (D45).
    let has_channel = doc
        .get("channels")
        .and_then(|channels| channels.get("telegram"))
        .is_some();
    if !has_channel && doc.get("telegram").is_some_and(|item| item.is_table()) {
        let old = doc
            .remove("telegram")
            .and_then(|item| item.into_table().ok());
        if let Some(old) = old {
            let mut table = toml_edit::Table::new();
            table.insert("type", toml_edit::value("telegram"));
            for (key, item) in old.iter() {
                table.insert(key, item.clone());
            }
            table.decor_mut().clone_from(old.decor());
            if let Some(position) = old.position() {
                table.set_position(Some(position));
            }
            let channels = doc
                .entry("channels")
                .or_insert_with(|| {
                    let mut channels = toml_edit::Table::new();
                    channels.set_implicit(true);
                    toml_edit::Item::Table(channels)
                })
                .as_table_mut()
                .ok_or_else(|| ConfigError::Edit("channels n'est pas une table".into()))?;
            channels.insert("telegram", toml_edit::Item::Table(table));
            changes.push("[telegram] devient [channels.telegram] (type = \"telegram\")".into());
        }
    }

    Ok((!changes.is_empty()).then(|| (doc.to_string(), changes)))
}

/// `argos-daemon config set <clé> <valeur>` : change une valeur en gardant le reste du
/// fichier et ses commentaires. `clé` est un chemin pointé (`assistant.engine`) ; la
/// valeur est lue comme une valeur TOML (`true`, `30`, `"texte"`), sinon comme du texte.
/// Le résultat doit rester une configuration valide.
pub fn set(path: &Path, key: &str, value: &str) -> Result<(), ConfigError> {
    let mut doc: toml_edit::DocumentMut = fs::read_to_string(path)?
        .parse()
        .map_err(|error: toml_edit::TomlError| ConfigError::Edit(error.to_string()))?;
    let parts: Vec<&str> = key.split('.').collect();
    let (last, tables) = parts
        .split_last()
        .filter(|(last, _)| !last.is_empty())
        .ok_or_else(|| ConfigError::Edit(format!("clé invalide : {key}")))?;
    let mut table = doc.as_table_mut();
    for part in tables {
        table = table
            .entry(part)
            .or_insert_with(toml_edit::table)
            .as_table_mut()
            .ok_or_else(|| ConfigError::Edit(format!("{part} n'est pas une section")))?;
    }
    let parsed = value
        .parse::<toml_edit::Value>()
        .unwrap_or_else(|_| value.into());
    match table.get_mut(last).and_then(|item| item.as_value_mut()) {
        // Garder le commentaire de fin de ligne.
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = parsed;
            *existing.decor_mut() = decor;
        }
        None => {
            table.insert(last, toml_edit::value(parsed));
        }
    }
    let text = doc.to_string();
    let config: Config = toml::from_str(&text)?;
    config.models.check().map_err(ConfigError::Policy)?;
    fs::write(path, text)?;
    Ok(())
}

/// Jeton exigé par l'API, lu dans `path` ou créé (lisible par l'utilisateur seul).
pub fn load_or_create_token(path: &Path) -> io::Result<String> {
    if let Ok(token) = fs::read_to_string(path) {
        return Ok(token.trim().to_string());
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut file = create_private_token(path)?;
    writeln!(file, "{token}")?;
    Ok(token)
}

#[cfg(unix)]
fn create_private_token(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn create_private_token(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ},
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // Owner Rights : accès uniquement au propriétaire, sans héritage des permissions du dossier.
    let descriptor: Vec<u16> = "D:P(A;;FA;;;OW)".encode_utf16().chain(Some(0)).collect();
    let mut sd = std::ptr::null_mut();
    // SAFETY : Win32 initialise le descripteur ; il reste vivant jusqu'à la création.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let handle = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        let result = if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(fs::File::from_raw_handle(handle as _))
        };
        LocalFree(sd as _);
        result
    }
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
        assert!(config.peers.is_empty());
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
        assert_eq!(config.peers.keys().collect::<Vec<_>>(), ["pi", "vps"]);
        assert_eq!(config.peers["pi"].ssh.as_deref(), Some("pi"));

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
        assert_eq!(
            config.node.mcp_listen,
            Some("127.0.0.1:7881".parse().unwrap())
        );
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
    fn the_old_telegram_section_is_migrated_once_with_its_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let old = "[assistant]\nengine = \"claude\"\n[api]\nlisten = \"127.0.0.1:9000\"\n\n\
                   # Mon bot\n[telegram]\nchat_id = 42 # moi\n\n[board]\npreempt = true\n";
        fs::write(&path, old).unwrap();

        let Init::Migrated { changes, backup } = init(&path).unwrap() else {
            panic!("migration attendue");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(fs::read_to_string(&backup).unwrap(), old);
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("[telegram]"), "{text}");
        assert!(
            text.contains(
                "# Mon bot\n[channels.telegram]\ntype = \"telegram\"\nchat_id = 42 # moi"
            ),
            "{text}"
        );
        // L'ordre des sections est gardé.
        assert!(
            text.find("[channels.telegram]") < text.find("[board]"),
            "{text}"
        );
        let config = Config::load_or_create(&path).unwrap();
        assert!(config.channels.contains_key("telegram"));
        assert!(config.board.preempt);

        assert_eq!(init(&path).unwrap(), Init::UpToDate);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn init_creates_the_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(init(&path).unwrap(), Init::Created);
        assert_eq!(init(&path).unwrap(), Init::UpToDate);
    }

    #[test]
    fn set_changes_one_value_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        init(&path).unwrap();
        set(&path, "assistant.engine", "codex").unwrap();
        set(&path, "assistant.model", "gpt-6.1-sol").unwrap();
        set(&path, "board.preempt", "true").unwrap();
        set(&path, "voice.wake_word", "Jarvis").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("engine = \"codex\"            # \"claude\" ou \"codex\""),
            "{text}"
        );
        assert!(text.contains("# Configuration du daemon Argos."));
        let config = Config::load_or_create(&path).unwrap();
        assert_eq!(config.assistant.engine, EngineKind::Codex);
        assert_eq!(config.assistant.model.as_deref(), Some("gpt-6.1-sol"));
        assert!(config.board.preempt);
        assert!(text.contains("[voice]\nwake_word = \"Jarvis\""), "{text}");

        // Une valeur qui casserait la configuration est refusée, le fichier reste.
        assert!(set(&path, "assistant.engine", "gemini").is_err());
        assert!(set(&path, "assistant.", "x").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
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
