//! Façade du moteur vocal pour la GUI : push-to-talk, parole, événements.

use crate::capture::{ActiveCapture, AudioSource, CaptureSink, CpalSource};
use crate::stt::{self, Transcriber, Turn};
use crate::tts::{SaySpeaker, Speaker, SpeechQueue};
use crate::whisper::WhisperServer;
use crate::{VoiceConfig, VoiceError};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

/// Capacité du flux d'événements : un abonné qui prend plus de ~10 s de retard perd
/// les plus anciens (des niveaux micro, surtout) plutôt que de faire gonfler la mémoire.
const EVENT_CAPACITY: usize = 256;
/// Un appui de moins de 300 ms est un faux départ : pas de transcription.
const MIN_FINAL_SAMPLES: usize = crate::audio::SAMPLE_RATE as usize * 3 / 10;

/// Ce que le moteur publie, pour l'orbe et la console. Sérialisé tel quel vers le front
/// (`{"type": "partial", "text": "..."}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    /// Niveau du micro pendant l'appui (RMS sur 50 ms, entre 0 et 1).
    Level {
        rms: f32,
    },
    /// Hypothèse en cours d'appui ; chacune remplace la précédente.
    Partial {
        text: String,
    },
    /// Transcription au relâchement ; toujours après la dernière partielle.
    Final {
        text: String,
    },
    SpeechStarted,
    /// Phrase confiée à la synthèse, émise au lancement de chaque `say` : son texte
    /// exact, pour animer la parole (le TTS système ne donne pas de niveau sonore).
    Utterance {
        text: String,
    },
    /// Fin du discours : dit jusqu'au bout, ou coupé par un barge-in.
    SpeechFinished {
        interrupted: bool,
    },
    /// Erreur survenue hors d'un appel (partielle, synthèse, flux micro).
    Error {
        message: String,
    },
}

/// Les trois backends du moteur ; des faux dans les tests, une API payante un jour.
pub struct Backends {
    pub source: Arc<dyn AudioSource>,
    pub transcriber: Arc<dyn Transcriber>,
    pub speaker: Arc<dyn Speaker>,
}

/// Le moteur vocal. `Send + Sync` : il se partage derrière un `Arc` entre les commandes
/// Tauri. Aucune méthode ne bloque le thread appelant ; `press` et `release` sont
/// asynchrones, les méthodes de parole ne font qu'envoyer un message.
pub struct VoiceEngine {
    events: broadcast::Sender<VoiceEvent>,
    source: Arc<dyn AudioSource>,
    transcriber: Arc<dyn Transcriber>,
    speech: SpeechQueue,
    partial_interval: Duration,
    listening: Mutex<Option<Listening>>,
    server: Mutex<Option<WhisperServer>>,
}

/// Un appui en cours.
struct Listening {
    capture: Box<dyn ActiveCapture>,
    sink: CaptureSink,
    turn: Arc<Turn>,
    partials: JoinHandle<()>,
}

impl VoiceEngine {
    /// Moteur local complet : lance `whisper-server` avec le modèle configuré (et attend
    /// qu'il soit chargé), micro par défaut, voix `say` configurée.
    pub async fn start(config: &VoiceConfig) -> Result<Self, VoiceError> {
        let server = WhisperServer::start(config).await?;
        let engine = Self::with_backends(
            Backends {
                source: Arc::new(CpalSource),
                transcriber: Arc::new(server.client(&config.language)),
                speaker: Arc::new(SaySpeaker::new(&config.voice)),
            },
            config.partial_interval(),
        );
        *engine.server.lock().await = Some(server);
        Ok(engine)
    }

