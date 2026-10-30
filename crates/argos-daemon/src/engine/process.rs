//! Lancement, écriture et arrêt des process de moteur.

use super::{EngineError, EngineEvent};
use serde_json::Value;
use std::collections::VecDeque;
use std::io;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

/// Délai laissé à un process pour sortir proprement avant d'être tué.
const GRACE: Duration = Duration::from_secs(5);
/// Lignes de stderr conservées pour diagnostiquer une sortie anormale.
const STDERR_TAIL: usize = 20;

#[derive(Debug, Clone)]
pub(crate) struct ProcessTree {
    #[cfg(unix)]
    pgid: i32,
    #[cfg(windows)]
    job: argos_core::windows_job::Job,
}
impl ProcessTree {
    pub fn kill(&self) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(self.pgid, libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = self.job.kill();
        }
    }
}

pub(super) struct Spawned {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    pub stdout: Lines<BufReader<ChildStdout>>,
    pub tree: ProcessTree,
    stderr: StderrTail,
}

pub(super) fn spawn(mut command: Command) -> Result<Spawned, EngineError> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Groupe de process dédié : un kill atteint aussi les outils lancés par l'agent.
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0000_0004); // CREATE_SUSPENDED : attacher le job avant exécution.
    let mut child = command
        .spawn()
        .map_err(|source| EngineError::Spawn { program, source })?;
    #[cfg(unix)]
    let tree = ProcessTree {
        pgid: child.id().ok_or(EngineError::Closed)? as i32,
    };
    #[cfg(windows)]
    let tree = ProcessTree {
        job: argos_core::windows_job::Job::assign_suspended(
            child.raw_handle().ok_or(EngineError::Closed)?,
            child.id().ok_or(EngineError::Closed)?,
        )?,
    };
    let stdin = child.stdin.take();
    let stdout = BufReader::new(child.stdout.take().expect("stdout est redirigé")).lines();
    let stderr = StderrTail::start(child.stderr.take().expect("stderr est redirigé"));
    Ok(Spawned {
        child,
        stdin,
        stdout,
        tree,
        stderr,
    })
}

/// Écrit une ligne JSON. Prend stdin seul pour pouvoir écrire pendant qu'on lit stdout.
pub(super) async fn write_json(stdin: &mut Option<ChildStdin>, value: &Value) -> io::Result<()> {
    let stdin = stdin
        .as_mut()
        .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await
}

impl Spawned {
    /// Prochaine ligne de stdout dans un délai donné (phase d'initialisation).
    pub async fn read_line_within(&mut self, limit: Duration) -> Result<String, EngineError> {
        match timeout(limit, self.stdout.next_line()).await {
            Ok(Ok(Some(line))) => Ok(line),
            Ok(Ok(None)) => Err(EngineError::Protocol(format!(
                "le moteur s'est arrêté : {}",
                self.stderr.snapshot().join(" | ")
            ))),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err(EngineError::Protocol("le moteur ne répond pas".into())),
        }
    }

    /// Ferme stdin, transmet les dernières lignes de stdout, attend la sortie du
    /// process (et tue son groupe s'il traîne), puis produit l'événement de fin.
    pub async fn finish(mut self, mut on_line: impl FnMut(&str)) -> EngineEvent {
        self.stdin = None;
        let deadline = Instant::now() + GRACE;
        while let Ok(Ok(Some(line))) = timeout_at(deadline, self.stdout.next_line()).await {
            on_line(&line);
        }
        let status = match timeout_at(deadline, self.child.wait()).await {
            Ok(Ok(status)) => Some(status),
            _ => {
                self.tree.kill();
                self.child.wait().await.ok()
            }
        };
        EngineEvent::Exited {
            code: status.and_then(|status| status.code()),
            stderr_tail: self.stderr.collect().await,
        }
    }
}

struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
    task: JoinHandle<()>,
}

impl StderrTail {
    fn start(stderr: ChildStderr) -> Self {
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&lines);
        let task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let mut tail = sink.lock().expect("verrou stderr empoisonné");
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
        Self { lines, task }
    }

    fn snapshot(&self) -> Vec<String> {
        snapshot(&self.lines)
    }

    async fn collect(self) -> Vec<String> {
        let StderrTail { lines, task } = self;
        let _ = timeout(Duration::from_secs(1), task).await;
        snapshot(&lines)
    }
}

fn snapshot(lines: &Mutex<VecDeque<String>>) -> Vec<String> {
    lines
        .lock()
        .expect("verrou stderr empoisonné")
        .iter()
        .cloned()
        .collect()
}
