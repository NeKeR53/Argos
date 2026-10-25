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
        .with_state(supervisor)
}

/// Pas de flux SSE ni de session : le protocole autorise à les refuser ainsi.
async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle(
    State(supervisor): State<Supervisor>,
    Path(token): Path<String>,
    body: String,
) -> Response {
    let Some(caller) = supervisor.agent_for_token(&token) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(rpc_error(Value::Null, -32700, "JSON illisible")),
        )
            .into_response();
    };
    // Notifications et réponses du client : rien à rendre.
    let (Some(id), Some(method)) = (message.get("id").cloned(), message["method"].as_str()) else {
        return StatusCode::ACCEPTED.into_response();
    };
    let reply = match dispatch(&supervisor, &caller, method, &message["params"]).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, error)) => rpc_error(id, code, &error),
    };
    Json(reply).into_response()
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
            let tools: Vec<Value> = tools::tools_for(record.level)
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
