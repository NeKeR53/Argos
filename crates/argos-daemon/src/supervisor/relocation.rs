//! Déplacement d'un Agent d'un nœud à un autre (D47) : sa mémoire part avec son
//! historique, son dossier dédié est copié en entier, puis le registre le place sur le
//! nœud d'arrivée. Le registre ne change qu'une fois mémoire et workspace arrivés.

use super::{Supervisor, exited};
use crate::memory::{MemoryOp, MemoryOutput, MemoryPackage};
use crate::sync::tree::Tree;
use crate::sync::{
    BATCH, CHUNK, Chunk, Finish, Index, ReadChunk, SyncRequest, SyncResponse, WriteOp,
};
use argos_core::{AgentId, AgentRecord, AgentState, Level};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Ce qu'un déplacement a fait, pour le compte rendu à l'Assistant.
#[derive(Debug, Clone)]
pub struct Moved {
    pub record: AgentRecord,
    /// Fichiers du workspace copiés (`None` : dossier de projet, rien copié).
    pub copied: Option<usize>,
    /// Fichiers trop gros pour voyager, restés sur l'ancienne machine.
    pub skipped: Vec<String>,
    /// Ancien workspace, laissé en place.
    pub left: PathBuf,
}

impl Supervisor {
    /// Déplace un Agent vers `target` (`None` : le hub). `workdir` : dossier de projet sur
    /// la machine d'arrivée ; sans lui, le dossier dédié de l'Agent est copié.
    pub async fn move_agent(
        &self,
        id: &AgentId,
        target: Option<&str>,
        workdir: Option<PathBuf>,
    ) -> Result<Moved, String> {
        let said = |error: super::SupervisorError| error.to_string();
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        let record = self.record(id).map_err(said)?;
        if record.level != Level::Agent || record.state == AgentState::Retired {
            return Err("seul un Agent vivant se déplace".into());
        }
        let from = record.node.clone();
        if from.as_deref() == target {
            return Err(format!("il est déjà sur {}", place_name(target)));
        }
        for node in [from.as_deref(), target].into_iter().flatten() {
            if !self.nodes().is_known(node) {
                return Err(format!("nœud inconnu : {node}. Voir list_nodes."));
            }
            if !self.nodes().is_connected(node) {
                return Err(format!(
                    "nœud {node} injoignable : impossible d'y prendre ou d'y poser sa mémoire"
                ));
            }
        }
        if self.inspect(id).map_err(said)?.busy {
            return Err("attends la fin de son tour avant de le déplacer".into());
        }
        if !self.children(id).map_err(said)?.is_empty() {
            return Err("termine et retire ses Workers avant de le déplacer".into());
        }
        if self.registry().has_sync(id).map_err(|e| e.to_string())? {
            return Err("ferme la synchro de son workspace avant de le déplacer".into());
        }
        let dedicated = self.dedicated_name(&record);
        let destination = match (&workdir, &dedicated) {
            (Some(dir), _) => {
                if !dir.is_absolute() || (target.is_none() && !dir.is_dir()) {
                    return Err(format!(
                        "workspace invalide : indique un dossier existant avec son chemin absolu sur {}",
                        place_name(target)
                    ));
                }
                None
            }
            (None, Some(name)) => Some(name.clone()),
            (None, None) => {
                return Err(format!(
                    "son workspace est un dossier de projet ({}) : indique workdir, le dossier du projet sur {}. Les fichiers d'un projet ne sont pas copiés.",
                    record.workdir.display(),
                    place_name(target)
                ));
            }
        };

        // Sa session s'arrête : sa conversation reste sur l'ancienne machine.
        if let Some(session) = live.take() {
            let _ = session.control.park();
            if !exited(session.exited.clone()).await {
                session.control.kill();
                exited(session.exited).await;
            }
            self.settle(id);
        }

        let package = self.export_memory(&record).await?;
        let (new_workdir, copied, skipped) = match destination {
            None => (workdir.expect("workdir donné"), None, vec![]),
            Some(name) => {
                let (dir, copied, skipped) = self
                    .copy_workspace(id, from.as_deref(), &record.workdir, target, &name)
                    .await?;
                (dir, Some(copied), skipped)
            }
        };
        self.import_memory(id, target, package.clone()).await?;
        self.registry()
            .relocate(id, target, &new_workdir)
            .map_err(|error| error.to_string())?;
        // L'ancienne mémoire est mise de côté ; le miroir du hub suit le nouveau nœud.
        match &from {
            Some(node) => {
                if let Ok(Err(error)) | Err(error) =
                    self.nodes().memory(node, id, MemoryOp::Archive).await
                {
                    eprintln!("nœud {node} : mémoire de {id} non mise de côté : {error}");
                }
            }
            None => {
                let memory = self.memory().clone();
                let agent = id.clone();
                let files = package.files.clone();
                let mirrored = tokio::task::spawn_blocking(move || {
                    memory.archive(&agent)?;
                    memory.mirror(&agent, &files)
                })
                .await;
                if !matches!(mirrored, Ok(Ok(()))) {
                    eprintln!("miroir de la mémoire de {id} non refait après son départ");
                }
            }
        }
        self.memory_log(
            id,
            format!(
                "déplacé de {} vers {} ; workspace {}",
                place_name(from.as_deref()),
                place_name(target),
                new_workdir.display()
            ),
        );
        self.publish_status(id);
        Ok(Moved {
            record: self.record(id).map_err(said)?,
            copied,
            skipped,
            left: record.workdir,
        })
    }

