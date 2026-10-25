//! API WebSocket face à un superviseur sur faux CLI : aucun token.

mod common;

use argos_core::protocol::{Command, DaemonEvent, Reply, Request, ServerMessage};
use argos_core::{AgentId, PROTOCOL_VERSION};
use argos_daemon::api;
use common::world::{World, world};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const TOKEN: &str = "jeton-de-test";

/// Lance l'API sur un port libre ; rend son adresse.
async fn serve(world: &World) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(api::serve(listener, world.supervisor.clone(), TOKEN.into()));
    format!("ws://{address}")
}

async fn connect(address: &str) -> Client {
    let (mut client, _) = tokio_tungstenite::connect_async(format!("{address}/?token={TOKEN}"))
        .await
        .unwrap();
    assert_eq!(
        receive(&mut client).await,
        ServerMessage::Hello {
            protocol: PROTOCOL_VERSION
        }
    );
    client
}

async fn receive(client: &mut Client) -> ServerMessage {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("aucun message dans le délai")
            .expect("connexion fermée")
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

/// Envoie une commande et rend sa réponse, en sautant les événements intercalés.
async fn ask(client: &mut Client, id: u64, command: Command) -> Reply {
    let request = serde_json::to_string(&Request { id, command }).unwrap();
    client.send(Message::text(request)).await.unwrap();
    loop {
        if let ServerMessage::Reply { id: got, reply } = receive(client).await
            && got == id
        {
            return reply;
        }
    }
}

#[tokio::test]
async fn a_wrong_token_is_refused() {
    let w = world();
    let address = serve(&w).await;
    assert!(
        tokio_tungstenite::connect_async(format!("{address}/?token=faux"))
            .await
            .is_err()
    );
    assert!(tokio_tungstenite::connect_async(address).await.is_err());
}

#[tokio::test]
async fn the_tree_lists_every_living_agent() {
    let w = world();
    let mut client = connect(&serve(&w).await).await;
    match ask(&mut client, 1, Command::Tree).await {
        Reply::Tree { agents } => {
            let ids: Vec<AgentId> = agents.into_iter().map(|status| status.record.id).collect();
            assert_eq!(
                ids,
                vec![
                    w.assistant.id.clone(),
                    w.agent.id.clone(),
                    w.worker.id.clone()
                ]
            );
        }
        other => panic!("arbre attendu, reçu {other:?}"),
    }
}

#[tokio::test]
async fn a_message_wakes_the_agent_and_its_events_are_streamed() {
    let w = world();
    let mut client = connect(&serve(&w).await).await;
    let assistant = w.assistant.id.clone();

    let reply = ask(
        &mut client,
        1,
        Command::Send {
            agent: assistant.clone(),
            text: "bonjour".into(),
        },
    )
    .await;
    assert_eq!(
        reply,
        Reply::Done {
            affected: vec![assistant.clone()]
        }
    );

    loop {
        if let ServerMessage::Event {
            event: DaemonEvent::Message { agent, text },
        } = receive(&mut client).await
        {
            assert_eq!((agent, text.as_str()), (assistant, "reply 1"));
            break;
        }
    }
}

#[tokio::test]
async fn the_user_never_addresses_a_worker() {
    let w = world();
    let mut client = connect(&serve(&w).await).await;
    let worker = w.worker.id.clone();
    for (id, command) in [
        (
            1,
            Command::Send {
                agent: worker.clone(),
                text: "bonjour".into(),
            },
        ),
        (
            2,
            Command::Stop {
                agent: worker.clone(),
            },
        ),
        (
            3,
            Command::Retire {
                agent: worker.clone(),
            },
        ),
    ] {
        assert!(matches!(
            ask(&mut client, id, command).await,
            Reply::Error { .. }
        ));
    }
    // Lire son état reste permis.
    assert!(matches!(
        ask(&mut client, 4, Command::Inspect { agent: worker }).await,
        Reply::Agent { .. }
    ));
}

#[tokio::test]
async fn panic_stop_reaches_workers_through_the_cascade() {
    let w = world();
    let mut client = connect(&serve(&w).await).await;
    w.supervisor.send(&w.worker.id, "SLOW").await.unwrap();

    let reply = ask(&mut client, 1, Command::PanicStop).await;
    assert_eq!(
        reply,
        Reply::Done {
            affected: vec![w.worker.id.clone()]
        }
    );
    assert!(!w.supervisor.inspect(&w.worker.id).unwrap().running);
}

#[tokio::test]
async fn the_board_of_an_agent_can_be_read() {
    let w = world();
    let mission = w
        .supervisor
        .registry()
        .add_mission(&w.agent.id, "éclairage")
        .unwrap();
    let mut client = connect(&serve(&w).await).await;
    match ask(
        &mut client,
        1,
        Command::Board {
            agent: w.agent.id.clone(),
        },
    )
    .await
    {
        Reply::Board { board } => assert_eq!(board.missions, vec![mission]),
        other => panic!("board attendu, reçu {other:?}"),
    }
    // L'Assistant n'a pas de board.
    let reply = ask(
        &mut client,
        2,
        Command::Board {
            agent: w.assistant.id.clone(),
        },
    )
    .await;
    assert!(matches!(reply, Reply::Error { .. }));
}

#[tokio::test]
async fn errors_come_back_as_replies() {
    let w = world();
    let mut client = connect(&serve(&w).await).await;

    let reply = ask(
        &mut client,
        1,
        Command::Inspect {
            agent: AgentId::new("fantome"),
        },
    )
    .await;
    assert!(matches!(reply, Reply::Error { .. }));

    client.send(Message::text("pas du json")).await.unwrap();
    match receive(&mut client).await {
        ServerMessage::Reply {
            id: 0,
            reply: Reply::Error { message },
        } => {
            assert!(message.starts_with("requête illisible"));
        }
        other => panic!("erreur attendue, reçu {other:?}"),
    }
}
