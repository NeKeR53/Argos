//! Côté hub : une tâche par synchro. Elle regarde la copie locale, reçoit les
//! changements du nœud, rapproche les deux arbres de l'état commun (`plan`) et exécute
//! les copies, suppressions et mises de côté. L'état commun est écrit après chaque passe,
//! pour qu'un redémarrage du hub ne confonde pas une suppression et un ajout.
//!
//! Garde-fou : une passe qui supprimerait d'un coup une grande part des fichiers suivis
//! (copie locale vidée, workspace remplacé) est bloquée et signalée, jamais exécutée.

use super::plan::{Action, reconcile};
use super::tree::{self, Tree};
use super::{
    BATCH, CHUNK, Delta, Entry, Finish, Index, ReadChunk, SyncRequest, SyncResponse, WriteOp,
};
use crate::registry::SyncRecord;
use crate::supervisor::Supervisor;
use argos_core::protocol::{DaemonEvent, SyncState, SyncStatus};
use argos_core::{AgentId, AgentState, Level};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Nouvel essai après une passe en échec ou une ouverture refusée par le nœud.
const RETRY: Duration = Duration::from_secs(5);
/// Une passe ne supprime pas plus que cela d'un coup sans intervention humaine.
const MASS_DELETE_MIN: usize = 20;

#[derive(Clone)]
pub struct Syncs {
    inner: Arc<Inner>,
}

struct Inner {
    /// `$ARGOS_HOME/sync` : un dossier par synchro, avec son état commun.
    dir: PathBuf,
    running: Mutex<HashMap<String, Handle>>,
}

struct Handle {
    node: String,
    signals: mpsc::UnboundedSender<Signal>,
    status: Arc<Mutex<SyncStatus>>,
}

enum Signal {
    /// Changements vus par le watcher du nœud.
    Remote(Delta),
    /// Le nœud vient de se connecter ou de se perdre.
    Node,
    Close(oneshot::Sender<()>),
}

