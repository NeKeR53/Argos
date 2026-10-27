//! Fonctions déterministes publiées par les Agents, appelables sans tour de modèle.

use crate::AgentId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Description publique : la requête et ses références de secrets restent au daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionInfo {
    pub agent: AgentId,
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedFunction {
    pub agent: AgentId,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub request: FunctionRequest,
}

impl PublishedFunction {
    pub fn info(&self) -> FunctionInfo {
        FunctionInfo {
            agent: self.agent.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }
}

/// Requête HTTP à destination fixe ; les paramètres du chemin/query sont encodés.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    10_000
}
