//! Façade du moteur vocal pour la GUI : push-to-talk, parole, événements.

use crate::capture::{ActiveCapture, AudioSource, CaptureSink, CpalSource};
use crate::stt::{self, Transcriber, Turn};
use crate::tts::{KokoroSpeaker, SaySpeaker, Speaker, SpeechQueue};
use crate::vad::{Vad, VadEvent, VadSettings};
use crate::whisper::WhisperServer;
use crate::{VoiceConfig, VoiceError};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

/// Capacité du flux d'événements : un abonné qui prend plus de ~10 s de retard perd
/// les plus anciens (des niveaux micro, surtout) plutôt que de faire gonfler la mémoire.
const EVENT_CAPACITY: usize = 256;
/// Un appui de moins de 300 ms est un faux départ : pas de transcription.
const MIN_FINAL_SAMPLES: usize = crate::audio::SAMPLE_RATE as usize * 3 / 10;
/// Cadence à laquelle l'écoute mains libres lit le micro.
const HANDS_FREE_TICK: Duration = Duration::from_millis(20);

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
    /// Écoute mains libres : la voix de l'utilisateur vient d'être détectée. La parole en
    /// cours est déjà coupée (barge-in).
    HandsFreeStarted,
    /// Écoute mains libres : fin de l'énoncé et sa transcription (vide si rien de net).
    HandsFreeHeard {
        text: String,
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
    /// Push-to-talk en cours : l'écoute mains libres se tait pendant ce temps.
    pressed: Arc<AtomicBool>,
    hands_free: Mutex<Option<HandsFree>>,
    server: Mutex<Option<WhisperServer>>,
}

