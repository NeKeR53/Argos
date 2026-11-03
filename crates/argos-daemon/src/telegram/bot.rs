//! Client minimal de l'API Bot de Telegram : long polling, texte, boutons, vocaux.
//!
//! Le jeton fait partie de chaque URL : aucune erreur rendue ne la contient
//! (`reqwest::Error::without_url`), pour qu'il n'apparaisse jamais dans un journal.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;

/// Au-delà, Telegram refuse le message : le texte est découpé.
pub const MAX_TEXT: usize = 4096;
/// Marge laissée au-delà de l'attente du long polling avant de tenir la requête pour perdue.
const POLL_MARGIN: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Bot {
    http: reqwest::Client,
    /// `{api}/bot{jeton}`
    methods: String,
    /// `{api}/file/bot{jeton}`
    files: String,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub chat: Chat,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub voice: Option<FileRef>,
    #[serde(default)]
    pub audio: Option<FileRef>,
    /// Une photo, en plusieurs tailles, de la plus petite à la plus grande.
    #[serde(default)]
    pub photo: Vec<FileRef>,
    #[serde(default)]
    pub document: Option<FileRef>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct FileRef {
    pub file_id: String,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub message: Option<Message>,
}

#[derive(Deserialize)]
struct Reply<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct File {
    file_path: Option<String>,
}

#[derive(Deserialize)]
struct Sent {
    message_id: i64,
}

impl Bot {
    /// `api` : `https://api.telegram.org`, ou un faux serveur dans les tests.
    pub fn new(api: &str, token: &str) -> Self {
        let api = api.trim_end_matches('/');
        Self {
            http: reqwest::Client::new(),
            methods: format!("{api}/bot{token}"),
            files: format!("{api}/file/bot{token}"),
        }
    }

    /// Attend les nouveautés au plus `wait` ; `offset` confirme celles déjà traitées.
    pub async fn updates(&self, offset: i64, wait: Duration) -> Result<Vec<Update>, String> {
        self.call_with(
            "getUpdates",
            json!({
                "offset": offset,
                "timeout": wait.as_secs(),
                "allowed_updates": ["message", "callback_query"],
            }),
            wait + POLL_MARGIN,
        )
        .await
    }

    /// Envoie du texte brut, découpé si besoin ; rend l'identifiant du dernier message.
    pub async fn send_text(&self, chat: i64, text: &str) -> Result<i64, String> {
        let mut last = 0;
        for part in split(text, MAX_TEXT) {
            last = self
                .call::<Sent>("sendMessage", json!({"chat_id": chat, "text": part}))
                .await?
                .message_id;
        }
        Ok(last)
    }

    /// Texte suivi de boutons, un par ligne ; chaque bouton renvoie sa `data`.
    pub async fn send_buttons(
        &self,
        chat: i64,
        text: &str,
        buttons: &[(String, String)],
    ) -> Result<i64, String> {
        let keyboard: Vec<Value> = buttons
            .iter()
            .map(|(label, data)| json!([{"text": label, "callback_data": data}]))
            .collect();
        let sent: Sent = self
            .call(
                "sendMessage",
                json!({
                    "chat_id": chat,
                    "text": truncate(text, MAX_TEXT),
                    "reply_markup": {"inline_keyboard": keyboard},
                }),
            )
            .await?;
        Ok(sent.message_id)
    }

    /// Accuse réception d'un bouton, avec une courte notice affichée par l'app.
    pub async fn answer_button(&self, query: &str, notice: &str) -> Result<(), String> {
        self.call::<Value>(
            "answerCallbackQuery",
            json!({"callback_query_id": query, "text": notice}),
        )
        .await
        .map(drop)
    }

    /// Montre « en train d'écrire… » ou « enregistre un vocal… » quelques secondes.
    pub async fn action(&self, chat: i64, action: &str) {
        let _ = self
            .call::<Value>("sendChatAction", json!({"chat_id": chat, "action": action}))
            .await;
    }

    /// Télécharge un fichier reçu (un vocal, par exemple).
    pub async fn download(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let file: File = self.call("getFile", json!({"file_id": file_id})).await?;
        let path = file
            .file_path
            .ok_or("Telegram ne donne pas le chemin du fichier")?;
        let response = self
            .http
            .get(format!("{}/{path}", self.files))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(hidden)?;
        Ok(response.bytes().await.map_err(hidden)?.to_vec())
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, body: Value) -> Result<T, String> {
        self.call_with(method, body, REQUEST_TIMEOUT).await
    }

    async fn call_with<T: DeserializeOwned>(
        &self,
        method: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<T, String> {
        let response = self
            .http
            .post(format!("{}/{method}", self.methods))
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(hidden)?;
        read(response).await
    }
}

async fn read<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
    let status = response.status();
    let reply: Reply<T> = response
        .json()
        .await
        .map_err(|error| format!("réponse illisible ({status}) : {}", hidden(error)))?;
    match (reply.ok, reply.result) {
        (true, Some(result)) => Ok(result),
        _ => Err(reply
            .description
            .unwrap_or_else(|| format!("refus de Telegram ({status})"))),
    }
}

/// L'erreur sans l'URL, qui contient le jeton.
fn hidden(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

/// Découpe en morceaux d'au plus `max` caractères, de préférence à un saut de ligne.
pub fn split(text: &str, max: usize) -> Vec<String> {
    let mut parts = vec![];
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let cut = rest
            .char_indices()
            .nth(max)
            .map_or(rest.len(), |(index, _)| index);
        let at = rest[..cut].rfind('\n').filter(|&at| at > 0).unwrap_or(cut);
        parts.push(rest[..at].trim_end().to_string());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() || parts.is_empty() {
        parts.push(rest.to_string());
    }
    parts
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_texts_are_split_at_line_breaks() {
        assert_eq!(split("court", 10), ["court"]);
        assert_eq!(split("un\ndeux trois", 8), ["un", "deux tro", "is"]);
        assert_eq!(split("ééééé", 2), ["éé", "éé", "é"]);
    }

    #[test]
    fn captions_are_truncated_on_a_character() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("éééé", 3), "éé…");
    }
}
