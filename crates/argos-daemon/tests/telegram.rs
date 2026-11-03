//! Canal Telegram de bout en bout, sur un faux serveur Telegram et de faux ffmpeg,
//! whisper-cli et say : texte, vocal, envois de l'Assistant, inconnus, escalades. Aucun
//! token.

mod common;

use argos_core::EscalationState;
use argos_core::SurfaceKind;
use argos_core::protocol::{DaemonEvent, Origin};
use argos_daemon::channels::ChannelConfig;
use argos_daemon::telegram::bot::Bot;
use argos_daemon::telegram::speech::Speech;
use argos_daemon::telegram::{SpeechConfig, Telegram, TelegramConfig};
use axum::body::Bytes;
use axum::http::Uri;
use common::world::{World, poll, turn_completed, until, world, world_sharing_surfaces};
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
    bridge_with(w, chat_id, "").await
}

/// Le même, avec des réglages du canal en TOML.
async fn bridge_with(w: &World, chat_id: Option<i64>, settings: &str) -> FakeTelegram {
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
    let model = w.home.path().join("model.bin");
    std::fs::write(&model, "").unwrap();
    let config: TelegramConfig = toml::from_str(settings).unwrap();
    let config = TelegramConfig { chat_id, ..config };
    let speech = Speech {
        ffmpeg,
        whisper_cli: whisper,
        config: SpeechConfig {
            whisper_model: Some(model),
            ..SpeechConfig::default()
        },
        scratch: w.home.path().join("telegram"),
    };
    let meta = ChannelConfig::telegram(config.clone()).meta();
    let channel = Telegram::new(
        w.supervisor.clone(),
        w.assistant.id.clone(),
        "telegram",
        Bot::new(&url, "TOKEN"),
        config,
        speech,
        w.home.path(),
    );
    channel.spawn();
    w.supervisor.channels().register("telegram", meta, channel);
    tokio::spawn(argos_daemon::channels::follow(
        w.supervisor.clone(),
        w.assistant.id.clone(),
    ));
    fake
}

/// L'Assistant appelle `channel_send`, comme il le ferait par MCP.
async fn channel_send(w: &World, args: Value) -> Result<String, String> {
    argos_daemon::tools::call(&w.supervisor, &w.assistant, "channel_send", &args).await
}

#[tokio::test]
async fn a_text_message_reaches_the_assistant_and_its_reply_comes_back() {
    let mut w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let assistant = w.assistant.id.clone();

    fake.push(message(1, CHAT, json!({"text": "Bonjour"})));
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, from: Origin::User, text }
            if *agent == assistant && text == "[Canal telegram]\nBonjour")
    })
    .await;
    // Sans channel_send pendant le tour, sa réponse revient d'elle-même.
    let reply = fake.wait_for("sendMessage", "reply 1").await;
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["chat_id"],
        CHAT
    );

    // Un message tapé dans la console n'y renvoie rien.
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

    // L'Assistant peut aussi y écrire de lui-même.
    let sent = channel_send(&w, json!({"channel": "telegram", "text": "Bonjour à toi"}))
        .await
        .unwrap();
    assert_eq!(sent, "Envoyé sur telegram.");
    fake.wait_for("sendMessage", "Bonjour à toi").await;
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

    let unknown = channel_send(&w, json!({"channel": "signal", "text": "salut"}))
        .await
        .unwrap_err();
    assert!(unknown.contains("telegram"), "{unknown}");
}

#[tokio::test]
async fn a_voice_message_is_transcribed_and_answered_in_text() {
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
                && text == "[Canal telegram, message vocal transcrit]\nAllume la lumière")
    })
    .await;
    // Argos ne répond jamais en vocal : la réponse au vocal part en texte.
    fake.wait_for("sendMessage", "reply 1").await;
    assert!(fake.sent("sendVoice").is_empty());
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
    assert!(reply.contains("[channels.telegram]"));
    // Sans interlocuteur, l'Assistant ne peut rien y envoyer, et le sait.
    let listed = argos_daemon::tools::call(&w.supervisor, &w.assistant, "channel_list", &json!({}))
        .await
        .unwrap();
    assert!(listed.contains("chat_id pas encore configuré"), "{listed}");
    let refused = channel_send(&w, json!({"channel": "telegram", "text": "Salut"}))
        .await
        .unwrap_err();
    assert!(refused.contains("indisponible"), "{refused}");
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

