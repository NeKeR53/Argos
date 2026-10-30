//! Synthèse vocale en streaming : une file qui reçoit le texte par fragments, prononce
//! chaque phrase dès qu'elle est complète, et se coupe net au barge-in.

use crate::VoiceError;
use crate::engine::VoiceEvent;
use crate::sentences::SentenceSplitter;
use crate::stt::BoxFuture;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, mpsc};

/// Un backend de synthèse : `say` en local, ou plus tard une API.
pub trait Speaker: Send + Sync {
    /// Prononce `text` ; le futur se termine quand la phrase est dite. Lâcher le futur
    /// doit couper le son immédiatement : c'est ainsi que la file interrompt.
    fn speak(&self, text: String) -> BoxFuture<'static, Result<(), VoiceError>>;
}

/// La commande système `say` de macOS, un process par phrase.
#[derive(Debug, Clone)]
pub struct SaySpeaker {
    program: PathBuf,
    voice: String,
}

impl SaySpeaker {
    pub fn new(voice: &str) -> Self {
        Self {
            program: PathBuf::from("say"),
            voice: voice.to_string(),
        }
    }
}

impl Speaker for SaySpeaker {
    fn speak(&self, text: String) -> BoxFuture<'static, Result<(), VoiceError>> {
        let mut command = Command::new(&self.program);
        // `--` : un texte qui commence par un tiret n'est pas pris pour une option.
        command
            .args(["-v", &self.voice, "--", &text])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // Le futur lâché tue `say` (SIGKILL) : le son s'arrête aussitôt.
            .kill_on_drop(true);
        let program = self.program.display().to_string();
        Box::pin(async move {
            let output = command
                .output()
                .await
                .map_err(|source| VoiceError::Spawn { program, source })?;
            if output.status.success() {
                Ok(())
            } else {
                Err(VoiceError::Speech(format!(
                    "say a échoué ({}) : {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )))
            }
        })
    }
}

/// Kokoro ONNX local. Un seul processus Python sert toute la session : le modèle de
/// 325 Mo n'est chargé qu'une fois, et chaque phrase lui est envoyée sur stdin. Lâcher le
/// futur d'une phrase coupe le processus (barge-in) ; la phrase suivante en relance un.
#[derive(Debug, Clone)]
pub struct KokoroSpeaker {
    python: PathBuf,
    model: PathBuf,
    voices: PathBuf,
    voice: String,
    /// Le processus entre deux phrases. Pris le temps d'une phrase, il est rendu ensuite.
    worker: Arc<StdMutex<Option<KokoroWorker>>>,
}

impl KokoroSpeaker {
    pub fn new(python: PathBuf, model: PathBuf, voices: PathBuf, voice: String) -> Self {
        Self {
            python,
            model,
            voices,
            voice,
            worker: Arc::new(StdMutex::new(None)),
        }
    }
}

/// Le processus Python de Kokoro et ses tubes.
pub struct KokoroWorker {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}

impl std::fmt::Debug for KokoroWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KokoroWorker")
            .field("pid", &self.child.id())
            .finish()
    }
}

impl KokoroWorker {
    async fn start(speaker: &KokoroSpeaker) -> Result<Self, VoiceError> {
        let program = speaker.python.display().to_string();
        let mut child = Command::new(&speaker.python)
            .arg("-c")
            .arg(include_str!("kokoro_worker.py"))
            .arg(&speaker.model)
            .arg(&speaker.voices)
            .arg(&speaker.voice)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Les traces de Kokoro (chargement, erreurs) restent visibles dans le terminal.
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| VoiceError::Spawn { program, source })?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| VoiceError::Speech("entrée Kokoro indisponible".into()))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| VoiceError::Speech("sortie Kokoro indisponible".into()))?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output).lines(),
        })
    }

    /// Envoie une phrase et attend qu'elle soit dite. `Ok(Err(message))` : Kokoro n'a pas pu
    /// la dire, mais le processus reste utilisable. `Err` : le processus est à reprendre.
    async fn say(&mut self, sentence: &str) -> Result<Result<(), String>, VoiceError> {
        let io = |error: std::io::Error| VoiceError::Speech(format!("Kokoro : {error}"));
        // Une phrase tient sur une ligne : les retours à la ligne deviennent des espaces.
        let line = serde_json::to_string(&sentence.replace(['\n', '\r'], " "))
            .map_err(|e| VoiceError::Speech(e.to_string()))?;
        self.input
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(io)?;
        self.input.flush().await.map_err(io)?;
        let reply = self
            .output
            .next_line()
            .await
            .map_err(io)?
            .ok_or_else(|| VoiceError::Speech("Kokoro s'est arrêté".into()))?;
        Ok(match reply.strip_prefix("erreur ") {
            Some(message) => Err(message.to_string()),
            None if reply == "ok" => Ok(()),
            None => Err(format!("réponse inattendue : {reply}")),
        })
    }
}

