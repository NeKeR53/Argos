//! Côté hub : une connexion par nœud déclaré, rétablie quand elle tombe. Une session
//! lancée sur un nœud se pilote comme une session locale (même [`Session`]) : ses
//! commandes partent dans la connexion, ses événements en reviennent.
//!
//! Connexion perdue : chaque session du nœud reçoit un `Exited` ; le superviseur parke
//! l'agent, qui reprendra par son session-id quand le nœud sera de nouveau joignable.

use super::transport::{self, NodeConfig};
use super::{FromNode, NODE_PROTOCOL, RemoteSpec, ToNode};
use crate::engine::{Command, EngineError, EngineEvent, Session};
use crate::supervisor::Supervisor;
use argos_core::protocol::NodeStatus;
use futures_util::{SinkExt, StreamExt};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Délai laissé au nœud pour lancer une session.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(60);
/// Attente entre deux tentatives de connexion, doublée à chaque échec.
const RETRY_FIRST: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// Les nœuds connus du hub. Vide : tout tourne sur le hub.
#[derive(Clone, Default)]
pub struct Nodes {
    links: Arc<BTreeMap<String, Arc<Link>>>,
}

impl std::fmt::Debug for Nodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.links.keys()).finish()
    }
}

struct Link {
    name: String,
    config: NodeConfig,
    sessions_seq: AtomicU64,
    state: Mutex<LinkState>,
}

#[derive(Default)]
struct LinkState {
    outbox: Option<mpsc::UnboundedSender<ToNode>>,
    /// Dernière erreur de connexion, pour l'utilisateur.
    error: Option<String>,
    /// Événements attendus, par session.
    sessions: HashMap<u64, mpsc::UnboundedSender<EngineEvent>>,
    /// Lancements en attente de la réponse du nœud.
    spawning: HashMap<u64, oneshot::Sender<Result<String, String>>>,
}

impl Nodes {
    pub fn new(configs: BTreeMap<String, NodeConfig>) -> Self {
        let links = configs
            .into_iter()
            .map(|(name, config)| {
                let link = Link {
                    name: name.clone(),
                    config,
                    sessions_seq: AtomicU64::new(0),
                    state: Mutex::default(),
                };
                (name, Arc::new(link))
            })
            .collect();
        Self {
            links: Arc::new(links),
        }
    }

    pub fn is_known(&self, name: &str) -> bool {
        self.links.contains_key(name)
    }

    pub fn status(&self) -> Vec<NodeStatus> {
        self.links
            .values()
            .map(|link| {
                let state = locked(&link.state);
                NodeStatus {
                    name: link.name.clone(),
                    connected: state.outbox.is_some(),
                    error: state.error.clone(),
                }
            })
            .collect()
    }

    /// Lance les connexions ; les requêtes MCP relayées par les nœuds sont servies par
    /// `supervisor`. `ssh` : binaire des tunnels.
    pub fn connect(&self, supervisor: Supervisor, ssh: PathBuf) {
        for link in self.links.values() {
            tokio::spawn(Arc::clone(link).run(supervisor.clone(), ssh.clone()));
        }
    }

    /// Lance une session sur un nœud. Erreur si le nœud est inconnu, injoignable ou
    /// refuse.
    pub async fn spawn(&self, node: &str, spec: RemoteSpec) -> Result<Session, EngineError> {
        let link = self
            .links
            .get(node)
            .ok_or_else(|| EngineError::Config(format!("nœud inconnu : {node}")))?;
        let session = link.sessions_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let (spawned_tx, spawned) = oneshot::channel();
        let outbox = {
            let mut state = locked(&link.state);
            let Some(outbox) = state.outbox.clone() else {
                return Err(unreachable(node, state.error.as_deref()));
            };
            // Inscrits avant l'envoi : aucun événement ne peut arriver avant eux.
            state.sessions.insert(session, events_tx);
            state.spawning.insert(session, spawned_tx);
            outbox
        };
        let forget = || {
            let mut state = locked(&link.state);
            state.sessions.remove(&session);
            state.spawning.remove(&session);
        };
        if outbox.send(ToNode::Spawn { session, spec }).is_err() {
            forget();
            return Err(unreachable(node, None));
        }
        let session_id = match tokio::time::timeout(SPAWN_TIMEOUT, spawned).await {
            Ok(Ok(Ok(session_id))) => session_id,
            Ok(Ok(Err(error))) => {
                forget();
                return Err(EngineError::Protocol(format!("nœud {node} : {error}")));
            }
            _ => {
                forget();
                return Err(unreachable(node, None));
            }
        };
        let name = node.to_string();
        Ok(Session::launch(
            session_id,
            None,
            move |mut commands, out| async move {
                loop {
                    tokio::select! {
                        command = commands.recv() => {
                            let closed = command.is_none();
                            let order = match command {
                                Some(Command::Send(text)) => ToNode::Send { session, text },
                                Some(Command::Interrupt) => ToNode::Interrupt { session },
                                Some(Command::Kill) => ToNode::Kill { session },
                                // Handle abandonné : comme en local, la session est parkée.
                                Some(Command::Park) | None => ToNode::Park { session },
                            };
                            let _ = outbox.send(order);
                            if closed {
                                // Plus de commandes : il ne reste qu'à attendre la fin.
                                return drain(&mut events, &out, &name).await;
                            }
                        }
                        event = events.recv() => match event {
                            Some(EngineEvent::Exited { code, stderr_tail }) => {
                                return EngineEvent::Exited { code, stderr_tail };
                            }
                            Some(event) => {
                                let _ = out.send(event);
                            }
                            None => return lost(&name),
                        },
                    }
                }
            },
        ))
    }
}

