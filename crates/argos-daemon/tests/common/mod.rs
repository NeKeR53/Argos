//! Aides partagées par les tests d'intégration.
#![allow(dead_code)]

pub mod world;

use argos_daemon::engine::{EngineEvent, Session, TurnOutcome};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub async fn next(session: &mut Session, limit: Duration) -> EngineEvent {
    tokio::time::timeout(limit, session.next_event())
        .await
        .expect("aucun événement dans le délai")
        .expect("flux d'événements fermé")
}

/// Lit les événements jusqu'à la fin du tour.
pub async fn turn(session: &mut Session, limit: Duration) -> (Vec<EngineEvent>, TurnOutcome) {
    let mut seen = vec![];
    loop {
        match next(session, limit).await {
            EngineEvent::TurnCompleted { outcome, .. } => return (seen, outcome),
            exited @ EngineEvent::Exited { .. } => {
                panic!("process terminé en plein tour : {exited:?}")
            }
            event => seen.push(event),
        }
    }
}

pub async fn exit_code(session: &mut Session, limit: Duration) -> Option<i32> {
    loop {
        if let EngineEvent::Exited { code, .. } = next(session, limit).await {
            return code;
        }
    }
}

pub fn message(text: &str) -> EngineEvent {
    EngineEvent::AgentMessage { text: text.into() }
}

/// Texte de tous les messages de l'agent, concaténé.
pub fn said(events: &[EngineEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::AgentMessage { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Appelle un outil MCP au nom d'un agent ; rend son texte et s'il s'agit d'une erreur.
pub async fn call_tool(
    w: &world::World,
    agent: &argos_core::AgentId,
    name: &str,
    arguments: Value,
) -> (String, bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(argos_daemon::mcp::serve(listener, w.supervisor.clone()));
    let token = w.supervisor.mcp_token(agent);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).to_string();
    let mut stream = TcpStream::connect(&address).await.unwrap();
    let request = format!(
        "POST /mcp/{token} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let reply: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    let result = &reply["result"];
    (
        result["content"][0]["text"].as_str().unwrap().into(),
        result["isError"].as_bool().unwrap(),
    )
}