impl Speaker for KokoroSpeaker {
    fn speak(&self, text: String) -> BoxFuture<'static, Result<(), VoiceError>> {
        let speaker = self.clone();
        Box::pin(async move {
            // Repris le temps de la phrase : si ce futur est lâché, le processus part avec.
            let taken = speaker
                .worker
                .lock()
                .expect("verrou Kokoro empoisonné")
                .take();
            // Un processus resté en réserve a pu mourir pendant la pause : la phrase est
            // alors redite par un neuf plutôt que perdue.
            let reused = match taken {
                Some(mut worker) => match worker.say(&text).await {
                    Ok(reply) => Some((worker, reply)),
                    Err(_) => None,
                },
                None => None,
            };
            let (worker, reply) = match reused {
                Some(done) => done,
                None => {
                    let mut fresh = KokoroWorker::start(&speaker).await?;
                    let reply = fresh.say(&text).await?;
                    (fresh, reply)
                }
            };
            match reply {
                Ok(()) => {
                    *speaker.worker.lock().expect("verrou Kokoro empoisonné") = Some(worker);
                    Ok(())
                }
                Err(message) => {
                    // Le processus est sain : il reste pour la phrase suivante.
                    *speaker.worker.lock().expect("verrou Kokoro empoisonné") = Some(worker);
                    Err(VoiceError::Speech(format!("Kokoro : {message}")))
                }
            }
        })
    }
}

enum SpeechCommand {
    Delta(String),
    Finish,
    Stop,
}

/// Poignée de la file de parole ; la tâche qui la sert s'arrête quand on la lâche.
pub(crate) struct SpeechQueue {
    commands: mpsc::UnboundedSender<SpeechCommand>,
}

impl SpeechQueue {
    /// Démarre la tâche de la file ; à appeler depuis un runtime tokio.
    pub fn spawn(speaker: Arc<dyn Speaker>, events: broadcast::Sender<VoiceEvent>) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        tokio::spawn(run(speaker, events, receiver));
        Self { commands }
    }

    pub fn delta(&self, text: &str) {
        let _ = self.commands.send(SpeechCommand::Delta(text.to_string()));
    }

    pub fn finish(&self) {
        let _ = self.commands.send(SpeechCommand::Finish);
    }

    pub fn stop(&self) {
        let _ = self.commands.send(SpeechCommand::Stop);
    }
}