/// L'écoute mains libres en cours : micro ouvert en continu, détection de parole.
struct HandsFree {
    capture: Box<dyn ActiveCapture>,
    task: JoinHandle<()>,
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
        config.check()?;
        let speaker: Arc<dyn Speaker> = if config.tts_backend == "kokoro" {
            Arc::new(KokoroSpeaker::new(
                config
                    .kokoro_python
                    .clone()
                    .ok_or_else(|| VoiceError::Config("kokoro_python absent".into()))?,
                config
                    .kokoro_model
                    .clone()
                    .ok_or_else(|| VoiceError::Config("kokoro_model absent".into()))?,
                config
                    .kokoro_voices
                    .clone()
                    .ok_or_else(|| VoiceError::Config("kokoro_voices absent".into()))?,
                config.kokoro_voice.clone(),
            ))
        } else {
            Arc::new(SaySpeaker::new(&config.voice))
        };
        let server = WhisperServer::start(config).await?;
        let engine = Self::with_backends(
            Backends {
                source: Arc::new(CpalSource),
                transcriber: Arc::new(server.client(&config.language)),
                speaker,
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
            pressed: Arc::new(AtomicBool::new(false)),
            hands_free: Mutex::new(None),
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
        self.pressed.store(true, Ordering::SeqCst);
        let sink = CaptureSink::new(self.events.clone());
        let source = Arc::clone(&self.source);
        let source_sink = sink.clone();
        // Ouvrir un périphérique audio peut prendre quelques dizaines de ms.
        let capture = tokio::task::spawn_blocking(move || source.start(source_sink))
            .await
            .map_err(|_| VoiceError::Closed)
            .flatten()
            .inspect_err(|_| self.pressed.store(false, Ordering::SeqCst))?;
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
        self.pressed.store(false, Ordering::SeqCst);
        let text = if samples.len() < MIN_FINAL_SAMPLES {
            String::new()
        } else {
            self.transcriber.transcribe(samples).await?
        };
        let _ = self.events.send(VoiceEvent::Final { text: text.clone() });
        Ok(text)
    }

    /// Active ou coupe l'écoute mains libres : le micro reste ouvert, chaque énoncé
    /// détecté coupe la parole en cours puis est transcrit ([`VoiceEvent::HandsFreeStarted`],
    /// [`VoiceEvent::HandsFreeHeard`]). Le push-to-talk reste utilisable à côté.
    pub async fn set_hands_free(&self, on: bool) -> Result<(), VoiceError> {
        let mut hands_free = self.hands_free.lock().await;
        if !on {
            if let Some(active) = hands_free.take() {
                stop_hands_free(active).await;
            }
            return Ok(());
        }
        if hands_free.is_some() {
            return Ok(());
        }
        let sink = CaptureSink::new(self.events.clone());
        let source = Arc::clone(&self.source);
        let source_sink = sink.clone();
        let capture = tokio::task::spawn_blocking(move || source.start(source_sink))
            .await
            .map_err(|_| VoiceError::Closed)??;
        let task = tokio::spawn(run_hands_free(HandsFreeLoop {
            sink,
            events: self.events.clone(),
            speech: self.speech.clone(),
            transcriber: Arc::clone(&self.transcriber),
            pressed: Arc::clone(&self.pressed),
            vad: Vad::new(VadSettings::default()),
        }));
        *hands_free = Some(HandsFree { capture, task });
        Ok(())
    }

    pub async fn hands_free(&self) -> bool {
        self.hands_free.lock().await.is_some()
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
        if let Some(active) = self.hands_free.lock().await.take() {
            stop_hands_free(active).await;
        }
        if let Some(server) = self.server.lock().await.take() {
            server.shutdown().await;
        }
    }
}

async fn stop_hands_free(active: HandsFree) {
    active.task.abort();
    let capture = active.capture;
    let _ = tokio::task::spawn_blocking(move || capture.stop()).await;
}

struct HandsFreeLoop {
    sink: CaptureSink,
    events: broadcast::Sender<VoiceEvent>,
    speech: SpeechQueue,
    transcriber: Arc<dyn Transcriber>,
    pressed: Arc<AtomicBool>,
    vad: Vad,
}

/// Lit le micro toutes les 20 ms et le passe au détecteur. La synthèse est suivie par
/// ses propres événements : pendant qu'elle parle, le détecteur compte avec l'écho.
async fn run_hands_free(mut state: HandsFreeLoop) {
    let mut own_events = state.events.subscribe();
    let mut playing = false;
    let mut tick = tokio::time::interval(HANDS_FREE_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        loop {
            match own_events.try_recv() {
                Ok(VoiceEvent::SpeechStarted) => playing = true,
                Ok(VoiceEvent::SpeechFinished { .. }) => playing = false,
                Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        let samples = state.sink.take();
        if state.pressed.load(Ordering::SeqCst) {
            state.vad.reset();
            continue;
        }
        for event in state.vad.push(&samples, playing) {
            match event {
                VadEvent::Started => {
                    state.speech.stop();
                    let _ = state.events.send(VoiceEvent::HandsFreeStarted);
                }
                VadEvent::Ended { samples } => {
                    // Transcrit à part : l'écoute continue pendant ce temps.
                    let transcriber = Arc::clone(&state.transcriber);
                    let events = state.events.clone();
                    tokio::spawn(async move {
                        let text = match transcriber.transcribe(samples).await {
                            Ok(text) => text,
                            Err(error) => {
                                let _ = events.send(VoiceEvent::Error {
                                    message: error.to_string(),
                                });
                                String::new()
                            }
                        };
                        let _ = events.send(VoiceEvent::HandsFreeHeard { text });
                    });
                }
            }
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

    /// Pousse `ms` millisecondes d'un signal d'amplitude `level`, au rythme du temps réel.
    async fn feed(source: &FakeSource, ms: u64, level: f32) {
        let frame: Vec<f32> = (0..320)
            .map(|i| if i % 2 == 0 { level } else { -level })
            .collect();
        for _ in 0..ms / 20 {
            source.push(&frame);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hands_free_cuts_the_speech_then_transcribes_the_utterance() {
        let mut f = fixture(Duration::from_millis(50));
        f.engine.set_hands_free(true).await.unwrap();
        assert!(f.engine.hands_free().await);
        f.engine
            .speak_delta("Une longue réponse qui parle encore. ");
        feed(&f.source, 400, 0.002).await;
        // Une voix franche par-dessus la synthèse : barge-in, puis la fin de l'énoncé.
        feed(&f.source, 600, 0.3).await;
        feed(&f.source, 1200, 0.002).await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let events: Vec<_> = drain(&mut f.events)
            .into_iter()
            .filter(|e| !matches!(e, VoiceEvent::Level { .. } | VoiceEvent::Utterance { .. }))
            .collect();
        let started = events
            .iter()
            .position(|e| *e == VoiceEvent::HandsFreeStarted);
        let cut = events
            .iter()
            .position(|e| *e == VoiceEvent::SpeechFinished { interrupted: true });
        assert!(started.is_some() && cut.is_some(), "{events:?}");
        let heard = events.iter().find_map(|e| match e {
            VoiceEvent::HandsFreeHeard { text } => Some(text.clone()),
            _ => None,
        });
        assert!(
            heard.is_some_and(|text| text.ends_with("échantillons")),
            "{events:?}"
        );

        f.engine.set_hands_free(false).await.unwrap();
        assert!(!f.engine.hands_free().await);
        assert_eq!(*f.source.stops.lock().unwrap(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn hands_free_stays_quiet_while_push_to_talk_is_held() {
        let mut f = fixture(Duration::from_millis(50));
        f.engine.set_hands_free(true).await.unwrap();
        f.engine.press().await.unwrap();
        // La source ne garde que le dernier sink : on alimente le push-to-talk, et
        // l'écoute mains libres, muette, ne doit rien ouvrir.
        feed(&f.source, 300, 0.002).await;
        feed(&f.source, 600, 0.3).await;
        f.engine.release().await.unwrap();
        let events = drain(&mut f.events);
        assert!(
            !events.contains(&VoiceEvent::HandsFreeStarted),
            "{events:?}"
        );
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
