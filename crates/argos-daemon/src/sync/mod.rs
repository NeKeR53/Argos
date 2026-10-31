//! Synchro de workspace (D44) : le workspace d'un Agent placé sur un nœud a une copie
//! locale sur le hub, tenue à jour dans les deux sens, sans git ni dépôt intermédiaire.
//!
//! - [`tree`] : un arbre de fichiers vu par la synchro (parcours, empreintes, lecture et
//!   écriture sûres), le même des deux côtés.
//! - [`plan`] : le rapprochement à trois états (commun, hub, nœud), sans effet de bord.
//! - [`hub`] : côté hub, une tâche par synchro qui rapproche les deux arbres.
//!
//! Le nœud ne décide rien : il parcourt, lit et écrit sous le workspace de l'Agent à la
//! demande du hub, et lui signale ce que son watcher a vu. Tout passe par la connexion
//! hub → nœud existante (voir `node`).

pub mod hub;
pub mod plan;
pub mod tree;

pub use hub::Syncs;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Un fichier au-delà de cette taille ne voyage pas ; il est signalé.
pub const MAX_FILE: u64 = 100 * 1024 * 1024;
/// Taille d'un morceau de fichier dans un message.
pub const CHUNK: u64 = 1024 * 1024;
/// Volume lu ou écrit au plus par aller-retour.
pub const BATCH: u64 = 4 * 1024 * 1024;
/// Suffixe de la version du hub mise de côté lors d'un conflit.
pub const CONFLICT_SUFFIX: &str = ".conflit-hub";
/// Préfixe des fichiers temporaires d'écriture ; jamais synchronisés.
pub const TEMP_PREFIX: &str = ".argos-sync-";

/// Un fichier ou un lien symbolique, chemin relatif à la racine avec des `/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Empreinte du contenu : blake3 en hexadécimal, ou `link:<cible>` pour un lien.
    pub hash: String,
    pub size: u64,
    /// Nanosecondes depuis l'époque ; sert seulement à éviter de recalculer l'empreinte.
    pub mtime: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exec: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Trop gros pour voyager : présent, mais jamais copié ni supprimé par la synchro.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub too_big: bool,
}

impl Entry {
    /// Même contenu, aux yeux de la synchro (l'horodatage ne compte pas).
    pub fn same(&self, other: &Entry) -> bool {
        self.hash == other.hash && self.exec == other.exec && self.too_big == other.too_big
    }

    /// Ne voyage pas : trop gros, ou devenu ignoré. Ni copié, ni supprimé.
    pub fn held(&self) -> bool {
        self.too_big || self.hash == IGNORED
    }
}

/// Empreinte d'un chemin connu devenu ignoré, qui existe encore.
pub const IGNORED: &str = "ignored";

/// Chemin relatif → entrée.
pub type Index = BTreeMap<String, Entry>;

/// Changements d'un index : `None` pour un chemin disparu.
pub type Delta = Vec<(String, Option<Entry>)>;

/// Un morceau de fichier à lire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadChunk {
    pub path: String,
    pub offset: u64,
    pub len: u64,
}

/// Un morceau lu, en base64, ou la raison pour laquelle il ne l'a pas été.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub path: String,
    pub offset: u64,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Une écriture dans l'arbre. `expect` est l'empreinte que le chemin doit avoir au moment
/// d'écrire (`None` : absent) ; sinon l'opération est refusée, pour ne jamais écraser un
/// changement que le hub n'a pas encore vu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WriteOp {
    /// Un morceau d'un fichier, écrit dans un fichier temporaire ; `finish` le met en
    /// place quand son contenu est complet.
    Put {
        path: String,
        offset: u64,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finish: Option<Finish>,
    },
    Link {
        path: String,
        target: String,
        expect: Option<String>,
    },
    Delete {
        path: String,
        expect: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finish {
    pub hash: String,
    pub exec: bool,
    pub expect: Option<String>,
}

impl WriteOp {
    pub fn path(&self) -> &str {
        match self {
            WriteOp::Put { path, .. }
            | WriteOp::Link { path, .. }
            | WriteOp::Delete { path, .. } => path,
        }
    }
}

/// Demande du hub au nœud, sur l'arbre d'une synchro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum SyncRequest {
    /// Ouvre l'arbre et son watcher ; rend l'index complet. `workdir` : celui de l'Agent,
    /// relatif au dossier de données du nœud ou absolu.
    Open {
        workdir: std::path::PathBuf,
        excludes: Vec<String>,
        /// Chemins de l'état commun : sortis du parcours mais présents, ils sont devenus
        /// ignorés, pas supprimés.
        known: Vec<String>,
    },
    Read {
        chunks: Vec<ReadChunk>,
    },
    Write {
        ops: Vec<WriteOp>,
    },
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum SyncResponse {
    Index {
        entries: Index,
    },
    Data {
        chunks: Vec<Chunk>,
    },
    /// Une erreur par opération refusée, dans l'ordre (`None` : faite).
    Written {
        results: Vec<Option<String>>,
    },
    Closed,
}

/// Ce qui a changé d'un index à l'autre, au sens de la synchro.
pub fn diff(old: &Index, new: &Index) -> Delta {
    let mut delta: Delta = new
        .iter()
        .filter(|(path, entry)| !old.get(*path).is_some_and(|old| old.same(entry)))
        .map(|(path, entry)| (path.clone(), Some(entry.clone())))
        .collect();
    delta.extend(
        old.keys()
            .filter(|path| !new.contains_key(*path))
            .map(|path| (path.clone(), None)),
    );
    delta
}
