//! Voix de l'app (D37) : branche le moteur `argos-voice` sur le push-to-talk, l'orbe et
//! le front. Réservée à l'Assistant (D3) : le front envoie la transcription à l'Assistant
//! seulement, et ne fait lire que ses réponses.
//!
//! Sans section `[voice]` (ou avec un modèle introuvable), l'app tourne sans voix : l'état
//! [`VoiceStatus`] dit pourquoi, à l'orbe comme à la console.
//!
//! Événements émis vers le front :
//! - [`EVENT_VOICE_STATUS`] : [`VoiceStatus`] (`{"status": "disabled" | "starting" |
//!   "ready" | "failed", "reason"?}`) ;
//! - [`EVENT_VOICE`] : un `argos_voice::VoiceEvent` tel quel (`{"type": "partial", …}`) ;
//! - [`EVENT_VOICE_TRANSCRIPT`] : `{"text", "simulated"}`, transcription finale d'un appui
//!   (ou injectée par `voice_simulate` en debug), que le front envoie à l'Assistant.

use crate::shell::{self, VoiceMode};
use argos_voice::{VoiceConfig, VoiceEngine, VoiceError, VoiceEvent};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::async_runtime::{self, Sender};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::broadcast::error::RecvError;

pub const EVENT_VOICE_STATUS: &str = "voice-status";
pub const EVENT_VOICE: &str = "voice-event";
pub const EVENT_VOICE_TRANSCRIPT: &str = "voice-transcript";

