//! Un arbre de fichiers vu par la synchro : parcours avec empreintes, lecture par
//! morceaux et écritures conditionnelles. Sert tel quel au hub (copie locale) et au nœud
//! (workspace de l'Agent).
//!
//! Ne voyagent pas : `.git/`, ce qu'ignorent les `.gitignore` de l'arbre, les exclusions
//! de la synchro et les fichiers temporaires d'écriture. Aucune opération ne sort de la
//! racine : un chemin absolu, avec `..`, ou qui passe par un lien symbolique est refusé.

use super::{
    CONFLICT_SUFFIX, Chunk, Entry, Finish, IGNORED, Index, MAX_FILE, ReadChunk, TEMP_PREFIX,
    WriteOp,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

pub struct Tree {
    root: PathBuf,
    excludes: Vec<String>,
    /// Dernier parcours : évite de recalculer l'empreinte d'un fichier inchangé.
    cache: Index,
    /// Chemins connus de la synchro. Un chemin connu qui sort du parcours alors qu'il
    /// existe encore (devenu ignoré) n'est pas une suppression.
    known: BTreeSet<String>,
    /// Arbre complet, pour une copie : `.git` et fichiers ignorés compris.
    whole: bool,
}

impl Tree {
    pub fn new(
        root: PathBuf,
        excludes: Vec<String>,
        known: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            root,
            excludes,
            cache: Index::new(),
            known: known.into_iter().collect(),
            whole: false,
        }
    }

    /// Arbre complet d'un dossier à copier (D47) : `.git` et fichiers ignorés compris.
    pub fn whole(root: PathBuf) -> Self {
        Self {
            whole: true,
            ..Self::new(root, vec![], [])
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Parcourt l'arbre. Les fichiers inchangés (taille et date) gardent leur empreinte.
    pub fn scan(&mut self) -> io::Result<Index> {
        if !self.root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("dossier introuvable : {}", self.root.display()),
            ));
        }
        let mut overrides = ignore::overrides::OverrideBuilder::new(&self.root);
        for pattern in &self.excludes {
            overrides
                .add(&format!("!{pattern}"))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        }
        let overrides = overrides
            .build()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let walk = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .ignore(false)
            .parents(false)
            .git_global(false)
            .git_exclude(false)
            .git_ignore(!self.whole)
            .require_git(false)
            .follow_links(false)
            .overrides(overrides)
            .filter_entry({
                let whole = self.whole;
                move |entry| {
                    let name = entry.file_name().to_string_lossy();
                    (whole || name != ".git") && !name.starts_with(TEMP_PREFIX)
                }
            })
            .build();
        let mut index = Index::new();
        for entry in walk {
            let Ok(entry) = entry else { continue };
            let Some(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                continue;
            }
            let Some(rel) = relative(&self.root, entry.path()) else {
                continue;
            };
            if let Ok(Some(found)) = self.entry_of(&rel, entry.path()) {
                index.insert(rel, found);
            }
        }
        // Un chemin connu absent du parcours mais toujours là est devenu ignoré.
        for rel in &self.known {
            if index.contains_key(rel) {
                continue;
            }
            if let Ok(path) = self.inside(rel)
                && fs::symlink_metadata(&path).is_ok()
            {
                index.insert(
                    rel.clone(),
                    Entry {
                        hash: IGNORED.into(),
                        size: 0,
                        mtime: 0,
                        exec: false,
                        link: None,
                        too_big: false,
                    },
                );
            }
        }
        self.known = index.keys().cloned().collect();
        self.cache = index.clone();
        Ok(index)
    }

    /// Fichiers trop gros pour voyager, au dernier parcours.
    pub fn too_big(&self) -> Vec<String> {
        self.cache
            .iter()
            .filter(|(_, entry)| entry.too_big)
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Empreinte actuelle d'un chemin (`None` : absent).
    pub fn current(&self, rel: &str) -> Result<Option<String>, String> {
        let path = self.inside(rel)?;
        match self.entry_of(rel, &path) {
            Ok(entry) => Ok(entry.map(|entry| entry.hash)),
            Err(error) if error.kind() == io::ErrorKind::IsADirectory => Ok(Some("dir".into())),
            Err(error) => Err(format!("{rel} : {error}")),
        }
    }

    /// Lit des morceaux ; un morceau illisible porte son erreur sans bloquer les autres.
    pub fn read(&self, chunks: &[ReadChunk]) -> Vec<Chunk> {
        chunks
            .iter()
            .map(|chunk| {
                let (data, error) = match self.read_one(chunk) {
                    Ok(data) => (BASE64.encode(data), None),
                    Err(error) => (String::new(), Some(format!("{} : {error}", chunk.path))),
                };
                Chunk {
                    path: chunk.path.clone(),
                    offset: chunk.offset,
                    data,
                    error,
                }
            })
            .collect()
    }

    fn read_one(&self, chunk: &ReadChunk) -> Result<Vec<u8>, String> {
        let path = self.inside(&chunk.path)?;
        let failed = |error: io::Error| error.to_string();
        if fs::symlink_metadata(&path).map_err(failed)?.is_symlink() {
            return Err("lien symbolique".into());
        }
        let mut file = fs::File::open(&path).map_err(failed)?;
        file.seek(SeekFrom::Start(chunk.offset)).map_err(failed)?;
        let mut data = Vec::new();
        file.take(chunk.len.min(super::CHUNK))
            .read_to_end(&mut data)
            .map_err(failed)?;
        Ok(data)
    }

    /// Applique les écritures dans l'ordre ; une erreur par opération refusée.
    pub fn write(&mut self, ops: Vec<WriteOp>) -> Vec<Option<String>> {
        ops.into_iter()
            .map(|op| {
                let rel = op.path().to_string();
                self.apply(op).err().map(|error| format!("{rel} : {error}"))
            })
            .collect()
    }

    /// Conflit, côté hub : renomme la version locale en `<nom>.conflit-hub` (ou
    /// `.conflit-hub-2`…) ; rend le nouveau chemin.
    pub fn set_aside(&mut self, rel: &str, expect: Option<&str>) -> Result<String, String> {
        self.check(rel, expect)?;
        let path = self.inside(rel)?;
        let target = (1..)
            .map(|n| match n {
                1 => format!("{rel}{CONFLICT_SUFFIX}"),
                n => format!("{rel}{CONFLICT_SUFFIX}-{n}"),
            })
            .find(|candidate| {
                self.inside(candidate)
                    .is_ok_and(|path| fs::symlink_metadata(path).is_err())
            })
            .expect("un nom libre finit par se trouver");
        fs::rename(&path, self.inside(&target)?).map_err(|error| format!("{rel} : {error}"))?;
        Ok(target)
    }

    fn apply(&mut self, op: WriteOp) -> Result<(), String> {
        match op {
            WriteOp::Put {
                path: rel,
                offset,
                data,
                finish,
            } => {
                let path = self.inside(&rel)?;
                let data = BASE64.decode(data).map_err(|error| error.to_string())?;
                let temp = temp_of(&path);
                self.make_parents(&rel)?;
                let mut file = if offset == 0 {
                    fs::File::create(&temp)
                } else {
                    fs::OpenOptions::new().append(true).open(&temp)
                }
                .map_err(|error| error.to_string())?;
                let length = file.metadata().map_err(|error| error.to_string())?.len();
                if length != offset {
                    let _ = fs::remove_file(&temp);
                    return Err("morceau hors d'ordre".into());
                }
                file.write_all(&data).map_err(|error| error.to_string())?;
                drop(file);
                match finish {
                    Some(finish) => self.finish(&rel, &path, &temp, finish).inspect_err(|_| {
                        let _ = fs::remove_file(&temp);
                    }),
                    None => Ok(()),
                }
            }
            WriteOp::Link {
                path: rel,
                target,
                expect,
            } => {
                self.check(&rel, expect.as_deref())?;
                let path = self.inside(&rel)?;
                self.make_parents(&rel)?;
                if fs::symlink_metadata(&path).is_ok() {
                    fs::remove_file(&path).map_err(|error| error.to_string())?;
                }
                symlink(&target, &path).map_err(|error| error.to_string())
            }
            WriteOp::Delete { path: rel, expect } => {
                self.check(&rel, expect.as_deref())?;
                let path = self.inside(&rel)?;
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                self.cache.remove(&rel);
                self.prune(&path);
                Ok(())
            }
        }
    }

    fn finish(
        &mut self,
        rel: &str,
        path: &Path,
        temp: &Path,
        finish: Finish,
    ) -> Result<(), String> {
        let hash = hash_file(temp).map_err(|error| error.to_string())?;
        if hash != finish.hash {
            return Err("contenu incomplet ou altéré en route".into());
        }
        self.check(rel, finish.expect.as_deref())?;
        set_exec(temp, finish.exec).map_err(|error| error.to_string())?;
        fs::rename(temp, path).map_err(|error| error.to_string())
    }

    /// Refuse d'écrire sur un chemin qui a changé depuis ce que le hub en sait.
    fn check(&self, rel: &str, expect: Option<&str>) -> Result<(), String> {
        let current = self.current(rel)?;
        if current.as_deref() == expect {
            Ok(())
        } else {
            Err("modifié entre-temps, laissé tel quel".into())
        }
    }

    /// Chemin absolu d'un chemin relatif, sans sortir de la racine ni passer par un lien.
    fn inside(&self, rel: &str) -> Result<PathBuf, String> {
        let parts: Vec<&str> = rel.split('/').collect();
        if rel.is_empty()
            || rel.contains('\\')
            || rel.contains('\0')
            || parts.iter().any(|part| matches!(*part, "" | "." | ".."))
        {
            return Err(format!("chemin refusé : {rel}"));
        }
        let mut path = self.root.clone();
        for (i, part) in parts.iter().enumerate() {
            path.push(part);
            if i + 1 < parts.len()
                && fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_symlink())
            {
                return Err(format!("chemin refusé (lien symbolique) : {rel}"));
            }
        }
        Ok(path)
    }

    fn make_parents(&self, rel: &str) -> Result<(), String> {
        let path = self.inside(rel)?;
        let parent = path.parent().expect("sous la racine");
        fs::create_dir_all(parent).map_err(|error| error.to_string())
    }

    /// Retire les dossiers devenus vides au-dessus d'un fichier supprimé.
    fn prune(&self, path: &Path) {
        let mut dir = path.parent();
        while let Some(current) = dir {
            if current == self.root || fs::remove_dir(current).is_err() {
                break;
            }
            dir = current.parent();
        }
    }

    fn entry_of(&self, rel: &str, path: &Path) -> io::Result<Option<Entry>> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if meta.is_dir() {
            return Err(io::Error::new(io::ErrorKind::IsADirectory, "dossier"));
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos() as u64);
        if meta.is_symlink() {
            let target = fs::read_link(path)?.to_string_lossy().into_owned();
            return Ok(Some(Entry {
                hash: format!("link:{target}"),
                size: 0,
                mtime,
                exec: false,
                link: Some(target),
                too_big: false,
            }));
        }
        let size = meta.len();
        let exec = is_exec(&meta);
        if size > MAX_FILE {
            return Ok(Some(Entry {
                hash: format!("too-big:{size}:{mtime}"),
                size,
                mtime,
                exec,
                link: None,
                too_big: true,
            }));
        }
        let hash = match self.cache.get(rel) {
            Some(cached)
                if cached.size == size && cached.mtime == mtime && cached.link.is_none() =>
            {
                cached.hash.clone()
            }
            _ => hash_file(path)?,
        };
        Ok(Some(Entry {
            hash,
            size,
            mtime,
            exec,
            link: None,
            too_big: false,
        }))
    }
}

