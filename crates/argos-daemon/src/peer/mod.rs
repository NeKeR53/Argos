//! Nœuds pairs (D48) : plus de hub. Chaque nœud fait tourner le daemon complet, possède
//! ses agents en entier et se relie aux autres par des liens WebSocket, ouverts
//! directement ou dans un tunnel SSH (voir `crate::node::transport`). Un lien ouvert par
//! l'un des deux pairs sert ensuite dans les deux sens.
//!
//! Sur un lien passent :
//! - l'**annuaire** : chaque nœud annonce la fiche de ses agents à l'ouverture, puis à
//!   chaque changement ; le pair en garde une copie (`node` = le détenteur) ;
//! - des **appels** avec réponse ([`PeerCall`]) : un outil qui vise un agent détenu
//!   ailleurs, un message à lui remettre, une opération sur sa mémoire, l'arbre d'un
//!   workspace, l'arrivée d'un agent qui change de nœud ;
//! - les changements vus par le watcher d'une synchro ouverte par le pair (D44).
//!
//! Un message pour un pair injoignable attend dans une file persistante du registre et
//! part dans l'ordre à la reconnexion.

mod calls;

use crate::memory::{MemoryOp, MemoryOutput, MemoryPackage};
use crate::node::token_in;
use crate::node::transport::{self, NodeConfig};
use crate::registry::Rows;
use crate::supervisor::Supervisor;
use crate::sync::{Delta, SyncRequest, SyncResponse};
use argos_core::protocol::{NodeStatus, Origin};
use argos_core::{AgentId, AgentRecord};
use calls::Trees;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as Handshake, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::StatusCode;

/// Version du protocole entre pairs.
pub const PEER_PROTOCOL: u32 = 1;

/// Attente entre deux tentatives de connexion, doublée à chaque échec.
const RETRY_FIRST: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// Un message sur un lien, dans un sens ou dans l'autre.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerMessage {
    Hello {
        node: String,
        protocol: u32,
    },
    /// Fiches des agents détenus par l'émetteur ; `full` : la liste complète.
    Directory {
        agents: Vec<AgentRecord>,
        full: bool,
    },
    Call {
        call: u64,
        request: PeerCall,
    },
    Reply {
        call: u64,
        result: Result<PeerOutput, String>,
    },
    /// Ce que le watcher d'une synchro ouverte par le pair a vu changer.
    SyncChanged {
        sync: String,
        delta: Delta,
    },
}

/// Ce qu'un nœud demande à son pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum PeerCall {
    /// Un outil MCP qui vise un agent détenu par le pair, au nom de `caller`.
    Tool {
        caller: AgentId,
        name: String,
        args: Value,
    },
    /// Un message pour un agent détenu par le pair.
    Deliver {
        to: AgentId,
        from: Origin,
        text: String,
    },
    Memory {
        agent: AgentId,
        op: MemoryOp,
    },
    /// L'arbre d'un workspace : copie d'un déplacement, ou synchro D44.
    Sync {
        sync: String,
        request: SyncRequest,
    },
    /// Un agent arrive : le pair en devient le détenteur.
    Adopt {
        arrival: Box<Arrival>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PeerOutput {
    Done,
    Text(String),
    Memory(Result<MemoryOutput, String>),
    Sync(SyncResponse),
}

/// Tout ce que possède un agent qui change de nœud.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arrival {
    pub rows: Rows,
    /// Son workspace à l'arrivée.
    pub workdir: PathBuf,
    /// Sa mémoire, avec son historique ; absente quand elle est déjà là (passation).
    pub memory: Option<MemoryPackage>,
    /// Canaux de discussion de l'Assistant (D45) : section de config et jeton.
    #[serde(default)]
    pub channels: Vec<ChannelTransfer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelTransfer {
    pub id: String,
    /// La section `[channels.<id>]`, en TOML.
    pub config: String,
    /// Fichiers secrets qu'elle désigne : nom de fichier et contenu.
    pub secrets: Vec<(String, String)>,
}

/// Les pairs de ce nœud : ceux qu'il joint lui-même (déclarés dans `[peers]`) et ceux
/// qui l'ont joint.
#[derive(Clone)]
pub struct Peers {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Peers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peers")
            .field("name", &self.inner.name)
            .finish()
    }
}