/// Niveau d'animation de la parole : `say` ne donne pas de niveau sonore.
const SPEAKING_LEVEL: f32 = 0.6;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VoiceStatus {
    /// Pas de voix, par réglage : section `[voice]` absente, modèle introuvable…
    Disabled {
        reason: String,
    },
    /// `whisper-server` charge le modèle.
    Starting,
    Ready,
    /// Le moteur n'a pas pu démarrer (binaire absent, serveur en échec).
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
struct Transcript {
    text: String,
    simulated: bool,
}

/// État de la voix partagé par l'app (`app.manage`).
pub struct Voice {
    status: Mutex<VoiceStatus>,
    engine: Mutex<Option<Arc<VoiceEngine>>>,
    orb: Arc<Mutex<OrbVoice>>,
    /// Appuis et relâchements, traités dans l'ordre par une seule tâche : un relâchement
    /// attend que l'ouverture du micro (jusqu'à 5 s) soit finie.
    ptt: Mutex<Option<Sender<bool>>>,
    /// Push-to-talk tenu : la répétition de touche ne rouvre rien.
    held: Mutex<bool>,
    /// `[gui] speak_typed_replies` : lire aussi les réponses aux demandes tapées.
    pub speak_typed_replies: bool,
}

/// Ce que le front demande à `voice_status`.
#[derive(Debug, Clone, Serialize)]
pub struct VoiceInfo {
    #[serde(flatten)]
    pub status: VoiceStatus,
    pub speak_typed_replies: bool,
}

impl Voice {
    pub fn new(speak_typed_replies: bool) -> Self {
        Self {
            status: Mutex::new(VoiceStatus::Starting),
            engine: Mutex::new(None),
            orb: Arc::default(),
            ptt: Mutex::new(None),
            held: Mutex::new(false),
            speak_typed_replies,
        }
    }

    pub fn info(&self) -> VoiceInfo {
        VoiceInfo {
            status: locked(&self.status).clone(),
            speak_typed_replies: self.speak_typed_replies,
        }
    }

    fn engine(&self) -> Option<Arc<VoiceEngine>> {
        locked(&self.engine).clone()
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Lit `[voice]` dans le texte de `config.toml` ; l'erreur est la raison, lisible, de
/// faire sans voix.
pub fn voice_config(config: Option<&str>) -> Result<VoiceConfig, String> {
    let Some(text) = config else {
        return Err("pas de config.toml dans $ARGOS_HOME : voix désactivée".into());
    };
    let table: toml::Table =
        toml::from_str(text).map_err(|error| format!("config.toml illisible : {error}"))?;
    if !table.contains_key("voice") {
        return Err("pas de section [voice] dans config.toml : voix désactivée".into());
    }
    VoiceConfig::from_toml(text).map_err(|error| error.to_string())
}

fn set_status<R: Runtime>(app: &AppHandle<R>, status: VoiceStatus) {
    if let Some(voice) = app.try_state::<Voice>() {
        *locked(&voice.status) = status.clone();
    }
    let _ = app.emit(EVENT_VOICE_STATUS, status);
}

/// Au démarrage : sans réglage valide, pas de voix ; sinon, démarre le moteur dans une
/// tâche (le chargement du modèle prend quelques secondes) sans bloquer l'app.
pub fn start<R: Runtime>(app: &AppHandle<R>, config: Option<&str>) {
    let config = match voice_config(config) {
        Ok(config) => config,
        Err(reason) => {
            eprintln!("voix : {reason}");
            set_status(app, VoiceStatus::Disabled { reason });
            return;
        }
    };
    set_status(app, VoiceStatus::Starting);
    let app = app.clone();
    async_runtime::spawn(async move {
        match VoiceEngine::start(&config).await {
            Ok(engine) => {
                let engine = Arc::new(engine);
                let Some(voice) = app.try_state::<Voice>() else {
                    return;
                };
                *locked(&voice.engine) = Some(Arc::clone(&engine));
                forward_events(&app, &engine, Arc::clone(&voice.orb));
                *locked(&voice.ptt) = Some(spawn_ptt_worker(app.clone(), engine));
                eprintln!("voix : prête");
                set_status(&app, VoiceStatus::Ready);
            }
            Err(error) => {
                eprintln!("voix : {error}");
                set_status(
                    &app,
                    VoiceStatus::Failed {
                        reason: error.to_string(),
                    },
                );
            }
        }
    });
}

/// Relaie les événements du moteur vers le front et l'orbe.
fn forward_events<R: Runtime>(app: &AppHandle<R>, engine: &VoiceEngine, orb: Arc<Mutex<OrbVoice>>) {
    let mut events = engine.subscribe();
    let app = app.clone();
    async_runtime::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                // Niveaux micro perdus : rien de grave.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            };
            if let VoiceEvent::Error { message } = &event {
                eprintln!("voix : {message}");
            }
            let _ = app.emit(EVENT_VOICE, &event);
            let state = locked(&orb).apply(&event);
            if let Some((mode, level)) = state {
                shell::set_voice_state(&app, mode, level);
            }
        }
    });
}

/// Une seule tâche traite appuis (`true`) et relâchements (`false`), dans l'ordre.
fn spawn_ptt_worker<R: Runtime>(app: AppHandle<R>, engine: Arc<VoiceEngine>) -> Sender<bool> {
    let (sender, mut receiver) = async_runtime::channel::<bool>(16);
    async_runtime::spawn(async move {
        while let Some(pressed) = receiver.recv().await {
            if pressed {
                if let Err(error) = engine.press().await {
                    report(&app, &error);
                    settle_orb(&app, OrbVoice::release);
                }
                continue;
            }
            match engine.release().await {
                Ok(text) => {
                    let _ = app.emit(
                        EVENT_VOICE_TRANSCRIPT,
                        Transcript {
                            text,
                            simulated: false,
                        },
                    );
                }
                // Appui qui a échoué : rien à transcrire.
                Err(VoiceError::NotListening) => {}
                Err(error) => report(&app, &error),
            }
        }
    });
    sender
}

fn report<R: Runtime>(app: &AppHandle<R>, error: &VoiceError) {
    eprintln!("voix : {error}");
    let _ = app.emit(
        EVENT_VOICE,
        VoiceEvent::Error {
            message: error.to_string(),
        },
    );
}

