//! Types partagés entre le daemon et la GUI. Vocabulaire : docs/GLOSSARY.md.

pub mod protocol;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// Version du protocole entre la GUI et le daemon.
pub const PROTOCOL_VERSION: u32 = 1;

/// Forme texte stable d'une énumération, identique à sa forme JSON (stockage, logs).
macro_rules! text_form {
    ($type:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $type {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($type::$variant => $text),+
                }
            }
        }

        impl FromStr for $type {
            type Err = UnknownValue;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $($text => Ok($type::$variant),)+
                    _ => Err(UnknownValue {
                        kind: stringify!($type),
                        value: text.into(),
                    }),
                }
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Texte qui ne correspond à aucune valeur connue d'une énumération.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue {
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} inconnu : {:?}", self.kind, self.value)
    }
}

impl std::error::Error for UnknownValue {}

/// Niveau d'un agent dans la hiérarchie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Assistant,
    Agent,
    Worker,
}

text_form!(Level {
    Assistant => "assistant",
    Agent => "agent",
    Worker => "worker",
});

impl Level {
    /// Le niveau que celui-ci contrôle : chaque niveau ne pilote que celui juste en dessous.
    pub fn child(self) -> Option<Level> {
        match self {
            Level::Assistant => Some(Level::Agent),
            Level::Agent => Some(Level::Worker),
            Level::Worker => None,
        }
    }

    /// Seuls l'Assistant et les Agents ont une mémoire persistante.
    pub fn has_memory(self) -> bool {
        self != Level::Worker
    }
}

/// Le CLI qui fait tourner un agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Claude,
    Codex,
}

text_form!(EngineKind {
    Claude => "claude",
    Codex => "codex",
});

impl EngineKind {
    /// Nom du binaire à lancer : celui du moteur.
    pub fn binary(self) -> &'static str {
        self.as_str()
    }
}

/// Cycle de vie d'un agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Son process tourne.
    Active,
    /// Pas de process ; il se reprend par son session-id.
    Parked,
    /// Retiré pour de bon. Gardé pour l'historique.
    Retired,
}

text_form!(AgentState {
    Active => "active",
    Parked => "parked",
    Retired => "retired",
});

/// Identifiant stable d'un agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Fiche d'identité d'un Agent : sert à décider réutiliser ou créer (D20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Charter {
    pub name: String,
    /// Périmètre : projet, domaine ou tâche récurrente dont l'Agent a la charge.
    pub scope: String,
}

/// Un agent tel que le registre le connaît.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub level: Level,
    /// `None` pour l'Assistant seulement.
    pub parent: Option<AgentId>,
    pub engine: EngineKind,
    pub model: Option<String>,
    /// Profil de la politique de modèles dont viennent moteur et modèle (D22) ; il
    /// donne la chaîne de repli. Absent pour l'Assistant, choisi par l'utilisateur.
    pub profile: Option<String>,
    pub workdir: PathBuf,
    /// Présente pour les Agents seulement.
    pub charter: Option<Charter>,
    pub state: AgentState,
    /// Session du moteur à reprendre ; inconnue tant que l'agent n'a jamais tourné.
    pub session_id: Option<String>,
    /// Secondes depuis l'époque Unix.
    pub created_at: u64,
}

/// État d'une tâche du board d'un Agent (D17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    InProgress,
    Blocked,
    Done,
    Cancelled,
    /// Son Worker a disparu (crash, retrait) : à relancer.
    Interrupted,
}

