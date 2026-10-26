//! Essais avec les vrais outils du Mac (`say`, `ffmpeg`, `whisper-server`), sans micro.
//! Ignorés par défaut : ils chargent un modèle de 1,6 Go et parlent dans les haut-parleurs.
//!
//! ```bash
//! cargo test -p argos-voice --test real_tools -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Modèle : `ARGOS_WHISPER_MODEL`, sinon le large-v3-turbo déjà présent sur la machine.

use argos_voice::{
    ActiveCapture, AudioSource, Backends, BoxFuture, CaptureSink, SaySpeaker, Speaker, Transcriber,
    VoiceConfig, VoiceEngine, VoiceError, VoiceEvent, WhisperServer,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const DEFAULT_MODEL: &str = "/Users/alexandre/.cache/short-editing/ggml-large-v3-turbo.bin";
const PHRASE: &str = "Bonjour Argos, peux-tu réserver une table pour quatre personnes demain soir \
                      au restaurant italien près de la gare ?";

fn model() -> PathBuf {
    std::env::var_os("ARGOS_WHISPER_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MODEL))
}

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().unwrap();
    assert!(status.success(), "{program} {args:?} : {status}");
}

/// `say` → AIFF → `ffmpeg` → WAV 16 kHz mono → échantillons f32.
fn synthesize(dir: &Path, text: &str) -> Vec<f32> {
    let aiff = dir.join("phrase.aiff");
    let wav = dir.join("phrase.wav");
    run(
        "say",
        &["-v", "Thomas", "-o", aiff.to_str().unwrap(), "--", text],
    );
    run(
        "ffmpeg",
        &[
            "-loglevel",
            "error",
            "-y",
            "-i",
            aiff.to_str().unwrap(),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            wav.to_str().unwrap(),
        ],
    );
    read_wav(&std::fs::read(wav).unwrap())
}

/// Lit le bloc `data` d'un WAV PCM 16 bits mono.
fn read_wav(bytes: &[u8]) -> Vec<f32> {
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            return bytes[offset + 8..offset + 8 + size]
                .chunks_exact(2)
                .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0)
                .collect();
        }
        offset += 8 + size + size % 2;
    }
    panic!("pas de bloc data");
}

/// Source qui rejoue un tampon en temps réel, par blocs de 20 ms, comme un micro.
struct ReplaySource {
    samples: Arc<Vec<f32>>,
}

struct Replay {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioSource for ReplaySource {
    fn start(&self, sink: CaptureSink) -> Result<Box<dyn ActiveCapture>, VoiceError> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let samples = Arc::clone(&self.samples);
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            for (index, block) in samples.chunks(320).enumerate() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let due = started + Duration::from_millis(20 * index as u64);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                sink.push(block);
            }
            // Puis du silence, tant que le bouton reste appuyé.
            while !flag.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
                sink.push(&[0.0; 320]);
            }
        });
        Ok(Box::new(Replay {
            stop,
            thread: Some(thread),
        }))
    }
}

impl ActiveCapture for Replay {
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

struct NoSource;

impl AudioSource for NoSource {
    fn start(&self, _: CaptureSink) -> Result<Box<dyn ActiveCapture>, VoiceError> {
        Err(VoiceError::Capture("pas de micro dans ce test".into()))
    }
}

struct NoTranscriber;

impl Transcriber for NoTranscriber {
    fn transcribe(&self, _: Vec<f32>) -> BoxFuture<'_, Result<String, VoiceError>> {
        Box::pin(async { Err(VoiceError::Transcription("pas de STT dans ce test".into())) })
    }
}

async fn next_event(
    rx: &mut broadcast::Receiver<VoiceEvent>,
    wanted: impl Fn(&VoiceEvent) -> bool,
) -> VoiceEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.unwrap();
            if wanted(&event) {
                return event;
            }
        }
    })
    .await
    .expect("événement attendu")
}