fn settle_orb<R: Runtime>(app: &AppHandle<R>, change: fn(&mut OrbVoice) -> OrbState) {
    if let Some(voice) = app.try_state::<Voice>() {
        let (mode, level) = change(&mut locked(&voice.orb));
        shell::set_voice_state(app, mode, level);
    }
}

/// Push-to-talk, appelé par le raccourci global à chaque appui et relâchement.
pub fn push_to_talk<R: Runtime>(app: &AppHandle<R>, pressed: bool) {
    let Some(voice) = app.try_state::<Voice>() else {
        return;
    };
    {
        let mut held = locked(&voice.held);
        if *held == pressed {
            // Répétition de touche, ou relâchement sans appui.
            return;
        }
        *held = pressed;
    }
    // Le front l'apprend toujours : il interrompt l'Assistant, ou dit pourquoi il n'y a
    // pas de voix.
    let _ = app.emit(shell::EVENT_PUSH_TO_TALK, shell::PushToTalk { pressed });
    let Some(sender) = locked(&voice.ptt).clone() else {
        return;
    };
    settle_orb(
        app,
        if pressed {
            OrbVoice::press
        } else {
            OrbVoice::release
        },
    );
    let _ = sender.try_send(pressed);
}

/// Transcription injectée comme si l'utilisateur l'avait dite (mode d'essai sans micro).
#[cfg(debug_assertions)]
pub fn simulate<R: Runtime>(app: &AppHandle<R>, text: String) {
    let _ = app.emit(
        EVENT_VOICE_TRANSCRIPT,
        Transcript {
            text,
            simulated: true,
        },
    );
}

pub fn speak_delta<R: Runtime>(app: &AppHandle<R>, text: &str) {
    // Trace de dev : ce que l'Assistant fait dire, pour suivre la boucle dans le journal.
    #[cfg(debug_assertions)]
    eprintln!("voix : à dire {text:?}");
    if let Some(engine) = app.try_state::<Voice>().and_then(|voice| voice.engine()) {
        engine.speak_delta(text);
    }
}

pub fn finish_speech<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(debug_assertions)]
    eprintln!("voix : fin de la réponse");
    if let Some(engine) = app.try_state::<Voice>().and_then(|voice| voice.engine()) {
        engine.finish_speech();
    }
}

pub fn stop_speech<R: Runtime>(app: &AppHandle<R>) {
    if let Some(engine) = app.try_state::<Voice>().and_then(|voice| voice.engine()) {
        engine.stop_speech();
    }
}

/// Arrêt du moteur (parole coupée, micro fermé, `whisper-server` arrêté). Sans effet
/// au second appel : le moteur est retiré.
pub async fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    let engine = app
        .try_state::<Voice>()
        .and_then(|voice| locked(&voice.engine).take());
    if let Some(engine) = engine {
        engine.shutdown().await;
        eprintln!("voix : arrêtée");
    }
}

/// État vocal de l'orbe : mode et niveau (0 à 1).
pub type OrbState = (Option<VoiceMode>, f32);

/// Ce que l'orbe montre de la voix : écoute pendant l'appui (niveau du micro), parole
/// entre `SpeechStarted` et `SpeechFinished`, rien sinon. L'écoute passe avant la parole.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct OrbVoice {
    listening: bool,
    speaking: bool,
}

impl OrbVoice {
    pub fn press(&mut self) -> OrbState {
        self.listening = true;
        (Some(VoiceMode::Listening), 0.0)
    }

    pub fn release(&mut self) -> OrbState {
        self.listening = false;
        self.current()
    }

