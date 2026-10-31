//! Protocole de l'API WebSocket entre la GUI et le daemon. Chaque message est un objet
//! JSON sur une trame texte. Le client envoie des [`Request`] ; le daemon répond par
//! [`ServerMessage::Reply`] (même `id`) et pousse en continu des [`ServerMessage::Event`].

use crate::{
    AgentId, AgentRecord, Board, EngineKind, Escalation, SurfaceDoc, SurfaceMeta, Trigger,
    TurnOutcome, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Capacités configurées pour un Agent ou héritées par un Worker.
    Capabilities {
        agent: AgentId,
    },
    /// Le board d'un Agent (missions, tâches).
    Board {
        agent: AgentId,
    },
    /// Passe une mission en tête du board de son Agent ; avec la préemption active, ses
    /// Workers moins prioritaires sont mis au repos (D14).
    PrioritizeMission {
        mission: String,
    },
    /// Les escalades ouvertes, puis les dernières résolues (D12).
    Escalations,
    /// Répond à une escalade : la réponse part directement à l'Agent qui attend.
    AnswerEscalation {
        escalation: String,
        answer: String,
    },
    /// Écarte une escalade sans répondre ; l'Agent est prévenu de ne plus l'attendre.
    DismissEscalation {
        escalation: String,
    },
    /// Message à un agent ; un agent parké est réveillé. Les pièces jointes partent avec :
    /// les images en entrée native du moteur, les fichiers texte dans le message, les
    /// autres enregistrés sur le hub avec leur chemin.
    Send {
        agent: AgentId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
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
    /// Les surfaces d'un agent (D26).
    Surfaces {
        agent: AgentId,
    },
    /// Une surface avec son code, pour l'afficher.
    Surface {
        surface: String,
    },
    /// Texte envoyé depuis une surface (`argos.send`) : remis à son agent.
    SurfaceSend {
        surface: String,
        text: String,
    },
    /// Événement structuré émis par une surface (`argos.emit`) : remis à son agent.
    SurfaceEmit {
        surface: String,
        name: String,
        payload: Value,
    },
    /// Appel déterministe d'une fonction publiée par le propriétaire de la surface.
    SurfaceCall {
        surface: String,
        function: String,
        arguments: Value,
    },
    /// L'utilisateur ferme définitivement une surface.
    SurfaceDelete {
        surface: String,
    },
    /// Les déclencheurs dont un agent est propriétaire ou cible (D29).
    Triggers {
        agent: AgentId,
    },
    /// Actionne un déclencheur (l'interrupteur, ou n'importe lequel pour l'essayer).
    FireTrigger {
        trigger: String,
    },
    /// Active ou suspend un déclencheur.
    SetTrigger {
        trigger: String,
        enabled: bool,
    },
    DeleteTrigger {
        trigger: String,
    },
    /// Les nœuds déclarés et l'état de leur connexion (D32).
    Nodes,
    /// Les profils de la politique de modèles et leurs modèles, pour choisir.
    Models,
    /// Change le moteur et le modèle d'un agent (override de l'utilisateur, D21). Sa
    /// session en cours est parkée ; il reprend sur le nouveau modèle au prochain message.
    SetModel {
        agent: AgentId,
        engine: EngineKind,
        model: String,
    },
    /// Rattache un Agent à un workspace ; sa prochaine session repart de sa mémoire.
    SetWorkspace {
        agent: AgentId,
        workdir: String,
    },
    /// Les synchros de workspace et leur état (D44).
    Syncs,
    /// Ouvre une synchro entre le workspace d'un Agent placé sur un nœud et un dossier du
    /// hub, vide ou absent. `excludes` : motifs `.gitignore` en plus de ceux du projet.
    OpenSync {
        agent: AgentId,
        local: String,
        #[serde(default)]
        excludes: Vec<String>,
    },
    /// Ferme une synchro ; les deux copies restent en place.
    CloseSync {
        sync: String,
    },
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
    Capabilities {
        config: Value,
        argos_enabled: bool,
    },
    Surfaces {
        surfaces: Vec<SurfaceMeta>,
    },
    Surface {
        surface: SurfaceDoc,
    },
    FunctionResult {
        result: Value,
    },
    Triggers {
        triggers: Vec<Trigger>,
    },
    Escalations {
        escalations: Vec<Escalation>,
    },
    Nodes {
        nodes: Vec<NodeStatus>,
    },
    Syncs {
        syncs: Vec<SyncStatus>,
    },
    Sync {
        sync: SyncStatus,
    },
    Models {
        profiles: Vec<ProfileInfo>,
        models: Vec<ModelRef>,
    },
    /// Commande exécutée ; `affected` liste les agents touchés (stop, retrait…).
    Done {
        affected: Vec<AgentId>,
    },
    Error {
        message: String,
    },
}

/// Fichier joint à un message de l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Nom du fichier, tel que l'utilisateur l'a choisi.
    pub name: String,
    /// Type MIME annoncé par la console (`image/png`, `application/pdf`…).
    pub media_type: String,
    /// Contenu encodé en base64.
    pub data: String,
}

/// Un profil de la politique de modèles (D22) : sa chaîne, premier choix d'abord.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub name: String,
    pub description: String,
    pub models: Vec<ModelRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub engine: EngineKind,
    pub model: String,
}

/// Un nœud déclaré dans la configuration du hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
    pub name: String,
    pub connected: bool,
    /// Dernière erreur de connexion, tant que le nœud est injoignable.
    pub error: Option<String>,
}

/// Une synchro de workspace (D44) et où elle en est.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub id: String,
    pub agent: AgentId,
    pub node: String,
    /// Copie locale sur le hub.
    pub local: String,
    pub state: SyncState,
    /// Dernière erreur, tant qu'elle dure.
    pub error: Option<String>,
    /// Fichiers suivis (dans l'état commun).
    pub files: usize,
    /// Versions du hub mises de côté lors des derniers conflits (`<nom>.conflit-hub`).
    pub conflicts: Vec<String>,
    /// Fichiers qui ne voyagent pas : trop gros.
    pub held: Vec<String>,
    /// Dernière passe complète réussie, en secondes depuis l'époque.
    pub synced_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// Les deux côtés sont d'accord.
    Idle,
    /// Parcours ou copie en cours.
    Syncing,
    /// Nœud injoignable : la copie locale reste utilisable, rapprochement au retour.
    Offline,
    Error,
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
    /// Fragment du message que l'agent est en train d'écrire, pour le faire parler au fil
    /// de l'eau (D37). Le message complet suit avec `message`.
    MessageDelta {
        agent: AgentId,
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
    ToolDetail {
        agent: AgentId,
        id: String,
        name: String,
        input: Option<String>,
        output: Option<String>,
        finished: bool,
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
    /// Une surface a été créée, mise à jour (nouvelle `version`) ou supprimée.
    SurfaceUpdated {
        surface: SurfaceMeta,
        deleted: bool,
    },
    /// Données poussées par l'agent vers son cockpit (`argos.on('data')`).
    SurfaceData {
        surface: String,
        data: Value,
    },
    /// Une escalade a été ouverte, répondue ou écartée.
    EscalationUpdated {
        escalation: Escalation,
    },
    /// Une synchro de workspace a changé d'état, ou a été fermée.
    SyncUpdated {
        sync: SyncStatus,
        closed: bool,
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
                    text: "bonjour".into(),
                    attachments: vec![],
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
