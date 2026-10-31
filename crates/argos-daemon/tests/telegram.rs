//! Pont Telegram de bout en bout, sur un faux serveur Telegram et de faux ffmpeg,
//! whisper-cli et say : texte, vocal, inconnus, escalades. Aucun token.

mod common;

use argos_core::EscalationState;
use argos_core::protocol::{DaemonEvent, Origin};
use argos_daemon::telegram::bot::Bot;
use argos_daemon::telegram::speech::Speech;
use argos_daemon::telegram::{SpeechConfig, Telegram, TelegramConfig};
use axum::body::Bytes;
use axum::http::Uri;
use common::world::{World, poll, turn_completed, until, world};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CHAT: i64 = 42;

/// Faux serveur de l'API Bot : sert les nouveautés qu'on lui confie et note les envois.
#[derive(Clone, Default)]
struct FakeTelegram {
    updates: Arc<Mutex<VecDeque<Value>>>,
    sent: Arc<Mutex<Vec<(String, String)>>>,
    next_id: Arc<Mutex<i64>>,
}

impl FakeTelegram {
    async fn start() -> (Self, String) {
        let fake = Self::default();
        let state = fake.clone();
        let app = axum::Router::new().fallback(move |uri: Uri, body: Bytes| {
            let state = state.clone();
            async move { state.serve(uri, body).await }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, url)
    }

    async fn serve(&self, uri: Uri, body: Bytes) -> Vec<u8> {
        let path = uri.path();
        if path.starts_with("/file/botTOKEN/") {
            return b"OGG-RECU".to_vec();
        }
        let method = path.strip_prefix("/botTOKEN/").expect("jeton dans l'URL");
        let result = match method {
            "getUpdates" => {
                let pending: Vec<Value> = self.updates.lock().unwrap().drain(..).collect();
                if pending.is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                json!(pending)
            }
            "getFile" => json!({"file_path": "voice/1.oga"}),
            "sendMessage" => {
                let mut id = self.next_id.lock().unwrap();
                *id += 1;
                json!({"message_id": *id})
            }
            _ => json!(true),
        };
        if method != "getUpdates" {
            self.sent.lock().unwrap().push((
                method.to_string(),
                String::from_utf8_lossy(&body).into_owned(),
            ));
        }
        json!({"ok": true, "result": result})
            .to_string()
            .into_bytes()
    }

    fn push(&self, update: Value) {
        self.updates.lock().unwrap().push_back(update);
    }

    fn sent(&self, method: &str) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, body)| body.clone())
            .collect()
    }

    /// Attend un envoi par `method` dont le corps contient `needle`.
    async fn wait_for(&self, method: &str, needle: &str) -> String {
        poll(|| {
            self.sent(method)
                .into_iter()
                .find(|body| body.contains(needle))
        })
        .await
    }
}

fn message(id: i64, chat: i64, content: Value) -> Value {
    let mut message = json!({"message_id": 100 + id, "chat": {"id": chat}});
    message
        .as_object_mut()
        .unwrap()
        .extend(content.as_object().unwrap().clone());
    json!({"update_id": id, "message": message})
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/usr/bin/env bash\nset -eu\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Le pont branché sur le faux serveur, avec de faux programmes de vocaux.
async fn bridge(w: &World, chat_id: Option<i64>) -> FakeTelegram {
    let (fake, url) = FakeTelegram::start().await;
    let bin = w.home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // ffmpeg : copie l'entrée (-i) vers la sortie (dernier argument).
    let ffmpeg = script(
        &bin,
        "ffmpeg",
        r#"input=""; while [ $# -gt 1 ]; do [ "$1" = -i ] && input="$2"; shift; done; cp "$input" "$1""#,
    );
    let whisper = script(&bin, "whisper-cli", r#"echo "  Allume la lumière  ""#);
    let say = script(
        &bin,
        "say",
        r#"while [ $# -gt 0 ]; do [ "$1" = -o ] && out="$2"; shift; done; echo AIFF > "$out""#,
    );
    let model = w.home.path().join("model.bin");
    std::fs::write(&model, "").unwrap();
    let config: TelegramConfig = toml::from_str("").unwrap();
    let config = TelegramConfig { chat_id, ..config };
    let speech = Speech {
        ffmpeg,
        whisper_cli: whisper,
        say,
        config: SpeechConfig {
            whisper_model: Some(model),
            ..SpeechConfig::default()
        },
        scratch: w.home.path().join("telegram"),
    };
    Telegram::new(
        w.supervisor.clone(),
        w.assistant.id.clone(),
        Bot::new(&url, "TOKEN"),
        config,
        speech,
        w.home.path(),
    )
    .spawn();
    fake
}

#[tokio::test]
async fn a_text_message_reaches_the_assistant_and_its_reply_comes_back() {
    let mut w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let assistant = w.assistant.id.clone();

    fake.push(message(1, CHAT, json!({"text": "Bonjour"})));
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, from: Origin::User, text }
            if *agent == assistant && text == "[Depuis Telegram]\nBonjour")
    })
    .await;
    let reply = fake.wait_for("sendMessage", "reply 1").await;
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["chat_id"],
        CHAT
    );
    assert!(
        fake.sent("sendChatAction")
            .iter()
            .any(|b| b.contains("typing"))
    );
    // La nouveauté est confirmée : un redémarrage ne la rejouerait pas.
    poll(|| {
        (std::fs::read_to_string(w.home.path().join("telegram-offset")).ok()? == "2").then_some(())
    })
    .await;

    // Un message tapé dans la console ramène les réponses à la console.
    w.supervisor
        .send(&assistant, "depuis la console")
        .await
        .unwrap();
    turn_completed(&mut w.events, &assistant).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        fake.sent("sendMessage")
            .iter()
            .all(|b| !b.contains("reply 2"))
    );
}

