//! Canaux de discussion (D45) : les voies par lesquelles l'utilisateur parle à l'Assistant
//! hors de la console et de l'orbe.
//!
//! Un type de canal (Telegram…) est un connecteur qui transporte, il ne décide pas (D5).
//! Un canal est une instance configurée d'un type, `[channels.<id>]`. Un message reçu est
//! remis à l'Assistant préfixé par son canal ([`prefix`]). L'Assistant y écrit avec
//! `channel_send`, dans la limite d'un plafond horaire. Filet de sécurité ([`follow`]) : un
//! tour ouvert par un canal qui se termine sans aucun `channel_send` y renvoie la dernière
//! réponse de l'Assistant, en vocal pour un vocal.

use crate::supervisor::Supervisor;
use crate::telegram::TelegramConfig;
use argos_core::AgentId;
use argos_core::protocol::{ChannelCapabilities, ChannelStatus, DaemonEvent};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Fenêtre du plafond d'envois.
const HOUR: Duration = Duration::from_secs(3600);

/// Une section `[channels.<id>]` de `config.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConfig {
    /// Envois de l'Assistant au-delà desquels le daemon refuse, par heure glissante.
    #[serde(default = "default_max_per_hour")]
    pub max_per_hour: u32,
    #[serde(flatten)]
    pub kind: ChannelKind,
}

/// Les types de canal connus du daemon, choisis par `type = "…"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChannelKind {
    Telegram(TelegramConfig),
}

fn default_max_per_hour() -> u32 {
    30
}

impl ChannelConfig {
    /// Un canal Telegram au plafond par défaut, pour l'ancienne section `[telegram]`.
    pub fn telegram(config: TelegramConfig) -> Self {
        Self {
            max_per_hour: default_max_per_hour(),
            kind: ChannelKind::Telegram(config),
        }
    }

    /// Ce que le daemon sait du canal sans l'avoir démarré.
    pub fn meta(&self) -> Meta {
        match &self.kind {
            ChannelKind::Telegram(telegram) => Meta {
                kind: "telegram",
                capabilities: crate::telegram::CAPABILITIES,
                escalations: telegram.escalations,
                max_per_hour: self.max_per_hour,
            },
        }
    }
}

/// Type, capacités et réglages communs d'un canal.
#[derive(Debug, Clone, Copy)]
pub struct Meta {
    pub kind: &'static str,
    pub capabilities: ChannelCapabilities,
    pub escalations: bool,
    pub max_per_hour: u32,
}

/// Un connecteur démarré.
pub trait Channel: Send + Sync {
    /// Arrête le connecteur : l'Assistant part sur un autre nœud (D48).
    fn stop(&self) {}
    /// `Some` quand le canal ne peut pas encore servir.
    fn unavailable(&self) -> Option<String> {
        None
    }
    /// Envoie à l'unique interlocuteur, en texte. `Ok(Some(…))` : envoyé, avec une remarque
    /// pour l'Assistant.
    fn send(&self, text: String) -> BoxFuture<'_, Result<Option<String>, String>>;
}

const VOICE_MARK: &str = ", message vocal transcrit]";

/// Préfixe d'un message reçu sur le canal `id`.
pub fn prefix(id: &str, voice: bool) -> String {
    if voice {
        format!("[Canal {id}{VOICE_MARK}")
    } else {
        format!("[Canal {id}]")
    }
}

/// Le canal d'un message préfixé par [`prefix`].
fn origin(text: &str) -> Option<String> {
    let line = text.lines().next()?.strip_prefix("[Canal ")?;
    match line.strip_suffix(VOICE_MARK) {
        Some(id) => Some(id.to_string()),
        None => Some(line.strip_suffix(']')?.to_string()),
    }
}

/// Tour de l'Assistant ouvert par un message reçu sur un canal.
struct Turn {
    channel: String,
    /// Dernier message de l'Assistant pendant ce tour.
    last: Option<String>,
    /// L'Assistant a écrit sur un canal pendant ce tour : pas de renvoi.
    answered: bool,
}