impl Syncs {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                dir,
                running: Mutex::default(),
            }),
        }
    }

    /// Relance les synchros déclarées, au démarrage du hub.
    pub fn start(&self, supervisor: &Supervisor) {
        let records = supervisor.registry().syncs().unwrap_or_default();
        for record in records {
            if let Ok(agent) = supervisor.record(&record.agent)
                && let Some(node) = agent.node
            {
                self.spawn(supervisor, record, node);
            }
        }
    }

    pub fn list(&self) -> Vec<SyncStatus> {
        let mut all: Vec<SyncStatus> = locked(&self.inner.running)
            .values()
            .map(|handle| locked(&handle.status).clone())
            .collect();
        all.sort_by(|a, b| a.local.cmp(&b.local));
        all
    }

    /// Ouvre une synchro vers un dossier du hub vide ou absent : la première passe y copie
    /// le workspace du nœud, sans fusionner deux arbres étrangers.
    pub fn open(
        &self,
        supervisor: &Supervisor,
        agent: &AgentId,
        local: &Path,
        excludes: &[String],
    ) -> Result<SyncStatus, String> {
        let record = supervisor
            .record(agent)
            .map_err(|error| error.to_string())?;
        if record.level != Level::Agent || record.state == AgentState::Retired {
            return Err("seul le workspace d'un Agent vivant se synchronise".into());
        }
        let Some(node) = record.node.clone() else {
            return Err("cet Agent tourne sur le hub : son workspace est déjà ici".into());
        };
        if !supervisor.nodes().is_known(&node) {
            return Err(format!("nœud inconnu : {node}"));
        }
        if !local.is_absolute() {
            return Err("la copie locale doit être un chemin absolu".into());
        }
        let existing = supervisor
            .registry()
            .syncs()
            .map_err(|error| error.to_string())?;
        if let Some(other) = existing
            .iter()
            .find(|other| other.local.starts_with(local) || local.starts_with(&other.local))
        {
            return Err(format!(
                "{} est déjà synchronisé ({})",
                other.local.display(),
                other.id
            ));
        }
        match std::fs::read_dir(local) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(format!(
                        "{} n'est pas vide : choisis un dossier vide ou absent",
                        local.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(local)
                    .map_err(|error| format!("{} : {error}", local.display()))?;
            }
            Err(error) => return Err(format!("{} : {error}", local.display())),
        }
        let excludes: Vec<String> = excludes
            .iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        let sync = supervisor
            .registry()
            .add_sync(agent, local, &excludes)
            .map_err(|error| error.to_string())?;
        supervisor.memory_log(
            agent,
            format!(
                "synchro {} ouverte : workspace copié sur le hub dans {}",
                sync.id,
                local.display()
            ),
        );
        Ok(self.spawn(supervisor, sync, node))
    }

    /// Ferme une synchro. Les deux copies restent en place ; seul l'état commun est oublié.
    pub async fn close(&self, supervisor: &Supervisor, id: &str) -> Result<SyncStatus, String> {
        let record = supervisor
            .registry()
            .sync(id)
            .map_err(|error| error.to_string())?;
        let handle = locked(&self.inner.running).remove(id);
        let status = match handle {
            Some(handle) => {
                let (done, closed) = oneshot::channel();
                if handle.signals.send(Signal::Close(done)).is_ok() {
                    let _ = tokio::time::timeout(Duration::from_secs(30), closed).await;
                }
                locked(&handle.status).clone()
            }
            None => initial_status(&record, ""),
        };
        supervisor
            .registry()
            .remove_sync(id)
            .map_err(|error| error.to_string())?;
        let _ = std::fs::remove_dir_all(self.inner.dir.join(id));
        supervisor.memory_log(&record.agent, format!("synchro {id} fermée"));
        supervisor.publish_event(DaemonEvent::SyncUpdated {
            sync: status.clone(),
            closed: true,
        });
        Ok(status)
    }

    /// Un nœud s'est connecté ou perdu : ses synchros rouvrent leur arbre distant.
    pub fn node_changed(&self, node: &str) {
        for handle in locked(&self.inner.running).values() {
            if handle.node == node {
                let _ = handle.signals.send(Signal::Node);
            }
        }
    }

    pub fn remote_changed(&self, node: &str, sync: &str, delta: Delta) {
        if let Some(handle) = locked(&self.inner.running).get(sync)
            && handle.node == node
        {
            let _ = handle.signals.send(Signal::Remote(delta));
        }
    }

    fn spawn(&self, supervisor: &Supervisor, record: SyncRecord, node: String) -> SyncStatus {
        let status = Arc::new(Mutex::new(initial_status(&record, &node)));
        let (signals, received) = mpsc::unbounded_channel();
        let base_file = self.inner.dir.join(&record.id).join("base.json");
        let base = load_base(&base_file);
        let engine = Engine {
            supervisor: supervisor.clone(),
            tree: Arc::new(Mutex::new(Tree::new(
                record.local.clone(),
                record.excludes.clone(),
                base.keys().cloned(),
            ))),
            record: record.clone(),
            node: node.clone(),
            base,
            base_file,
            remote: None,
            status: Arc::clone(&status),
        };
        let snapshot = locked(&status).clone();
        locked(&self.inner.running).insert(
            record.id,
            Handle {
                node,
                signals,
                status,
            },
        );
        tokio::spawn(engine.run(received));
        supervisor.publish_event(DaemonEvent::SyncUpdated {
            sync: snapshot.clone(),
            closed: false,
        });
        snapshot
    }
}

struct Engine {
    supervisor: Supervisor,
    record: SyncRecord,
    node: String,
    tree: Arc<Mutex<Tree>>,
    base: Index,
    base_file: PathBuf,
    /// Index du nœud, tenu à jour par ses changements ; `None` tant qu'il n'est pas ouvert.
    remote: Option<Index>,
    status: Arc<Mutex<SyncStatus>>,
}

/// Bilan d'une passe.
#[derive(Default)]
struct Outcome {
    failures: Vec<String>,
    /// (chemin, version du hub mise de côté)
    conflicts: Vec<(String, String)>,
}