fn say_processes() -> Vec<String> {
    let output = Command::new("pgrep").args(["-x", "say"]).output().unwrap();
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrais outils : whisper-server + modèle large-v3-turbo"]
async fn a_spoken_french_sentence_is_transcribed_by_whisper_server() {
    let dir = tempfile::tempdir().unwrap();
    let samples = synthesize(dir.path(), PHRASE);
    let audio_seconds = samples.len() as f64 / 16_000.0;
    println!("audio : {audio_seconds:.2} s");

    let config = VoiceConfig::new(model());
    let started = Instant::now();
    let server = WhisperServer::start(&config).await.unwrap();
    println!(
        "whisper-server prêt en {:.2} s sur {}",
        started.elapsed().as_secs_f64(),
        server.addr()
    );

    let engine = VoiceEngine::with_backends(
        Backends {
            source: Arc::new(ReplaySource {
                samples: Arc::new(samples),
            }),
            transcriber: Arc::new(server.client(&config.language)),
            speaker: Arc::new(SaySpeaker::new(&config.voice)),
        },
        config.partial_interval(),
    );
    let mut rx = engine.subscribe();

    let pressed = Instant::now();
    engine.press().await.unwrap();
    // L'utilisateur relâche 300 ms après la fin de sa phrase.
    tokio::time::sleep(Duration::from_secs_f64(audio_seconds + 0.3)).await;
    let released = Instant::now();
    let text = engine.release().await.unwrap();
    let final_latency = released.elapsed();

    let mut levels = 0;
    let mut partials = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            VoiceEvent::Level { .. } => levels += 1,
            VoiceEvent::Partial { text } => partials.push(text),
            VoiceEvent::Error { message } => panic!("erreur : {message}"),
            _ => {}
        }
    }
    for (index, partial) in partials.iter().enumerate() {
        println!("partielle {} : {partial}", index + 1);
    }
    println!("finale : {text}");
    println!(
        "{levels} niveaux micro, {} partielles en {:.2} s d'appui",
        partials.len(),
        (released - pressed).as_secs_f64()
    );
    println!(
        "latence de la finale après relâchement : {} ms",
        final_latency.as_millis()
    );

    let lower = text.to_lowercase();
    for word in ["réserver", "table", "demain", "restaurant", "gare"] {
        assert!(lower.contains(word), "« {word} » manque dans « {text} »");
    }
    // Whisper écrit les nombres en chiffres ou en lettres selon le contexte.
    assert!(
        lower.contains("quatre") || lower.contains("4 personnes"),
        "{text}"
    );
    assert!(partials.len() >= 3, "{partials:?}");
    assert!(levels > 100);

    let addr = server.addr();
    server.shutdown().await;
    assert!(
        std::net::TcpStream::connect(addr).is_err(),
        "whisper-server écoute encore"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrais outils : whisper-server + modèle large-v3-turbo"]
async fn the_engine_starts_and_stops_its_own_whisper_server() {
    let toml = format!("[voice]\nwhisper_model = {:?}\n", model().to_str().unwrap());
    let config = VoiceConfig::from_toml(&toml).unwrap();
    let started = Instant::now();
    let engine = VoiceEngine::start(&config).await.unwrap();
    println!(
        "VoiceEngine::start en {:.2} s",
        started.elapsed().as_secs_f64()
    );
    engine.shutdown().await;
    // Un modèle absent est une erreur claire, pas une panique.
    let missing = VoiceEngine::start(&VoiceConfig::new("/nulle/part.bin")).await;
    assert!(matches!(missing, Err(VoiceError::ModelMissing(_))));
}

struct SilentTranscriber;

impl Transcriber for SilentTranscriber {
    fn transcribe(&self, _: Vec<f32>) -> BoxFuture<'_, Result<String, VoiceError>> {
        Box::pin(async { Ok(String::new()) })
    }
}

