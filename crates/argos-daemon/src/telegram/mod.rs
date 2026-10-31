//! Pont Telegram (D43) : l'utilisateur parle à l'Assistant depuis son téléphone, en texte
//! ou en vocal, et y reçoit ses réponses et les escalades.
//!
//! Le pont relaie, il ne décide pas (D5). Un message reçu est remis à l'Assistant au nom
//! de l'utilisateur, préfixé [`PREFIX`] ou [`VOICE_PREFIX`]. Les réponses de l'Assistant
//! suivent le dernier canal de l'utilisateur : après un message Telegram, elles partent
//! sur Telegram (en vocal si on lui a parlé en vocal), jusqu'au prochain message tapé
//! dans la console. Les escalades y sont signalées avec leurs réponses en boutons.
//!
//! Long polling (`getUpdates`) : aucun port ouvert, rien à exposer. Un seul `chat_id` est
//! servi ; tant qu'il n'est pas configuré, le bot répond à chacun son identifiant.

pub mod bot;
pub mod speech;

use crate::supervisor::Supervisor;
use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentId, Escalation, EscalationState};
use bot::{Bot, MAX_CAPTION, Update};
use serde::Deserialize;
use speech::Speech;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

/// Préfixe d'un message tapé dans Telegram.
pub const PREFIX: &str = "[Depuis Telegram]";
/// Préfixe d'un vocal transcrit.
pub const VOICE_PREFIX: &str = "[Depuis Telegram, message vocal transcrit]";
/// Attente d'un long polling.
const POLL_WAIT: Duration = Duration::from_secs(50);
/// Pause après un échec réseau, avant de réessayer.
const RETRY: Duration = Duration::from_secs(5);

/// Section `[telegram]` de `config.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    /// Le seul interlocuteur servi ; absent, le bot donne son identifiant à qui lui écrit.
    #[serde(default)]
    pub chat_id: Option<i64>,
    /// Un vocal reçu appelle une réponse vocale.
    #[serde(default = "yes")]
    pub voice_replies: bool,
    /// Signaler les escalades sur Telegram.
    #[serde(default = "yes")]
    pub escalations: bool,
    /// Fichier du jeton du bot ; par défaut `$ARGOS_HOME/telegram-token`.
    #[serde(default)]
    pub token_file: Option<PathBuf>,
    #[serde(default = "default_api")]
    pub api: String,
    #[serde(default = "default_ffmpeg")]
    pub ffmpeg: PathBuf,
    #[serde(default = "default_whisper_cli")]
    pub whisper_cli: PathBuf,
}

/// Ce que le pont lit de la section `[voice]` (les autres réglages sont ceux de la GUI).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpeechConfig {
    #[serde(default)]
    pub whisper_model: Option<PathBuf>,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_tts_backend")]
    pub tts_backend: String,
    #[serde(default)]
    pub kokoro_python: Option<PathBuf>,
    #[serde(default)]
    pub kokoro_model: Option<PathBuf>,
    #[serde(default)]
    pub kokoro_voices: Option<PathBuf>,
    #[serde(default = "default_kokoro_voice")]
    pub kokoro_voice: String,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            whisper_model: None,
            language: default_language(),
            voice: default_voice(),
            tts_backend: default_tts_backend(),
            kokoro_python: None,
            kokoro_model: None,
            kokoro_voices: None,
            kokoro_voice: default_kokoro_voice(),
        }
    }
}

impl SpeechConfig {
    /// Les chemins avec `~/` développé.
    fn expanded(mut self) -> Self {
        for path in [
            &mut self.whisper_model,
            &mut self.kokoro_python,
            &mut self.kokoro_model,
            &mut self.kokoro_voices,
        ] {
            *path = path.take().map(|p| expand_home(&p));
        }
        self
    }
}

fn yes() -> bool {
    true
}
fn default_api() -> String {
    "https://api.telegram.org".into()
}
fn default_ffmpeg() -> PathBuf {
    "ffmpeg".into()
}
fn default_whisper_cli() -> PathBuf {
    "whisper-cli".into()
}
fn default_language() -> String {
    "fr".into()
}
fn default_voice() -> String {
    "Thomas".into()
}
fn default_tts_backend() -> String {
    "say".into()
}
fn default_kokoro_voice() -> String {
    "ff_siwis".into()
}