/// Regarde l'arbre : un signal par rafale de changements, une fois l'arbre calme depuis
/// 300 ms (2 s au plus). Le watcher s'arrête quand on le lâche.
pub fn watch(
    root: &Path,
) -> notify::Result<(notify::RecommendedWatcher, mpsc::UnboundedReceiver<()>)> {
    use notify::Watcher as _;
    let (raw_tx, mut raw) = mpsc::unbounded_channel::<()>();
    let git = root.join(".git");
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let relevant = event.paths.is_empty()
            || event.paths.iter().any(|path| {
                !path.starts_with(&git)
                    && !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(TEMP_PREFIX))
            });
        if relevant {
            let _ = raw_tx.send(());
        }
    })?;
    watcher.watch(root, notify::RecursiveMode::Recursive)?;
    let (calm_tx, calm) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while raw.recv().await.is_some() {
            let started = tokio::time::Instant::now();
            loop {
                let quiet = tokio::time::sleep(Duration::from_millis(300));
                tokio::select! {
                    more = raw.recv() => if more.is_none() { return },
                    _ = quiet => break,
                }
                if started.elapsed() > Duration::from_secs(2) {
                    break;
                }
            }
            if calm_tx.send(()).is_err() {
                return;
            }
        }
    });
    Ok((watcher, calm))
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    let rel = parts?.join("/");
    (!rel.is_empty()).then_some(rel)
}