/// Filet de sécurité : suit les tours de l'Assistant et renvoie sur son canal la réponse
/// d'un tour qui n'a rien envoyé nulle part.
pub async fn follow(supervisor: Supervisor, assistant: AgentId) {
    let mut events = supervisor.subscribe();
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let channels = supervisor.channels();
        let due = match event {
            DaemonEvent::Input { agent, text, .. } if agent == assistant => {
                channels.opened(&text);
                None
            }
            DaemonEvent::Message { agent, text } if agent == assistant => {
                channels.said(text);
                None
            }
            DaemonEvent::TurnCompleted { agent, .. } if agent == assistant => channels.completed(),
            _ => None,
        };
        if let Some((channel, text)) = due
            && let Err(error) = channels.send(&channel, &text).await
        {
            eprintln!("canal {channel} : réponse non renvoyée : {error}");
        }
    }
}

/// Les canaux configurés, démarrés ou non, par identifiant.
#[derive(Default)]
pub struct Channels {
    entries: Mutex<BTreeMap<String, Entry>>,
    turn: Mutex<Option<Turn>>,
    /// Le filet de sécurité ([`follow`]) tourne déjà.
    following: std::sync::atomic::AtomicBool,
}

struct Entry {
    meta: Meta,
    state: State,
    sent: VecDeque<Instant>,
}

enum State {
    Open(Arc<dyn Channel>),
    /// Le démarrage a échoué (jeton introuvable…) : visible, mais sans envoi.
    Failed(String),
}

impl Channels {
    pub fn register(&self, id: &str, meta: Meta, channel: Arc<dyn Channel>) {
        self.insert(id, meta, State::Open(channel));
    }

    /// Arrête un canal et l'oublie.
    pub fn stop(&self, id: &str) {
        if let Some(entry) = locked(&self.entries).remove(id)
            && let State::Open(channel) = entry.state
        {
            channel.stop();
        }
    }

    fn is_open(&self, id: &str) -> bool {
        locked(&self.entries)
            .get(id)
            .is_some_and(|entry| matches!(entry.state, State::Open(_)))
    }

    /// Un canal configuré qui n'a pas pu démarrer.
    pub fn fail(&self, id: &str, meta: Meta, error: String) {
        self.insert(id, meta, State::Failed(error));
    }

    fn insert(&self, id: &str, meta: Meta, state: State) {
        locked(&self.entries).insert(
            id.to_string(),
            Entry {
                meta,
                state,
                sent: VecDeque::new(),
            },
        );
    }

    /// Un message remis à l'Assistant : un tour s'ouvre, venu d'un canal ou non.
    fn opened(&self, text: &str) {
        *locked(&self.turn) = origin(text).map(|channel| Turn {
            channel,
            last: None,
            answered: false,
        });
    }

    fn said(&self, text: String) {
        if let Some(turn) = locked(&self.turn).as_mut() {
            turn.last = Some(text);
        }
    }

    /// Fin du tour : ce qu'il faut renvoyer, s'il n'a rien envoyé.
    fn completed(&self) -> Option<(String, String)> {
        let turn = locked(&self.turn).take()?;
        if turn.answered {
            return None;
        }
        Some((turn.channel, turn.last?))
    }

    pub fn list(&self) -> Vec<ChannelStatus> {
        let now = Instant::now();
        let mut entries = locked(&self.entries);
        entries
            .iter_mut()
            .map(|(id, entry)| {
                entry.forget_before(now);
                let unavailable = match &entry.state {
                    State::Open(channel) => channel.unavailable(),
                    State::Failed(error) => Some(error.clone()),
                };
                ChannelStatus {
                    id: id.clone(),
                    kind: entry.meta.kind.into(),
                    capabilities: entry.meta.capabilities,
                    active: matches!(entry.state, State::Open(_)),
                    unavailable,
                    escalations: entry.meta.escalations,
                    max_per_hour: entry.meta.max_per_hour,
                    sent_last_hour: entry.sent.len() as u32,
                }
            })
            .collect()
    }