/// Relaie les derniers événements d'une session jusqu'à sa fin.
async fn drain(
    events: &mut mpsc::UnboundedReceiver<EngineEvent>,
    out: &mpsc::UnboundedSender<EngineEvent>,
    node: &str,
) -> EngineEvent {
    while let Some(event) = events.recv().await {
        if matches!(event, EngineEvent::Exited { .. }) {
            return event;
        }
        let _ = out.send(event);
    }
    lost(node)
}

fn lost(node: &str) -> EngineEvent {
    EngineEvent::Exited {
        code: None,
        stderr_tail: vec![format!("connexion au nœud {node} perdue")],
    }
}

fn unreachable(node: &str, error: Option<&str>) -> EngineError {
    EngineError::Config(match error {
        Some(error) => format!("nœud {node} injoignable : {error}"),
        None => format!("nœud {node} injoignable"),
    })
}

impl Link {
    /// Se connecte, sert la connexion, puis recommence quand elle tombe.
    async fn run(self: Arc<Self>, supervisor: Supervisor, ssh: PathBuf) {
        let mut wait = RETRY_FIRST;
        loop {
            match self.session(&supervisor, &ssh).await {
                // La connexion a tenu : on réessaie vite.
                Ok(()) => wait = RETRY_FIRST,
                Err(error) => {
                    if locked(&self.state).error.as_ref() != Some(&error) {
                        eprintln!("nœud {} : {error}", self.name);
                    }
                    locked(&self.state).error = Some(error);
                }
            }
            self.disconnected();
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(RETRY_MAX);
        }
    }

    /// Une connexion, de l'ouverture à la perte. `Ok` si elle a été établie.
    async fn session(&self, supervisor: &Supervisor, ssh: &std::path::Path) -> Result<(), String> {
        let token = self
            .config
            .token()
            .map_err(|error| format!("jeton : {error}"))?;
        // Le tunnel vit aussi longtemps que cette fonction.
        let opened = transport::open(&self.config, ssh).await?;
        let url = format!("{}/?token={token}", opened.url.trim_end_matches('/'));
        let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|error| format!("connexion : {error}"))?;
        let (mut sink, mut source) = socket.split();

        // Le premier message dit qui répond : un nœud mal configuré est refusé.
        match next_message(&mut source).await {
            Some(FromNode::Hello { node, protocol }) if protocol == NODE_PROTOCOL => {
                if node != self.name {
                    return Err(format!("le nœud se présente comme « {node} »"));
                }
            }
            Some(FromNode::Hello { protocol, .. }) => {
                return Err(format!(
                    "protocole du nœud v{protocol}, attendu v{NODE_PROTOCOL}"
                ));
            }
            _ => return Err("pas de présentation du nœud".into()),
        }

        let (outbox, mut queued) = mpsc::unbounded_channel::<ToNode>();
        {
            let mut state = locked(&self.state);
            state.outbox = Some(outbox.clone());
            state.error = None;
        }
        eprintln!("nœud {} connecté", self.name);
        let writer = tokio::spawn(async move {
            while let Some(order) = queued.recv().await {
                let text = serde_json::to_string(&order).expect("ordre sérialisable");
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
        });
        while let Some(message) = next_message(&mut source).await {
            self.handle(message, supervisor, &outbox);
        }
        writer.abort();
        drop(opened);
        Ok(())
    }

    fn handle(
        &self,
        message: FromNode,
        supervisor: &Supervisor,
        outbox: &mpsc::UnboundedSender<ToNode>,
    ) {
        match message {
            FromNode::Hello { .. } => {}
            FromNode::Spawned {
                session,
                session_id,
            } => {
                if let Some(waiting) = locked(&self.state).spawning.remove(&session) {
                    let _ = waiting.send(Ok(session_id));
                }
            }
            FromNode::SpawnFailed { session, error } => {
                let mut state = locked(&self.state);
                state.sessions.remove(&session);
                if let Some(waiting) = state.spawning.remove(&session) {
                    let _ = waiting.send(Err(error));
                }
            }
            FromNode::Event { session, event } => {
                let mut state = locked(&self.state);
                let last = matches!(event, EngineEvent::Exited { .. });
                if let Some(events) = state.sessions.get(&session) {
                    let _ = events.send(event);
                }
                if last {
                    state.sessions.remove(&session);
                }
            }
            FromNode::McpCall { call, token, body } => {
                let supervisor = supervisor.clone();
                let outbox = outbox.clone();
                tokio::spawn(async move {
                    let (status, reply) = crate::mcp::answer(&supervisor, &token, &body).await;
                    let _ = outbox.send(ToNode::McpReply {
                        call,
                        status: status.as_u16(),
                        body: reply.map(|reply| reply.to_string()).unwrap_or_default(),
                    });
                });
            }
        }
    }

    /// Connexion perdue : les sessions du nœud se terminent, les lancements échouent.
    fn disconnected(&self) {
        let (sessions, spawning) = {
            let mut state = locked(&self.state);
            state.outbox = None;
            (
                std::mem::take(&mut state.sessions),
                std::mem::take(&mut state.spawning),
            )
        };
        for events in sessions.into_values() {
            let _ = events.send(lost(&self.name));
        }
        for waiting in spawning.into_values() {
            let _ = waiting.send(Err("connexion perdue".into()));
        }
    }
}

async fn next_message<S>(source: &mut S) -> Option<FromNode>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match source.next().await? {
            Ok(Message::Text(text)) => match serde_json::from_str(text.as_str()) {
                Ok(message) => return Some(message),
                Err(error) => eprintln!("message du nœud illisible : {error}"),
            },
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("verrou empoisonné")
}