fn temp_of(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!("{TEMP_PREFIX}{name}"))
}

#[cfg(unix)]
fn is_exec(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_exec(path: &Path, exec: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    let mode = permissions.mode();
    permissions.set_mode(if exec {
        mode | ((mode & 0o444) >> 2)
    } else {
        mode & !0o111
    });
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_exec(_: &Path, _: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(not(unix))]
fn symlink(_: &str, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "liens symboliques non pris en charge",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(path: &str, content: &[u8], expect: Option<&str>) -> WriteOp {
        WriteOp::Put {
            path: path.into(),
            offset: 0,
            data: BASE64.encode(content),
            finish: Some(Finish {
                hash: hash_bytes(content),
                exec: false,
                expect: expect.map(String::from),
            }),
        }
    }

    #[test]
    fn scans_respect_git_gitignore_and_excludes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref").unwrap();
        fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/bin"), "x").unwrap();
        fs::write(root.join("debug.log"), "x").unwrap();
        fs::create_dir_all(root.join("node_modules/a")).unwrap();
        fs::write(root.join("node_modules/a/index.js"), "x").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        let mut tree = Tree::new(root.into(), vec!["node_modules/".into()], []);
        let index = tree.scan().unwrap();
        let paths: Vec<&str> = index.keys().map(String::as_str).collect();
        assert_eq!(paths, [".gitignore", "src/main.rs"]);
        assert_eq!(index["src/main.rs"].hash, hash_bytes(b"fn main() {}"));
    }

    #[test]
    fn a_known_file_that_becomes_ignored_is_not_reported_as_deleted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), "secret").unwrap();
        let mut tree = Tree::new(dir.path().into(), vec![], []);
        assert!(tree.scan().unwrap().contains_key(".env"));
        fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
        let index = tree.scan().unwrap();
        assert_eq!(index[".env"].hash, "ignored");
        // Après un redémarrage, l'état commun redonne les chemins connus.
        let mut fresh = Tree::new(dir.path().into(), vec![], [".env".to_string()]);
        assert_eq!(fresh.scan().unwrap()[".env"].hash, "ignored");
    }

    #[test]
    fn writes_are_refused_when_the_path_changed_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let mut tree = Tree::new(dir.path().into(), vec![], []);
        assert_eq!(tree.write(vec![put("a/b.txt", b"one", None)]), [None]);
        assert_eq!(fs::read(dir.path().join("a/b.txt")).unwrap(), b"one");
        // Le hub croit le fichier absent : refusé, rien n'est écrasé.
        let refused = tree.write(vec![put("a/b.txt", b"two", None)]);
        assert!(refused[0].as_ref().unwrap().contains("modifié entre-temps"));
        assert_eq!(fs::read(dir.path().join("a/b.txt")).unwrap(), b"one");
        let one = hash_bytes(b"one");
        assert_eq!(tree.write(vec![put("a/b.txt", b"two", Some(&one))]), [None]);
        let two = hash_bytes(b"two");
        let deleted = tree.write(vec![WriteOp::Delete {
            path: "a/b.txt".into(),
            expect: Some(two),
        }]);
        assert_eq!(deleted, [None]);
        assert!(!dir.path().join("a").exists(), "dossier vide retiré");
        assert!(tree.scan().unwrap().is_empty(), "aucun temporaire laissé");
    }

    #[test]
    fn paths_cannot_leave_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        let mut tree = Tree::new(dir.path().into(), vec![], []);
        for path in ["../x", "/etc/passwd", "a/../../x", "", "escape/x"] {
            let result = tree.write(vec![put(path, b"x", None)]);
            assert!(result[0].as_ref().unwrap().contains("refusé"), "{path}");
        }
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[test]
    fn large_files_go_in_ordered_chunks_and_are_verified() {
        let dir = tempfile::tempdir().unwrap();
        let mut tree = Tree::new(dir.path().into(), vec![], []);
        let content = vec![7u8; 10];
        let chunk = |offset: usize, finish: Option<Finish>| WriteOp::Put {
            path: "big".into(),
            offset: offset as u64,
            data: BASE64.encode(&content[offset..offset + 5]),
            finish,
        };
        let finish = Finish {
            hash: hash_bytes(&content),
            exec: true,
            expect: None,
        };
        assert_eq!(
            tree.write(vec![chunk(0, None), chunk(5, Some(finish))]),
            [None, None]
        );
        let index = tree.scan().unwrap();
        assert_eq!(index["big"].size, 10);
        #[cfg(unix)]
        assert!(index["big"].exec);
        let read = tree.read(&[
            ReadChunk {
                path: "big".into(),
                offset: 5,
                len: 100,
            },
            ReadChunk {
                path: "absent".into(),
                offset: 0,
                len: 1,
            },
        ]);
        assert_eq!(BASE64.decode(&read[0].data).unwrap(), vec![7u8; 5]);
        assert!(
            read[1].error.is_some(),
            "un morceau illisible n'empêche pas les autres"
        );
        let bad = Finish {
            hash: hash_bytes(b"autre"),
            exec: false,
            expect: Some(hash_bytes(&content)),
        };
        let refused = tree.write(vec![chunk(0, Some(bad))]);
        assert!(refused[0].as_ref().unwrap().contains("altéré"));
        assert_eq!(fs::read(dir.path().join("big")).unwrap(), content);
    }

    #[test]
    fn a_conflicting_local_version_is_set_aside_under_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "hub").unwrap();
        fs::write(dir.path().join("a.rs.conflit-hub"), "ancien").unwrap();
        let mut tree = Tree::new(dir.path().into(), vec![], []);
        let hub = hash_bytes(b"hub");
        assert_eq!(
            tree.set_aside("a.rs", Some(&hub)).unwrap(),
            "a.rs.conflit-hub-2"
        );
        assert!(!dir.path().join("a.rs").exists());
    }
}
