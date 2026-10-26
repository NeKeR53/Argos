//! Reconnaissance vocale : le trait [`Transcriber`] et la boucle des hypothèses
//! partielles pendant l'appui.

use crate::VoiceError;
use crate::audio::SAMPLE_RATE;
use crate::capture::CaptureSink;
use crate::engine::VoiceEvent;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::{Instant, MissedTickBehavior};

/// Futur renvoyé par les backends ; `Send` pour tourner sur le runtime de Tauri.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Pas d'hypothèse partielle sur moins d'une demi-seconde : Whisper y invente des mots.
pub(crate) const MIN_PARTIAL_SAMPLES: usize = SAMPLE_RATE as usize / 2;

/// Un backend de reconnaissance : `whisper-server` en local, ou plus tard une API.
pub trait Transcriber: Send + Sync {
    /// Transcrit `samples` (16 kHz mono) et rend le texte nettoyé.
    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'_, Result<String, VoiceError>>;
}

/// Un appui en cours, vu des hypothèses partielles. Une fois fermé (au relâchement),
/// plus aucune partielle n'est publiée : la transcription finale est toujours la dernière.
pub(crate) struct Turn {
    open: Mutex<bool>,
    events: broadcast::Sender<VoiceEvent>,
}

impl Turn {
    pub fn new(events: broadcast::Sender<VoiceEvent>) -> Self {
        Self {
            open: Mutex::new(true),
            events,
        }
    }

    /// Publie si l'appui est encore ouvert. Le verrou est tenu pendant l'envoi, pour
    /// qu'une partielle ne puisse pas se glisser après la fermeture.
    fn publish(&self, event: VoiceEvent) {
        let open = self.open.lock().expect("verrou de l'appui empoisonné");
        if *open {
            let _ = self.events.send(event);
        }
    }

    pub fn close(&self) {
        *self.open.lock().expect("verrou de l'appui empoisonné") = false;
    }
}