    /// Envoie au nom de l'Assistant. `Ok` porte le compte rendu qui lui est rendu.
    pub async fn send(&self, id: &str, text: &str) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("message vide".into());
        }
        let channel = {
            let mut entries = locked(&self.entries);
            let Some(entry) = entries.get_mut(id) else {
                let known: Vec<&String> = entries.keys().collect();
                return Err(if known.is_empty() {
                    "aucun canal n'est configuré".into()
                } else {
                    format!("canal {id} inconnu (canaux : {known:?})")
                });
            };
            let channel = match &entry.state {
                State::Open(channel) => Arc::clone(channel),
                State::Failed(error) => return Err(format!("canal {id} indisponible : {error}")),
            };
            if let Some(reason) = channel.unavailable() {
                return Err(format!("canal {id} indisponible : {reason}"));
            }
            let now = Instant::now();
            entry.forget_before(now);
            if entry.sent.len() >= entry.meta.max_per_hour as usize {
                return Err(format!(
                    "plafond atteint sur {id} : {} envois dans l'heure ; rien n'est parti",
                    entry.meta.max_per_hour
                ));
            }
            // Compté avant l'envoi : deux appels simultanés ne dépassent pas le plafond.
            entry.sent.push_back(now);
            channel
        };
        let degraded = channel.send(text.to_string()).await?;
        if let Some(turn) = locked(&self.turn).as_mut() {
            turn.answered = true;
        }
        let mut notice = format!("Envoyé sur {id}");
        if let Some(degraded) = degraded {
            notice.push_str(&format!(", {degraded}"));
        }
        Ok(format!("{notice}."))
    }
}

impl Entry {
    fn forget_before(&mut self, now: Instant) {
        while self
            .sent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= HOUR)
        {
            self.sent.pop_front();
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- Canaux qui suivent l'Assistant d'un nœud à l'autre (D48) ---------------------------

/// Démarre les canaux de `config.toml` qui ne tournent pas encore, si l'Assistant est
/// détenu par ce nœud. Le filet de sécurité n'est lancé qu'une fois.
pub fn start_configured(supervisor: &Supervisor) {
    let assistant = match supervisor.registry().assistant() {
        Ok(Some(assistant)) if assistant.node.is_none() => assistant,
        _ => return,
    };
    let config = match crate::config::Config::load_or_create(&supervisor.home().join("config.toml"))
    {
        Ok(config) => config,
        Err(error) => {
            eprintln!("canaux non démarrés : {error}");
            return;
        }
    };
    let channels = supervisor.channels();
    if !config.channels.is_empty()
        && !channels
            .following
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        tokio::spawn(follow(supervisor.clone(), assistant.id.clone()));
    }
    for (id, channel) in &config.channels {
        if channels.is_open(id) {
            continue;
        }
        let ChannelKind::Telegram(telegram) = channel.kind.clone();
        let meta = channel.meta();
        match crate::telegram::Telegram::from_config(
            supervisor.clone(),
            assistant.id.clone(),
            id,
            telegram,
            config.voice.clone(),
            supervisor.home(),
        ) {
            Ok(bridge) => {
                bridge.spawn();
                channels.register(id, meta, bridge);
                println!("canal {id} actif (telegram)");
            }
            Err(error) => {
                eprintln!("canal {id} désactivé : {error}");
                channels.fail(id, meta, error);
            }
        }
    }
}

/// Les canaux à emmener avec l'Assistant : leur section de configuration (sans chemin de
/// jeton propre à cette machine) et leurs fichiers, jeton et position de relevé.
pub fn transfers(home: &std::path::Path) -> Vec<crate::peer::ChannelTransfer> {
    let Ok(text) = std::fs::read_to_string(home.join("config.toml")) else {
        return vec![];
    };
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return vec![];
    };
    let Some(channels) = document
        .get("channels")
        .and_then(|item| item.as_table_like())
    else {
        return vec![];
    };
    let mut moved = vec![];
    for (id, item) in channels.iter() {
        let Some(table) = item.as_table_like() else {
            continue;
        };
        let mut section = toml_edit::Table::new();
        let mut token_file = home.join(format!("{id}-token"));
        for (key, value) in table.iter() {
            if key == "token_file" {
                if let Some(path) = value.as_str() {
                    token_file = expand(path);
                }
                continue;
            }
            section.insert(key, value.clone());
        }
        let mut document = toml_edit::DocumentMut::new();
        let mut wrapper = toml_edit::Table::new();
        wrapper.set_implicit(true);
        wrapper.insert(id, toml_edit::Item::Table(section));
        document.insert("channels", toml_edit::Item::Table(wrapper));
        let mut secrets = vec![];
        if let Ok(token) = std::fs::read_to_string(&token_file) {
            secrets.push((format!("{id}-token"), token));
        }
        if let Ok(offset) = std::fs::read_to_string(home.join(format!("{id}-offset"))) {
            secrets.push((format!("{id}-offset"), offset));
        }
        moved.push(crate::peer::ChannelTransfer {
            id: id.to_string(),
            config: document.to_string(),
            secrets,
        });
    }
    moved
}