impl Engine {
    async fn run(mut self, mut signals: mpsc::UnboundedReceiver<Signal>) {
        let (_watcher, mut local) = match tree::watch(&self.record.local) {
            Ok(watch) => watch,
            Err(error) => {
                self.update(|status| {
                    status.state = SyncState::Error;
                    status.error = Some(format!(
                        "copie locale {} : {error}",
                        self.record.local.display()
                    ));
                });
                // Rien à faire sans copie locale : on attend seulement la fermeture.
                while let Some(signal) = signals.recv().await {
                    if let Signal::Close(done) = signal {
                        let _ = done.send(());
                        return;
                    }
                }
                return;
            }
        };
        let mut dirty = true;
        let mut retry: Option<tokio::time::Instant> = None;
        loop {
            if self.remote.is_none()
                && self.supervisor.nodes().is_connected(&self.node)
                && let Err(error) = self.open_remote().await
            {
                self.update(|status| {
                    status.state = SyncState::Error;
                    status.error = Some(error);
                });
                retry = Some(tokio::time::Instant::now() + RETRY);
            }
            if self.remote.is_none() {
                if !self.supervisor.nodes().is_connected(&self.node) {
                    self.update(|status| {
                        status.state = SyncState::Offline;
                        status.error = None;
                    });
                }
            } else if dirty {
                dirty = false;
                match self.round().await {
                    Ok(outcome) => {
                        let failed = !outcome.failures.is_empty();
                        let conflicts = !outcome.conflicts.is_empty();
                        self.report(outcome);
                        if failed {
                            retry = Some(tokio::time::Instant::now() + RETRY);
                        }
                        // Les versions mises de côté partent au nœud à la passe suivante.
                        dirty = conflicts;
                    }
                    Err(error) => {
                        self.update(|status| {
                            status.state = SyncState::Error;
                            status.error = Some(error);
                        });
                        retry = Some(tokio::time::Instant::now() + RETRY);
                    }
                }
                if dirty {
                    continue;
                }
            }
            let wake =
                retry.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
            tokio::select! {
                signal = signals.recv() => match signal {
                    None => return,
                    Some(Signal::Remote(delta)) => {
                        if let Some(remote) = &mut self.remote {
                            apply_delta(remote, delta);
                            dirty = true;
                        }
                    }
                    Some(Signal::Node) => {
                        self.remote = None;
                        dirty = true;
                    }
                    Some(Signal::Close(done)) => {
                        if self.remote.is_some() {
                            let _ = self.request(SyncRequest::Close).await;
                        }
                        let _ = done.send(());
                        return;
                    }
                },
                Some(()) = local.recv() => dirty = true,
                _ = tokio::time::sleep_until(wake), if retry.is_some() => {
                    retry = None;
                    dirty = true;
                }
            }
        }
    }

    async fn open_remote(&mut self) -> Result<(), String> {
        let agent = self
            .supervisor
            .record(&self.record.agent)
            .map_err(|error| error.to_string())?;
        if agent.state == AgentState::Retired {
            return Err("l'Agent est retiré : ferme cette synchro".into());
        }
        self.update(|status| status.state = SyncState::Syncing);
        let response = self
            .request(SyncRequest::Open {
                copy: false,
                workdir: agent.workdir,
                excludes: self.record.excludes.clone(),
                known: self.base.keys().cloned().collect(),
            })
            .await?;
        let SyncResponse::Index { entries } = response else {
            return Err("réponse inattendue du nœud".into());
        };
        self.remote = Some(entries);
        Ok(())
    }

