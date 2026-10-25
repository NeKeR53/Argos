//! API WebSocket du daemon pour la GUI (protocole : `argos_core::protocol`). Locale et
//! protégée par un jeton : elle pilote des agents qui exécutent des commandes.

use crate::supervisor::Supervisor;
use argos_core::protocol::{Command, Reply, Request, ServerMessage};
use argos_core::{AgentId, Level, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use std::fmt::Display;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as Handshake, Response,
};
use tokio_tungstenite::tungstenite::http::StatusCode;

/// Accepte les connexions jusqu'à une erreur d'écoute. Chaque client fournit le jeton
/// dans l'URL : `ws://127.0.0.1:7878/?token=…`.
pub async fn serve(
    listener: TcpListener,
    supervisor: Supervisor,
    token: String,
) -> std::io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(connection(stream, supervisor.clone(), token.clone()));
    }
}

async fn connection(stream: TcpStream, supervisor: Supervisor, token: String) {
    // Signature imposée par tungstenite, erreur volumineuse comprise.
    #[allow(clippy::result_large_err)]
    let authorize = |handshake: &Handshake, response: Response| {
        if token_in(handshake.uri().query()) == Some(token.as_str()) {
            return Ok(response);
        }
        let mut refusal = ErrorResponse::new(Some("jeton absent ou invalide".into()));
        *refusal.status_mut() = StatusCode::UNAUTHORIZED;
        Err(refusal)
    };
    let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, authorize).await else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    let (outbox, mut queued) = mpsc::unbounded_channel::<ServerMessage>();
    let mut events = supervisor.subscribe();
    let _ = outbox.send(ServerMessage::Hello {
        protocol: PROTOCOL_VERSION,
    });

    // Un seul écrivain : réponses et événements partent dans l'ordre où ils arrivent.
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                queued = queued.recv() => match queued {
                    Some(message) => message,
                    None => break,
                },
                event = events.recv() => match event {
                    Ok(event) => ServerMessage::Event { event },
                    // Client trop lent : il a perdu des événements et se resynchronise
                    // avec `tree`.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            let text = serde_json::to_string(&message).expect("message sérialisable");
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(frame)) = source.next().await {
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => break,
            _ => continue,
        };
        match serde_json::from_str::<Request>(text.as_str()) {
            // Chaque commande dans sa tâche : un réveil lent ne retarde pas un stop.
            Ok(request) => {
                let supervisor = supervisor.clone();
                let outbox = outbox.clone();
                tokio::spawn(async move {
                    let reply = handle(&supervisor, request.command).await;
                    let _ = outbox.send(ServerMessage::Reply {
                        id: request.id,
                        reply,
                    });
                });
            }
            Err(error) => {
                let _ = outbox.send(ServerMessage::Reply {
                    id: 0,
                    reply: Reply::Error {
                        message: format!("requête illisible : {error}"),
                    },
                });
            }
        }
    }
    writer.abort();
}

fn token_in(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
}

async fn handle(supervisor: &Supervisor, command: Command) -> Reply {
    run(supervisor, command)
        .await
        .unwrap_or_else(|message| Reply::Error { message })
}

async fn run(supervisor: &Supervisor, command: Command) -> Result<Reply, String> {
    let done = |affected| Reply::Done { affected };
    Ok(match command {
        Command::Tree => Reply::Tree {
            agents: supervisor.tree().map_err(said)?,
        },
        Command::Inspect { agent } => Reply::Agent {
            agent: supervisor.inspect(&agent).map_err(said)?,
        },
        Command::Board { agent } => Reply::Board {
            board: supervisor.registry().board(&agent).map_err(said)?,
        },
        Command::Send { agent, text } => {
            addressable(supervisor, &agent)?;
            supervisor.send(&agent, text).await.map_err(said)?;
            done(vec![agent])
        }
        Command::Interrupt { agent } => {
            addressable(supervisor, &agent)?;
            supervisor.interrupt(&agent).await.map_err(said)?;
            done(vec![agent])
        }
        Command::Park { agent } => {
            addressable(supervisor, &agent)?;
            supervisor.park(&agent).await.map_err(said)?;
            done(vec![agent])
        }
        Command::Stop { agent } => {
            addressable(supervisor, &agent)?;
            done(supervisor.stop(&agent).await.map_err(said)?)
        }
        Command::Retire { agent } => {
            addressable(supervisor, &agent)?;
            done(supervisor.retire(&agent).await.map_err(said)?)
        }
        Command::PanicStop => done(supervisor.panic_stop().await.map_err(said)?),
    })
}

/// L'utilisateur s'adresse à l'Assistant et aux Agents, jamais à un Worker (D2).
fn addressable(supervisor: &Supervisor, agent: &AgentId) -> Result<(), String> {
    if supervisor.record(agent).map_err(said)?.level == Level::Worker {
        return Err(format!(
            "{agent} est un Worker : l'utilisateur passe par son Agent"
        ));
    }
    Ok(())
}

fn said(error: impl Display) -> String {
    error.to_string()
}