/// Ouvre le vrai micro une seconde. Sans permission micro (TCC) pour le terminal,
/// macOS ne refuse pas le flux : il livre du silence, d'où le niveau affiché.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai micro : peut déclencher la demande de permission de macOS"]
async fn the_default_microphone_delivers_16khz_audio() {
    let engine = VoiceEngine::with_backends(
        Backends {
            source: Arc::new(argos_voice::CpalSource),
            transcriber: Arc::new(SilentTranscriber),
            speaker: Arc::new(SaySpeaker::new("Thomas")),
        },
        Duration::from_millis(700),
    );
    let mut rx = engine.subscribe();
    let opening = Instant::now();
    engine.press().await.unwrap();
    println!("micro ouvert en {} ms", opening.elapsed().as_millis());
    tokio::time::sleep(Duration::from_secs(1)).await;
    engine.release().await.unwrap();
    let mut levels = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let VoiceEvent::Level { rms } = event {
            levels.push(rms);
        }
    }
    let loudest = levels.iter().cloned().fold(0.0f32, f32::max);
    println!("{} niveaux en 1 s, RMS max {loudest:.5}", levels.len());
    // ~20 fenêtres de 50 ms : le flux est bien ramené à 16 kHz.
    assert!((17..=21).contains(&levels.len()), "{}", levels.len());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrais outils : say, parle dans les haut-parleurs"]
async fn streamed_speech_is_cut_by_stop() {
    assert!(say_processes().is_empty(), "un `say` tourne déjà");
    let engine = VoiceEngine::with_backends(
        Backends {
            source: Arc::new(NoSource),
            transcriber: Arc::new(NoTranscriber),
            speaker: Arc::new(SaySpeaker::new("Thomas")),
        },
        Duration::from_millis(700),
    );
    let mut rx = engine.subscribe();

    let first_delta = Instant::now();
    for delta in [
        "Bonjour, je suis Argos, ",
        "et je vais lire une phrase assez longue, ",
        "pour que l'on ait largement le temps de m'interrompre au milieu. ",
        "Cette deuxième phrase ne doit jamais être prononcée.",
    ] {
        engine.speak_delta(delta);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    next_event(&mut rx, |e| *e == VoiceEvent::SpeechStarted).await;
    println!(
        "premier `say` lancé {} ms après le premier fragment (fragments espacés de 30 ms)",
        first_delta.elapsed().as_millis()
    );
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let running = say_processes();
    assert_eq!(running.len(), 1, "{running:?}");

    let stopped = Instant::now();
    engine.stop_speech();
    while !say_processes().is_empty() {
        assert!(
            stopped.elapsed() < Duration::from_secs(1),
            "`say` survit à stop()"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "`say` disparu {} ms après stop_speech()",
        stopped.elapsed().as_millis()
    );
    assert!(stopped.elapsed() < Duration::from_millis(300));
    assert_eq!(
        next_event(&mut rx, |e| matches!(e, VoiceEvent::SpeechFinished { .. })).await,
        VoiceEvent::SpeechFinished { interrupted: true }
    );
    // La phrase suivante, déjà dans la file, ne part pas.
    engine.finish_speech();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(say_processes().is_empty());

    // Démarrage de `say` : durée totale d'une phrase courte moins sa durée audio.
    let dir = tempfile::tempdir().unwrap();
    let aiff = dir.path().join("court.aiff");
    run(
        "say",
        &[
            "-v",
            "Thomas",
            "-o",
            aiff.to_str().unwrap(),
            "--",
            "Bonjour.",
        ],
    );
    let info = Command::new("afinfo").arg(&aiff).output().unwrap();
    let audio = String::from_utf8_lossy(&info.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("estimated duration: "))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap();
    let speaking = Instant::now();
    SaySpeaker::new("Thomas")
        .speak("Bonjour.".into())
        .await
        .unwrap();
    let total = speaking.elapsed().as_secs_f64();
    println!(
        "« Bonjour. » : {:.0} ms au total pour {:.0} ms d'audio, soit ~{:.0} ms de démarrage",
        total * 1_000.0,
        audio * 1_000.0,
        (total - audio) * 1_000.0
    );
}
