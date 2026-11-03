//! Ce qu'un nœud fait des appels de ses pairs, et la passation des fiches d'un ancien hub.

use super::{Arrival, PeerCall, PeerMessage, PeerOutput, Peers, locked};
use crate::supervisor::Supervisor;
use crate::sync::tree::{self, Tree};
use crate::sync::{SyncRequest, SyncResponse};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Répond à un appel de `peer`.
pub(super) async fn answer(
    peers: &Peers,
    supervisor: &Supervisor,
    peer: &str,
    request: PeerCall,
    outbox: &mpsc::UnboundedSender<PeerMessage>,
) -> Result<PeerOutput, String> {
    match request {
        PeerCall::Tool { caller, name, args } => {
            let caller = supervisor
                .record(&caller)
                .map_err(|error| format!("appelant inconnu ici : {error}"))?;
            // Exécuté ici sans renvoi : deux nœuds en désaccord ne se renvoient pas l'appel.
            crate::tools::call_here(supervisor, &caller, &name, &args)
                .await
                .map(PeerOutput::Text)
        }
        PeerCall::Deliver { to, from, text } => {
            supervisor
                .deliver(&to, from, text)
                .await
                .map_err(|error| error.to_string())?;
            Ok(PeerOutput::Done)
        }
        PeerCall::Memory { agent, op } => {
            let record = supervisor
                .record(&agent)
                .map_err(|error| error.to_string())?;
            if record.node.is_some() {
                return Err(format!("{agent} n'est pas détenu par {}", peers.name()));
            }
            Ok(PeerOutput::Memory(supervisor.memory_op(&record, op).await))
        }
        PeerCall::Sync { sync, request } => {
            let root = supervisor.home().to_path_buf();
            peers
                .inner
                .trees
                .request(peer, &sync, request, &root, outbox)
                .await
                .map(PeerOutput::Sync)
        }
        PeerCall::Adopt { arrival } => supervisor.adopt(*arrival, peer).await.map(PeerOutput::Text),
    }
}

/// Passation (D48, T5) : les fiches qu'un ancien hub tenait pour des agents placés sur
/// `peer` lui sont confiées, avec tout ce qu'elles possèdent. Mémoire et workspace y
/// sont déjà.
pub(super) async fn hand_over(peers: Peers, supervisor: Supervisor, peer: String) {
    let ids = supervisor.registry().handovers(&peer).unwrap_or_default();
    for id in ids {
        let rows = supervisor.registry().export_rows(&id);
        let Ok(rows) = rows else { continue };
        let workdir = rows.agents[0].workdir.clone();
        let arrival = Arrival {
            rows,
            workdir,
            memory: None,
            channels: vec![],
        };
        match peers.adopt(&peer, arrival).await {
            Ok(_) => {
                let released = {
                    let registry = supervisor.registry();
                    registry
                        .release(&id, &peer)
                        .and_then(|()| registry.handed_over(&id))
                };
                match released {
                    Ok(()) => eprintln!("nœud {peer} : {id} lui est confié"),
                    Err(error) => eprintln!("nœud {peer} : passation de {id} : {error}"),
                }
            }
            Err(error) => {
                eprintln!("nœud {peer} : passation de {id} remise : {error}");
                return;
            }
        }
    }
}

/// Arbres de workspace ouverts par les pairs, par pair et par nom.
#[derive(Default)]
pub(super) struct Trees {
    open: Mutex<HashMap<(String, String), Served>>,
}

struct Served {
    tree: Arc<Mutex<Tree>>,
    _watcher: Option<notify::RecommendedWatcher>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl Trees {
    /// Oublie les arbres d'un pair parti ; il les rouvrira.
    pub(super) fn forget(&self, peer: &str) {
        locked(&self.open).retain(|(owner, _), _| owner != peer);
    }

    pub(super) fn forget_all(&self) {
        locked(&self.open).clear();
    }

    async fn request(
        &self,
        peer: &str,
        sync: &str,
        request: SyncRequest,
        home: &std::path::Path,
        outbox: &mpsc::UnboundedSender<PeerMessage>,
    ) -> Result<SyncResponse, String> {
        let key = (peer.to_string(), sync.to_string());
        match request {
            SyncRequest::Open {
                workdir,
                excludes,
                known,
                copy,
            } => {
                let root = home.join(&workdir);
                if copy {
                    std::fs::create_dir_all(&root)
                        .map_err(|error| format!("{} : {error}", root.display()))?;
                } else if !root.is_dir() {
                    return Err(format!("workspace introuvable : {}", root.display()));
                }
                locked(&self.open).remove(&key);
                let (tree, watcher, calm) = if copy {
                    (Tree::whole(root), None, None)
                } else {
                    // Le watcher d'abord : rien de ce qui change pendant le parcours n'est perdu.
                    let (watcher, calm) =
                        tree::watch(&root).map_err(|error| format!("watcher : {error}"))?;
                    (Tree::new(root, excludes, known), Some(watcher), Some(calm))
                };
                let tree = Arc::new(Mutex::new(tree));
                let scanning = Arc::clone(&tree);
                let index = tokio::task::spawn_blocking(move || locked(&scanning).scan())
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                let task = calm.map(|mut calm| {
                    let watched = Arc::clone(&tree);
                    let outbox = outbox.clone();
                    let sync = sync.to_string();
                    let mut sent = index.clone();
                    tokio::spawn(async move {
                        while calm.recv().await.is_some() {
                            let scanning = Arc::clone(&watched);
                            let Ok(Ok(index)) =
                                tokio::task::spawn_blocking(move || locked(&scanning).scan()).await
                            else {
                                continue;
                            };
                            let delta = crate::sync::diff(&sent, &index);
                            if !delta.is_empty() {
                                let _ = outbox.send(PeerMessage::SyncChanged {
                                    sync: sync.clone(),
                                    delta,
                                });
                            }
                            sent = index;
                        }
                    })
                });
                locked(&self.open).insert(
                    key,
                    Served {
                        tree,
                        _watcher: watcher,
                        task,
                    },
                );
                Ok(SyncResponse::Index { entries: index })
            }
            SyncRequest::Read { chunks } => {
                self.with_tree(&key, move |tree| {
                    Ok(SyncResponse::Data {
                        chunks: tree.read(&chunks),
                    })
                })
                .await
            }
            SyncRequest::Write { ops } => {
                self.with_tree(&key, move |tree| {
                    Ok(SyncResponse::Written {
                        results: tree.write(ops),
                    })
                })
                .await
            }
            SyncRequest::Close => {
                locked(&self.open).remove(&key);
                Ok(SyncResponse::Closed)
            }
        }
    }

    async fn with_tree(
        &self,
        key: &(String, String),
        operation: impl FnOnce(&mut Tree) -> Result<SyncResponse, String> + Send + 'static,
    ) -> Result<SyncResponse, String> {
        let tree = locked(&self.open)
            .get(key)
            .map(|served| Arc::clone(&served.tree))
            .ok_or_else(|| format!("arbre {} non ouvert ici", key.1))?;
        tokio::task::spawn_blocking(move || operation(&mut locked(&tree)))
            .await
            .map_err(|error| error.to_string())?
    }
}
