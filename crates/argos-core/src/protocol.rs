//! Protocole de l'API WebSocket entre la GUI et le daemon. Chaque message est un objet
//! JSON sur une trame texte. Le client envoie des [`Request`] ; le daemon répond par
//! [`ServerMessage::Reply`] (même `id`) et pousse en continu des [`ServerMessage::Event`].

use crate::{AgentId, AgentRecord, Board, TurnOutcome, Usage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Choisi par le client, rendu dans la réponse.
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

/// Ce que l'utilisateur peut demander. Les Workers ne reçoivent jamais d'ordre de
/// l'utilisateur (D2) : seul le panic stop les atteint, par cascade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// L'arbre courant (agents non retirés).
    Tree,
    Inspect {
        agent: AgentId,
    },
    /// Le board d'un Agent (missions, tâches).
    Board {
        agent: AgentId,
    },
    /// Message à un agent ; un agent parké est réveillé.
    Send {
        agent: AgentId,
        text: String,
    },
    Interrupt {
        agent: AgentId,
    },
    /// Arrêt propre du process ; l'agent se reprendra plus tard.
    Park {
        agent: AgentId,
    },
    /// Arrêt dur de l'agent et de tout son sous-arbre (D14).
    Stop {
        agent: AgentId,
    },
    /// Retrait définitif de l'agent et de son sous-arbre.
    Retire {
        agent: AgentId,
    },
    /// Arrêt dur de tout l'arbre (D16).
    PanicStop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Premier message de chaque connexion.
    Hello {
        protocol: u32,
    },
    Reply {
        id: u64,
        reply: Reply,
    },
    Event {
        event: DaemonEvent,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Tree {
        agents: Vec<AgentStatus>,
    },
    Agent {
        agent: AgentStatus,
    },
    Board {
        board: Board,
    },
    /// Commande exécutée ; `affected` liste les agents touchés (stop, retrait…).
    Done {
        affected: Vec<AgentId>,
    },
    Error {
        message: String,
    },
}

/// Un agent du registre, avec son état d'exécution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    #[serde(flatten)]
    pub record: AgentRecord,
    /// Son process tourne.
    pub running: bool,
    /// Un tour est en cours.
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// Ce qu'un agent vient de recevoir : message de l'utilisateur, mission de
    /// l'Assistant, rapport d'un enfant, question d'un pair, consigne d'Argos.
    Input {
        agent: AgentId,
        from: Origin,
        text: String,
    },
    /// Ce que l'agent a dit.
    Message {
        agent: AgentId,
        text: String,
    },
    Tool {
        agent: AgentId,
        name: String,
    },
    TurnCompleted {
        agent: AgentId,
        outcome: TurnOutcome,
        usage: Option<Usage>,
    },
    /// Le process de l'agent s'est terminé (parking, stop ou panne).
    Exited {
        agent: AgentId,
        code: Option<i32>,
    },
    /// L'agent a changé dans le registre ou en exécution.
    AgentUpdated {
        agent: AgentStatus,
    },
    /// Information courte du daemon pour l'utilisateur (repli de modèle, plafond…).
    Notice {
        agent: Option<AgentId>,
        text: String,
    },
}

/// Expéditeur d'un message remis à un agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// L'utilisateur, depuis la console ou la voix.
    User,
    /// Un autre agent : parent, enfant ou pair (D40).
    Agent { agent: AgentId },
    /// Argos lui-même : compaction, reprise après un repli.
    Argos,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inputs_say_who_sent_them() {
        let event = DaemonEvent::Input {
            agent: AgentId::new("mail"),
            from: Origin::Agent {
                agent: AgentId::new("notes"),
            },
            text: "[Question de …]".into(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "input", "agent": "mail", "from": {"kind": "agent", "agent": "notes"}, "text": "[Question de …]"})
        );
        assert_eq!(
            serde_json::to_value(Origin::User).unwrap(),
            json!({"kind": "user"})
        );
    }

    #[test]
    fn requests_are_flat_objects() {
        let request: Request = serde_json::from_value(json!({
            "id": 7, "command": "send", "agent": "a1", "text": "bonjour"
        }))
        .unwrap();
        assert_eq!(
            request,
            Request {
                id: 7,
                command: Command::Send {
                    agent: AgentId::new("a1"),
                    text: "bonjour".into()
                }
            }
        );
        let panic: Request =
            serde_json::from_value(json!({"id": 8, "command": "panic_stop"})).unwrap();
        assert_eq!(panic.command, Command::PanicStop);
    }

    #[test]
    fn server_messages_are_tagged() {
        let reply = ServerMessage::Reply {
            id: 7,
            reply: Reply::Error {
                message: "agent inconnu".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            json!({"type": "reply", "id": 7, "reply": {"kind": "error", "message": "agent inconnu"}})
        );
        let event = ServerMessage::Event {
            event: DaemonEvent::TurnCompleted {
                agent: AgentId::new("a1"),
                outcome: TurnOutcome::Failed {
                    message: "quota".into(),
                },
                usage: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "event", "event": {
                "type": "turn_completed", "agent": "a1",
                "outcome": {"status": "failed", "message": "quota"}, "usage": null
            }})
        );
    }
}
