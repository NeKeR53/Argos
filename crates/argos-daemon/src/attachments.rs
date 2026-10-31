//! Pièces jointes d'un message de l'utilisateur. Les images partent en entrée native du
//! moteur ; un fichier texte est recopié dans le message ; un autre fichier (PDF…) est
//! enregistré sur le hub et l'agent reçoit son chemin, à lire avec ses outils.

use crate::engine::{Image, Message};
use argos_core::AgentRecord;
use argos_core::protocol::Attachment;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::fmt::Write as _;
use std::path::Path;
use uuid::Uuid;

/// Pièces jointes au plus par message.
pub const MAX_ATTACHMENTS: usize = 8;
/// Taille d'une pièce jointe décodée, au plus.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
/// Au-delà, un fichier texte est enregistré plutôt que recopié dans le message.
const MAX_INLINE_TEXT: usize = 100 * 1024;
/// Images que Claude et Codex lisent nativement.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Le message à remettre : le texte de l'utilisateur, suivi d'une ligne par pièce jointe.
/// Les fichiers à enregistrer vont dans `dir` (créé au besoin).
pub fn prepare(
    record: &AgentRecord,
    text: &str,
    attachments: &[Attachment],
    dir: &Path,
) -> Result<Message, String> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(format!("{MAX_ATTACHMENTS} pièces jointes au plus"));
    }
    let mut message = Message {
        text: text.to_string(),
        images: vec![],
    };
    for attachment in attachments {
        let name = clean_name(&attachment.name);
        let bytes = STANDARD
            .decode(attachment.data.trim())
            .map_err(|_| format!("{name} : contenu illisible"))?;
        if bytes.len() > MAX_BYTES {
            return Err(format!(
                "{name} : {} au plus par pièce jointe",
                human_size(MAX_BYTES)
            ));
        }
        let media_type = attachment.media_type.trim().to_ascii_lowercase();
        let note = if IMAGE_TYPES.contains(&media_type.as_str()) {
            message.images.push(Image {
                media_type,
                data: STANDARD.encode(&bytes),
            });
            format!("[Image jointe : {name}]")
        } else if let Some(content) = inline_text(&media_type, &name, &bytes) {
            let fence = if content.contains("```") {
                "````"
            } else {
                "```"
            };
            format!("[Fichier joint : {name}]\n{fence}\n{content}\n{fence}")
        } else {
            // Le chemin n'existe que sur le hub : un agent placé ailleurs ne le lirait pas.
            if let Some(node) = &record.node {
                return Err(format!(
                    "{name} : un fichier de ce type ne peut pas encore partir vers un agent du nœud « {node} » (images et texte seulement)"
                ));
            }
            std::fs::create_dir_all(dir).map_err(|e| format!("{name} : {e}"))?;
            let path = dir.join(format!(
                "{}-{name}",
                &Uuid::new_v4().simple().to_string()[..8]
            ));
            std::fs::write(&path, &bytes).map_err(|e| format!("{name} : {e}"))?;
            format!(
                "[Fichier joint : {name} ({media_type}, {}), enregistré dans {} : lis-le avec tes outils]",
                human_size(bytes.len()),
                path.display()
            )
        };
        if !message.text.is_empty() {
            message.text.push_str("\n\n");
        }
        let _ = write!(message.text, "{note}");
    }
    if message.text.trim().is_empty() {
        return Err("message vide".into());
    }
    Ok(message)
}

/// Le contenu d'un fichier texte assez petit pour aller dans le message.
fn inline_text(media_type: &str, name: &str, bytes: &[u8]) -> Option<String> {
    let textual = media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json" | "application/xml" | "application/x-yaml" | "application/toml"
        )
        || TEXT_EXTENSIONS
            .iter()
            .any(|extension| name.to_ascii_lowercase().ends_with(extension));
    if !textual || bytes.len() > MAX_INLINE_TEXT {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

const TEXT_EXTENSIONS: [&str; 14] = [
    ".txt", ".md", ".csv", ".json", ".yaml", ".yml", ".toml", ".xml", ".log", ".rs", ".ts", ".py",
    ".sh", ".html",
];

/// Nom de fichier sans chemin ni caractère qui gênerait sur le disque.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "fichier".into()
    } else {
        cleaned.chars().take(120).collect()
    }
}

fn human_size(bytes: usize) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} Mo", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} Ko", b / 1024),
        b => format!("{b} o"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argos_core::{AgentId, AgentState, EngineKind, Level};

    fn record(node: Option<&str>) -> AgentRecord {
        AgentRecord {
            id: AgentId::new("a1"),
            level: Level::Agent,
            parent: None,
            engine: EngineKind::Claude,
            model: None,
            profile: None,
            node: node.map(String::from),
            workdir: "/tmp".into(),
            charter: None,
            state: AgentState::Parked,
            session_id: None,
            created_at: 0,
        }
    }

    fn attachment(name: &str, media_type: &str, bytes: &[u8]) -> Attachment {
        Attachment {
            name: name.into(),
            media_type: media_type.into(),
            data: STANDARD.encode(bytes),
        }
    }

    #[test]
    fn images_go_native_text_inline_and_other_files_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let message = prepare(
            &record(None),
            "Regarde",
            &[
                attachment("../capture.png", "image/png", b"\x89PNG"),
                attachment("notes.md", "", b"# Titre"),
                attachment("devis.pdf", "application/pdf", b"%PDF-1.7"),
            ],
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            message.images,
            [Image {
                media_type: "image/png".into(),
                data: STANDARD.encode(b"\x89PNG"),
            }]
        );
        assert!(
            message
                .text
                .starts_with("Regarde\n\n[Image jointe : capture.png]")
        );
        assert!(
            message
                .text
                .contains("[Fichier joint : notes.md]\n```\n# Titre\n```")
        );
        let saved: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(saved.len(), 1);
        let path = saved[0].as_ref().unwrap().path();
        assert!(path.to_string_lossy().ends_with("-devis.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7");
        assert!(message.text.contains(&path.display().to_string()));
    }

    #[test]
    fn limits_and_remote_agents_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = attachment("devis.pdf", "application/pdf", b"%PDF");
        assert!(
            prepare(
                &record(Some("pi")),
                "x",
                std::slice::from_ref(&pdf),
                dir.path()
            )
            .is_err()
        );
        // Une image part aussi vers un nœud : elle voyage dans le message.
        let png = attachment("a.png", "image/png", b"png");
        assert!(prepare(&record(Some("pi")), "", &[png], dir.path()).is_ok());
        let broken = Attachment {
            data: "%%%".into(),
            ..pdf.clone()
        };
        assert!(prepare(&record(None), "x", &[broken], dir.path()).is_err());
        let many = vec![pdf; MAX_ATTACHMENTS + 1];
        assert!(prepare(&record(None), "x", &many, dir.path()).is_err());
        assert!(prepare(&record(None), "  ", &[], dir.path()).is_err());
    }

    #[test]
    fn names_lose_their_path_and_odd_characters() {
        assert_eq!(clean_name("/etc/passwd"), "passwd");
        assert_eq!(clean_name("..\\..\\x:y.txt"), "x_y.txt");
        assert_eq!(clean_name("..."), "fichier");
    }
}
