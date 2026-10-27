//! Serveur MCP d'Argos : HTTP « streamable », réponses JSON simples, sans session ni flux
//! SSE. Chaque session d'agent reçoit une URL `/mcp/<jeton>` : le jeton identifie l'appelant
//! et fixe les outils qu'il voit (voir `tools`). Seul le sous-ensemble utile du protocole
//! est servi : `initialize`, `ping`, `tools/list`, `tools/call`.

use crate::supervisor::Supervisor;
use crate::tools;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// Version proposée quand le client n'en demande pas.
const DEFAULT_PROTOCOL: &str = "2025-06-18";

pub async fn serve(listener: TcpListener, supervisor: Supervisor) -> std::io::Result<()> {
    axum::serve(listener, router(supervisor)).await
}

pub fn router(supervisor: Supervisor) -> Router {
    Router::new()
        .route(
            "/mcp/{token}",
            post(handle).get(no_stream).delete(no_stream),
        )
        .route("/hooks/{trigger}/{secret}", post(hook))
        .with_state(supervisor)
}

/// Webhook entrant (D30) : le corps de l'appel accompagne la consigne du déclencheur.
/// Un déclencheur inconnu, suspendu, d'une autre nature ou un mauvais secret : 404.
async fn hook(
    State(supervisor): State<Supervisor>,
    Path((trigger, secret)): Path<(String, String)>,
    body: String,
) -> StatusCode {
    let Ok(found) = supervisor.registry().trigger(&trigger) else {
        return StatusCode::NOT_FOUND;
    };
    let authorized = found.kind == argos_core::TriggerKind::Webhook
        && found.enabled
        && found.secret.as_deref() == Some(secret.as_str());
    if !authorized {
        return StatusCode::NOT_FOUND;
    }
    let mut body: String = body.chars().take(MAX_HOOK_BODY).collect();
    if body.trim().is_empty() {
        body = "(appel sans contenu)".into();
    }
    crate::events::fire(&supervisor, &found, Some(format!("Reçu :\n{body}")));
    StatusCode::ACCEPTED
}

/// Caractères du corps d'un webhook transmis à l'agent, au plus.
const MAX_HOOK_BODY: usize = 20_000;

/// Pas de flux SSE ni de session : le protocole autorise à les refuser ainsi.
pub(crate) async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle(
    State(supervisor): State<Supervisor>,
    Path(token): Path<String>,
    body: String,
) -> Response {
    let (status, reply) = answer(&supervisor, &token, &body).await;
    match reply {
        Some(reply) => (status, Json(reply)).into_response(),
        None => status.into_response(),
    }
}

/// Répond à une requête MCP d'une session ; aussi appelé pour les sessions des autres
/// nœuds, dont le nœud relaie les requêtes au hub (`node`).
pub async fn answer(
    supervisor: &Supervisor,
    token: &str,
    body: &str,
) -> (StatusCode, Option<Value>) {
    let Some(caller) = supervisor.agent_for_token(token) else {
        return (StatusCode::NOT_FOUND, None);
    };
    let Ok(message) = serde_json::from_str::<Value>(body) else {
        return (
            StatusCode::BAD_REQUEST,
            Some(rpc_error(Value::Null, -32700, "JSON illisible")),
        );
    };
    // Notifications et réponses du client : rien à rendre.
    let (Some(id), Some(method)) = (message.get("id").cloned(), message["method"].as_str()) else {
        return (StatusCode::ACCEPTED, None);
    };
    let reply = match dispatch(supervisor, &caller, method, &message["params"]).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, error)) => rpc_error(id, code, &error),
    };
    (StatusCode::OK, Some(reply))
}

async fn dispatch(
    supervisor: &Supervisor,
    caller: &argos_core::AgentId,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    let internal = |error: crate::supervisor::SupervisorError| (-32603, error.to_string());
    match method {
        // Les quatre méthodes servies n'ont pas changé d'une version à l'autre : on
        // accepte celle du client.
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "argos", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Outils d'Argos pour piloter tes enfants dans la hiérarchie Assistant → Agents → Workers et rendre compte à ton parent.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let record = supervisor.record(caller).map_err(internal)?;
            let tools: Vec<Value> = tools::tools_for(record.level, supervisor.policy())
                .into_iter()
                .map(|tool| {
                    json!({"name": tool.name, "description": tool.description, "inputSchema": tool.schema})
                })
                .collect();
            Ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let record = supervisor.record(caller).map_err(internal)?;
            let name = params["name"]
                .as_str()
                .ok_or((-32602, "nom d'outil manquant".to_string()))?;
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            // Une erreur d'outil est un résultat que l'agent lit, pas une erreur de protocole.
            let (text, is_error) = match tools::call(supervisor, &record, name, &arguments).await {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        _ => Err((-32601, format!("méthode inconnue : {method}"))),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
