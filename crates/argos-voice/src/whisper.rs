//! STT local : `whisper-server` (whisper.cpp) lancé en sous-process, qui garde le modèle
//! chargé, et le client qui lui envoie l'audio.
//!
//! Vérifié avec whisper.cpp 1.9.4 (Homebrew) : `GET /health` répond 200 une fois le
//! modèle chargé ; `POST /inference` prend un multipart (`file`, `language`,
//! `response_format=json`, `temperature`) et rend `{"text": "..."}` ou `{"error": "..."}`.

use crate::audio;
use crate::http::{self, Multipart};
use crate::stt::{BoxFuture, Transcriber};
use crate::{VoiceConfig, VoiceError};
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep, timeout};

/// Un gros modèle lu à froid depuis le disque peut demander plusieurs secondes.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const READY_POLL: Duration = Duration::from_millis(100);
/// Délai laissé à `whisper-server` pour sortir après SIGTERM avant d'être tué.
const GRACE: Duration = Duration::from_secs(2);
/// Garde-fou d'une requête : le large-v3-turbo transcrit 30 s d'audio en ~1 s sur M5 Pro.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Lignes de stderr gardées pour expliquer un échec de démarrage.
const STDERR_TAIL: usize = 20;

/// Un `whisper-server` en cours d'exécution, sur un port libre de la boucle locale.
/// Lâcher la valeur tue le process ; [`WhisperServer::shutdown`] l'arrête proprement.
pub struct WhisperServer {
    child: Child,
    addr: SocketAddr,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl WhisperServer {
    /// Lance le serveur avec le modèle configuré et attend qu'il soit prêt.
    pub async fn start(config: &VoiceConfig) -> Result<Self, VoiceError> {
        config.check()?;
        let port = free_port()?;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut command = Command::new(&config.whisper_server);
        command
            .arg("--model")
            .arg(&config.whisper_model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["--language", &config.language])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|source| VoiceError::Spawn {
            program: config.whisper_server.display().to_string(),
            source,
        })?;
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&stderr);
        let pipe = child.stderr.take().expect("stderr est redirigé");
        tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut tail = sink.lock().expect("verrou stderr empoisonné");
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
        let mut server = Self {
            child,
            addr,
            stderr,
        };
        server.wait_ready().await?;
        Ok(server)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Client prêt à transcrire avec ce serveur.
    pub fn client(&self, language: &str) -> WhisperClient {
        WhisperClient::new(self.addr, language)
    }

    async fn wait_ready(&mut self) -> Result<(), VoiceError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                // Laisse au lecteur de stderr le temps de récupérer les dernières lignes.
                sleep(Duration::from_millis(50)).await;
                return Err(VoiceError::ServerStart(format!(
                    "sorti ({status}) : {}",
                    self.stderr_tail()
                )));
            }
            if let Ok(Ok(response)) =
                timeout(READY_POLL, http::request(self.addr, "GET", "/health", None)).await
                && response.status == 200
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(VoiceError::ServerStart(format!(
                    "pas prêt après {} s : {}",
                    READY_TIMEOUT.as_secs(),
                    self.stderr_tail()
                )));
            }
            sleep(READY_POLL).await;
        }
    }

    fn stderr_tail(&self) -> String {
        let tail = self.stderr.lock().expect("verrou stderr empoisonné");
        tail.iter()
            .filter(|line| !line.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// SIGTERM, puis SIGKILL si le process traîne.
    pub async fn shutdown(mut self) {
        if let Some(pid) = self.child.id() {
            // SAFETY : kill ne touche à aucune mémoire ; au pire le process est déjà sorti.
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        if timeout(GRACE, self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
    }
}

/// Port TCP libre sur la boucle locale, au moment de l'appel.
fn free_port() -> Result<u16, VoiceError> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|error| VoiceError::ServerStart(format!("aucun port local libre : {error}")))
}

/// Client de `whisper-server` : un WAV 16 kHz mono par requête.
#[derive(Debug, Clone)]
pub struct WhisperClient {
    addr: SocketAddr,
    language: String,
}

impl WhisperClient {
    pub fn new(addr: SocketAddr, language: &str) -> Self {
        Self {
            addr,
            language: language.to_string(),
        }
    }

    async fn post(&self, samples: Vec<f32>) -> Result<String, VoiceError> {
        let wav = audio::encode_wav(&audio::pad_for_whisper(&samples));
        let (content_type, body) = inference_body(&self.language, &wav);
        let response = timeout(
            REQUEST_TIMEOUT,
            http::request(
                self.addr,
                "POST",
                "/inference",
                Some((&content_type, &body)),
            ),
        )
        .await
        .map_err(|_| {
            VoiceError::Transcription(format!("pas de réponse en {} s", REQUEST_TIMEOUT.as_secs()))
        })?
        .map_err(|error| VoiceError::Transcription(error.to_string()))?;
        parse_inference(response.status, &response.body)
    }
}

impl Transcriber for WhisperClient {
    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'_, Result<String, VoiceError>> {
        Box::pin(self.post(samples))
    }
}