/// Pose les canaux qui arrivent avec l'Assistant : fichiers puis sections de config.
pub fn receive(
    home: &std::path::Path,
    moved: &[crate::peer::ChannelTransfer],
) -> Result<(), String> {
    let path = home.join("config.toml");
    if !path.exists() {
        // Un nœud sans configuration reçoit d'abord celle par défaut.
        crate::config::Config::load_or_create(&path).map_err(|error| error.to_string())?;
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("config.toml illisible : {error}"))?;
    for channel in moved {
        for (name, content) in &channel.secrets {
            if name.contains('/') || name.contains("..") {
                continue;
            }
            write_private(&home.join(name), content)?;
        }
        let incoming = channel
            .config
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| format!("canal {} : {error}", channel.id))?;
        let Some(section) = incoming
            .get("channels")
            .and_then(|item| item.get(&channel.id))
            .cloned()
        else {
            continue;
        };
        if document.get("channels").is_none() {
            let mut wrapper = toml_edit::Table::new();
            wrapper.set_implicit(true);
            document.insert("channels", toml_edit::Item::Table(wrapper));
        }
        document["channels"][&channel.id] = section;
    }
    std::fs::write(&path, document.to_string())
        .map_err(|error| format!("{} : {error}", path.display()))
}

/// Retire de ce nœud les canaux partis avec l'Assistant : sections et fichiers.
pub fn forget(
    home: &std::path::Path,
    moved: &[crate::peer::ChannelTransfer],
) -> Result<(), String> {
    let path = home.join("config.toml");
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("config.toml illisible : {error}"))?;
    if let Some(channels) = document
        .get_mut("channels")
        .and_then(|item| item.as_table_like_mut())
    {
        for channel in moved {
            channels.remove(&channel.id);
        }
    }
    for channel in moved {
        for (name, _) in &channel.secrets {
            if !name.contains('/') {
                let _ = std::fs::remove_file(home.join(name));
            }
        }
    }
    std::fs::write(&path, document.to_string()).map_err(|error| error.to_string())
}

fn expand(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::PathBuf::from(home).join(rest),
        _ => std::path::PathBuf::from(path),
    }
}