    /// Nom du dossier dédié de l'Agent, si son workspace en est un : celui qu'Argos crée
    /// par défaut, sous `agents/` des données du hub ou du nœud.
    fn dedicated_name(&self, record: &AgentRecord) -> Option<String> {
        let parent = record.workdir.parent()?;
        let dedicated = match &record.node {
            None => parent == self.agents_dir(),
            Some(_) => parent == Path::new("agents"),
        };
        dedicated
            .then(|| record.workdir.file_name()?.to_str().map(String::from))
            .flatten()
    }

    async fn export_memory(&self, record: &AgentRecord) -> Result<MemoryPackage, String> {
        let output = match &record.node {
            Some(node) => {
                self.nodes()
                    .memory(node, &record.id, MemoryOp::Export)
                    .await?
            }
            None => {
                let memory = self.memory().clone();
                let agent = record.id.clone();
                tokio::task::spawn_blocking(move || memory.apply(&agent, MemoryOp::Export))
                    .await
                    .map_err(|error| error.to_string())?
            }
        };
        match output {
            Ok(MemoryOutput::Package(package)) => Ok(package),
            Ok(_) => Err("mémoire : réponse inattendue à l'export".into()),
            Err(error) => Err(format!("mémoire non exportée : {error}")),
        }
    }

    async fn import_memory(
        &self,
        id: &AgentId,
        target: Option<&str>,
        package: MemoryPackage,
    ) -> Result<(), String> {
        let op = MemoryOp::Import { package };
        let result = match target {
            Some(node) => self.nodes().memory(node, id, op).await?,
            None => {
                let memory = self.memory().clone();
                let agent = id.clone();
                tokio::task::spawn_blocking(move || memory.apply(&agent, op))
                    .await
                    .map_err(|error| error.to_string())?
            }
        };
        result
            .map(drop)
            .map_err(|error| format!("mémoire non posée sur {} : {error}", place_name(target)))
    }