    /// Nouvel état à montrer après cet événement, s'il change quelque chose.
    pub fn apply(&mut self, event: &VoiceEvent) -> Option<OrbState> {
        match event {
            VoiceEvent::Level { rms } if self.listening => {
                Some((Some(VoiceMode::Listening), level_from_rms(*rms)))
            }
            VoiceEvent::SpeechStarted => {
                self.speaking = true;
                Some(self.current())
            }
            VoiceEvent::SpeechFinished { .. } => {
                self.speaking = false;
                Some(self.current())
            }
            _ => None,
        }
    }

    fn current(&self) -> OrbState {
        if self.listening {
            (Some(VoiceMode::Listening), 0.0)
        } else if self.speaking {
            (Some(VoiceMode::Speaking), SPEAKING_LEVEL)
        } else {
            (None, 0.0)
        }
    }
}

/// La voix parlée tient entre 0,01 et 0,2 de RMS : racine pour étaler les faibles niveaux.
pub fn level_from_rms(rms: f32) -> f32 {
    if !rms.is_finite() {
        return 0.0;
    }
    (rms.max(0.0) * 5.0).sqrt().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_voice_section_the_app_runs_without_voice() {
        assert!(
            voice_config(None)
                .unwrap_err()
                .contains("pas de config.toml")
        );
        let reason = voice_config(Some("[assistant]\nengine = \"claude\"\n")).unwrap_err();
        assert!(reason.contains("pas de section [voice]"), "{reason}");
        let reason = voice_config(Some("[voice\n")).unwrap_err();
        assert!(reason.contains("config.toml illisible"), "{reason}");
    }

    #[test]
    fn an_invalid_voice_section_says_why() {
        let reason =
            voice_config(Some("[voice]\nwhisper_model = \"/nulle/part/ggml.bin\"\n")).unwrap_err();
        assert!(reason.contains("/nulle/part/ggml.bin"), "{reason}");
        let reason = voice_config(Some("[voice]\nlanguage = \"fr\"\n")).unwrap_err();
        assert!(reason.contains("whisper_model"), "{reason}");
    }

    #[test]
    fn a_valid_section_is_read() {
        let dir = std::env::temp_dir().join(format!("argos-gui-voice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("ggml.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let text = format!("[voice]\nwhisper_model = {:?}\nvoice = \"Amélie\"\n", model);
        let config = voice_config(Some(&text)).unwrap();
        assert_eq!(config.whisper_model, model);
        assert_eq!(config.voice, "Amélie");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_orb_listens_during_the_press_then_follows_the_speech() {
        let mut orb = OrbVoice::default();
        // Niveau hors appui : ignoré.
        assert_eq!(orb.apply(&VoiceEvent::Level { rms: 0.2 }), None);
        assert_eq!(orb.press(), (Some(VoiceMode::Listening), 0.0));
        assert_eq!(
            orb.apply(&VoiceEvent::Level { rms: 0.2 }),
            Some((Some(VoiceMode::Listening), 1.0))
        );
        assert_eq!(orb.release(), (None, 0.0));

        assert_eq!(
            orb.apply(&VoiceEvent::SpeechStarted),
            Some((Some(VoiceMode::Speaking), SPEAKING_LEVEL))
        );
        // Barge-in : l'appui reprend la main sur la parole.
        assert_eq!(orb.press(), (Some(VoiceMode::Listening), 0.0));
        assert_eq!(
            orb.apply(&VoiceEvent::SpeechFinished { interrupted: true }),
            Some((Some(VoiceMode::Listening), 0.0))
        );
        assert_eq!(orb.release(), (None, 0.0));
        assert_eq!(orb.apply(&VoiceEvent::Partial { text: "x".into() }), None);
    }

    #[test]
    fn the_microphone_level_is_spread_and_bounded() {
        assert_eq!(level_from_rms(0.0), 0.0);
        assert!((level_from_rms(0.05) - 0.5).abs() < 1e-6);
        assert_eq!(level_from_rms(3.0), 1.0);
        assert_eq!(level_from_rms(f32::NAN), 0.0);
        assert_eq!(level_from_rms(-1.0), 0.0);
    }
}