struct Inner {
    name: String,
    /// Jeton qu'un pair doit présenter pour se relier à ce nœud.
    token: String,
    declared: BTreeMap<String, NodeConfig>,
    links: Mutex<HashMap<String, Link>>,
    errors: Mutex<HashMap<String, String>>,
    epochs: AtomicU64,
    calls: AtomicU64,
    pending: Mutex<HashMap<u64, Pending>>,
    trees: Trees,
    /// Réveille la file d'un pair : nouveau message, ou lien rétabli.
    flush: Mutex<HashMap<String, Arc<Notify>>>,
    supervisor: OnceLock<Supervisor>,
    /// Tâches d'écoute, de connexion et de lien : arrêtées par [`Peers::halt`].
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

#[derive(Clone)]
struct Link {
    epoch: u64,
    /// Le nœud qui a ouvert le lien : sert à n'en garder qu'un entre deux pairs.
    opener: String,
    outbox: mpsc::UnboundedSender<PeerMessage>,
    /// Fermé quand un autre lien remplace celui-ci.
    _replaced: Arc<oneshot::Sender<()>>,
}

struct Pending {
    peer: String,
    reply: oneshot::Sender<Result<PeerOutput, String>>,
}

impl Peers {
    pub fn new(
        name: impl Into<String>,
        token: impl Into<String>,
        declared: BTreeMap<String, NodeConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                name: name.into(),
                token: token.into(),
                declared,
                links: Mutex::default(),
                errors: Mutex::default(),
                epochs: AtomicU64::new(0),
                calls: AtomicU64::new(0),
                pending: Mutex::default(),
                trees: Trees::default(),
                flush: Mutex::default(),
                supervisor: OnceLock::new(),
                tasks: Mutex::default(),
            }),
        }
    }

    /// Ce nœud tout seul, sans pair (tests, installation d'un seul nœud).
    pub fn alone(name: impl Into<String>) -> Self {
        Self::new(name, String::new(), BTreeMap::new())
    }

    /// Nom de ce nœud dans le maillage.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Accepte les pairs sur `listener` et joint ceux qui sont déclarés.
    pub fn start(&self, supervisor: Supervisor, listener: Option<TcpListener>, ssh: PathBuf) {
        let _ = self.inner.supervisor.set(supervisor);
        if let Some(listener) = listener {
            let peers = self.clone();
            let accept = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let link = tokio::spawn(peers.clone().accepted(stream));
                    locked(&peers.inner.tasks).push(link.abort_handle());
                }
            });
            locked(&self.inner.tasks).push(accept.abort_handle());
        }
        for (name, config) in &self.inner.declared {
            let dial = tokio::spawn(self.clone().dial(name.clone(), config.clone(), ssh.clone()));
            locked(&self.inner.tasks).push(dial.abort_handle());
        }
    }

    /// Coupe ce nœud du maillage : écoute, connexions et liens s'arrêtent. Pour un arrêt,
    /// ou pour simuler un nœud éteint ; [`Peers::start`] le relie de nouveau.
    pub fn halt(&self) {
        for task in locked(&self.inner.tasks).drain(..) {
            task.abort();
        }
        locked(&self.inner.links).clear();
        locked(&self.inner.pending).clear();
        self.inner.trees.forget_all();
    }

    fn supervisor(&self) -> Option<&Supervisor> {
        self.inner.supervisor.get()
    }

    /// Un pair déclaré, relié, ou détenteur d'un agent connu de l'annuaire.
    pub fn is_known(&self, name: &str) -> bool {
        name == self.inner.name
            || self.inner.declared.contains_key(name)
            || locked(&self.inner.links).contains_key(name)
            || self.supervisor().is_some_and(|supervisor| {
                supervisor
                    .registry()
                    .tree()
                    .unwrap_or_default()
                    .iter()
                    .any(|agent| agent.node.as_deref() == Some(name))
            })
    }

    pub fn is_connected(&self, name: &str) -> bool {
        locked(&self.inner.links).contains_key(name)
    }

    /// Les pairs déclarés ou reliés, et s'ils sont joignables.
    pub fn status(&self) -> Vec<NodeStatus> {
        let links = locked(&self.inner.links);
        let errors = locked(&self.inner.errors);
        let mut names: BTreeSet<String> = self.inner.declared.keys().cloned().collect();
        names.extend(links.keys().cloned());
        if let Some(supervisor) = self.supervisor() {
            names.extend(
                supervisor
                    .registry()
                    .tree()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|agent| agent.node),
            );
        }
        names.remove(&self.inner.name);
        names
            .into_iter()
            .map(|name| NodeStatus {
                connected: links.contains_key(&name),
                error: (!links.contains_key(&name))
                    .then(|| errors.get(&name).cloned())
                    .flatten(),
                name,
            })
            .collect()
    }

    /// Appel à un pair, avec sa réponse.
    pub async fn call(
        &self,
        peer: &str,
        request: PeerCall,
        wait: Duration,
    ) -> Result<PeerOutput, String> {
        let outbox = locked(&self.inner.links)
            .get(peer)
            .map(|link| link.outbox.clone())
            .ok_or_else(|| self.unreachable(peer))?;
        let call = self.inner.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let (reply, answer) = oneshot::channel();
        locked(&self.inner.pending).insert(
            call,
            Pending {
                peer: peer.into(),
                reply,
            },
        );
        if outbox.send(PeerMessage::Call { call, request }).is_err() {
            locked(&self.inner.pending).remove(&call);
            return Err(self.unreachable(peer));
        }
        match tokio::time::timeout(wait, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(self.unreachable(peer)),
            Err(_) => {
                locked(&self.inner.pending).remove(&call);
                Err(format!(
                    "nœud {peer} : pas de réponse en {} s",
                    wait.as_secs()
                ))
            }
        }
    }

    fn unreachable(&self, peer: &str) -> String {
        match locked(&self.inner.errors).get(peer) {
            Some(error) => format!("nœud {peer} injoignable : {error}"),
            None => format!("nœud {peer} injoignable"),
        }
    }

    /// Un outil qui vise un agent détenu par `peer`, exécuté là-bas.
    pub async fn tool(
        &self,
        peer: &str,
        caller: &AgentId,
        name: &str,
        args: &Value,
    ) -> Result<String, String> {
        let request = PeerCall::Tool {
            caller: caller.clone(),
            name: name.into(),
            args: args.clone(),
        };
        match self.call(peer, request, Duration::from_secs(900)).await? {
            PeerOutput::Text(text) => Ok(text),
            other => Err(format!("nœud {peer} : réponse inattendue ({other:?})")),
        }
    }

    /// Opération sur la mémoire d'un agent détenu par `peer`.
    pub async fn memory(
        &self,
        peer: &str,
        agent: &AgentId,
        op: MemoryOp,
    ) -> Result<Result<MemoryOutput, String>, String> {
        let request = PeerCall::Memory {
            agent: agent.clone(),
            op,
        };
        match self.call(peer, request, Duration::from_secs(60)).await? {
            PeerOutput::Memory(result) => Ok(result),
            other => Err(format!("nœud {peer} : réponse inattendue ({other:?})")),
        }
    }

    /// Demande sur l'arbre d'un workspace détenu par `peer`.
    pub async fn sync(
        &self,
        peer: &str,
        sync: &str,
        request: SyncRequest,
    ) -> Result<SyncResponse, String> {
        let request = PeerCall::Sync {
            sync: sync.into(),
            request,
        };
        match self.call(peer, request, Duration::from_secs(300)).await? {
            PeerOutput::Sync(response) => Ok(response),
            other => Err(format!("nœud {peer} : réponse inattendue ({other:?})")),
        }
    }

    /// Confie un agent à `peer`, qui en devient le détenteur.
    pub async fn adopt(&self, peer: &str, arrival: Arrival) -> Result<String, String> {
        let request = PeerCall::Adopt {
            arrival: Box::new(arrival),
        };
        match self.call(peer, request, Duration::from_secs(300)).await? {
            PeerOutput::Text(text) => Ok(text),
            other => Err(format!("nœud {peer} : réponse inattendue ({other:?})")),
        }
    }

    /// Message pour un agent détenu par `peer` : mis en file, puis remis dès que le lien
    /// le permet, dans l'ordre.
    pub fn deliver(
        &self,
        peer: &str,
        to: &AgentId,
        from: Origin,
        text: String,
    ) -> Result<(), String> {
        let supervisor = self.supervisor().ok_or("nœud pas encore démarré")?;
        let message = serde_json::to_value(PeerCall::Deliver {
            to: to.clone(),
            from,
            text,
        })
        .map_err(|error| error.to_string())?;
        supervisor
            .registry()
            .enqueue(peer, &message)
            .map_err(|error| error.to_string())?;
        self.wake_queue(peer);
        Ok(())
    }

    fn wake_queue(&self, peer: &str) {
        let notify = {
            let mut flush = locked(&self.inner.flush);
            if let Some(notify) = flush.get(peer) {
                notify.notify_one();
                return;
            }
            let notify = Arc::new(Notify::new());
            flush.insert(peer.to_string(), Arc::clone(&notify));
            notify
        };
        tokio::spawn(self.clone().drain_queue(peer.to_string(), notify));
    }

    /// Vide la file d'un pair tant qu'il répond ; s'endort sinon, jusqu'au prochain réveil.
    async fn drain_queue(self, peer: String, notify: Arc<Notify>) {
        loop {
            let Some(supervisor) = self.supervisor().cloned() else {
                return;
            };
            let next = supervisor.registry().next_queued(&peer).ok().flatten();
            let Some((seq, message)) = next else {
                notify.notified().await;
                continue;
            };
            if !self.is_connected(&peer) {
                notify.notified().await;
                continue;
            }
            let request: PeerCall = match serde_json::from_value(message) {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("nœud {peer} : message en file illisible, écarté : {error}");
                    let _ = supervisor.registry().dequeue(seq);
                    continue;
                }
            };
            match self.call(&peer, request, Duration::from_secs(60)).await {
                Ok(_) => {
                    let _ = supervisor.registry().dequeue(seq);
                }
                Err(error) if !self.is_connected(&peer) => {
                    eprintln!("nœud {peer} : message gardé en file ({error})");
                }
                // Refusé par le pair (agent inconnu, retiré) : il ne passera jamais.
                Err(error) => {
                    eprintln!("nœud {peer} : message écarté : {error}");
                    let _ = supervisor.registry().dequeue(seq);
                }
            }
        }
    }

    /// Annonce une fiche d'agent détenu ici à tous les pairs reliés.
    pub fn announce(&self, records: Vec<AgentRecord>) {
        if records.is_empty() {
            return;
        }
        for link in locked(&self.inner.links).values() {
            let _ = link.outbox.send(PeerMessage::Directory {
                agents: records.clone(),
                full: false,
            });
        }
    }

    /// Joint un pair déclaré, et recommence quand le lien tombe.
    async fn dial(self, name: String, config: NodeConfig, ssh: PathBuf) {
        let mut wait = RETRY_FIRST;
        loop {
            // Un lien ouvert par le pair suffit : on attend qu'il tombe.
            if self.is_connected(&name) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            match self.dial_once(&name, &config, &ssh).await {
                Ok(()) => wait = RETRY_FIRST,
                Err(error) => {
                    let mut errors = locked(&self.inner.errors);
                    if errors.get(&name) != Some(&error) {
                        eprintln!("nœud {name} : {error}");
                    }
                    errors.insert(name.clone(), error);
                }
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(RETRY_MAX);
        }
    }

    async fn dial_once(
        &self,
        name: &str,
        config: &NodeConfig,
        ssh: &std::path::Path,
    ) -> Result<(), String> {
        let token = config.token().map_err(|error| format!("jeton : {error}"))?;
        // Le tunnel vit aussi longtemps que cette fonction.
        let opened = transport::open(config, ssh).await?;
        let url = format!("{}/?token={token}", opened.url.trim_end_matches('/'));
        let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|error| format!("connexion : {error}"))?;
        let result = self
            .serve(socket, Some(name.to_string()), self.inner.name.clone())
            .await;
        drop(opened);
        result
    }

    async fn accepted(self, stream: TcpStream) {
        let token = self.inner.token.clone();
        // Signature imposée par tungstenite, erreur volumineuse comprise.
        #[allow(clippy::result_large_err)]
        let authorize = |handshake: &Handshake, response: HandshakeResponse| {
            if !token.is_empty() && token_in(handshake.uri().query()) == Some(token.as_str()) {
                return Ok(response);
            }
            let mut refusal = ErrorResponse::new(Some("jeton absent ou invalide".into()));
            *refusal.status_mut() = StatusCode::UNAUTHORIZED;
            Err(refusal)
        };
        let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, authorize).await else {
            return;
        };
        if let Err(error) = self.serve(socket, None, String::new()).await {
            eprintln!("pair entrant refusé : {error}");
        }
    }

    /// Sert un lien, de la présentation à la perte. `expected` : le nom du pair joint ;
    /// `opener` : le nœud qui a ouvert le lien (vide : le pair).
    async fn serve<S>(
        &self,
        socket: tokio_tungstenite::WebSocketStream<S>,
        expected: Option<String>,
        opener: String,
    ) -> Result<(), String>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut source) = socket.split();
        let hello = PeerMessage::Hello {
            node: self.inner.name.clone(),
            protocol: PEER_PROTOCOL,
        };
        sink.send(Message::text(
            serde_json::to_string(&hello).expect("sérialisable"),
        ))
        .await
        .map_err(|error| format!("présentation : {error}"))?;
        let peer = match next_message(&mut source).await {
            Some(PeerMessage::Hello { node, protocol }) if protocol == PEER_PROTOCOL => node,
            Some(PeerMessage::Hello { protocol, .. }) => {
                return Err(format!(
                    "protocole du pair v{protocol}, attendu v{PEER_PROTOCOL}"
                ));
            }
            _ => return Err("pas de présentation du pair".into()),
        };
        if peer == self.inner.name {
            return Err(format!("le pair porte le même nom que ce nœud ({peer})"));
        }
        if let Some(expected) = &expected
            && *expected != peer
        {
            return Err(format!("le nœud se présente comme « {peer} »"));
        }
        let opener = if opener.is_empty() {
            peer.clone()
        } else {
            opener
        };

        let (outbox, mut queued) = mpsc::unbounded_channel::<PeerMessage>();
        let (replaced_tx, mut replaced) = oneshot::channel();
        let epoch = self.inner.epochs.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut links = locked(&self.inner.links);
            // Deux liens entre les mêmes pairs : on garde celui qu'a ouvert le plus petit nom.
            if let Some(existing) = links.get(&peer) {
                let keeper = self.inner.name.clone().min(peer.clone());
                if existing.opener == keeper && opener != keeper {
                    return Ok(());
                }
            }
            links.insert(
                peer.clone(),
                Link {
                    epoch,
                    opener,
                    outbox: outbox.clone(),
                    _replaced: Arc::new(replaced_tx),
                },
            );
        }
        locked(&self.inner.errors).remove(&peer);
        eprintln!("nœud {peer} connecté");
        let writer = tokio::spawn(async move {
            while let Some(message) = queued.recv().await {
                let text = serde_json::to_string(&message).expect("message sérialisable");
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
        });
        if let Some(supervisor) = self.supervisor().cloned() {
            let owned = supervisor.registry().owned().unwrap_or_default();
            let _ = outbox.send(PeerMessage::Directory {
                agents: owned,
                full: true,
            });
            supervisor.syncs().node_changed(&peer);
            tokio::spawn(calls::hand_over(self.clone(), supervisor, peer.clone()));
        }
        self.wake_queue(&peer);

        loop {
            let message = tokio::select! {
                message = next_message(&mut source) => message,
                _ = &mut replaced => break,
            };
            let Some(message) = message else { break };
            self.handle(&peer, message, &outbox);
        }

        writer.abort();
        {
            let mut links = locked(&self.inner.links);
            if links.get(&peer).is_some_and(|link| link.epoch == epoch) {
                links.remove(&peer);
                // Les watchers des synchros du pair s'arrêtent ; il les rouvrira.
                self.inner.trees.forget(&peer);
                eprintln!("nœud {peer} déconnecté");
                // Ces réponses ne viendront plus : les appels échouent tout de suite.
                locked(&self.inner.pending).retain(|_, pending| pending.peer != peer);
            }
        }
        if let Some(supervisor) = self.supervisor() {
            supervisor.syncs().node_changed(&peer);
        }
        Ok(())
    }

    fn handle(
        &self,
        peer: &str,
        message: PeerMessage,
        outbox: &mpsc::UnboundedSender<PeerMessage>,
    ) {
        let Some(supervisor) = self.supervisor().cloned() else {
            return;
        };
        match message {
            PeerMessage::Hello { .. } => {}
            PeerMessage::Directory { agents, .. } => {
                for record in agents {
                    let accepted = supervisor.registry().upsert_replica(&record, peer);
                    match accepted {
                        Ok(true) => supervisor.publish_status(&record.id),
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("nœud {peer} : fiche {} ignorée : {error}", record.id)
                        }
                    }
                }
            }
            PeerMessage::Call { call, request } => {
                let peers = self.clone();
                let outbox = outbox.clone();
                let peer = peer.to_string();
                tokio::spawn(async move {
                    let result = calls::answer(&peers, &supervisor, &peer, request, &outbox).await;
                    let _ = outbox.send(PeerMessage::Reply { call, result });
                });
            }
            PeerMessage::Reply { call, result } => {
                if let Some(pending) = locked(&self.inner.pending).remove(&call) {
                    let _ = pending.reply.send(result);
                }
            }
            PeerMessage::SyncChanged { sync, delta } => {
                supervisor.syncs().remote_changed(peer, &sync, delta);
            }
        }
    }
}

async fn next_message<S>(source: &mut S) -> Option<PeerMessage>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match source.next().await? {
            Ok(Message::Text(text)) => match serde_json::from_str(text.as_str()) {
                Ok(message) => return Some(message),
                Err(error) => eprintln!("message de pair illisible : {error}"),
            },
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
