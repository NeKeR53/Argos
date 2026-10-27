//! Côté nœud : le daemon lancé en mode `node` (`argos-daemon node`). Il n'a ni registre
//! ni Assistant ; il lance les sessions que le hub lui demande, lui en relaie les
//! événements et sert aux agents un mandataire MCP local dont il transmet les requêtes au
//! hub par la même connexion.
//!
//! Un seul hub à la fois : une nouvelle connexion remplace la précédente. Quand la
//! connexion tombe, les sessions qu'elle portait sont parkées ; le hub les reprendra par
//! leur session-id au prochain message.

use super::{FromNode, NODE_PROTOCOL, RemoteSpec, ToNode, token_in};
use crate::engine::{McpServer, Session, SessionControl, SessionSpec, prepare_codex_home};
use crate::memory::{Memory, MemoryOp};
use argos_core::AgentId;
use argos_core::EngineKind;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as Handshake, Response as HandshakeResponse,
};

/// Délai laissé à une session pour sortir après le parking qui suit une déconnexion.
const PARK_GRACE: Duration = Duration::from_secs(10);
/// Une requête MCP sans réponse du hub au bout de ce délai est abandonnée.
const MCP_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_FUNCTIONS: usize = 32;

#[derive(Debug, Clone)]
pub struct RunnerSettings {
    /// Nom du nœud, tel que le hub le connaît.
    pub name: String,
    /// Dossier de données du nœud : répertoires de travail relatifs, `CODEX_HOME`.
    pub home: PathBuf,
    pub user_codex_home: PathBuf,
    /// Binaires à lancer à la place de `claude` / `codex` (tests).
    pub programs: HashMap<EngineKind, PathBuf>,
}

#[derive(Clone)]
struct Runner {
    inner: Arc<Inner>,
}

struct Inner {
    settings: RunnerSettings,
    token: String,
    /// Adresse du mandataire MCP, à donner aux sessions.
    mcp_url: String,
    /// La connexion du hub en cours.
    hub: Mutex<Option<Hub>>,
    epochs: AtomicU64,
    calls: AtomicU64,
    /// Requêtes MCP relayées au hub, en attente de sa réponse.
    pending: Mutex<HashMap<u64, oneshot::Sender<(u16, String)>>>,
    /// Les mémoires des agents du nœud, qui vivent ici (D23).
    memory: Memory,
}

struct Hub {
    epoch: u64,
    outbox: mpsc::UnboundedSender<FromNode>,
    /// Fermé pour couper cette connexion quand une autre la remplace.
    _replaced: oneshot::Sender<()>,
}

/// Sert la connexion du hub sur `listener` et le mandataire MCP sur `mcp_listener`,
/// jusqu'à une erreur d'écoute.
pub async fn serve(
    listener: TcpListener,
    mcp_listener: TcpListener,
    settings: RunnerSettings,
    token: String,
) -> std::io::Result<()> {
    let runner = Runner {
        inner: Arc::new(Inner {
            token,
            mcp_url: format!("http://{}", mcp_listener.local_addr()?),
            hub: Mutex::default(),
            epochs: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            pending: Mutex::default(),
            memory: Memory::new(settings.home.join("memory")),
            settings,
        }),
    };
    let router = axum::Router::new()
        .route(
            "/mcp/{token}",
            post(proxy)
                .get(crate::mcp::no_stream)
                .delete(crate::mcp::no_stream),
        )
        .with_state(runner.clone());
    let accept = async {
        loop {
            let (stream, _) = listener.accept().await?;
            tokio::spawn(runner.clone().connection(stream));
        }
    };
    tokio::select! {
        accepted = accept => accepted,
        served = axum::serve(mcp_listener, router) => served,
    }
}