    /// Copie le dossier dédié vers celui de la machine d'arrivée : même nom, ou un nom
    /// neuf si ce dossier existe déjà (un ancien séjour), pour ne rien écraser. D'un nœud
    /// à un autre, la copie passe par le hub.
    async fn copy_workspace(
        &self,
        id: &AgentId,
        from: Option<&str>,
        source: &Path,
        target: Option<&str>,
        name: &str,
    ) -> Result<(PathBuf, usize, Vec<String>), String> {
        let label = format!("move-{id}");
        let mut origin = Side::new(self, from, source.to_path_buf(), format!("{label}-from"));
        let index = origin.open().await?;
        let result = async {
            if let (Some(_), Some(_)) = (from, target) {
                // Nœud → nœud : étape sur le hub, effacée ensuite.
                let staging = self.inner.settings.home.join("moves").join(id.as_str());
                let _ = std::fs::remove_dir_all(&staging);
                let mut hop = Side::new(self, None, staging.clone(), format!("{label}-hop"));
                hop.open().await?;
                let skipped = transfer(&origin, &index, &hop).await?;
                let hop_index = hop.open().await?;
                let placed = self.place(id, target, name, &label, &hop, &hop_index).await;
                let _ = std::fs::remove_dir_all(&staging);
                let (dir, copied, mut more) = placed?;
                more.extend(skipped);
                Ok((dir, copied, more))
            } else {
                self.place(id, target, name, &label, &origin, &index).await
            }
        }
        .await;
        origin.close().await;
        result
    }

    /// Pose l'arbre `index` de `from` dans un dossier dédié vide de la machine d'arrivée.
    async fn place(
        &self,
        id: &AgentId,
        target: Option<&str>,
        name: &str,
        label: &str,
        from: &Side<'_>,
        index: &Index,
    ) -> Result<(PathBuf, usize, Vec<String>), String> {
        for attempt in 0..5 {
            let dir_name = if attempt == 0 {
                name.to_string()
            } else {
                let suffix = &uuid::Uuid::new_v4().simple().to_string()[..4];
                format!(
                    "{}-{suffix}",
                    name.rsplit_once('-').map_or(name, |(base, _)| base)
                )
            };
            let dir = match target {
                Some(_) => Path::new("agents").join(&dir_name),
                None => self.agents_dir().join(&dir_name),
            };
            let mut to = Side::new(self, target, dir.clone(), format!("{label}-to"));
            let existing = to.open().await?;
            if !existing.is_empty() {
                to.close().await;
                continue;
            }
            let copied = index.values().filter(|entry| !entry.held()).count();
            let skipped = transfer(from, index, &to).await;
            to.close().await;
            return skipped
                .map(|skipped| (dir, copied, skipped))
                .map_err(|error| {
                    format!(
                        "workspace de {id} non copié vers {} : {error}",
                        place_name(target)
                    )
                });
        }
        Err("aucun dossier libre pour son workspace".into())
    }
}

fn place_name(node: Option<&str>) -> String {
    match node {
        Some(node) => format!("le nœud {node}"),
        None => "le hub".into(),
    }
}