    /// Moteur sur des backends fournis. À appeler depuis un runtime tokio (la file de
    /// parole y démarre sa tâche).
    pub fn with_backends(backends: Backends, partial_interval: Duration) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let speech = SpeechQueue::spawn(backends.speaker, events.clone());
        Self {
            events,
            source: backends.source,
            transcriber: backends.transcriber,
            speech,
            partial_interval,
            listening: Mutex::new(None),
            server: Mutex::new(None),
        }
    }

    /// Nouvel abonné au flux d'événements (ceux d'avant l'abonnement sont perdus).
    pub fn subscribe(&self) -> broadcast::Receiver<VoiceEvent> {
        self.events.subscribe()
    }

    /// Appui du push-to-talk : coupe la parole en cours (barge-in) et ouvre le micro.
    /// Sans effet si l'écoute est déjà ouverte.
    pub async fn press(&self) -> Result<(), VoiceError> {
        self.speech.stop();
        let mut listening = self.listening.lock().await;
        if listening.is_some() {
            return Ok(());
        }
        let sink = CaptureSink::new(self.events.clone());
        let source = Arc::clone(&self.source);
        let source_sink = sink.clone();
        // Ouvrir un périphérique audio peut prendre quelques dizaines de ms.
        let capture = tokio::task::spawn_blocking(move || source.start(source_sink))
            .await
            .map_err(|_| VoiceError::Closed)??;
        let turn = Arc::new(Turn::new(self.events.clone()));
        let partials = tokio::spawn(stt::run_partials(
            Arc::clone(&self.transcriber),
            sink.clone(),
            Arc::clone(&turn),
            self.partial_interval,
        ));
        *listening = Some(Listening {
            capture,
            sink,
            turn,
            partials,
        });
        Ok(())
    }

    /// Relâchement : ferme le micro et rend la transcription finale (vide pour un appui
    /// trop bref). Elle est aussi publiée en [`VoiceEvent::Final`].
    pub async fn release(&self) -> Result<String, VoiceError> {
        let listening = self.listening.lock().await.take();
        let Some(listening) = listening else {
            return Err(VoiceError::NotListening);
        };
        let samples = stop_listening(listening).await;
        let text = if samples.len() < MIN_FINAL_SAMPLES {
            String::new()
        } else {
            self.transcriber.transcribe(samples).await?
        };
        let _ = self.events.send(VoiceEvent::Final { text: text.clone() });
        Ok(text)
    }

    /// Fragment de la réponse à prononcer ; chaque phrase part dès qu'elle est complète.
    pub fn speak_delta(&self, delta: &str) {
        self.speech.delta(delta);
    }

    /// Fin de la réponse : le reste est prononcé même sans ponctuation finale.
    pub fn finish_speech(&self) {
        self.speech.finish();
    }

    /// Coupe immédiatement la phrase en cours et oublie la suite.
    pub fn stop_speech(&self) {
        self.speech.stop();
    }

    /// Arrêt complet : parole coupée, micro fermé, `whisper-server` arrêté proprement.
    /// Le moteur reste utilisable pour la parole, plus pour la transcription.
    pub async fn shutdown(&self) {
        self.speech.stop();
        if let Some(listening) = self.listening.lock().await.take() {
            stop_listening(listening).await;
        }
        if let Some(server) = self.server.lock().await.take() {
            server.shutdown().await;
        }
    }
}