#[tokio::test]
async fn a_photo_and_a_document_reach_the_assistant_as_attachments() {
    let mut w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let assistant = w.assistant.id.clone();

    // Une photo : la plus grande taille, en image native, légende comprise.
    fake.push(message(
        1,
        CHAT,
        json!({
            "photo": [{"file_id": "petite"}, {"file_id": "grande", "file_size": 8}],
            "caption": "Qu'est-ce que c'est ?",
        }),
    ));
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, text, .. }
            if *agent == assistant
                && text == "[Canal telegram]\nQu'est-ce que c'est ?\n\n[Image jointe : photo-101.jpg]")
    })
    .await;
    assert!(
        fake.sent("getFile")
            .iter()
            .any(|body| body.contains("grande"))
    );
    // Le faux Claude accuse réception de l'image, et sa réponse revient sur le canal.
    fake.wait_for("sendMessage", "image reçue").await;

    // Un PDF : enregistré sur le hub, l'Assistant en reçoit le chemin.
    fake.push(message(
        2,
        CHAT,
        json!({"document": {"file_id": "d1", "file_name": "devis.pdf", "mime_type": "application/pdf"}}),
    ));
    let saved = poll(|| {
        std::fs::read_dir(w.home.path().join("attachments").join(assistant.as_str()))
            .ok()?
            .next()
            .map(|entry| entry.unwrap().path())
    })
    .await;
    assert!(saved.to_string_lossy().ends_with("-devis.pdf"));
    assert_eq!(std::fs::read(&saved).unwrap(), b"OGG-RECU");

    // Trop lourd : refusé sans téléchargement.
    fake.push(message(
        3,
        CHAT,
        json!({"document": {"file_id": "lourd", "file_name": "film.mov", "file_size": 50_000_000}}),
    ));
    fake.wait_for("sendMessage", "Fichier trop lourd").await;
    assert!(
        fake.sent("getFile")
            .iter()
            .all(|body| !body.contains("lourd"))
    );
}

#[tokio::test]
async fn a_new_surface_is_announced_with_its_keyless_link() {
    let w = world_sharing_surfaces();
    let fake = bridge(&w, Some(CHAT)).await;
    let surface = w
        .supervisor
        .registry()
        .add_surface(&w.agent.id, SurfaceKind::Cockpit, "Lumières", "<p>ok</p>")
        .unwrap();
    w.supervisor.publish_surface(surface.clone(), false);

    let announce = fake
        .wait_for("sendMessage", "Domotique a créé la surface « Lumières »")
        .await;
    assert!(announce.contains(&format!("http://mac.local:7890/surfaces/{}", surface.id)));
    assert!(!announce.contains("?k="));

    // Une nouvelle version n'est pas annoncée à nouveau.
    let updated = w
        .supervisor
        .registry()
        .update_surface(&surface.id, Some("<p>v2</p>"), None)
        .unwrap();
    w.supervisor.publish_surface(updated, false);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let announces = fake
        .sent("sendMessage")
        .into_iter()
        .filter(|body| body.contains("a créé la surface"))
        .count();
    assert_eq!(announces, 1);
}

#[tokio::test]
async fn surfaces_open_only_on_the_mac_are_not_announced() {
    let w = world();
    let fake = bridge(&w, Some(CHAT)).await;
    let surface = w
        .supervisor
        .registry()
        .add_surface(&w.agent.id, SurfaceKind::Cockpit, "Lumières", "<p>ok</p>")
        .unwrap();
    w.supervisor.publish_surface(surface, false);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(fake.sent("sendMessage").is_empty());
}

/// Vrais programmes : un vocal fabriqué par `say` (macOS) puis converti par ffmpeg est
/// retranscrit. Demande say, ffmpeg, whisper-cli et `ARGOS_WHISPER_MODEL`.
#[tokio::test]
#[ignore]
async fn real_speech_transcription() {
    let scratch = tempfile::tempdir().unwrap();
    let aiff = scratch.path().join("vocal.aiff");
    let ogg = scratch.path().join("vocal.ogg");
    let made = std::process::Command::new("say")
        .args(["-v", "Thomas", "-o"])
        .arg(&aiff)
        .arg("La lumière du salon est allumée.")
        .status()
        .unwrap();
    assert!(made.success());
    let converted = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(&aiff)
        .args(["-c:a", "libopus"])
        .arg(&ogg)
        .status()
        .unwrap();
    assert!(converted.success());
    let speech = Speech {
        ffmpeg: "ffmpeg".into(),
        whisper_cli: "whisper-cli".into(),
        config: SpeechConfig {
            whisper_model: std::env::var_os("ARGOS_WHISPER_MODEL").map(PathBuf::from),
            ..SpeechConfig::default()
        },
        scratch: scratch.path().into(),
    };
    let heard = speech
        .transcribe(&std::fs::read(&ogg).unwrap())
        .await
        .unwrap()
        .to_lowercase();
    assert!(heard.contains("lumière"), "entendu : {heard}");
}
