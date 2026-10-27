//! Multi-nœud (D32) : le hub garde le registre, l'Assistant et le plan de contrôle ; un
//! nœud distant ne fait qu'exécuter les sessions des agents qui y sont placés.
//!
//! Le hub ouvre vers chaque nœud une connexion WebSocket (directe, ou dans un tunnel
//! SSH, voir `transport`) qui porte tout : lancement et pilotage des sessions, leurs
//! événements, et les requêtes MCP de leurs agents, que le nœud sert sur un mandataire
//! local et relaie au hub. Le nœud n'ouvre donc aucune connexion vers le hub.
//!
//! - [`link`] : côté hub, une connexion par nœud, rétablie quand elle tombe.
//! - [`runner`] : côté nœud, le daemon lancé en mode `node`.
//!
//! La mémoire d'un agent vit sur son nœud (D23), avec son git d'annulation. Le hub y
//! envoie chaque opération et en tient un miroir sans historique (D25), mis à jour par
//! le nœud après chaque écriture et à chaque connexion : quand le nœud est injoignable,
//! le hub lit le miroir et refuse les écritures.

pub mod link;
pub mod runner;
pub mod service;
pub mod transport;

pub use link::Nodes;

use crate::engine::{Access, EngineEvent};
use crate::memory::{MemoryOp, MemoryOutput};
use argos_core::{AgentId, EngineKind, FunctionRequest};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version du protocole entre hub et nœud.
pub const NODE_PROTOCOL: u32 = 4;

/// Ce qu'il faut au nœud pour lancer une session ; il complète le reste lui-même
/// (binaires, `CODEX_HOME`, adresse de son mandataire MCP).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteSpec {
    pub agent: AgentId,
    pub engine: EngineKind,
    /// Relatif : à partir du dossier de données du nœud.
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub instructions: Option<String>,
    pub resume: Option<String>,
    pub access: Access,
    /// Jeton MCP de la session ; sans lui, l'agent tourne sans les outils d'Argos.
    pub mcp_token: Option<String>,
}

/// Du hub vers le nœud. `session` est un numéro choisi par le hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToNode {
    Spawn {
        session: u64,
        spec: RemoteSpec,
    },
    Send {
        session: u64,
        text: String,
    },
    Interrupt {
        session: u64,
    },
    Park {
        session: u64,
    },
    Kill {
        session: u64,
    },
    /// Opération sur la mémoire d'un agent du nœud, qui y vit (D23).
    Memory {
        call: u64,
        agent: AgentId,
        op: MemoryOp,
    },
    /// Fonction publiée : exécutée ici, même si son Agent est parké.
    Function {
        call: u64,
        agent: AgentId,
        request: FunctionRequest,
        arguments: serde_json::Value,
    },
    CancelFunction {
        call: u64,
    },
    /// Réponse du hub à une requête MCP relayée ; `body` vide quand il n'y a rien à rendre.
    McpReply {
        call: u64,
        status: u16,
        body: String,
    },
}

/// Du nœud vers le hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromNode {
    Hello {
        node: String,
        protocol: u32,
    },
    Spawned {
        session: u64,
        session_id: String,
    },
    SpawnFailed {
        session: u64,
        error: String,
    },
    /// Événement d'une session ; après `exited`, plus rien ne vient de cette session.
    Event {
        session: u64,
        event: EngineEvent,
    },
    MemoryReply {
        call: u64,
        result: Result<MemoryOutput, String>,
    },
    FunctionReply {
        call: u64,
        result: Result<serde_json::Value, String>,
    },
    /// Copie complète de la mémoire d'un agent, pour le miroir du hub : après chaque
    /// écriture, et pour toutes les mémoires à chaque connexion.
    MemorySnapshot {
        agent: AgentId,
        files: Vec<(String, String)>,
    },
    /// Requête MCP d'un agent du nœud, à servir par le hub.
    McpCall {
        call: u64,
        token: String,
        body: String,
    },
}

/// Jeton d'une URL `…/?token=…`.
pub(crate) fn token_in(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
}