/// Toutes les `interval`, transcrit le tampon accumulé et publie une hypothèse partielle.
///
/// Une seule requête à la fois : les échéances qui tombent pendant une requête sont
/// sautées, et la suivante porte sur le tampon le plus récent. Chaque hypothèse remplace
/// donc la précédente sans que rien ne s'accumule, ni ici ni dans `whisper-server`.
/// Une requête plus longue que `interval` repousse la suivante d'un `interval` entier.
/// Le moteur abandonne la tâche au relâchement.
pub(crate) async fn run_partials(
    transcriber: Arc<dyn Transcriber>,
    sink: CaptureSink,
    turn: Arc<Turn>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval_at(Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_len = 0;
    loop {
        ticker.tick().await;
        let len = sink.len();
        if len < MIN_PARTIAL_SAMPLES || len == last_len {
            continue;
        }
        last_len = len;
        let started = Instant::now();
        let result = transcriber.transcribe(sink.snapshot()).await;
        if started.elapsed() >= interval {
            // Serveur plus lent que la cadence : on lui laisse une respiration plutôt que
            // d'enchaîner les requêtes, sinon la finale attendrait toujours une partielle.
            ticker.reset();
        }
        match result {
            Ok(text) if text.is_empty() => {}
            Ok(text) => turn.publish(VoiceEvent::Partial { text }),
            Err(error) => {
                // Inutile d'insister : la finale signalera le problème si elle échoue aussi.
                turn.publish(VoiceEvent::Error {
                    message: error.to_string(),
                });
                return;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Faux backend : répond après `delay` par le nombre d'échantillons reçus, et
    /// mesure le nombre de requêtes simultanées.
    #[derive(Default)]
    pub struct SlowTranscriber {
        pub delay: Duration,
        pub calls: Mutex<Vec<usize>>,
        in_flight: AtomicUsize,
        pub max_in_flight: AtomicUsize,
    }

    impl SlowTranscriber {
        pub fn new(delay: Duration) -> Self {
            Self {
                delay,
                ..Self::default()
            }
        }
    }

    impl Transcriber for SlowTranscriber {
        fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'_, Result<String, VoiceError>> {
            Box::pin(async move {
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_in_flight.fetch_max(now, Ordering::SeqCst);
                self.calls.lock().unwrap().push(samples.len());
                tokio::time::sleep(self.delay).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("{} échantillons", samples.len()))
            })
        }
    }

    fn partials(rx: &mut broadcast::Receiver<VoiceEvent>) -> Vec<String> {
        let mut texts = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let VoiceEvent::Partial { text } = event {
                texts.push(text);
            }
        }
        texts
    }

    #[tokio::test(start_paused = true)]
    async fn slow_partials_replace_each_other_instead_of_queueing() {
        let (events, mut rx) = broadcast::channel(1024);
        let sink = CaptureSink::new(events.clone());
        let transcriber = Arc::new(SlowTranscriber::new(Duration::from_millis(1_500)));
        let turn = Arc::new(Turn::new(events));
        let task = tokio::spawn(run_partials(
            transcriber.clone(),
            sink.clone(),
            turn.clone(),
            Duration::from_millis(700),
        ));

        // 6 s de parole, par blocs de 100 ms.
        for _ in 0..60 {
            sink.push(&[0.1; 1_600]);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        task.abort();

        let calls = transcriber.calls.lock().unwrap().clone();
        assert_eq!(transcriber.max_in_flight.load(Ordering::SeqCst), 1);
        // Sans saut d'échéance il y en aurait 8 en file ; une requête dure 1,5 s, suivie
        // d'une respiration de 700 ms : une toutes les 2,2 s.
        assert!(calls.len() <= 3, "{calls:?}");
        assert!(calls.len() >= 2, "{calls:?}");
        assert!(calls.windows(2).all(|w| w[0] < w[1]), "{calls:?}");
        // Chaque requête porte sur le tampon le plus récent, pas sur un vieux.
        assert!(*calls.last().unwrap() >= 16_000 * 4, "{calls:?}");
        assert!(partials(&mut rx).len() >= 2);
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_sent_for_silence_or_an_unchanged_buffer() {
        let (events, mut rx) = broadcast::channel(1024);
        let sink = CaptureSink::new(events.clone());
        let transcriber = Arc::new(SlowTranscriber::new(Duration::from_millis(10)));
        let turn = Arc::new(Turn::new(events));
        let task = tokio::spawn(run_partials(
            transcriber.clone(),
            sink.clone(),
            turn,
            Duration::from_millis(700),
        ));
        // Trop court pour une partielle.
        sink.push(&[0.1; 4_000]);
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(transcriber.calls.lock().unwrap().is_empty());
        // Assez long : une seule requête, puis plus rien tant que le tampon ne bouge pas.
        sink.push(&[0.1; 8_000]);
        tokio::time::sleep(Duration::from_millis(3_000)).await;
        task.abort();
        assert_eq!(*transcriber.calls.lock().unwrap(), vec![12_000]);
        assert_eq!(partials(&mut rx), vec!["12000 échantillons".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_turn_publishes_nothing() {
        let (events, mut rx) = broadcast::channel(1024);
        let sink = CaptureSink::new(events.clone());
        let transcriber = Arc::new(SlowTranscriber::new(Duration::from_millis(500)));
        let turn = Arc::new(Turn::new(events));
        let task = tokio::spawn(run_partials(
            transcriber.clone(),
            sink.clone(),
            turn.clone(),
            Duration::from_millis(700),
        ));
        sink.push(&[0.1; 16_000]);
        // La requête part à 700 ms ; on ferme pendant qu'elle est en vol.
        tokio::time::sleep(Duration::from_millis(900)).await;
        turn.close();
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        task.abort();
        assert_eq!(transcriber.calls.lock().unwrap().len(), 1);
        assert!(partials(&mut rx).is_empty());
    }
}
