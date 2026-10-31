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
    /// `Some` quand le canal ne peut pas encore servir.
    fn unavailable(&self) -> Option<String> {
        None
    }
    /// Envoie à l'unique interlocuteur. `voice` n'est demandé que si le type sait parler.
    /// `Ok(Some(…))` : envoyé, mais dégradé (par exemple en texte faute de synthèse).
    fn send(&self, text: String, voice: bool) -> BoxFuture<'_, Result<Option<String>, String>>;
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

/// Le canal d'un message préfixé par [`prefix`], et s'il vient d'un vocal.
fn origin(text: &str) -> Option<(String, bool)> {
    let line = text.lines().next()?.strip_prefix("[Canal ")?;
    match line.strip_suffix(VOICE_MARK) {
        Some(id) => Some((id.to_string(), true)),
        None => Some((line.strip_suffix(']')?.to_string(), false)),
    }
}

/// Tour de l'Assistant ouvert par un message reçu sur un canal.
struct Turn {
    channel: String,
    voice: bool,
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
        if let Some((channel, text, voice)) = due
            && let Err(error) = channels.send(&channel, &text, voice).await
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
        *locked(&self.turn) = origin(text).map(|(channel, voice)| Turn {
            channel,
            voice,
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
    fn completed(&self) -> Option<(String, String, bool)> {
        let turn = locked(&self.turn).take()?;
        if turn.answered {
            return None;
        }
        Some((turn.channel, turn.last?, turn.voice))
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
    pub async fn send(&self, id: &str, text: &str, voice: bool) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("message vide".into());
        }
        let (channel, speaks) = {
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
            (channel, entry.meta.capabilities.voice_out)
        };
        let degraded = channel.send(text.to_string(), voice && speaks).await?;
        if let Some(turn) = locked(&self.turn).as_mut() {
            turn.answered = true;
        }
        let mut notice = format!("Envoyé sur {id}");
        if voice && !speaks {
            notice.push_str(", en texte : ce canal n'envoie pas de vocal");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Canal qui note ce qu'on lui envoie.
    #[derive(Default)]
    struct Notebook(Mutex<Vec<(String, bool)>>);

    impl Channel for Notebook {
        fn send(&self, text: String, voice: bool) -> BoxFuture<'_, Result<Option<String>, String>> {
            locked(&self.0).push((text, voice));
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
    async fn a_voice_request_on_a_text_channel_goes_out_as_text_and_says_so() {
        let channels = Channels::default();
        let notebook = Arc::new(Notebook::default());
        channels.register("carnet", meta(10), notebook.clone());
        let notice = channels.send("carnet", " Bonjour ", true).await.unwrap();
        assert!(notice.contains("en texte"), "{notice}");
        assert_eq!(*locked(&notebook.0), vec![("Bonjour".to_string(), false)]);
        assert_eq!(channels.list()[0].sent_last_hour, 1);
    }

    #[tokio::test]
    async fn the_hourly_cap_refuses_and_unknown_channels_are_named() {
        let channels = Channels::default();
        channels.register("carnet", meta(2), Arc::new(Notebook::default()));
        channels.send("carnet", "un", false).await.unwrap();
        channels.send("carnet", "deux", false).await.unwrap();
        let refused = channels.send("carnet", "trois", false).await.unwrap_err();
        assert!(refused.contains("plafond"), "{refused}");
        let unknown = channels.send("signal", "salut", false).await.unwrap_err();
        assert!(unknown.contains("carnet"), "{unknown}");
        assert!(channels.send("carnet", "  ", false).await.is_err());
    }

    #[tokio::test]
    async fn a_channel_that_failed_to_start_is_listed_but_refuses() {
        let channels = Channels::default();
        channels.fail("carnet", meta(5), "jeton introuvable".into());
        let listed = channels.list();
        assert!(!listed[0].active);
        assert_eq!(listed[0].unavailable.as_deref(), Some("jeton introuvable"));
        let refused = channels.send("carnet", "salut", false).await.unwrap_err();
        assert!(refused.contains("jeton introuvable"), "{refused}");
    }

    #[test]
    fn a_prefix_tells_its_channel_and_whether_it_was_spoken() {
        assert_eq!(
            origin("[Canal telegram]\nSalut"),
            Some(("telegram".into(), false))
        );
        assert_eq!(
            origin(&format!("{}\nSalut", prefix("bot-perso", true))),
            Some(("bot-perso".into(), true))
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
            Some(("carnet".into(), "C'est fait.".into(), true))
        );

        channels.opened("[Canal carnet]\nSalut");
        channels
            .send("carnet", "Réponse choisie", false)
            .await
            .unwrap();
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