/// Écrit un fichier lisible par l'utilisateur seul.
fn write_private(path: &std::path::Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|error| format!("{} : {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canal qui note ce qu'on lui envoie.
    #[derive(Default)]
    struct Notebook(Mutex<Vec<String>>);

    impl Channel for Notebook {
        fn send(&self, text: String) -> BoxFuture<'_, Result<Option<String>, String>> {
            locked(&self.0).push(text);
            Box::pin(async { Ok(None) })
        }
    }

    /// Un carnet : texte seul.
    fn meta(max_per_hour: u32) -> Meta {
        Meta {
            kind: "carnet",
            capabilities: ChannelCapabilities {
                text: true,
                voice_in: false,
                voice_out: false,
                buttons: false,
                attachments: false,
            },
            escalations: false,
            max_per_hour,
        }
    }

    #[tokio::test]
    async fn a_message_goes_out_trimmed_and_counts_toward_the_cap() {
        let channels = Channels::default();
        let notebook = Arc::new(Notebook::default());
        channels.register("carnet", meta(10), notebook.clone());
        let notice = channels.send("carnet", " Bonjour ").await.unwrap();
        assert_eq!(notice, "Envoyé sur carnet.");
        assert_eq!(*locked(&notebook.0), vec!["Bonjour".to_string()]);
        assert_eq!(channels.list()[0].sent_last_hour, 1);
    }

    #[tokio::test]
    async fn the_hourly_cap_refuses_and_unknown_channels_are_named() {
        let channels = Channels::default();
        channels.register("carnet", meta(2), Arc::new(Notebook::default()));
        channels.send("carnet", "un").await.unwrap();
        channels.send("carnet", "deux").await.unwrap();
        let refused = channels.send("carnet", "trois").await.unwrap_err();
        assert!(refused.contains("plafond"), "{refused}");
        let unknown = channels.send("signal", "salut").await.unwrap_err();
        assert!(unknown.contains("carnet"), "{unknown}");
        assert!(channels.send("carnet", "  ").await.is_err());
    }

    #[tokio::test]
    async fn a_channel_that_failed_to_start_is_listed_but_refuses() {
        let channels = Channels::default();
        channels.fail("carnet", meta(5), "jeton introuvable".into());
        let listed = channels.list();
        assert!(!listed[0].active);
        assert_eq!(listed[0].unavailable.as_deref(), Some("jeton introuvable"));
        let refused = channels.send("carnet", "salut").await.unwrap_err();
        assert!(refused.contains("jeton introuvable"), "{refused}");
    }

    #[test]
    fn a_prefix_tells_its_channel() {
        assert_eq!(origin("[Canal telegram]\nSalut"), Some("telegram".into()));
        assert_eq!(
            origin(&format!("{}\nSalut", prefix("bot-perso", true))),
            Some("bot-perso".into())
        );
        assert_eq!(origin("Salut"), None);
    }

    #[tokio::test]
    async fn a_turn_without_channel_send_hands_back_its_last_reply_only() {
        let channels = Channels::default();
        channels.register("carnet", meta(10), Arc::new(Notebook::default()));

        channels.opened("[Canal carnet, message vocal transcrit]\nSalut");
        channels.said("Je regarde.".into());
        channels.said("C'est fait.".into());
        assert_eq!(
            channels.completed(),
            Some(("carnet".into(), "C'est fait.".into()))
        );

        channels.opened("[Canal carnet]\nSalut");
        channels.send("carnet", "Réponse choisie").await.unwrap();
        channels.said("Réponse choisie".into());
        assert_eq!(channels.completed(), None, "déjà envoyé par channel_send");

        channels.opened("tapé dans la console");
        channels.said("Bonjour".into());
        assert_eq!(channels.completed(), None);
    }

    #[test]
    fn a_channel_section_picks_its_type() {
        let config: ChannelConfig = toml::from_str("type = \"telegram\"\nchat_id = 7").unwrap();
        assert_eq!(config.max_per_hour, 30);
        let ChannelKind::Telegram(telegram) = &config.kind;
        assert_eq!(telegram.chat_id, Some(7));
        assert_eq!(config.meta().kind, "telegram");
        assert!(toml::from_str::<ChannelConfig>("type = \"pigeon\"").is_err());
    }
}