fn inference_body(language: &str, wav: &[u8]) -> (String, Vec<u8>) {
    Multipart::new()
        .text("language", language)
        .text("response_format", "json")
        // Décodage glouton : déterministe, et le plus rapide pour les partielles.
        .text("temperature", "0.0")
        .file("file", "audio.wav", "audio/wav", wav)
        .finish()
}

fn parse_inference(status: u16, body: &[u8]) -> Result<String, VoiceError> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        VoiceError::Transcription(format!(
            "réponse illisible (HTTP {status}) : {}",
            String::from_utf8_lossy(body)
        ))
    })?;
    if let Some(error) = value.get("error").and_then(|error| error.as_str()) {
        return Err(VoiceError::Transcription(error.to_string()));
    }
    match value.get("text").and_then(|text| text.as_str()) {
        Some(text) if status == 200 => Ok(clean_transcript(text)),
        _ => Err(VoiceError::Transcription(format!(
            "réponse inattendue (HTTP {status}) : {value}"
        ))),
    }
}

/// Retire les annotations de Whisper (`[BLANK_AUDIO]`, `[Musique]`…) et normalise les
/// espaces : il ne reste que ce qui a été dit.
fn clean_transcript(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inference_request_carries_language_and_wav() {
        let wav = audio::encode_wav(&[0.0; 16]);
        let (content_type, body) = inference_body("fr", &wav);
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"language\"\r\n\r\nfr\r\n"));
        assert!(text.contains("name=\"response_format\"\r\n\r\njson\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
        let start = body.windows(4).position(|w| w == b"RIFF").unwrap();
        assert_eq!(&body[start..start + wav.len()], &wav[..]);
    }

    #[test]
    fn inference_responses_are_interpreted() {
        assert_eq!(
            parse_inference(200, br#"{"text":" Bonjour,\n tout le monde.\n"}"#).unwrap(),
            "Bonjour, tout le monde."
        );
        assert!(matches!(
            parse_inference(200, br#"{"error":"failed to read audio"}"#),
            Err(VoiceError::Transcription(message)) if message == "failed to read audio"
        ));
        assert!(parse_inference(500, b"<html>").is_err());
    }

    #[test]
    fn whisper_annotations_are_removed() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO]\n"), "");
        assert_eq!(
            clean_transcript(" [Musique] Bonjour  à tous [rires] !"),
            "Bonjour à tous !"
        );
    }

    #[tokio::test]
    async fn a_missing_binary_is_reported_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let mut config = VoiceConfig::new(&model);
        config.whisper_server = dir.path().join("pas-de-serveur");
        assert!(matches!(
            WhisperServer::start(&config).await,
            Err(VoiceError::Spawn { .. })
        ));
    }

    #[tokio::test]
    async fn a_server_that_dies_at_startup_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let script = dir.path().join("faux-serveur");
        std::fs::write(&script, "#!/bin/sh\necho 'modèle corrompu' >&2\nexit 3\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        let mut config = VoiceConfig::new(&model);
        config.whisper_server = script;
        match WhisperServer::start(&config).await {
            Err(VoiceError::ServerStart(message)) => {
                assert!(message.contains("modèle corrompu"), "{message}")
            }
            Err(other) => panic!("erreur inattendue : {other}"),
            Ok(_) => panic!("le faux serveur ne devrait pas être prêt"),
        }
    }
}
