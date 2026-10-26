//! Réglages du moteur vocal, lus depuis la section `[voice]` d'un fichier TOML.

use crate::VoiceError;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// En dessous, les requêtes partielles se marchent dessus sans rien apporter.
const MIN_PARTIAL_INTERVAL_MS: u64 = 100;

/// Exemple de section, qui sert aussi de documentation :
///
/// ```toml
/// [voice]
/// whisper_model = "~/.cache/whisper/ggml-large-v3-turbo.bin"
/// whisper_server = "whisper-server"   # binaire, cherché dans le PATH
/// language = "fr"
/// voice = "Thomas"                    # voix de `say` (`say -v '?'` pour la liste)
/// partial_interval_ms = 700           # cadence des hypothèses partielles
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VoiceConfig {
    /// Chemin du modèle ggml ; `~/` est développé en dossier personnel.
    pub whisper_model: PathBuf,
    #[serde(default = "default_whisper_server")]
    pub whisper_server: PathBuf,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_partial_interval_ms")]
    pub partial_interval_ms: u64,
}

fn default_whisper_server() -> PathBuf {
    PathBuf::from("whisper-server")
}

fn default_language() -> String {
    "fr".into()
}

fn default_voice() -> String {
    "Thomas".into()
}

fn default_partial_interval_ms() -> u64 {
    700
}

#[derive(Deserialize)]
struct Document {
    voice: Option<VoiceConfig>,
}

impl VoiceConfig {
    /// Réglages par défaut autour d'un modèle donné.
    pub fn new(whisper_model: impl Into<PathBuf>) -> Self {
        Self {
            whisper_model: whisper_model.into(),
            whisper_server: default_whisper_server(),
            language: default_language(),
            voice: default_voice(),
            partial_interval_ms: default_partial_interval_ms(),
        }
    }

    /// Lit la section `[voice]` d'un document TOML complet (les autres sections sont
    /// ignorées), puis la vérifie.
    pub fn from_toml(text: &str) -> Result<Self, VoiceError> {
        let document: Document =
            toml::from_str(text).map_err(|error| VoiceError::Config(error.to_string()))?;
        let mut config = document
            .voice
            .ok_or_else(|| VoiceError::Config("section [voice] absente".into()))?;
        config.whisper_model = expand_home(&config.whisper_model);
        config.check()?;
        Ok(config)
    }

    /// Vérifie ce qui peut l'être sans rien lancer : le modèle existe, la cadence est
    /// raisonnable. Le binaire, lui, n'est vérifié qu'au lancement.
    pub fn check(&self) -> Result<(), VoiceError> {
        if !self.whisper_model.is_file() {
            return Err(VoiceError::ModelMissing(self.whisper_model.clone()));
        }
        if self.partial_interval_ms < MIN_PARTIAL_INTERVAL_MS {
            return Err(VoiceError::Config(format!(
                "partial_interval_ms doit valoir au moins {MIN_PARTIAL_INTERVAL_MS}"
            )));
        }
        if self.language.trim().is_empty() || self.voice.trim().is_empty() {
            return Err(VoiceError::Config(
                "language et voice ne peuvent pas être vides".into(),
            ));
        }
        Ok(())
    }

    pub fn partial_interval(&self) -> Duration {
        Duration::from_millis(self.partial_interval_ms)
    }
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_model(dir: &tempfile::TempDir, rest: &str) -> String {
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        format!(
            "[api]\nlisten = \"127.0.0.1:1\"\n\n[voice]\nwhisper_model = {:?}\n{rest}",
            model.to_str().unwrap()
        )
    }

    #[test]
    fn defaults_fill_the_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let config = VoiceConfig::from_toml(&with_model(&dir, "")).unwrap();
        assert_eq!(config.whisper_server, PathBuf::from("whisper-server"));
        assert_eq!(config.language, "fr");
        assert_eq!(config.voice, "Thomas");
        assert_eq!(config.partial_interval(), Duration::from_millis(700));
    }

    #[test]
    fn explicit_fields_win() {
        let dir = tempfile::tempdir().unwrap();
        let text = with_model(
            &dir,
            "whisper_server = \"/opt/bin/ws\"\nlanguage = \"en\"\nvoice = \"Amélie\"\npartial_interval_ms = 500\n",
        );
        let config = VoiceConfig::from_toml(&text).unwrap();
        assert_eq!(config.whisper_server, PathBuf::from("/opt/bin/ws"));
        assert_eq!(config.language, "en");
        assert_eq!(config.voice, "Amélie");
        assert_eq!(config.partial_interval_ms, 500);
    }

    #[test]
    fn a_missing_model_is_a_clear_error() {
        let error = VoiceConfig::from_toml("[voice]\nwhisper_model = \"/nulle/part/ggml.bin\"\n")
            .unwrap_err();
        assert!(matches!(error, VoiceError::ModelMissing(_)));
        assert!(error.to_string().contains("/nulle/part/ggml.bin"));
    }

    #[test]
    fn a_missing_section_or_field_is_reported() {
        assert!(matches!(
            VoiceConfig::from_toml("[api]\nlisten = \"x\"\n"),
            Err(VoiceError::Config(_))
        ));
        let error = VoiceConfig::from_toml("[voice]\nvoice = \"Thomas\"\n").unwrap_err();
        assert!(error.to_string().contains("whisper_model"));
    }

    #[test]
    fn a_too_fast_partial_cadence_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let text = with_model(&dir, "partial_interval_ms = 10\n");
        assert!(matches!(
            VoiceConfig::from_toml(&text),
            Err(VoiceError::Config(_))
        ));
    }

    #[test]
    fn the_home_prefix_is_expanded() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            expand_home(Path::new("~/models/x.bin")),
            PathBuf::from(home).join("models/x.bin")
        );
        assert_eq!(
            expand_home(Path::new("/abs/x.bin")),
            PathBuf::from("/abs/x.bin")
        );
    }
}