text_form!(TaskState {
    Pending => "pending",
    InProgress => "in_progress",
    Blocked => "blocked",
    Done => "done",
    Cancelled => "cancelled",
    Interrupted => "interrupted",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionState {
    Open,
    Done,
    Cancelled,
}

text_form!(MissionState {
    Open => "open",
    Done => "done",
    Cancelled => "cancelled",
});

/// Demande de haut niveau confiée à un Agent par l'Assistant (D19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mission {
    pub id: String,
    pub agent: AgentId,
    pub text: String,
    pub state: MissionState,
    pub created_at: u64,
}

/// Unité de travail du board d'un Agent, exécutée par un Worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub agent: AgentId,
    pub mission: Option<String>,
    pub title: String,
    pub detail: Option<String>,
    pub state: TaskState,
    /// Le Worker qui s'en charge ou s'en est chargé.
    pub worker: Option<AgentId>,
    /// Dernier compte rendu du Worker.
    pub result: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Le board d'un Agent, dans l'ordre de priorité : missions ouvertes, puis tâches de ces
/// missions et tâches hors mission non terminées.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub agent: AgentId,
    pub missions: Vec<Mission>,
    pub tasks: Vec<Task>,
}

/// Nature d'une surface (D26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    /// Vivante et bidirectionnelle : l'agent la met à jour et lui pousse des données.
    Cockpit,
    /// Snapshot autonome, comme des graphiques de synthèse.
    Artifact,
}

text_form!(SurfaceKind {
    Cockpit => "cockpit",
    Artifact => "artifact",
});

/// Une page HTML produite par un agent et hébergée par l'app (D26–D28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceMeta {
    pub id: String,
    pub agent: AgentId,
    pub kind: SurfaceKind,
    pub title: String,
    /// Incrémentée à chaque nouveau code : la GUI recharge la page.
    pub version: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Une surface avec son code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceDoc {
    #[serde(flatten)]
    pub meta: SurfaceMeta,
    pub html: String,
}

/// Issue d'un tour d'agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed { message: String },
}

/// Consommation d'un tour. `input_tokens` inclut les tokens servis par le cache : c'est
/// la taille du contexte envoyé au modèle pour ce tour.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    /// Taille de la fenêtre de contexte du modèle, si le moteur la donne.
    pub context_window: Option<u64>,
}

impl Usage {
    /// Part de la fenêtre de contexte occupée par ce tour (0 à 1).
    pub fn context_fill(&self) -> Option<f64> {
        let window = self.context_window.filter(|window| *window > 0)?;
        Some(self.input_tokens as f64 / window as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_controls_only_the_next_one() {
        assert_eq!(Level::Assistant.child(), Some(Level::Agent));
        assert_eq!(Level::Agent.child(), Some(Level::Worker));
        assert_eq!(Level::Worker.child(), None);
    }

    #[test]
    fn workers_have_no_memory() {
        assert!(Level::Assistant.has_memory());
        assert!(Level::Agent.has_memory());
        assert!(!Level::Worker.has_memory());
    }

    #[test]
    fn serializes_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&Level::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(
            serde_json::to_string(&EngineKind::Codex).unwrap(),
            "\"codex\""
        );
    }

    #[test]
    fn text_form_matches_json_form_and_round_trips() {
        fn check<T>(values: &[T])
        where
            T: Copy + PartialEq + fmt::Debug + Serialize + FromStr + fmt::Display,
            T::Err: fmt::Debug,
        {
            for &value in values {
                let json = serde_json::to_string(&value).unwrap();
                assert_eq!(json, format!("\"{value}\""));
                assert_eq!(value.to_string().parse::<T>().unwrap(), value);
            }
        }
        check(&[Level::Assistant, Level::Agent, Level::Worker]);
        check(&[EngineKind::Claude, EngineKind::Codex]);
        check(&[AgentState::Active, AgentState::Parked, AgentState::Retired]);
        check(&[
            TaskState::Pending,
            TaskState::InProgress,
            TaskState::Blocked,
            TaskState::Done,
            TaskState::Cancelled,
            TaskState::Interrupted,
        ]);
        check(&[
            MissionState::Open,
            MissionState::Done,
            MissionState::Cancelled,
        ]);
    }

    #[test]
    fn unknown_text_is_rejected() {
        let error = "chef".parse::<Level>().unwrap_err();
        assert_eq!(error.to_string(), "Level inconnu : \"chef\"");
    }
}