/// Ferme le micro et les partielles, et rend l'audio de l'appui.
async fn stop_listening(listening: Listening) -> Vec<f32> {
    let Listening {
        capture,
        sink,
        turn,
        partials,
    } = listening;
    partials.abort();
    turn.close();
    let _ = tokio::task::spawn_blocking(move || capture.stop()).await;
    sink.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::tests::SlowTranscriber;
    use crate::tts::tests::{FakeSpeaker, Spoken};
    use std::sync::Mutex as StdMutex;

    /// Fausse source : garde le sink pour que le test y pousse l'audio à la main.
    #[derive(Default)]
    struct FakeSource {
        sink: StdMutex<Option<CaptureSink>>,
        starts: StdMutex<usize>,
        stops: Arc<StdMutex<usize>>,
    }

    struct FakeCapture {
        stops: Arc<StdMutex<usize>>,
    }

    impl ActiveCapture for FakeCapture {
        fn stop(self: Box<Self>) {
            *self.stops.lock().unwrap() += 1;
        }
    }

    impl AudioSource for FakeSource {
        fn start(&self, sink: CaptureSink) -> Result<Box<dyn ActiveCapture>, VoiceError> {
            *self.sink.lock().unwrap() = Some(sink);
            *self.starts.lock().unwrap() += 1;
            Ok(Box::new(FakeCapture {
                stops: Arc::clone(&self.stops),
            }))
        }
    }

    impl FakeSource {
        fn push(&self, samples: &[f32]) {
            self.sink.lock().unwrap().as_ref().unwrap().push(samples);
        }
    }

    struct BrokenSource;

    impl AudioSource for BrokenSource {
        fn start(&self, _: CaptureSink) -> Result<Box<dyn ActiveCapture>, VoiceError> {
            Err(VoiceError::Capture("aucun périphérique d'entrée".into()))
        }
    }

    struct Fixture {
        engine: Arc<VoiceEngine>,
        source: Arc<FakeSource>,
        transcriber: Arc<SlowTranscriber>,
        speaker: Arc<FakeSpeaker>,
        events: broadcast::Receiver<VoiceEvent>,
    }

    fn fixture(transcription_delay: Duration) -> Fixture {
        let source = Arc::new(FakeSource::default());
        let transcriber = Arc::new(SlowTranscriber::new(transcription_delay));
        let speaker = Arc::new(FakeSpeaker::new(Duration::from_millis(50)));
        let engine = Arc::new(VoiceEngine::with_backends(
            Backends {
                source: source.clone(),
                transcriber: transcriber.clone(),
                speaker: speaker.clone(),
            },
            Duration::from_millis(700),
        ));
        let events = engine.subscribe();
        Fixture {
            engine,
            source,
            transcriber,
            speaker,
            events,
        }
    }

    fn drain(rx: &mut broadcast::Receiver<VoiceEvent>) -> Vec<VoiceEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn the_engine_can_live_behind_an_arc_in_tauri() {
        assert_send_sync::<VoiceEngine>();
        assert_send_sync::<Arc<VoiceEngine>>();
    }

    #[tokio::test(start_paused = true)]
    async fn press_and_release_give_partials_then_the_final_transcript() {
        let mut f = fixture(Duration::from_millis(100));
        f.engine.press().await.unwrap();
        // Blocs décalés de 50 ms par rapport aux échéances (700 ms, 1,4 s) : pas d'ex æquo.
        tokio::time::sleep(Duration::from_millis(50)).await;
        for _ in 0..15 {
            f.source.push(&[0.2; 1_600]);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let text = f.engine.release().await.unwrap();
        assert_eq!(text, "24000 échantillons");
        assert_eq!(*f.source.stops.lock().unwrap(), 1);

        let events = drain(&mut f.events);
        let levels = events
            .iter()
            .filter(|e| matches!(e, VoiceEvent::Level { rms } if (rms - 0.2).abs() < 1e-6))
            .count();
        assert_eq!(levels, 30); // 1,5 s par fenêtres de 50 ms
        let partials: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                VoiceEvent::Partial { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            partials,
            vec![
                "11200 échantillons".to_string(),
                "22400 échantillons".into()
            ]
        );
        assert_eq!(
            events.last(),
            Some(&VoiceEvent::Final {
                text: "24000 échantillons".into()
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_partial_arrives_after_the_final() {
        // Partielle lente, encore en vol au relâchement.
        let mut f = fixture(Duration::from_millis(600));
        f.engine.press().await.unwrap();
        f.source.push(&[0.2; 16_000]);
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(f.transcriber.calls.lock().unwrap().len(), 1);
        f.engine.release().await.unwrap();
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        let events = drain(&mut f.events);
        assert!(matches!(events.last(), Some(VoiceEvent::Final { .. })));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, VoiceEvent::Partial { .. }))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn press_interrupts_the_speech() {
        let mut f = fixture(Duration::from_millis(10));
        f.engine
            .speak_delta("Voici une longue réponse. Et une suite.");
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.engine.press().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(
            f.speaker.log(),
            vec![
                Spoken::Started("Voici une longue réponse.".into()),
                Spoken::Cut("Voici une longue réponse.".into()),
            ]
        );
        assert_eq!(
            drain(&mut f.events),
            vec![
                VoiceEvent::SpeechStarted,
                VoiceEvent::Utterance {
                    text: "Voici une longue réponse.".into()
                },
                VoiceEvent::SpeechFinished { interrupted: true }
            ]
        );
        // Un appui trop bref ne part pas en transcription.
        f.source.push(&[0.0; 1_000]);
        assert_eq!(f.engine.release().await.unwrap(), "");
        assert!(f.transcriber.calls.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn speech_goes_through_the_facade() {
        let mut f = fixture(Duration::from_millis(10));
        f.engine.speak_delta("Bonjour");
        f.engine.speak_delta(" Alexandre");
        f.engine.finish_speech();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            f.speaker.log().last(),
            Some(&Spoken::Done("Bonjour Alexandre".into()))
        );
        f.engine.speak_delta("Encore une phrase. ");
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.engine.stop_speech();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(
            f.speaker.log().last(),
            Some(&Spoken::Cut("Encore une phrase.".into()))
        );
        assert_eq!(
            drain(&mut f.events),
            vec![
                VoiceEvent::SpeechStarted,
                VoiceEvent::Utterance {
                    text: "Bonjour Alexandre".into()
                },
                VoiceEvent::SpeechFinished { interrupted: false },
                VoiceEvent::SpeechStarted,
                VoiceEvent::Utterance {
                    text: "Encore une phrase.".into()
                },
                VoiceEvent::SpeechFinished { interrupted: true },
            ]
        );
    }

    #[tokio::test]
    async fn release_without_press_and_double_press() {
        let f = fixture(Duration::from_millis(1));
        assert!(matches!(
            f.engine.release().await,
            Err(VoiceError::NotListening)
        ));
        f.engine.press().await.unwrap();
        f.engine.press().await.unwrap();
        assert_eq!(*f.source.starts.lock().unwrap(), 1);
        f.engine.release().await.unwrap();
        assert!(matches!(
            f.engine.release().await,
            Err(VoiceError::NotListening)
        ));
    }

    #[tokio::test]
    async fn a_microphone_failure_is_returned_and_the_engine_recovers() {
        let engine = VoiceEngine::with_backends(
            Backends {
                source: Arc::new(BrokenSource),
                transcriber: Arc::new(SlowTranscriber::new(Duration::ZERO)),
                speaker: Arc::new(FakeSpeaker::new(Duration::ZERO)),
            },
            Duration::from_millis(700),
        );
        assert!(matches!(engine.press().await, Err(VoiceError::Capture(_))));
        assert!(matches!(
            engine.release().await,
            Err(VoiceError::NotListening)
        ));
    }

    #[tokio::test]
    async fn engine_calls_work_from_concurrent_tasks() {
        let f = fixture(Duration::from_millis(1));
        let engine = Arc::clone(&f.engine);
        let pressed = tokio::spawn(async move { engine.press().await });
        pressed.await.unwrap().unwrap();
        f.source.push(&[0.3; 16_000]);
        let engine = Arc::clone(&f.engine);
        let text = tokio::spawn(async move { engine.release().await })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(text, "16000 échantillons");
    }

    #[test]
    fn events_serialize_for_the_front() {
        let json = |event| serde_json::to_string(&event).unwrap();
        assert_eq!(
            json(VoiceEvent::Partial { text: "bon".into() }),
            r#"{"type":"partial","text":"bon"}"#
        );
        assert_eq!(
            json(VoiceEvent::SpeechStarted),
            r#"{"type":"speech_started"}"#
        );
        assert_eq!(
            json(VoiceEvent::SpeechFinished { interrupted: true }),
            r#"{"type":"speech_finished","interrupted":true}"#
        );
        assert_eq!(
            json(VoiceEvent::Utterance {
                text: "Bonjour.".into()
            }),
            r#"{"type":"utterance","text":"Bonjour."}"#
        );
    }
}
