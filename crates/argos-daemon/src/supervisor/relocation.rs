//! Déplacement d'un agent d'un nœud à un autre (D47, D48). Le nœud qui le détient lui
//! envoie tout ce qu'il possède : fiche et Workers, board, déclencheurs, fonctions,
//! surfaces, escalades, capacités, mémoire avec son historique, dossier dédié copié en
//! entier, et pour l'Assistant ses canaux de discussion. Le nœud d'arrivée l'adopte et en
//! devient le seul détenteur ; celui de départ n'en garde qu'une copie d'annuaire.

use super::{Supervisor, exited};
use crate::memory::{MemoryOp, MemoryOutput, MemoryPackage};
use crate::peer::{Arrival, ChannelTransfer};
use crate::sync::tree::Tree;
use crate::sync::{
    BATCH, CHUNK, Chunk, Finish, Index, ReadChunk, SyncRequest, SyncResponse, WriteOp,
};
use argos_core::{AgentId, AgentRecord, AgentState, Level};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Ce qu'un déplacement a fait, pour le compte rendu.
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
    /// Déplace un agent détenu ici vers le nœud `target`. `workdir` : dossier de projet
    /// sur la machine d'arrivée ; sans lui, le dossier dédié de l'agent est copié.
    pub async fn move_agent(
        &self,
        id: &AgentId,
        target: &str,
        workdir: Option<PathBuf>,
    ) -> Result<Moved, String> {
        let said = |error: super::SupervisorError| error.to_string();
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        let record = self.record(id).map_err(said)?;
        if record.level == Level::Worker || record.state == AgentState::Retired {
            return Err("seuls l'Assistant et un Agent vivant se déplacent".into());
        }
        if let Some(owner) = &record.node {
            return Err(format!(
                "{id} est détenu par le nœud {owner} : c'est lui qui le déplace"
            ));
        }
        if target == self.nodes().name() {
            return Err(format!("il est déjà sur {target}"));
        }
        if !self.nodes().is_known(target) {
            return Err(format!("nœud inconnu : {target}. Voir list_nodes."));
        }
        if !self.nodes().is_connected(target) {
            return Err(format!(
                "nœud {target} injoignable : impossible d'y poser l'agent"
            ));
        }
        if self.inspect(id).map_err(said)?.busy {
            return Err("attends la fin de son tour avant de le déplacer".into());
        }
        if record.level == Level::Agent && !self.children(id).map_err(said)?.is_empty() {
            return Err("termine et retire ses Workers avant de le déplacer".into());
        }
        if self.registry().has_sync(id).map_err(|e| e.to_string())? {
            return Err("ferme la synchro de son workspace avant de le déplacer".into());
        }
        let dedicated = self.dedicated_dir(&record);
        if workdir.is_none() && dedicated.is_none() {
            return Err(format!(
                "son workspace est un dossier de projet ({}) : indique workdir, le dossier du projet sur {target}. Les fichiers d'un projet ne sont pas copiés.",
                record.workdir.display()
            ));
        }
        if let Some(dir) = &workdir
            && !dir.is_absolute()
        {
            return Err(
                "workdir : indique le chemin absolu du projet sur la machine d'arrivée".into(),
            );
        }

        // Sa session s'arrête : sa conversation reste sur cette machine.
        if let Some(session) = live.take() {
            let _ = session.control.park();
            if !exited(session.exited.clone()).await {
                session.control.kill();
                exited(session.exited).await;
            }
            self.settle(id);
        }

        let package = self.export_memory(&record).await?;
        let (arrival_dir, copied, skipped) = match (&workdir, &dedicated) {
            (Some(dir), _) => (dir.clone(), None, vec![]),
            (None, Some(relative)) => {
                let (dir, copied, skipped) = self
                    .copy_workspace(id, &record.workdir, target, relative)
                    .await?;
                (dir, Some(copied), skipped)
            }
            (None, None) => unreachable!("refusé plus haut"),
        };
        let rows = self.registry().export_rows(id).map_err(|e| e.to_string())?;
        let channels = if record.level == Level::Assistant {
            crate::channels::transfers(self.home())
        } else {
            vec![]
        };
        let arrival = Arrival {
            rows,
            workdir: arrival_dir,
            memory: Some(package),
            channels: channels.clone(),
        };
        self.nodes().adopt(target, arrival).await?;

        // Le nœud d'arrivée le détient : ici, il n'en reste qu'une copie d'annuaire.
        self.registry()
            .release(id, target)
            .map_err(|error| error.to_string())?;
        if !channels.is_empty() {
            self.stop_channels(&channels);
        }
        let memory = self.memory().clone();
        let agent = id.clone();
        let archived = tokio::task::spawn_blocking(move || memory.archive(&agent)).await;
        if !matches!(archived, Ok(Ok(()))) {
            eprintln!("mémoire de {id} non mise de côté après son départ");
        }
        self.publish_status(id);
        Ok(Moved {
            record: self.record(id).map_err(said)?,
            copied,
            skipped,
            left: record.workdir,
        })
    }

    /// Reçoit un agent qui arrive d'un pair (D48) : ce nœud en devient le détenteur.
    pub async fn adopt(&self, arrival: Arrival, from: &str) -> Result<String, String> {
        let Some(main) = arrival.rows.agents.first().cloned() else {
            return Err("arrivée sans agent".into());
        };
        let workdir = if arrival.workdir.is_absolute() {
            arrival.workdir.clone()
        } else {
            self.home().join(&arrival.workdir)
        };
        let skipped = self
            .registry()
            .import_rows(&arrival.rows, &workdir)
            .map_err(|error| error.to_string())?;
        if let Some(package) = arrival.memory {
            let memory = self.memory().clone();
            let agent = main.id.clone();
            tokio::task::spawn_blocking(move || memory.import(&agent, &package))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("mémoire non posée : {error}"))?;
        }
        let mut report = format!("{} détenu par {}", main.id, self.nodes().name());
        if !arrival.channels.is_empty() {
            match crate::channels::receive(self.home(), &arrival.channels) {
                Ok(()) => {
                    crate::channels::start_configured(self);
                    let ids: Vec<&str> = arrival.channels.iter().map(|c| c.id.as_str()).collect();
                    report.push_str(&format!(" ; canaux repris : {}", ids.join(", ")));
                    // La voix (whisper, synthèse) est propre à chaque machine : elle ne voyage pas.
                    let config =
                        crate::config::Config::load_or_create(&self.home().join("config.toml"));
                    let heard = config
                        .ok()
                        .and_then(|config| config.voice)
                        .is_some_and(|voice| voice.whisper_model.is_some());
                    if !heard {
                        report.push_str(&format!(
                            " ; vocaux non compris : pas de whisper_model dans [voice] sur {}",
                            self.nodes().name()
                        ));
                    }
                }
                Err(error) => report.push_str(&format!(" ; canaux non repris : {error}")),
            }
        }
        for agent in &arrival.rows.agents {
            self.publish_status(&agent.id);
        }
        self.memory_log(
            &main.id,
            format!(
                "arrivé sur le nœud {} depuis {from} ; workspace {}",
                self.nodes().name(),
                workdir.display()
            ),
        );
        if !skipped.is_empty() {
            report.push_str(&format!(
                " ; lignes laissées de côté : {}",
                skipped.join(" ; ")
            ));
        }
        Ok(report)
    }

    /// Le dossier dédié de l'agent, relatif aux données du nœud, si son workspace en est
    /// un : `agents/<nom>` pour un Agent, `assistant` pour l'Assistant.
    fn dedicated_dir(&self, record: &AgentRecord) -> Option<PathBuf> {
        let name = record.workdir.file_name()?.to_str()?;
        let parent = record.workdir.parent()?;
        if parent == self.agents_dir() || parent == Path::new("agents") {
            return Some(Path::new("agents").join(name));
        }
        if record.level == Level::Assistant
            && (parent == self.home() || parent.as_os_str().is_empty())
        {
            return Some(PathBuf::from(name));
        }
        None
    }

    async fn export_memory(&self, record: &AgentRecord) -> Result<MemoryPackage, String> {
        match self.memory_op(record, MemoryOp::Export).await {
            Ok(MemoryOutput::Package(package)) => Ok(package),
            Ok(_) => Err("mémoire : réponse inattendue à l'export".into()),
            Err(error) => Err(format!("mémoire non exportée : {error}")),
        }
    }

    /// Copie le dossier dédié vers le même chemin relatif chez `target`, ou un nom neuf
    /// si ce dossier y existe déjà (un ancien séjour), pour ne rien écraser.
    async fn copy_workspace(
        &self,
        id: &AgentId,
        source: &Path,
        target: &str,
        relative: &Path,
    ) -> Result<(PathBuf, usize, Vec<String>), String> {
        let label = format!("move-{id}");
        std::fs::create_dir_all(source)
            .map_err(|error| format!("{} : {error}", source.display()))?;
        let local = Arc::new(Mutex::new(Tree::whole(source.to_path_buf())));
        let scanning = Arc::clone(&local);
        let index = tokio::task::spawn_blocking(move || lock(&scanning).scan())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("{} : {error}", source.display()))?;
        let name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent")
            .to_string();
        let parent = relative.parent().map(Path::to_path_buf).unwrap_or_default();
        for attempt in 0..5 {
            let dir_name = if attempt == 0 {
                name.clone()
            } else {
                let suffix = &uuid::Uuid::new_v4().simple().to_string()[..4];
                let base = name
                    .rsplit_once('-')
                    .map_or(name.as_str(), |(base, _)| base);
                format!("{base}-{suffix}")
            };
            let dir = parent.join(&dir_name);
            let opened = self
                .nodes()
                .sync(
                    target,
                    &label,
                    SyncRequest::Open {
                        workdir: dir.clone(),
                        excludes: vec![],
                        known: vec![],
                        copy: true,
                    },
                )
                .await?;
            let SyncResponse::Index { entries } = opened else {
                return Err(format!("nœud {target} : réponse inattendue à l'ouverture"));
            };
            if !entries.is_empty() {
                let _ = self.nodes().sync(target, &label, SyncRequest::Close).await;
                continue;
            }
            let copied = index.values().filter(|entry| !entry.held()).count();
            let result =
                transfer(&local, &index, |ops| self.remote_write(target, &label, ops)).await;
            let _ = self.nodes().sync(target, &label, SyncRequest::Close).await;
            return result
                .map(|skipped| (dir, copied, skipped))
                .map_err(|error| format!("workspace de {id} non copié vers {target} : {error}"));
        }
        Err("aucun dossier libre pour son workspace".into())
    }

    async fn remote_write(&self, peer: &str, label: &str, ops: Vec<WriteOp>) -> Result<(), String> {
        match self
            .nodes()
            .sync(peer, label, SyncRequest::Write { ops })
            .await?
        {
            SyncResponse::Written { results } => match results.into_iter().flatten().next() {
                Some(error) => Err(error),
                None => Ok(()),
            },
            _ => Err(format!("nœud {peer} : réponse inattendue à l'écriture")),
        }
    }

    fn stop_channels(&self, channels: &[ChannelTransfer]) {
        for channel in channels {
            self.channels().stop(&channel.id);
        }
        if let Err(error) = crate::channels::forget(self.home(), channels) {
            eprintln!("canaux non retirés de la configuration : {error}");
        }
    }
}

/// Copie chaque fichier et lien de `index`, lus dans l'arbre local, par morceaux. Rend
/// les fichiers trop gros pour voyager.
async fn transfer<F, Fut>(
    local: &Arc<Mutex<Tree>>,
    index: &Index,
    write: F,
) -> Result<Vec<String>, String>
where
    F: Fn(Vec<WriteOp>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
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
            write(vec![WriteOp::Link {
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
            let tree = Arc::clone(local);
            let read: Vec<Chunk> = tokio::task::spawn_blocking(move || lock(&tree).read(&chunks))
                .await
                .map_err(|error| error.to_string())?;
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
            write(ops).await?;
            offset = next;
            if offset >= entry.size {
                break;
            }
        }
    }
    Ok(skipped)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