/// Boucle de la file. Les commandes sont traitées dans l'ordre d'envoi : un `stop()`
/// suivi de nouveaux fragments coupe l'ancien discours puis prononce le nouveau.
async fn run(
    speaker: Arc<dyn Speaker>,
    events: broadcast::Sender<VoiceEvent>,
    mut commands: mpsc::UnboundedReceiver<SpeechCommand>,
) {
    let emit = |event| {
        let _ = events.send(event);
    };
    let mut splitter = SentenceSplitter::new();
    let mut pending: VecDeque<String> = VecDeque::new();
    let mut current: Option<BoxFuture<'static, Result<(), VoiceError>>> = None;
    // Un discours est en cours depuis SpeechStarted, jusqu'à SpeechFinished.
    let mut speaking = false;
    // Le flux de texte est terminé : plus rien n'arrivera pour ce discours.
    let mut finished = false;
    loop {
        if current.is_none() {
            if let Some(sentence) = pending.pop_front() {
                if !speaking {
                    speaking = true;
                    emit(VoiceEvent::SpeechStarted);
                }
                // Le texte exact de la phrase lancée : la GUI en tire l'animation de la bouche.
                emit(VoiceEvent::Utterance {
                    text: sentence.clone(),
                });
                current = Some(speaker.speak(sentence));
            } else if speaking && finished {
                speaking = false;
                finished = false;
                emit(VoiceEvent::SpeechFinished { interrupted: false });
            }
        }
        tokio::select! {
            result = async { current.as_mut().expect("gardé par la condition").await },
                if current.is_some() =>
            {
                current = None;
                if let Err(error) = result {
                    emit(VoiceEvent::Error { message: error.to_string() });
                }
            }
            command = commands.recv() => match command {
                // Moteur lâché : `current` part avec la tâche, et `say` avec lui.
                None => return,
                Some(SpeechCommand::Delta(text)) => {
                    finished = false;
                    pending.extend(splitter.push(&text));
                }
                Some(SpeechCommand::Finish) => {
                    pending.extend(splitter.finish());
                    finished = true;
                }
                Some(SpeechCommand::Stop) => {
                    // Lâcher le futur tue la phrase en cours.
                    current = None;
                    pending.clear();
                    splitter.clear();
                    finished = false;
                    if speaking {
                        speaking = false;
                        emit(VoiceEvent::SpeechFinished { interrupted: true });
                    }
                }
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Ce qui est arrivé à une phrase confiée au faux parleur.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Spoken {
        Started(String),
        Done(String),
        Cut(String),
    }

    /// Faux parleur : chaque phrase « dure » `per_char` par caractère, et l'on note si
    /// elle est allée au bout ou a été coupée.
    pub struct FakeSpeaker {
        pub per_char: Duration,
        pub log: Arc<Mutex<Vec<Spoken>>>,
    }

    impl FakeSpeaker {
        pub fn new(per_char: Duration) -> Self {
            Self {
                per_char,
                log: Arc::default(),
            }
        }

        pub fn log(&self) -> Vec<Spoken> {
            self.log.lock().unwrap().clone()
        }
    }

    /// Note `Cut` si la phrase est lâchée avant la fin.
    struct Guard {
        text: String,
        log: Arc<Mutex<Vec<Spoken>>>,
        done: bool,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let entry = if self.done {
                Spoken::Done(self.text.clone())
            } else {
                Spoken::Cut(self.text.clone())
            };
            self.log.lock().unwrap().push(entry);
        }
    }

    impl Speaker for FakeSpeaker {
        fn speak(&self, text: String) -> BoxFuture<'static, Result<(), VoiceError>> {
            let log = Arc::clone(&self.log);
            let duration = self.per_char * text.chars().count() as u32;
            Box::pin(async move {
                log.lock().unwrap().push(Spoken::Started(text.clone()));
                let mut guard = Guard {
                    text,
                    log,
                    done: false,
                };
                tokio::time::sleep(duration).await;
                guard.done = true;
                Ok(())
            })
        }
    }

    fn drain(rx: &mut broadcast::Receiver<VoiceEvent>) -> Vec<VoiceEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn utterance(text: &str) -> VoiceEvent {
        VoiceEvent::Utterance { text: text.into() }
    }

    #[tokio::test(start_paused = true)]
    async fn each_sentence_announces_its_exact_text_when_it_starts() {
        let speaker = Arc::new(FakeSpeaker::new(ms(10)));
        let (events, mut rx) = broadcast::channel(64);
        let queue = SpeechQueue::spawn(speaker.clone(), events);
        queue.delta("Première phrase. Seconde ");
        tokio::time::sleep(ms(1)).await;
        // La seconde n'est pas encore lancée : son texte n'est pas encore annoncé.
        assert_eq!(
            drain(&mut rx),
            vec![VoiceEvent::SpeechStarted, utterance("Première phrase.")]
        );
        queue.delta("phrase.");
        queue.finish();
        tokio::time::sleep(ms(1_000)).await;
        assert_eq!(
            drain(&mut rx),
            vec![
                utterance("Seconde phrase."),
                VoiceEvent::SpeechFinished { interrupted: false }
            ]
        );
        // L'annonce correspond à ce que le parleur a reçu, mot pour mot.
        assert_eq!(speaker.log()[2], Spoken::Started("Seconde phrase.".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn sentences_are_spoken_as_soon_as_complete_then_the_rest_at_the_end() {
        let speaker = Arc::new(FakeSpeaker::new(ms(10)));
        let (events, mut rx) = broadcast::channel(64);
        let queue = SpeechQueue::spawn(speaker.clone(), events);

        queue.delta("Bonjour ");
        queue.delta("à toi. Je");
        tokio::time::sleep(ms(5)).await;
        // La première phrase part avant la fin du flux.
        assert_eq!(
            speaker.log(),
            vec![Spoken::Started("Bonjour à toi.".into())]
        );
        assert_eq!(
            drain(&mut rx),
            vec![VoiceEvent::SpeechStarted, utterance("Bonjour à toi.")]
        );

        queue.delta(" regarde tes mails");
        queue.finish();
        tokio::time::sleep(ms(1_000)).await;
        assert_eq!(
            speaker.log(),
            vec![
                Spoken::Started("Bonjour à toi.".into()),
                Spoken::Done("Bonjour à toi.".into()),
                Spoken::Started("Je regarde tes mails".into()),
                Spoken::Done("Je regarde tes mails".into()),
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                utterance("Je regarde tes mails"),
                VoiceEvent::SpeechFinished { interrupted: false }
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_queue_waits_for_more_text_until_finish() {
        let speaker = Arc::new(FakeSpeaker::new(ms(1)));
        let (events, mut rx) = broadcast::channel(64);
        let queue = SpeechQueue::spawn(speaker.clone(), events);
        queue.delta("Une phrase. ");
        tokio::time::sleep(ms(500)).await;
        // Phrase dite, mais le flux continue : le discours n'est pas fini.
        assert_eq!(
            drain(&mut rx),
            vec![VoiceEvent::SpeechStarted, utterance("Une phrase.")]
        );
        queue.delta("Une autre.");
        queue.finish();
        tokio::time::sleep(ms(500)).await;
        assert_eq!(
            drain(&mut rx),
            vec![
                utterance("Une autre."),
                VoiceEvent::SpeechFinished { interrupted: false }
            ]
        );
        assert_eq!(speaker.log().len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_cuts_the_current_sentence_and_empties_the_queue() {
        let speaker = Arc::new(FakeSpeaker::new(ms(50)));
        let (events, mut rx) = broadcast::channel(64);
        let queue = SpeechQueue::spawn(speaker.clone(), events);
        queue.delta("Première phrase assez longue. Deuxième phrase. Troisième");
        tokio::time::sleep(ms(200)).await;
        queue.stop();
        tokio::time::sleep(ms(1)).await;
        assert_eq!(
            speaker.log(),
            vec![
                Spoken::Started("Première phrase assez longue.".into()),
                Spoken::Cut("Première phrase assez longue.".into()),
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                VoiceEvent::SpeechStarted,
                utterance("Première phrase assez longue."),
                VoiceEvent::SpeechFinished { interrupted: true }
            ]
        );
        // Rien de l'ancien discours ne ressort, même à la fin du flux.
        queue.finish();
        tokio::time::sleep(ms(5_000)).await;
        assert_eq!(speaker.log().len(), 2);
        assert!(drain(&mut rx).is_empty());

        // Un nouveau discours repart normalement après le barge-in.
        queue.delta("Nouveau départ.");
        queue.finish();
        tokio::time::sleep(ms(5_000)).await;
        assert_eq!(
            speaker.log()[2..],
            [
                Spoken::Started("Nouveau départ.".into()),
                Spoken::Done("Nouveau départ.".into()),
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                VoiceEvent::SpeechStarted,
                utterance("Nouveau départ."),
                VoiceEvent::SpeechFinished { interrupted: false }
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_kokoro_worker_that_died_while_idle_is_replaced_without_losing_the_sentence() {
        use std::os::unix::fs::PermissionsExt;
        // Faux Python : répond « ok » à chaque phrase, comme le vrai worker.
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python");
        std::fs::write(&python, "#!/bin/sh\nwhile read -r line; do echo ok; done\n").unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let speaker = KokoroSpeaker::new(python, "m".into(), "v".into(), "ff_siwis".into());

        speaker.speak("Première.".into()).await.unwrap();
        // Le processus en réserve meurt pendant la pause.
        let mut idle = speaker.worker.lock().unwrap().take().unwrap();
        idle.child.kill().await.unwrap();
        *speaker.worker.lock().unwrap() = Some(idle);

        speaker.speak("Après la pause.".into()).await.unwrap();
        let worker = speaker.worker.lock().unwrap().take().unwrap();
        assert!(
            worker.child.id().is_some(),
            "un processus neuf a pris le relais"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_when_silent_emits_nothing() {
        let speaker = Arc::new(FakeSpeaker::new(ms(1)));
        let (events, mut rx) = broadcast::channel(64);
        let queue = SpeechQueue::spawn(speaker.clone(), events);
        queue.stop();
        queue.finish();
        tokio::time::sleep(ms(100)).await;
        assert!(drain(&mut rx).is_empty());
        assert!(speaker.log().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_queue_cuts_the_speech() {
        let speaker = Arc::new(FakeSpeaker::new(ms(100)));
        let (events, _rx) = broadcast::channel(64);
        let queue = SpeechQueue::spawn(speaker.clone(), events);
        queue.delta("Une phrase qui ne finira pas. ");
        tokio::time::sleep(ms(10)).await;
        drop(queue);
        tokio::time::sleep(ms(10)).await;
        assert!(matches!(speaker.log().last(), Some(Spoken::Cut(_))));
    }
}
