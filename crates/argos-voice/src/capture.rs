//! Capture micro : la source ([`AudioSource`]) pousse du 16 kHz mono dans un
//! [`CaptureSink`], qui accumule le tampon de l'appui et publie le niveau sonore.

use crate::VoiceError;
use crate::audio::{self, Resampler, SAMPLE_RATE};
use crate::engine::VoiceEvent;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Fenêtre du niveau publié : 50 ms, soit 20 mises à jour par seconde pour l'orbe.
const LEVEL_WINDOW: usize = SAMPLE_RATE as usize / 20;
/// Hors app empaquetée, sans permission micro, CoreAudio bloque l'ouverture du flux
/// indéfiniment (vu sur macOS 27) : on rend la main avec une erreur plutôt que de geler
/// le push-to-talk.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// Une source audio. `start` ouvre le flux et rend la main dès qu'il tourne ; les
/// échantillons arrivent ensuite dans `sink` depuis le thread de la source.
pub trait AudioSource: Send + Sync {
    fn start(&self, sink: CaptureSink) -> Result<Box<dyn ActiveCapture>, VoiceError>;
}

/// Un flux ouvert. `stop` le ferme ; aucun échantillon n'arrive plus ensuite.
pub trait ActiveCapture: Send {
    fn stop(self: Box<Self>);
}

/// Destination des échantillons d'un appui, partagée entre la source et le moteur.
#[derive(Clone)]
pub struct CaptureSink {
    inner: Arc<SinkInner>,
}

struct SinkInner {
    samples: Mutex<Vec<f32>>,
    level: Mutex<LevelMeter>,
    events: broadcast::Sender<VoiceEvent>,
}

#[derive(Default)]
struct LevelMeter {
    sum_squares: f64,
    count: usize,
}

impl CaptureSink {
    pub(crate) fn new(events: broadcast::Sender<VoiceEvent>) -> Self {
        Self {
            inner: Arc::new(SinkInner {
                samples: Mutex::new(Vec::new()),
                level: Mutex::new(LevelMeter::default()),
                events,
            }),
        }
    }

    /// Ajoute des échantillons 16 kHz mono et publie le niveau toutes les 50 ms.
    pub fn push(&self, samples: &[f32]) {
        self.inner
            .samples
            .lock()
            .expect("verrou du tampon empoisonné")
            .extend_from_slice(samples);
        let mut levels = Vec::new();
        {
            let mut meter = self
                .inner
                .level
                .lock()
                .expect("verrou du niveau empoisonné");
            for &sample in samples {
                meter.sum_squares += f64::from(sample) * f64::from(sample);
                meter.count += 1;
                if meter.count == LEVEL_WINDOW {
                    levels.push((meter.sum_squares / LEVEL_WINDOW as f64).sqrt() as f32);
                    *meter = LevelMeter::default();
                }
            }
        }
        for rms in levels {
            let _ = self.inner.events.send(VoiceEvent::Level { rms });
        }
    }

    /// Signale une erreur survenue dans le thread de la source.
    pub fn report_error(&self, message: String) {
        let _ = self.inner.events.send(VoiceEvent::Error { message });
    }

    /// Copie du tampon accumulé depuis l'appui.
    pub(crate) fn snapshot(&self) -> Vec<f32> {
        self.inner
            .samples
            .lock()
            .expect("verrou du tampon empoisonné")
            .clone()
    }

    pub(crate) fn len(&self) -> usize {
        self.inner
            .samples
            .lock()
            .expect("verrou du tampon empoisonné")
            .len()
    }

    /// Vide le tampon et rend son contenu.
    pub(crate) fn take(&self) -> Vec<f32> {
        std::mem::take(
            &mut *self
                .inner
                .samples
                .lock()
                .expect("verrou du tampon empoisonné"),
        )
    }
}