    /// Une passe : parcours local, rapprochement, exécution, état commun écrit.
    async fn round(&mut self) -> Result<Outcome, String> {
        let tree = Arc::clone(&self.tree);
        let hub = tokio::task::spawn_blocking(move || locked(&tree).scan())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("copie locale : {error}"))?;
        let remote = self.remote.clone().unwrap_or_default();
        let actions = reconcile(&self.base, &hub, &remote);
        guard_mass_deletion(&self.base, &actions)?;
        if actions
            .iter()
            .any(|action| !matches!(action, Action::Agree { .. }))
        {
            self.update(|status| status.state = SyncState::Syncing);
        }
        let mut outcome = Outcome::default();
        let mut pulls = Vec::new();
        let mut pushes = Vec::new();
        let mut node_deletes = Vec::new();
        for action in actions {
            match action {
                Action::Agree { path, entry } => match entry {
                    Some(entry) => {
                        self.base.insert(path, entry);
                    }
                    None => {
                        self.base.remove(&path);
                    }
                },
                Action::Pull {
                    path,
                    entry,
                    expect,
                } => pulls.push((path, entry, expect)),
                Action::Push {
                    path,
                    entry,
                    expect,
                } => pushes.push((path, entry, expect)),
                Action::DeleteHub { path, expect } => {
                    let op = WriteOp::Delete {
                        path: path.clone(),
                        expect: Some(expect),
                    };
                    match self.local_write(vec![op]).await.remove(0) {
                        None => {
                            self.base.remove(&path);
                        }
                        Some(error) => outcome.failures.push(error),
                    }
                }
                Action::DeleteNode { path, expect } => node_deletes.push(WriteOp::Delete {
                    path,
                    expect: Some(expect),
                }),
                Action::Conflict { path, hub, node } => {
                    let tree = Arc::clone(&self.tree);
                    let aside_of = path.clone();
                    let aside = tokio::task::spawn_blocking(move || {
                        locked(&tree).set_aside(&aside_of, Some(&hub.hash))
                    })
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));
                    match aside {
                        Ok(aside) => {
                            outcome.conflicts.push((path.clone(), aside));
                            pulls.push((path, node, None));
                        }
                        Err(error) => outcome.failures.push(error),
                    }
                }
            }
        }
        if !node_deletes.is_empty() {
            let paths: Vec<String> = node_deletes
                .iter()
                .map(|op| op.path().to_string())
                .collect();
            let results = self.remote_write(node_deletes).await?;
            for (path, result) in paths.into_iter().zip(results) {
                match result {
                    None => {
                        self.base.remove(&path);
                        if let Some(remote) = &mut self.remote {
                            remote.remove(&path);
                        }
                    }
                    Some(error) => outcome.failures.push(error),
                }
            }
        }
        self.pull(pulls, &mut outcome).await?;
        self.push(pushes, &mut outcome).await?;
        save_base(&self.base_file, &self.base);
        Ok(outcome)
    }

    /// Du nœud vers le hub, par lots de morceaux.
    async fn pull(
        &mut self,
        files: Vec<(String, Entry, Option<String>)>,
        outcome: &mut Outcome,
    ) -> Result<(), String> {
        let mut links = Vec::new();
        let mut chunks = Vec::new();
        let mut entries = HashMap::new();
        for (path, entry, expect) in files {
            if let Some(target) = &entry.link {
                links.push(WriteOp::Link {
                    path: path.clone(),
                    target: target.clone(),
                    expect,
                });
            } else {
                chunks.extend(chunks_of(&path, &entry, expect.clone()));
            }
            entries.insert(path, entry);
        }
        if !links.is_empty() {
            let paths: Vec<String> = links.iter().map(|op| op.path().to_string()).collect();
            let results = self.local_write(links).await;
            for (path, result) in paths.into_iter().zip(results) {
                self.settle(&path, &entries, result, false, outcome);
            }
        }
        let mut failed: HashSet<String> = HashSet::new();
        for batch in batches(chunks) {
            let (reads, finishes): (Vec<ReadChunk>, Vec<Option<Finish>>) = batch
                .into_iter()
                .filter(|(read, _)| !failed.contains(&read.path))
                .unzip();
            if reads.is_empty() {
                continue;
            }
            let SyncResponse::Data { chunks: data } =
                self.request(SyncRequest::Read { chunks: reads }).await?
            else {
                return Err("réponse inattendue du nœud".into());
            };
            let mut ops = Vec::new();
            for (chunk, finish) in data.into_iter().zip(finishes) {
                if failed.contains(&chunk.path) {
                    continue;
                }
                if let Some(error) = chunk.error {
                    failed.insert(chunk.path.clone());
                    outcome.failures.push(error);
                    continue;
                }
                ops.push(WriteOp::Put {
                    path: chunk.path,
                    offset: chunk.offset,
                    data: chunk.data,
                    finish,
                });
            }
            let finishing: Vec<(String, bool)> = ops
                .iter()
                .map(|op| match op {
                    WriteOp::Put { path, finish, .. } => (path.clone(), finish.is_some()),
                    _ => unreachable!(),
                })
                .collect();
            let results = self.local_write(ops).await;
            for ((path, last), result) in finishing.into_iter().zip(results) {
                if result.is_some() {
                    failed.insert(path.clone());
                }
                if last || result.is_some() {
                    self.settle(&path, &entries, result, false, outcome);
                }
            }
        }
        Ok(())
    }

    /// Du hub vers le nœud, par lots de morceaux.
    async fn push(
        &mut self,
        files: Vec<(String, Entry, Option<String>)>,
        outcome: &mut Outcome,
    ) -> Result<(), String> {
        let mut links = Vec::new();
        let mut chunks = Vec::new();
        let mut entries = HashMap::new();
        for (path, entry, expect) in files {
            if let Some(target) = &entry.link {
                links.push(WriteOp::Link {
                    path: path.clone(),
                    target: target.clone(),
                    expect,
                });
            } else {
                chunks.extend(chunks_of(&path, &entry, expect.clone()));
            }
            entries.insert(path, entry);
        }
        if !links.is_empty() {
            let paths: Vec<String> = links.iter().map(|op| op.path().to_string()).collect();
            let results = self.remote_write(links).await?;
            for (path, result) in paths.into_iter().zip(results) {
                self.settle(&path, &entries, result, true, outcome);
            }
        }
        let mut failed: HashSet<String> = HashSet::new();
        for batch in batches(chunks) {
            let (reads, finishes): (Vec<ReadChunk>, Vec<Option<Finish>>) = batch
                .into_iter()
                .filter(|(read, _)| !failed.contains(&read.path))
                .unzip();
            if reads.is_empty() {
                continue;
            }
            let tree = Arc::clone(&self.tree);
            let data = tokio::task::spawn_blocking(move || locked(&tree).read(&reads))
                .await
                .map_err(|error| error.to_string())?;
            let mut ops = Vec::new();
            for (chunk, finish) in data.into_iter().zip(finishes) {
                if failed.contains(&chunk.path) {
                    continue;
                }
                if let Some(error) = chunk.error {
                    failed.insert(chunk.path.clone());
                    outcome.failures.push(error);
                    continue;
                }
                ops.push(WriteOp::Put {
                    path: chunk.path,
                    offset: chunk.offset,
                    data: chunk.data,
                    finish,
                });
            }
            if ops.is_empty() {
                continue;
            }
            let finishing: Vec<(String, bool)> = ops
                .iter()
                .map(|op| match op {
                    WriteOp::Put { path, finish, .. } => (path.clone(), finish.is_some()),
                    _ => unreachable!(),
                })
                .collect();
            let results = self.remote_write(ops).await?;
            for ((path, last), result) in finishing.into_iter().zip(results) {
                if result.is_some() {
                    failed.insert(path.clone());
                }
                if last || result.is_some() {
                    self.settle(&path, &entries, result, true, outcome);
                }
            }
        }
        Ok(())
    }

    /// Une copie finie : l'état commun (et l'index du nœud, après un envoi) la retient.
    fn settle(
        &mut self,
        path: &str,
        entries: &HashMap<String, Entry>,
        result: Option<String>,
        pushed: bool,
        outcome: &mut Outcome,
    ) {
        match result {
            Some(error) => outcome.failures.push(error),
            None => {
                let entry = entries[path].clone();
                if pushed && let Some(remote) = &mut self.remote {
                    remote.insert(path.to_string(), entry.clone());
                }
                self.base.insert(path.to_string(), entry);
            }
        }
    }

    async fn local_write(&self, ops: Vec<WriteOp>) -> Vec<Option<String>> {
        let count = ops.len();
        let tree = Arc::clone(&self.tree);
        tokio::task::spawn_blocking(move || locked(&tree).write(ops))
            .await
            .unwrap_or_else(|error| vec![Some(error.to_string()); count])
    }

    async fn remote_write(&self, ops: Vec<WriteOp>) -> Result<Vec<Option<String>>, String> {
        match self.request(SyncRequest::Write { ops }).await? {
            SyncResponse::Written { results } => Ok(results),
            _ => Err("réponse inattendue du nœud".into()),
        }
    }

    async fn request(&self, request: SyncRequest) -> Result<SyncResponse, String> {
        self.supervisor
            .nodes()
            .sync(&self.node, &self.record.id, request)
            .await
    }

    /// Fin d'une passe : état, conflits signalés à l'utilisateur.
    fn report(&self, outcome: Outcome) {
        let mut held = locked(&self.tree).too_big();
        held.extend(
            self.remote
                .iter()
                .flat_map(|remote| remote.iter())
                .filter(|(_, entry)| entry.too_big)
                .map(|(path, _)| path.clone()),
        );
        held.sort();
        held.dedup();
        let files = self.base.len();
        let failures = outcome.failures;
        self.update(|status| {
            status.files = files;
            status.held = held;
            if failures.is_empty() {
                status.state = SyncState::Idle;
                status.error = None;
                status.synced_at = Some(now());
            } else {
                status.state = SyncState::Error;
                status.error = Some(summarize(&failures));
            }
            for (_, aside) in &outcome.conflicts {
                if !status.conflicts.contains(aside) {
                    status.conflicts.push(aside.clone());
                }
            }
        });
        if outcome.conflicts.is_empty() {
            return;
        }
        let listed: Vec<String> = outcome
            .conflicts
            .iter()
            .map(|(path, aside)| format!("{path} (version du hub : {aside})"))
            .collect();
        let question = format!(
            "Synchro de ton workspace avec {} : {} fichier(s) modifié(s) des deux côtés — {}. \
             La version du nœud a gardé le chemin ; celle du hub est conservée à côté, des deux côtés.",
            self.record.local.display(),
            listed.len(),
            listed.join(", ")
        );
        let options = [
            "Fusionne les deux versions".to_string(),
            "Garde la version du nœud".to_string(),
        ];
        if self
            .supervisor
            .open_escalation(&self.record.agent, &question, &options)
            .is_err()
        {
            self.supervisor.publish_event(DaemonEvent::Notice {
                agent: Some(self.record.agent.clone()),
                text: question,
            });
        }
    }

    fn update(&self, change: impl FnOnce(&mut SyncStatus)) {
        let snapshot = {
            let mut status = locked(&self.status);
            let before = status.clone();
            change(&mut status);
            if *status == before {
                return;
            }
            status.clone()
        };
        self.supervisor.publish_event(DaemonEvent::SyncUpdated {
            sync: snapshot,
            closed: false,
        });
    }
}

