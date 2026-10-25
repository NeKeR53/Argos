//! Aides partagées par les tests d'intégration.
#![allow(dead_code)]

pub mod world;

use argos_daemon::engine::{EngineEvent, Session, TurnOutcome};
use std::time::Duration;

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