/// Où partent les réponses de l'Assistant : le dernier canal de l'utilisateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Console,
    Text,
    Voice,
}

/// Ce que le pont envoie, dans l'ordre, par une seule tâche.
enum Outgoing {
    Reply { text: String, voice: bool },
    Escalation(Escalation),
}

pub struct Telegram {
    bot: Bot,
    supervisor: Supervisor,
    assistant: AgentId,
    config: TelegramConfig,
    speech: Speech,
    /// Dernière nouveauté traitée, gardée sur disque : un redémarrage ne rejoue rien.
    offset_file: PathBuf,
    route: Mutex<Route>,
    /// Message Telegram d'une escalade → son identifiant, pour y répondre en citant.
    escalations: Mutex<HashMap<i64, String>>,
}

impl Telegram {
    /// Depuis `config.toml` : lit le jeton (`ARGOS_TELEGRAM_TOKEN`, sinon son fichier) et
    /// trouve les programmes des vocaux.
    pub fn from_config(
        supervisor: Supervisor,
        assistant: AgentId,
        config: TelegramConfig,
        voice: Option<SpeechConfig>,
        home: &Path,
    ) -> Result<Arc<Self>, String> {
        let token_file = config
            .token_file
            .clone()
            .map_or_else(|| home.join("telegram-token"), |p| expand_home(&p));
        let token = match std::env::var("ARGOS_TELEGRAM_TOKEN") {
            Ok(token) => token,
            Err(_) => std::fs::read_to_string(&token_file).map_err(|_| {
                format!(
                    "jeton du bot introuvable : ni ARGOS_TELEGRAM_TOKEN ni {}",
                    token_file.display()
                )
            })?,
        };
        let token = token.trim();
        if token.is_empty() {
            return Err(format!("jeton du bot vide dans {}", token_file.display()));
        }
        let bot = Bot::new(&config.api, token);
        let speech = Speech {
            ffmpeg: program(&config.ffmpeg),
            whisper_cli: program(&config.whisper_cli),
            say: program(Path::new("say")),
            config: voice.unwrap_or_default().expanded(),
            scratch: home.join("telegram"),
        };
        Ok(Self::new(supervisor, assistant, bot, config, speech, home))
    }

    pub fn new(
        supervisor: Supervisor,
        assistant: AgentId,
        bot: Bot,
        config: TelegramConfig,
        speech: Speech,
        home: &Path,
    ) -> Arc<Self> {
        Arc::new(Self {
            bot,
            supervisor,
            assistant,
            config,
            speech,
            offset_file: home.join("telegram-offset"),
            route: Mutex::new(Route::Console),
            escalations: Mutex::default(),
        })
    }

    /// Lance le relevé des messages, le suivi de l'Assistant et l'envoi.
    pub fn spawn(self: &Arc<Self>) {
        // Abonné avant tout relevé : aucune réponse ne peut passer avant lui.
        let events = self.supervisor.subscribe();
        let (outgoing, queue) = mpsc::unbounded_channel();
        tokio::spawn(Arc::clone(self).follow(events, outgoing));
        tokio::spawn(Arc::clone(self).send(queue));
        tokio::spawn(Arc::clone(self).poll());
    }

    async fn poll(self: Arc<Self>) {
        let mut offset = std::fs::read_to_string(&self.offset_file)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .unwrap_or(0);
        loop {
            match self.bot.updates(offset, POLL_WAIT).await {
                Ok(updates) => {
                    for update in updates {
                        offset = offset.max(update.update_id + 1);
                        let _ = std::fs::write(&self.offset_file, offset.to_string());
                        self.handle(update).await;
                    }
                }
                Err(error) => {
                    eprintln!("telegram : relevé impossible : {error}");
                    tokio::time::sleep(RETRY).await;
                }
            }
        }
    }