/// Refuse une passe qui supprimerait d'un coup une grande part des fichiers suivis.
fn guard_mass_deletion(base: &Index, actions: &[Action]) -> Result<(), String> {
    let limit = MASS_DELETE_MIN.max(base.len() / 2);
    let count = |hub: bool| {
        actions
            .iter()
            .filter(|action| {
                matches!(
                    (action, hub),
                    (Action::DeleteHub { .. }, true) | (Action::DeleteNode { .. }, false)
                )
            })
            .count()
    };
    for (hub, side) in [(true, "la copie locale"), (false, "le workspace du nœud")] {
        let deletions = count(hub);
        if deletions > limit {
            return Err(format!(
                "synchro suspendue : elle supprimerait {deletions} fichiers dans {side}. \
                 Si c'est voulu, ferme la synchro et rouvre-la vers un dossier vide."
            ));
        }
    }
    Ok(())
}

/// Les morceaux d'un fichier, le dernier portant de quoi le vérifier et le mettre en place.
fn chunks_of(
    path: &str,
    entry: &Entry,
    expect: Option<String>,
) -> Vec<(ReadChunk, Option<Finish>)> {
    let mut offsets: Vec<u64> = (0..entry.size).step_by(CHUNK as usize).collect();
    if offsets.is_empty() {
        offsets.push(0);
    }
    let last = offsets.len() - 1;
    offsets
        .into_iter()
        .enumerate()
        .map(|(i, offset)| {
            let read = ReadChunk {
                path: path.into(),
                offset,
                len: CHUNK.min(entry.size - offset),
            };
            let finish = (i == last).then(|| Finish {
                hash: entry.hash.clone(),
                exec: entry.exec,
                expect: expect.clone(),
            });
            (read, finish)
        })
        .collect()
}