#[tokio::test]
async fn a_voice_message_is_transcribed_and_answered_by_voice() {
    let mut w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let assistant = w.assistant.id.clone();

    fake.push(message(
        1,
        CHAT,
        json!({"voice": {"file_id": "v1", "duration": 2}}),
    ));
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, text, .. }
            if *agent == assistant
                && text == "[Depuis Telegram, message vocal transcrit]\nAllume la lumière")
    })
    .await;
    let voice = fake.wait_for("sendVoice", "reply 1").await;
    // Le faux ffmpeg recopie la sortie du faux say : c'est bien la synthèse qui part.
    assert!(voice.contains("AIFF"));
    assert!(voice.contains("audio/ogg"));
    assert!(fake.sent("sendMessage").is_empty());
    // Les fichiers de travail sont effacés.
    let scratch = std::fs::read_dir(w.home.path().join("telegram")).unwrap();
    assert_eq!(scratch.count(), 0);
}

#[tokio::test]
async fn strangers_are_ignored_and_an_unset_chat_learns_its_id() {
    let mut w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    fake.push(message(1, 7, json!({"text": "Je suis un inconnu"})));
    fake.push(message(2, CHAT, json!({"text": "/start"})));
    fake.wait_for("sendMessage", "Argos t'écoute").await;
    assert_eq!(fake.sent("sendMessage").len(), 1);
    assert!(w.events.try_recv().is_err(), "rien n'a été remis");

    let w = world();
    let fake = bridge(&w, None).await;
    fake.push(message(1, 7, json!({"text": "Salut"})));
    let reply = fake.wait_for("sendMessage", "chat_id = 7").await;
    assert!(reply.contains(r#""chat_id":7"#));
}

#[tokio::test]
async fn an_escalation_is_announced_and_answered_with_a_button() {
    let w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let escalation = w
        .supervisor
        .open_escalation(
            &w.agent.id,
            "Je supprime les vieux fichiers ?",
            &["Oui".into(), "Non".into()],
        )
        .unwrap();

    let announce = fake
        .wait_for("sendMessage", "Domotique a besoin de toi")
        .await;
    assert!(announce.contains("Je supprime les vieux fichiers ?"));
    assert!(announce.contains(&format!("e:{}:1", escalation.id)));

    fake.push(json!({
        "update_id": 1,
        "callback_query": {
            "id": "q1",
            "data": format!("e:{}:1", escalation.id),
            "message": {"message_id": 1, "chat": {"id": CHAT}},
        },
    }));
    fake.wait_for("answerCallbackQuery", "Réponse transmise : Non")
        .await;
    let answered = w.supervisor.registry().escalation(&escalation.id).unwrap();
    assert_eq!(answered.state, EscalationState::Answered);
    assert_eq!(answered.answer.as_deref(), Some("Non"));
}

#[tokio::test]
async fn quoting_an_escalation_answers_it_in_free_text() {
    let w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let escalation = w
        .supervisor
        .open_escalation(&w.agent.id, "Quel nom pour le dossier ?", &[])
        .unwrap();
    fake.wait_for("sendMessage", "Quel nom pour le dossier ?")
        .await;

    // Le faux serveur a donné l'identifiant 1 au message de l'escalade.
    fake.push(message(
        1,
        CHAT,
        json!({"text": "Archives 2026", "reply_to_message": {"message_id": 1, "chat": {"id": CHAT}}}),
    ));
    fake.wait_for("sendMessage", "Réponse transmise à Domotique")
        .await;
    let answered = w.supervisor.registry().escalation(&escalation.id).unwrap();
    assert_eq!(answered.answer.as_deref(), Some("Archives 2026"));
}

/// Vrais programmes : synthétise une phrase puis la retranscrit. Demande ffmpeg,
/// whisper-cli et `ARGOS_WHISPER_MODEL` ; `ARGOS_TTS=kokoro` essaie Kokoro (réglages
/// `ARGOS_KOKORO_PYTHON`, `ARGOS_KOKORO_MODEL`, `ARGOS_KOKORO_VOICES`).
#[tokio::test]
#[ignore]
async fn real_speech_round_trip() {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let scratch = tempfile::tempdir().unwrap();
    let speech = Speech {
        ffmpeg: "ffmpeg".into(),
        whisper_cli: "whisper-cli".into(),
        say: "say".into(),
        config: SpeechConfig {
            whisper_model: var("ARGOS_WHISPER_MODEL"),
            tts_backend: std::env::var("ARGOS_TTS").unwrap_or_else(|_| "say".into()),
            kokoro_python: var("ARGOS_KOKORO_PYTHON"),
            kokoro_model: var("ARGOS_KOKORO_MODEL"),
            kokoro_voices: var("ARGOS_KOKORO_VOICES"),
            ..SpeechConfig::default()
        },
        scratch: scratch.path().into(),
    };
    let ogg = speech
        .synthesize("**Bonjour**, la lumière du salon est allumée.")
        .await
        .unwrap();
    assert!(ogg.starts_with(b"OggS"));
    let heard = speech.transcribe(&ogg).await.unwrap().to_lowercase();
    assert!(heard.contains("lumière"), "entendu : {heard}");
}