    async fn handle(&self, update: Update) {
        if let Some(query) = update.callback_query {
            return self.button(query).await;
        }
        let Some(message) = update.message else {
            return;
        };
        let chat = message.chat.id;
        match self.config.chat_id {
            None => {
                self.say(
                    chat,
                    &format!(
                        "Ce bot est privé. Ton identifiant Telegram est {chat} : ajoute \
                         « chat_id = {chat} » à la section [telegram] de config.toml, puis \
                         relance le daemon Argos."
                    ),
                )
                .await;
                return;
            }
            // Un inconnu n'obtient rien, pas même une réponse.
            Some(allowed) if allowed != chat => return,
            Some(_) => {}
        }
        let (text, voice) = if let Some(text) = message.text {
            if text.trim() == "/start" {
                self.say(chat, "Argos t'écoute : écris-moi ou envoie un vocal.")
                    .await;
                return;
            }
            (text, false)
        } else if let Some(file) = message.voice.or(message.audio) {
            self.bot.action(chat, "typing").await;
            match self.hear(&file.file_id).await {
                Ok(text) if !text.is_empty() => (text, true),
                Ok(_) => return self.say(chat, "Je n'ai rien entendu dans ce vocal.").await,
                Err(error) => {
                    eprintln!("telegram : vocal illisible : {error}");
                    return self
                        .say(chat, &format!("Je n'ai pas pu écouter ce vocal : {error}"))
                        .await;
                }
            }
        } else {
            return self
                .say(chat, "Je ne lis que le texte et les vocaux pour l'instant.")
                .await;
        };
        let quoted = message
            .reply_to_message
            .and_then(|quoted| self.escalation_at(quoted.message_id));
        if let Some(escalation) = quoted {
            return self.answer(chat, &escalation, &text).await;
        }
        self.bot.action(chat, "typing").await;
        let prefix = if voice { VOICE_PREFIX } else { PREFIX };
        let delivered = self
            .supervisor
            .deliver(&self.assistant, Origin::User, format!("{prefix}\n{text}"))
            .await;
        if let Err(error) = delivered {
            self.say(chat, &format!("Argos n'a pas pu transmettre : {error}"))
                .await;
        }
    }

    async fn hear(&self, file_id: &str) -> Result<String, String> {
        let ogg = self.bot.download(file_id).await?;
        self.speech.transcribe(&ogg).await
    }

    /// Un bouton d'escalade : `e:<id>:<rang de l'option>`.
    async fn button(&self, query: bot::CallbackQuery) {
        let chat = query.message.as_ref().map(|message| message.chat.id);
        if chat.is_none() || chat != self.config.chat_id {
            return;
        }
        let parsed = query.data.as_deref().and_then(|data| {
            let (id, rank) = data.strip_prefix("e:")?.rsplit_once(':')?;
            Some((id.to_string(), rank.parse::<usize>().ok()?))
        });
        let Some((id, rank)) = parsed else {
            let _ = self.bot.answer_button(&query.id, "Bouton inconnu").await;
            return;
        };
        let option = self
            .supervisor
            .registry()
            .escalation(&id)
            .ok()
            .and_then(|escalation| escalation.options.get(rank).cloned());
        let notice = match option {
            None => "Escalade introuvable".to_string(),
            Some(option) => match self.supervisor.answer_escalation(&id, &option).await {
                Ok(_) => format!("Réponse transmise : {option}"),
                Err(error) => error.to_string(),
            },
        };
        let _ = self.bot.answer_button(&query.id, &notice).await;
    }

    /// Une réponse libre, donnée en citant le message de l'escalade.
    async fn answer(&self, chat: i64, escalation: &str, text: &str) {
        let notice = match self.supervisor.answer_escalation(escalation, text).await {
            Ok(escalation) => format!("Réponse transmise à {}.", self.name(&escalation.agent)),
            Err(error) => format!("Réponse non transmise : {error}"),
        };
        self.say(chat, &notice).await;
    }

    fn escalation_at(&self, message_id: i64) -> Option<String> {
        locked(&self.escalations).get(&message_id).cloned()
    }