impl Runner {
    async fn connection(self, stream: TcpStream) {
        let token = self.inner.token.clone();
        // Signature imposée par tungstenite, erreur volumineuse comprise.
        #[allow(clippy::result_large_err)]
        let authorize = |handshake: &Handshake, response: HandshakeResponse| {
            if token_in(handshake.uri().query()) == Some(token.as_str()) {
                return Ok(response);
            }
            let mut refusal = ErrorResponse::new(Some("jeton absent ou invalide".into()));
            *refusal.status_mut() = StatusCode::UNAUTHORIZED;
            Err(refusal)
        };
        let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, authorize).await else {
            return;
        };
        let (mut sink, mut source) = socket.split();
        let (outbox, mut queued) = mpsc::unbounded_channel::<FromNode>();
        let (replaced_tx, mut replaced) = oneshot::channel();
        let epoch = self.inner.epochs.fetch_add(1, Ordering::SeqCst) + 1;
        // L'ancienne connexion, s'il y en a une, voit son signal fermé et s'arrête.
        *locked(&self.inner.hub) = Some(Hub {
            epoch,
            outbox: outbox.clone(),
            _replaced: replaced_tx,
        });
        let _ = outbox.send(FromNode::Hello {
            node: self.inner.settings.name.clone(),
            protocol: NODE_PROTOCOL,
        });
        // Le hub resynchronise ses miroirs : la mémoire a pu changer pendant son absence.
        tokio::spawn(
            self.clone()
                .snapshots(self.inner.memory.agents(), outbox.clone()),
        );
        let writer = tokio::spawn(async move {
            while let Some(message) = queued.recv().await {
                let text = serde_json::to_string(&message).expect("message sérialisable");
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
        });

        let sessions: Sessions = Arc::default();
        let mut functions: HashMap<u64, tokio::task::JoinHandle<()>> = HashMap::new();
        loop {
            let frame = tokio::select! {
                frame = source.next() => frame,
                _ = &mut replaced => break,
            };
            let text = match frame {
                Some(Ok(Message::Text(text))) => text,
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => continue,
            };
            match serde_json::from_str::<ToNode>(text.as_str()) {
                Ok(order) => self.order(order, &sessions, &outbox, &mut functions),
                Err(error) => eprintln!("nœud : message du hub illisible : {error}"),
            }
        }

        writer.abort();
        for function in functions.into_values() {
            function.abort();
        }
        {
            let mut hub = locked(&self.inner.hub);
            if hub.as_ref().is_some_and(|hub| hub.epoch == epoch) {
                *hub = None;
                // Ces réponses ne viendront plus : les requêtes échouent tout de suite.
                locked(&self.inner.pending).clear();
            }
        }
        // Le hub est parti : ses sessions sont parkées, il les reprendra.
        let orphans: Vec<SessionControl> = locked(&sessions).drain().map(|(_, c)| c).collect();
        for control in orphans {
            let _ = control.park();
            tokio::spawn(async move {
                tokio::time::sleep(PARK_GRACE).await;
                control.kill();
            });
        }
    }

    fn order(
        &self,
        order: ToNode,
        sessions: &Sessions,
        outbox: &Outbox,
        functions: &mut HashMap<u64, tokio::task::JoinHandle<()>>,
    ) {
        let control = |session: u64| locked(sessions).get(&session).cloned();
        let gone = |session: u64| {
            // Session inconnue (déjà finie) : le hub doit le savoir pour la solder.
            let _ = outbox.send(FromNode::Event {
                session,
                event: crate::engine::EngineEvent::Exited {
                    code: None,
                    stderr_tail: vec!["session inconnue sur ce nœud".into()],
                },
            });
        };
        match order {
            ToNode::Spawn { session, spec } => {
                tokio::spawn(
                    self.clone()
                        .spawn(session, spec, sessions.clone(), outbox.clone()),
                );
            }
            ToNode::Send { session, text } => match control(session) {
                Some(control) => {
                    let _ = control.send(text);
                }
                None => gone(session),
            },
            ToNode::Interrupt { session } => {
                if let Some(control) = control(session) {
                    let _ = control.interrupt();
                }
            }
            ToNode::Park { session } => match control(session) {
                Some(control) => {
                    let _ = control.park();
                }
                None => gone(session),
            },
            ToNode::Kill { session } => match control(session) {
                Some(control) => control.kill(),
                None => gone(session),
            },
            ToNode::Memory { call, agent, op } => {
                tokio::spawn(self.clone().memory(call, agent, op, outbox.clone()));
            }
            ToNode::Function {
                call,
                agent: _,
                request,
                arguments,
            } => {
                functions.retain(|_, task| !task.is_finished());
                if functions.len() >= MAX_FUNCTIONS || functions.contains_key(&call) {
                    let _ = outbox.send(FromNode::FunctionReply {
                        call,
                        result: Err("trop de fonctions en cours sur ce nœud".into()),
                    });
                    return;
                }
                let outbox = outbox.clone();
                let task = tokio::spawn(async move {
                    let result = crate::functions::execute(&request, &arguments).await;
                    let _ = outbox.send(FromNode::FunctionReply { call, result });
                });
                functions.insert(call, task);
            }
            ToNode::CancelFunction { call } => {
                if let Some(task) = functions.remove(&call) {
                    task.abort();
                }
            }
            ToNode::McpReply { call, status, body } => {
                if let Some(waiting) = locked(&self.inner.pending).remove(&call) {
                    let _ = waiting.send((status, body));
                }
            }
        }
    }