/// Regroupe les morceaux par aller-retour, sans dépasser `BATCH` octets.
fn batches(chunks: Vec<(ReadChunk, Option<Finish>)>) -> Vec<Vec<(ReadChunk, Option<Finish>)>> {
    let mut all = Vec::new();
    let mut current = Vec::new();
    let mut size = 0;
    for chunk in chunks {
        if !current.is_empty() && size + chunk.0.len > BATCH {
            all.push(std::mem::take(&mut current));
            size = 0;
        }
        size += chunk.0.len;
        current.push(chunk);
    }
    if !current.is_empty() {
        all.push(current);
    }
    all
}

fn apply_delta(index: &mut Index, delta: Delta) {
    for (path, entry) in delta {
        match entry {
            Some(entry) => {
                index.insert(path, entry);
            }
            None => {
                index.remove(&path);
            }
        }
    }
}

fn summarize(failures: &[String]) -> String {
    match failures {
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} (et {} autre(s))", rest.len()),
        [] => String::new(),
    }
}

fn initial_status(record: &SyncRecord, node: &str) -> SyncStatus {
    SyncStatus {
        id: record.id.clone(),
        agent: record.agent.clone(),
        node: node.into(),
        local: record.local.to_string_lossy().into_owned(),
        state: SyncState::Offline,
        error: None,
        files: 0,
        conflicts: Vec::new(),
        held: Vec::new(),
        synced_at: None,
    }
}