    /// Suit l'Assistant : le canal de l'utilisateur, ses réponses, les escalades.
    async fn follow(
        self: Arc<Self>,
        mut events: broadcast::Receiver<DaemonEvent>,
        outgoing: mpsc::UnboundedSender<Outgoing>,
    ) {
        let mut announced = HashSet::new();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            let next = match event {
                DaemonEvent::Input {
                    agent,
                    from: Origin::User,
                    text,
                } if agent == self.assistant => {
                    *locked(&self.route) = route_of(&text);
                    None
                }
                DaemonEvent::Message { agent, text } if agent == self.assistant => {
                    match *locked(&self.route) {
                        Route::Console => None,
                        route => Some(Outgoing::Reply {
                            text,
                            voice: route == Route::Voice && self.config.voice_replies,
                        }),
                    }
                }
                DaemonEvent::EscalationUpdated { escalation }
                    if self.config.escalations
                        && escalation.state == EscalationState::Open
                        && announced.insert(escalation.id.clone()) =>
                {
                    Some(Outgoing::Escalation(escalation))
                }
                _ => None,
            };
            if let Some(next) = next
                && outgoing.send(next).is_err()
            {
                return;
            }
        }
    }

    async fn send(self: Arc<Self>, mut queue: mpsc::UnboundedReceiver<Outgoing>) {
        while let Some(next) = queue.recv().await {
            let Some(chat) = self.config.chat_id else {
                continue;
            };
            match next {
                Outgoing::Reply { text, voice: true } => self.speak(chat, &text).await,
                Outgoing::Reply { text, voice: false } => self.say(chat, &text).await,
                Outgoing::Escalation(escalation) => self.announce(chat, &escalation).await,
            }
        }
    }

    /// Répond en vocal, le texte en légende ; en texte seul si la synthèse échoue.
    async fn speak(&self, chat: i64, text: &str) {
        self.bot.action(chat, "record_voice").await;
        match self.speech.synthesize(text).await {
            Ok(ogg) => {
                let short = text.chars().count() <= MAX_CAPTION;
                let sent = self.bot.send_voice(chat, ogg, short.then_some(text)).await;
                match sent {
                    Ok(()) if short => {}
                    Ok(()) => self.say(chat, text).await,
                    Err(error) => {
                        eprintln!("telegram : envoi du vocal impossible : {error}");
                        self.say(chat, text).await;
                    }
                }
            }
            Err(error) => {
                eprintln!("telegram : synthèse impossible : {error}");
                self.say(chat, text).await;
            }
        }
    }

    async fn announce(&self, chat: i64, escalation: &Escalation) {
        let text = format!(
            "{} a besoin de toi :\n{}\n\nRéponds en citant ce message{}.",
            self.name(&escalation.agent),
            escalation.question,
            if escalation.options.is_empty() {
                ""
            } else {
                ", ou choisis"
            }
        );
        let buttons: Vec<(String, String)> = escalation
            .options
            .iter()
            .enumerate()
            .map(|(rank, option)| (option.clone(), format!("e:{}:{rank}", escalation.id)))
            .collect();
        match self.bot.send_buttons(chat, &text, &buttons).await {
            Ok(message_id) => {
                locked(&self.escalations).insert(message_id, escalation.id.clone());
            }
            Err(error) => eprintln!("telegram : escalade non signalée : {error}"),
        }
    }

    async fn say(&self, chat: i64, text: &str) {
        if let Err(error) = self.bot.send_text(chat, text).await {
            eprintln!("telegram : envoi impossible : {error}");
        }
    }

    fn name(&self, agent: &AgentId) -> String {
        self.supervisor
            .record(agent)
            .ok()
            .and_then(|record| record.charter.map(|charter| charter.name))
            .unwrap_or_else(|| agent.to_string())
    }
}

/// Le canal d'un message de l'utilisateur à l'Assistant, d'après son préfixe.
fn route_of(text: &str) -> Route {
    if text.starts_with(VOICE_PREFIX) {
        Route::Voice
    } else if text.starts_with(PREFIX) {
        Route::Text
    } else {
        Route::Console
    }
}

/// Un programme nommé seul est cherché dans le `PATH`, puis chez Homebrew : un daemon
/// lancé par launchd n'a souvent qu'un `PATH` minimal.
fn program(name: &Path) -> PathBuf {
    if name.components().count() > 1 {
        return name.to_path_buf();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| name.to_path_buf())
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
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

    #[test]
    fn the_route_follows_the_last_channel() {
        assert_eq!(route_of("[Depuis Telegram]\nsalut"), Route::Text);
        assert_eq!(
            route_of("[Depuis Telegram, message vocal transcrit]\nsalut"),
            Route::Voice
        );
        assert_eq!(route_of("salut"), Route::Console);
    }

    #[test]
    fn an_empty_section_turns_the_bridge_on_with_defaults() {
        let config: TelegramConfig = toml::from_str("").unwrap();
        assert_eq!(config.chat_id, None);
        assert!(config.voice_replies && config.escalations);
        assert_eq!(config.api, "https://api.telegram.org");
    }
}