/// Copie chaque fichier et lien de `index` de `from` vers `to`, par morceaux. Rend les
/// fichiers trop gros pour voyager.
async fn transfer(from: &Side<'_>, index: &Index, to: &Side<'_>) -> Result<Vec<String>, String> {
    let mut skipped = vec![];
    for (path, entry) in index {
        if entry.too_big {
            skipped.push(path.clone());
            continue;
        }
        if entry.held() {
            continue;
        }
        if let Some(target) = &entry.link {
            to.write(vec![WriteOp::Link {
                path: path.clone(),
                target: target.clone(),
                expect: None,
            }])
            .await?;
            continue;
        }
        let mut offset = 0;
        loop {
            let mut chunks = vec![];
            let mut next = offset;
            while next < entry.size && next - offset < BATCH {
                chunks.push(ReadChunk {
                    path: path.clone(),
                    offset: next,
                    len: CHUNK,
                });
                next += CHUNK;
            }
            if chunks.is_empty() {
                // Fichier vide : un seul morceau, vide.
                chunks.push(ReadChunk {
                    path: path.clone(),
                    offset: 0,
                    len: 0,
                });
            }
            let read = from.read(chunks).await?;
            let last = read.len() - 1;
            let ops = read
                .into_iter()
                .enumerate()
                .map(|(i, chunk)| {
                    if let Some(error) = chunk.error {
                        return Err(error);
                    }
                    let finish = (i == last && next >= entry.size).then(|| Finish {
                        hash: entry.hash.clone(),
                        exec: entry.exec,
                        expect: None,
                    });
                    Ok(WriteOp::Put {
                        path: chunk.path,
                        offset: chunk.offset,
                        data: chunk.data,
                        finish,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            to.write(ops).await?;
            offset = next;
            if offset >= entry.size {
                break;
            }
        }
    }
    Ok(skipped)
}

/// Un bout de la copie : un dossier du hub, ou un workspace sur un nœud.
struct Side<'a> {
    supervisor: &'a Supervisor,
    node: Option<String>,
    dir: PathBuf,
    /// Nom de l'arbre ouvert côté nœud.
    label: String,
    tree: Option<Arc<Mutex<Tree>>>,
}

impl<'a> Side<'a> {
    fn new(supervisor: &'a Supervisor, node: Option<&str>, dir: PathBuf, label: String) -> Self {
        Self {
            supervisor,
            node: node.map(String::from),
            dir,
            label,
            tree: None,
        }
    }

    /// Ouvre (ou relit) l'arbre complet ; le dossier est créé s'il manque.
    async fn open(&mut self) -> Result<Index, String> {
        match &self.node {
            Some(node) => {
                let response = self
                    .supervisor
                    .nodes()
                    .sync(
                        node,
                        &self.label,
                        SyncRequest::Open {
                            workdir: self.dir.clone(),
                            excludes: vec![],
                            known: vec![],
                            copy: true,
                        },
                    )
                    .await?;
                match response {
                    SyncResponse::Index { entries } => Ok(entries),
                    _ => Err(format!("nœud {node} : réponse inattendue à l'ouverture")),
                }
            }
            None => {
                std::fs::create_dir_all(&self.dir)
                    .map_err(|error| format!("{} : {error}", self.dir.display()))?;
                let tree = Arc::new(Mutex::new(Tree::whole(self.dir.clone())));
                let scanning = Arc::clone(&tree);
                let index = tokio::task::spawn_blocking(move || lock(&scanning).scan())
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                self.tree = Some(tree);
                Ok(index)
            }
        }
    }

    async fn read(&self, chunks: Vec<ReadChunk>) -> Result<Vec<Chunk>, String> {
        match (&self.node, &self.tree) {
            (Some(node), _) => {
                match self
                    .supervisor
                    .nodes()
                    .sync(node, &self.label, SyncRequest::Read { chunks })
                    .await?
                {
                    SyncResponse::Data { chunks } => Ok(chunks),
                    _ => Err(format!("nœud {node} : réponse inattendue à la lecture")),
                }
            }
            (None, Some(tree)) => {
                let tree = Arc::clone(tree);
                tokio::task::spawn_blocking(move || lock(&tree).read(&chunks))
                    .await
                    .map_err(|error| error.to_string())
            }
            (None, None) => Err("arbre non ouvert".into()),
        }
    }

    /// Écrit ; la première opération refusée fait échouer la copie.
    async fn write(&self, ops: Vec<WriteOp>) -> Result<(), String> {
        let results = match (&self.node, &self.tree) {
            (Some(node), _) => {
                match self
                    .supervisor
                    .nodes()
                    .sync(node, &self.label, SyncRequest::Write { ops })
                    .await?
                {
                    SyncResponse::Written { results } => results,
                    _ => return Err(format!("nœud {node} : réponse inattendue à l'écriture")),
                }
            }
            (None, Some(tree)) => {
                let tree = Arc::clone(tree);
                tokio::task::spawn_blocking(move || lock(&tree).write(ops))
                    .await
                    .map_err(|error| error.to_string())?
            }
            (None, None) => return Err("arbre non ouvert".into()),
        };
        match results.into_iter().flatten().next() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn close(&self) {
        if let Some(node) = &self.node {
            let _ = self
                .supervisor
                .nodes()
                .sync(node, &self.label, SyncRequest::Close)
                .await;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