fn load_base(path: &Path) -> Index {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Écrit l'état commun d'un bloc : un fichier à moitié écrit ne doit jamais le remplacer.
fn save_base(path: &Path, base: &Index) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().expect("dans le dossier de la synchro"))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec(base).expect("index sérialisable"))?;
        std::fs::rename(temp, path)
    };
    if let Err(error) = write() {
        eprintln!("synchro : état commun {} : {error}", path.display());
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("verrou empoisonné")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(size: u64) -> Entry {
        Entry {
            hash: "h".into(),
            size,
            mtime: 0,
            exec: false,
            link: None,
            too_big: false,
        }
    }

    #[test]
    fn files_are_cut_in_chunks_and_grouped_in_bounded_batches() {
        let empty = chunks_of("vide", &entry(0), None);
        assert_eq!(empty.len(), 1);
        assert!(empty[0].1.is_some(), "un fichier vide se termine aussi");
        let big = chunks_of("gros", &entry(CHUNK * 5 + 1), Some("x".into()));
        assert_eq!(big.len(), 6);
        assert_eq!(big[5].0.len, 1);
        assert!(big[..5].iter().all(|(_, finish)| finish.is_none()));
        let grouped = batches(big);
        assert_eq!(grouped.iter().map(Vec::len).collect::<Vec<_>>(), [4, 2]);
    }

    #[test]
    fn mass_deletions_are_blocked() {
        let base: Index = (0..100).map(|i| (format!("f{i}"), entry(1))).collect();
        let deletes: Vec<Action> = (0..60)
            .map(|i| Action::DeleteNode {
                path: format!("f{i}"),
                expect: "h".into(),
            })
            .collect();
        assert!(
            guard_mass_deletion(&base, &deletes)
                .unwrap_err()
                .contains("60 fichiers")
        );
        assert!(guard_mass_deletion(&base, &deletes[..40]).is_ok());
    }
}
