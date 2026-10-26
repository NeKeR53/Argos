//! Adaptateurs de moteur : une session `claude` ou `codex` pilotée en subprocess,
//! exposée derrière une interface unique (docs/PHASE-1.md, tranche T1).

mod claude;
mod codex;
mod process;

pub use argos_core::{TurnOutcome, Usage};
pub use codex::prepare_codex_home;

use argos_core::EngineKind;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// Ce qu'il faut pour lancer ou reprendre une session.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub engine: EngineKind,
    /// Répertoire de travail de l'agent.
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// Consignes ajoutées au prompt système : le contrat de l'agent.
    pub instructions: Option<String>,
    /// Session à reprendre après un parking ou un crash.
    pub resume: Option<String>,
    /// Binaire à lancer à la place de `claude` / `codex` (tests).
    pub program: Option<PathBuf>,
    /// `CODEX_HOME` isolé, obligatoire pour Codex (voir [`prepare_codex_home`]).
    pub codex_home: Option<PathBuf>,
    /// Serveurs MCP donnés à l'agent (celui d'Argos, avec le jeton de la session).
    pub mcp_servers: Vec<McpServer>,
    /// L'agent ne modifie rien lui-même (ni commande, ni écriture de fichier) : il
    /// orchestre et délègue. C'est le cas de l'Assistant et des Agents.
    pub read_only: bool,
}

/// Un serveur MCP HTTP.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub url: String,
}

impl SessionSpec {
    pub fn new(engine: EngineKind, cwd: impl Into<PathBuf>) -> Self {
        Self {
            engine,
            cwd: cwd.into(),
            model: None,
            instructions: None,
            resume: None,
            program: None,
            codex_home: None,
            mcp_servers: vec![],
            read_only: false,
        }
    }

    fn program(&self) -> PathBuf {
        self.program
            .clone()
            .unwrap_or_else(|| self.engine.binary().into())
    }
}

/// Événement normalisé, identique quel que soit le moteur.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// Message complet de l'agent.
    AgentMessage { text: String },
    /// L'agent utilise un outil (commande, édition, appel MCP…).
    ToolActivity { name: String },
    TurnCompleted {
        outcome: TurnOutcome,
        usage: Option<Usage>,
    },
    /// Le moteur refuse de travailler : quota ou budget épuisé (D22 : repli).
    RateLimited { message: String },
    /// Le process s'est terminé ; aucun événement ne suit. `code` vaut `None` s'il a
    /// été tué. Sinon il vient du moteur : `claude` rend le statut de son dernier tour,
    /// donc 1 après un tour interrompu, même sur un parking demandé.
    Exited {
        code: Option<i32>,
        stderr_tail: Vec<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("impossible de lancer {program} : {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("erreur d'entrée/sortie avec le moteur : {0}")]
    Io(#[from] std::io::Error),
    #[error("réponse inattendue du moteur : {0}")]
    Protocol(String),
    #[error("configuration invalide : {0}")]
    Config(String),
    #[error("la session est terminée")]
    Closed,
}

enum Command {
    Send(String),
    Interrupt,
    Park,
}

type Events = mpsc::UnboundedSender<EngineEvent>;

/// Une session vivante. Le process appartient à une tâche de fond ; ce handle lui
/// transmet les commandes et reçoit ses événements, jusqu'à `Exited`. [`Session::split`]
/// sépare les deux, pour lire les événements dans une tâche à part.
pub struct Session {
    control: SessionControl,
    events: EventStream,
}

/// La moitié commande d'une session ; clonable.
#[derive(Clone)]
pub struct SessionControl {
    session_id: String,
    pgid: i32,
    exited: Arc<AtomicBool>,
    commands: mpsc::UnboundedSender<Command>,
}

/// La moitié événements d'une session.
pub struct EventStream {
    events: mpsc::UnboundedReceiver<EngineEvent>,
}

impl Session {
    pub async fn spawn(spec: SessionSpec) -> Result<Session, EngineError> {
        match spec.engine {
            EngineKind::Claude => claude::spawn(spec).await,
            EngineKind::Codex => codex::spawn(spec).await,
        }
    }

    pub fn split(self) -> (SessionControl, EventStream) {
        (self.control, self.events)
    }

    /// Identifiant à passer dans `SessionSpec::resume` pour reprendre la session.
    pub fn session_id(&self) -> &str {
        self.control.session_id()
    }

    pub fn send(&self, text: impl Into<String>) -> Result<(), EngineError> {
        self.control.send(text)
    }

    pub fn interrupt(&self) -> Result<(), EngineError> {
        self.control.interrupt()
    }

    pub fn park(&self) -> Result<(), EngineError> {
        self.control.park()
    }

    pub fn kill(&self) {
        self.control.kill()
    }

    pub async fn next_event(&mut self) -> Option<EngineEvent> {
        self.events.next_event().await
    }

    /// Démarre la tâche qui possède le process. `driver` rend l'événement de fin.
    fn launch<F, Fut>(session_id: String, pgid: i32, driver: F) -> Session
    where
        F: FnOnce(mpsc::UnboundedReceiver<Command>, Events) -> Fut,
        Fut: Future<Output = EngineEvent> + Send + 'static,
    {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let exited = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&exited);
        let driving = driver(command_rx, event_tx.clone());
        tokio::spawn(async move {
            let last = driving.await;
            flag.store(true, Ordering::SeqCst);
            let _ = event_tx.send(last);
        });
        Session {
            control: SessionControl {
                session_id,
                pgid,
                exited,
                commands,
            },
            events: EventStream { events },
        }
    }
}

impl SessionControl {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Envoie un message utilisateur à l'agent.
    pub fn send(&self, text: impl Into<String>) -> Result<(), EngineError> {
        self.command(Command::Send(text.into()))
    }

    /// Interrompt le tour en cours ; la session reste utilisable.
    pub fn interrupt(&self) -> Result<(), EngineError> {
        self.command(Command::Interrupt)
    }

    /// Parking : termine proprement le process. La session se reprend par son identifiant.
    pub fn park(&self) -> Result<(), EngineError> {
        self.command(Command::Park)
    }

    /// Arrêt dur du process et de tout ce qu'il a lancé (plan de contrôle, D14).
    pub fn kill(&self) {
        if !self.exited.load(Ordering::SeqCst) {
            process::kill_group(self.pgid);
        }
    }

    fn command(&self, command: Command) -> Result<(), EngineError> {
        self.commands.send(command).map_err(|_| EngineError::Closed)
    }
}

impl EventStream {
    /// Prochain événement ; `None` une fois `Exited` reçu.
    pub async fn next_event(&mut self) -> Option<EngineEvent> {
        self.events.recv().await
    }
}

fn forward(events: &Events, batch: Vec<EngineEvent>) {
    for event in batch {
        // Un handle abandonné n'écoute plus : ses événements peuvent être perdus.
        let _ = events.send(event);
    }
}
