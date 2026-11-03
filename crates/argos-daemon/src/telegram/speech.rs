//! Vocaux Telegram reçus, transcrits en local : conversion par `ffmpeg`, transcription
//! par `whisper-cli`. Le son ne quitte pas la machine. Argos ne répond jamais en vocal :
//! la réponse part en texte.
//!
//! Vérifié avec whisper.cpp 1.9.4 et ffmpeg 8 : `whisper-cli -nt -np` écrit la seule
//! transcription sur stdout.

use super::SpeechConfig;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

/// Garde-fou d'une commande : un long vocal se transcrit en quelques secondes.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Les programmes et réglages des vocaux.
#[derive(Debug, Clone)]
pub struct Speech {
    pub ffmpeg: PathBuf,
    pub whisper_cli: PathBuf,
    pub config: SpeechConfig,
    /// Fichiers de travail, effacés après chaque vocal.
    pub scratch: PathBuf,
}

impl Speech {
    /// Transcrit un vocal Ogg/Opus ; rend le texte entendu, vide s'il n'y a rien.
    pub async fn transcribe(&self, ogg: &[u8]) -> Result<String, String> {
        let model = self
            .config
            .whisper_model
            .as_deref()
            .ok_or("pas de whisper_model dans la section [voice] de config.toml")?;
        let work = Work::new(&self.scratch)?;
        let (input, wav) = (work.file("in.ogg"), work.file("in.wav"));
        std::fs::write(&input, ogg).map_err(|error| error.to_string())?;
        run(Command::new(&self.ffmpeg)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&input)
            .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
            .arg(&wav))
        .await?;
        let seconds = wav_seconds(&wav);
        let text = run(Command::new(&self.whisper_cli)
            .arg("-m")
            .arg(model)
            .args(["-l", &self.config.language, "-nt", "-np"])
            // Fenêtre de l'encodeur à la mesure du vocal plutôt que 30 s : quatre fois plus
            // rapide sur un vocal court, ce qui compte sur un Raspberry Pi.
            .args(["--audio-ctx", &audio_ctx(seconds).to_string()])
            // Le nom propre est mieux reconnu s'il est annoncé.
            .args(["--prompt", "Argos"])
            .arg("-f")
            .arg(&wav))
        .await?;
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// Durée d'un WAV 16 kHz mono 16 bits, d'après sa taille (en-tête de 44 octets).
fn wav_seconds(wav: &Path) -> f64 {
    let bytes = std::fs::metadata(wav).map(|meta| meta.len()).unwrap_or(0);
    bytes.saturating_sub(44) as f64 / 32_000.0
}

/// Contexte audio de whisper pour `seconds` de son : 1500 couvrent 30 s, avec une marge
/// d'une seconde et demie ; au moins 128, au plus 1500.
fn audio_ctx(seconds: f64) -> u32 {
    ((seconds * 50.0).ceil() as u32 + 75).clamp(128, 1500)
}

/// Un dossier de travail propre à un vocal, effacé quand on le lâche.
struct Work(PathBuf);

impl Work {
    fn new(scratch: &Path) -> Result<Self, String> {
        let dir = scratch.join(Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        Ok(Self(dir))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Lance la commande et rend sa sortie standard ; l'échec dit ce qu'elle a écrit.
async fn run(command: &mut Command) -> Result<String, String> {
    let program = PathBuf::from(command.as_std().get_program());
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("{} : {error}", program.display()))?;
    finish(child, &program).await
}

async fn finish(child: tokio::process::Child, program: &Path) -> Result<String, String> {
    let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{} ne répond plus", program.display()))?
        .map_err(|error| format!("{} : {error}", program.display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "{} a échoué ({}) : {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoder_window_fits_the_voice_message() {
        assert_eq!(audio_ctx(0.0), 128);
        assert_eq!(audio_ctx(4.0), 275);
        assert_eq!(audio_ctx(29.0), 1500);
        assert_eq!(audio_ctx(120.0), 1500);
    }
}
