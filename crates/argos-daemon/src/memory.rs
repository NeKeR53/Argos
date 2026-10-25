//! Mémoire persistante de l'Assistant et des Agents (D23–D25) : un dossier Markdown par
//! agent, compatible Obsidian, versionné par un git local qui sert de journal
//! d'annulation. L'agent tient `index.md`, `journal.md` et `notes/*.md` ; le daemon
//! réécrit `state.md` (état structuré) et ajoute des faits au journal. La session ne
//! charge qu'un digest (D24).

use argos_core::AgentId;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Taille maximale du digest injecté au démarrage d'une session.
const DIGEST_LIMIT: usize = 12_000;
/// Lignes de fin de journal reprises dans le digest.
const JOURNAL_TAIL: usize = 20;

const INDEX_TEMPLATE: &str = "# Index\n\nL'essentiel à savoir à chaque réveil : qui je suis, comment je m'organise, ce qui est en cours. Court : les détails vont dans notes/.\n";
const JOURNAL_TEMPLATE: &str = "# Journal\n\n";

#[derive(Debug, Clone)]
pub struct Memory {
    root: PathBuf,
}

/// Une ligne trouvée par [`Memory::search`].
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub file: String,
    pub line: usize,
    pub text: String,
}

impl Memory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn dir(&self, agent: &AgentId) -> PathBuf {
        self.root.join(agent.as_str())
    }

    /// Crée le dossier mémoire d'un agent s'il n'existe pas encore.
    pub fn ensure(&self, agent: &AgentId) -> io::Result<()> {
        let dir = self.dir(agent);
        if dir.join("index.md").exists() {
            return Ok(());
        }
        fs::create_dir_all(dir.join("notes"))?;
        fs::write(dir.join("index.md"), INDEX_TEMPLATE)?;
        fs::write(dir.join("journal.md"), JOURNAL_TEMPLATE)?;
        git(&dir, &["init", "-q"]);
        commit(&dir, "mémoire créée");
        Ok(())
    }

    /// Réécrit l'état structuré tenu par le daemon.
    pub fn write_state(&self, agent: &AgentId, state: &str) -> io::Result<()> {
        let dir = self.dir(agent);
        let path = dir.join("state.md");
        if fs::read_to_string(&path).ok().as_deref() == Some(state) {
            return Ok(());
        }
        fs::write(path, state)?;
        commit(&dir, "état mis à jour par Argos");
        Ok(())
    }

    /// Ajoute un fait daté au journal, au nom du daemon.
    pub fn log(&self, agent: &AgentId, fact: &str) -> io::Result<()> {
        let dir = self.dir(agent);
        if !dir.exists() {
            return Ok(());
        }
        let mut journal = OpenOptions::new()
            .append(true)
            .create(true)
            .open(dir.join("journal.md"))?;
        writeln!(journal, "- {} · {fact}", timestamp())?;
        commit(&dir, "journal");
        Ok(())
    }

    /// Ce que la session charge au démarrage : index, état, fin du journal.
    pub fn digest(&self, agent: &AgentId) -> io::Result<String> {
        let dir = self.dir(agent);
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap_or_default();
        let journal = read("journal.md");
        let lines: Vec<&str> = journal.lines().filter(|l| l.starts_with("- ")).collect();
        let tail = lines[lines.len().saturating_sub(JOURNAL_TAIL)..].join("\n");
        let notes = self.list(agent)?;
        let mut digest = format!(
            "## Ta mémoire ({})\n\n### index.md\n{}\n\n### state.md\n{}\n\n### Fin du journal\n{}\n\n### Fichiers\n{}\n",
            dir.display(),
            read("index.md").trim(),
            read("state.md").trim(),
            if tail.is_empty() { "(vide)" } else { &tail },
            notes.join(", ")
        );
        if digest.len() > DIGEST_LIMIT {
            let mut cut = DIGEST_LIMIT;
            while !digest.is_char_boundary(cut) {
                cut -= 1;
            }
            digest.truncate(cut);
            digest.push_str("\n… (digest tronqué : lis le reste avec memory_read)\n");
        }
        Ok(digest)
    }

    /// Fichiers de la mémoire, chemins relatifs.
    pub fn list(&self, agent: &AgentId) -> io::Result<Vec<String>> {
        let dir = self.dir(agent);
        let mut files: Vec<String> = ["index.md", "journal.md", "state.md"]
            .into_iter()
            .filter(|name| dir.join(name).exists())
            .map(String::from)
            .collect();
        if let Ok(entries) = fs::read_dir(dir.join("notes")) {
            let mut notes: Vec<String> = entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| name.ends_with(".md"))
                .map(|name| format!("notes/{name}"))
                .collect();
            notes.sort();
            files.extend(notes);
        }
        Ok(files)
    }

    pub fn read(&self, agent: &AgentId, file: &str) -> Result<String, String> {
        let path = resolve(&self.dir(agent), file, true)?;
        fs::read_to_string(&path).map_err(|_| format!("« {file} » n'existe pas"))
    }

    /// Écrit un fichier de la mémoire (pas `state.md`, tenu par le daemon) et commit.
    pub fn write(
        &self,
        agent: &AgentId,
        file: &str,
        content: &str,
        append: bool,
    ) -> Result<(), String> {
        let dir = self.dir(agent);
        let path = resolve(&dir, file, false)?;
        let said = |error: io::Error| error.to_string();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(said)?;
        }
        if append {
            let mut target = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .map_err(said)?;
            writeln!(target, "{}", content.trim_end()).map_err(said)?;
        } else {
            fs::write(&path, format!("{}\n", content.trim_end())).map_err(said)?;
        }
        commit(&dir, &format!("{file} écrit par l'agent"));
        Ok(())
    }

    /// Lignes qui contiennent `query` (sans tenir compte de la casse).
    pub fn search(&self, agent: &AgentId, query: &str) -> io::Result<Vec<Found>> {
        let query = query.to_lowercase();
        let dir = self.dir(agent);
        let mut found = vec![];
        for file in self.list(agent)? {
            let content = fs::read_to_string(dir.join(&file)).unwrap_or_default();
            for (index, text) in content.lines().enumerate() {
                if text.to_lowercase().contains(&query) {
                    found.push(Found {
                        file: file.clone(),
                        line: index + 1,
                        text: text.into(),
                    });
                }
            }
        }
        Ok(found)
    }
}

