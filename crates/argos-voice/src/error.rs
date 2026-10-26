//! Erreurs du moteur vocal.

use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("réglages vocaux invalides : {0}")]
    Config(String),
    #[error("modèle Whisper introuvable : {} (réglage voice.whisper_model)", .0.display())]
    ModelMissing(PathBuf),
    #[error("impossible de lancer {program} : {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("whisper-server n'a pas démarré : {0}")]
    ServerStart(String),
    #[error("transcription impossible : {0}")]
    Transcription(String),
    #[error("micro indisponible : {0}")]
    Capture(String),
    #[error("synthèse vocale impossible : {0}")]
    Speech(String),
    #[error("aucune écoute en cours")]
    NotListening,
    #[error("moteur vocal arrêté")]
    Closed,
}