    async fn spawn(self, session: u64, spec: RemoteSpec, sessions: Sessions, outbox: Outbox) {
        let spawned = match self.local_spec(spec) {
            Ok(spec) => Session::spawn(spec)
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };
        let (control, mut events) = match spawned {
            Ok(spawned) => spawned.split(),
            Err(error) => {
                let _ = outbox.send(FromNode::SpawnFailed { session, error });
                return;
            }
        };
        locked(&sessions).insert(session, control.clone());
        let _ = outbox.send(FromNode::Spawned {
            session,
            session_id: control.session_id().to_string(),
        });
        while let Some(event) = events.next_event().await {
            let last = matches!(event, crate::engine::EngineEvent::Exited { .. });
            let _ = outbox.send(FromNode::Event { session, event });
            if last {
                break;
            }
        }
        locked(&sessions).remove(&session);
    }

    async fn memory(self, call: u64, agent: AgentId, op: MemoryOp, outbox: Outbox) {
        let writes = op.writes();
        let memory = self.inner.memory.clone();
        let target = agent.clone();
        // git est lancé à chaque écriture : hors des threads de l'exécuteur.
        let result = tokio::task::spawn_blocking(move || memory.apply(&target, op))
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        let succeeded = result.is_ok();
        let _ = outbox.send(FromNode::MemoryReply { call, result });
        if writes && succeeded {
            self.snapshots(vec![agent], outbox).await;
        }
    }

    /// Envoie au hub la copie complète de ces mémoires.
    async fn snapshots(self, agents: Vec<AgentId>, outbox: Outbox) {
        let memory = self.inner.memory.clone();
        let copies = tokio::task::spawn_blocking(move || {
            agents
                .into_iter()
                .filter_map(|agent| Some((memory.snapshot(&agent).ok()?, agent)))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        for (files, agent) in copies {
            let _ = outbox.send(FromNode::MemorySnapshot { agent, files });
        }
    }

    /// Complète la demande du hub avec ce qui dépend du nœud.
    fn local_spec(&self, remote: RemoteSpec) -> Result<SessionSpec, String> {
        let settings = &self.inner.settings;
        let cwd = settings.home.join(&remote.cwd);
        std::fs::create_dir_all(&cwd).map_err(|error| format!("{}: {error}", cwd.display()))?;
        let mut spec = SessionSpec::new(remote.engine, cwd);
        spec.model = remote.model;
        spec.instructions = remote.instructions;
        spec.resume = remote.resume;
        spec.access = remote.access;
        spec.program = settings.programs.get(&remote.engine).cloned();
        if let Some(token) = remote.mcp_token {
            spec.mcp_servers = vec![McpServer {
                name: "argos".into(),
                url: format!("{}/mcp/{token}", self.inner.mcp_url),
            }];
        }
        if remote.engine == EngineKind::Codex {
            let home = settings.home.join("codex").join(remote.agent.as_str());
            prepare_codex_home(&home, &settings.user_codex_home)
                .map_err(|error| format!("CODEX_HOME : {error}"))?;
            spec.codex_home = Some(home);
        }
        Ok(spec)
    }
}

type Sessions = Arc<Mutex<HashMap<u64, SessionControl>>>;
type Outbox = mpsc::UnboundedSender<FromNode>;

/// Mandataire MCP : relaie la requête au hub et rend sa réponse telle quelle.
async fn proxy(State(runner): State<Runner>, Path(token): Path<String>, body: String) -> Response {
    let Some(outbox) = locked(&runner.inner.hub)
        .as_ref()
        .map(|hub| hub.outbox.clone())
    else {
        return (StatusCode::SERVICE_UNAVAILABLE, "hub injoignable").into_response();
    };
    let call = runner.inner.calls.fetch_add(1, Ordering::SeqCst) + 1;
    let (reply_tx, reply) = oneshot::channel();
    locked(&runner.inner.pending).insert(call, reply_tx);
    if outbox
        .send(FromNode::McpCall { call, token, body })
        .is_err()
    {
        locked(&runner.inner.pending).remove(&call);
        return (StatusCode::SERVICE_UNAVAILABLE, "hub injoignable").into_response();
    }
    match tokio::time::timeout(MCP_TIMEOUT, reply).await {
        Ok(Ok((status, body))) => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            if body.is_empty() {
                status.into_response()
            } else {
                (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
            }
        }
        // Connexion perdue ou hub muet : l'agent voit une erreur de transport.
        _ => {
            locked(&runner.inner.pending).remove(&call);
            (StatusCode::BAD_GATEWAY, "pas de réponse du hub").into_response()
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("verrou empoisonné")
}