/// Micro par défaut du système, via `cpal` (CoreAudio sur macOS).
///
/// Le flux `cpal` n'est pas `Send` sur toutes les plateformes : il vit dans un thread
/// dédié, piloté par un canal, pour que le moteur reste `Send + Sync`.
#[derive(Debug, Default)]
pub struct CpalSource;

struct CpalCapture {
    stop: std_mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioSource for CpalSource {
    fn start(&self, sink: CaptureSink) -> Result<Box<dyn ActiveCapture>, VoiceError> {
        let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("argos-voice-capture".into())
            .spawn(move || match open_default_input(sink) {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    // Bloque jusqu'à `stop` ou l'abandon de la capture.
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(|error| VoiceError::Capture(error.to_string()))?;
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(Box::new(CpalCapture {
                stop: stop_tx,
                thread: Some(thread),
            })),
            Ok(Err(error)) => Err(error),
            // Le thread reste bloqué dans CoreAudio ; s'il finit par ouvrir le flux, il le
            // referme aussitôt, puisque `stop_tx` est lâché ici.
            Err(std_mpsc::RecvTimeoutError::Timeout) => Err(VoiceError::Capture(format!(
                "le micro ne répond pas après {} s (permission micro de macOS refusée ou \
                 en attente ?)",
                START_TIMEOUT.as_secs()
            ))),
            Err(std_mpsc::RecvTimeoutError::Disconnected) => Err(VoiceError::Capture(
                "le thread de capture s'est arrêté".into(),
            )),
        }
    }
}

impl ActiveCapture for CpalCapture {
    fn stop(mut self: Box<Self>) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open_default_input(sink: CaptureSink) -> Result<cpal::Stream, VoiceError> {
    let capture_error = |error: cpal::Error| VoiceError::Capture(error.to_string());
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| VoiceError::Capture("aucun périphérique d'entrée".into()))?;
    let supported = device.default_input_config().map_err(capture_error)?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, sink),
        SampleFormat::I16 => build::<i16>(&device, &config, sink),
        SampleFormat::I32 => build::<i32>(&device, &config, sink),
        SampleFormat::U16 => build::<u16>(&device, &config, sink),
        other => {
            return Err(VoiceError::Capture(format!(
                "format d'échantillon non géré : {other}"
            )));
        }
    }
    .map_err(capture_error)?;
    stream.play().map_err(capture_error)?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sink: CaptureSink,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut resampler = Resampler::new(config.sample_rate);
    // Tampons réutilisés d'un rappel à l'autre : pas d'allocation dans le thread audio.
    let mut converted = Vec::new();
    let mut mono = Vec::new();
    let mut resampled = Vec::new();
    let errors = sink.clone();
    device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            converted.clear();
            converted.extend(data.iter().map(|&sample| sample.to_sample::<f32>()));
            mono.clear();
            audio::downmix(&converted, channels, &mut mono);
            resampled.clear();
            resampler.process(&mono, &mut resampled);
            sink.push(&resampled);
        },
        move |error| errors.report_error(format!("flux micro : {error}")),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sink_accumulates_and_publishes_levels() {
        let (events, mut rx) = broadcast::channel(64);
        let sink = CaptureSink::new(events);
        // 120 ms en trois morceaux : deux fenêtres complètes de 50 ms, 20 ms en attente.
        sink.push(&vec![0.5; 700]);
        sink.push(&vec![0.5; 700]);
        sink.push(&vec![0.0; 520]);
        assert_eq!(sink.len(), 1_920);

        let mut levels = Vec::new();
        while let Ok(VoiceEvent::Level { rms }) = rx.try_recv() {
            levels.push(rms);
        }
        assert_eq!(levels.len(), 2);
        assert!((levels[0] - 0.5).abs() < 1e-6);
        // 600 échantillons à 0,5 puis 200 de silence.
        assert!((levels[1] - (0.25f32 * 600.0 / 800.0).sqrt()).abs() < 1e-6);

        assert_eq!(sink.snapshot().len(), 1_920);
        assert_eq!(sink.take().len(), 1_920);
        assert_eq!(sink.len(), 0);
    }
}
