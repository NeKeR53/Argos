//! Moteur vocal local d'Argos (D37) : push-to-talk, STT et TTS en streaming, barge-in.
//!
//! Tout tourne sur le Mac : la capture micro passe par `cpal`, la reconnaissance par un
//! `whisper-server` (whisper.cpp) lancé en sous-process, la synthèse par la commande
//! système `say`. Seul le texte sort du moteur.
//!
//! La GUI ne voit que [`VoiceEngine`] : `press()` / `release()` pour le push-to-talk,
//! `speak_delta()` / `finish_speech()` / `stop_speech()` pour la parole, et un flux de
//! [`VoiceEvent`] pour animer l'orbe. Les trois backends ([`AudioSource`],
//! [`Transcriber`], [`Speaker`]) sont des traits : les tests les remplacent par des
//! faux, et un backend API payant pourra s'y brancher sans toucher à la façade.

pub mod audio;
pub mod capture;
pub mod config;
pub mod engine;
mod error;
mod http;
pub mod sentences;
pub mod stt;
pub mod tts;
pub mod whisper;

pub use capture::{ActiveCapture, AudioSource, CaptureSink, CpalSource};
pub use config::VoiceConfig;
pub use engine::{Backends, VoiceEngine, VoiceEvent};
pub use error::VoiceError;
pub use stt::{BoxFuture, Transcriber};
pub use tts::{SaySpeaker, Speaker};
pub use whisper::{WhisperClient, WhisperServer};