/// Chemin d'un fichier de la mémoire. Seuls `index.md`, `journal.md` et
/// `notes/<nom>.md` sont permis (plus `state.md` en lecture) : rien hors du dossier.
fn resolve(dir: &Path, file: &str, reading: bool) -> Result<PathBuf, String> {
    let file = file.trim();
    let allowed = match file {
        "index.md" | "journal.md" => true,
        "state.md" => reading,
        _ => file.strip_prefix("notes/").is_some_and(|name| {
            name.ends_with(".md")
                && name.len() > 3
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
                && !name.contains("..")
        }),
    };
    if !allowed {
        let hint = if file == "state.md" {
            "state.md est tenu par Argos"
        } else {
            "fichiers permis : index.md, journal.md, notes/<nom>.md"
        };
        return Err(format!("« {file} » refusé : {hint}"));
    }
    Ok(dir.join(file))
}

/// Commit de tout le dossier. Sans git installé, la mémoire marche sans historique.
fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=Argos",
            "-c",
            "user.email=argos@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            message,
        ],
    );
}

fn git(dir: &Path, args: &[&str]) {
    let _ = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Date et heure UTC, `AAAA-MM-JJ HH:MM`.
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// Jours depuis 1970-01-01 → (année, mois, jour), algorithme de H. Hinnant.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> (tempfile::TempDir, Memory, AgentId) {
        let root = tempfile::tempdir().unwrap();
        let memory = Memory::new(root.path());
        let agent = AgentId::new("a1");
        memory.ensure(&agent).unwrap();
        (root, memory, agent)
    }

    #[test]
    fn a_new_memory_has_its_files_and_history() {
        let (_root, memory, agent) = memory();
        assert_eq!(memory.list(&agent).unwrap(), vec!["index.md", "journal.md"]);
        assert!(memory.dir(&agent).join(".git").exists());
        // Idempotent : un second appel ne réécrit rien.
        memory
            .write(&agent, "index.md", "Je suis Domotique.", false)
            .unwrap();
        memory.ensure(&agent).unwrap();
        assert_eq!(
            memory.read(&agent, "index.md").unwrap(),
            "Je suis Domotique.\n"
        );
    }

    #[test]
    fn the_agent_writes_only_inside_its_memory() {
        let (_root, memory, agent) = memory();
        memory
            .write(&agent, "notes/éclairage.md", "Philips Hue", false)
            .unwrap();
        memory
            .write(&agent, "notes/éclairage.md", "3 pièces", true)
            .unwrap();
        assert_eq!(
            memory.read(&agent, "notes/éclairage.md").unwrap(),
            "Philips Hue\n3 pièces\n"
        );
        for refused in [
            "../x.md",
            "/etc/passwd",
            "notes/../../x.md",
            "notes/x.txt",
            "state.md",
            "autre.md",
        ] {
            assert!(
                memory.write(&agent, refused, "x", false).is_err(),
                "{refused}"
            );
        }
        memory.write_state(&agent, "# État").unwrap();
        assert_eq!(memory.read(&agent, "state.md").unwrap(), "# État");
    }

    #[test]
    fn the_digest_holds_index_state_and_recent_journal() {
        let (_root, memory, agent) = memory();
        memory
            .write(&agent, "index.md", "Je gère la maison.", false)
            .unwrap();
        memory
            .write_state(&agent, "Rôle : Agent « Domotique »")
            .unwrap();
        for n in 0..30 {
            memory.log(&agent, &format!("fait {n}")).unwrap();
        }
        let digest = memory.digest(&agent).unwrap();
        assert!(digest.contains("Je gère la maison."));
        assert!(digest.contains("Rôle : Agent « Domotique »"));
        assert!(digest.contains("fait 29"));
        assert!(
            !digest.contains("fait 9\n"),
            "seule la fin du journal est reprise"
        );
    }

    #[test]
    fn the_digest_is_bounded() {
        let (_root, memory, agent) = memory();
        memory
            .write(&agent, "index.md", &"é".repeat(20_000), false)
            .unwrap();
        let digest = memory.digest(&agent).unwrap();
        assert!(digest.len() < DIGEST_LIMIT + 200);
        assert!(digest.contains("digest tronqué"));
    }

    #[test]
    fn search_finds_lines_across_files() {
        let (_root, memory, agent) = memory();
        memory
            .write(&agent, "notes/matériel.md", "Pont Hue au salon", false)
            .unwrap();
        memory.log(&agent, "ampoule du SALON changée").unwrap();
        let found = memory.search(&agent, "salon").unwrap();
        let files: Vec<&str> = found.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(files, vec!["journal.md", "notes/matériel.md"]);
    }

    #[test]
    fn every_write_is_committed() {
        let (_root, memory, agent) = memory();
        memory.write(&agent, "index.md", "v2", false).unwrap();
        let log = Command::new("git")
            .arg("-C")
            .arg(memory.dir(&agent))
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        let log = String::from_utf8(log.stdout).unwrap();
        assert!(log.lines().count() >= 2, "{log}");
    }

    #[test]
    fn dates_are_converted_from_unix_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_751), (2026, 10, 25));
    }
}
