//! API WebSocket face à un superviseur sur faux CLI : aucun token.

mod common;

use argos_core::protocol::{Command, DaemonEvent, Reply, Request, ServerMessage};
use argos_core::{AgentId, EngineKind, PROTOCOL_VERSION};
use argos_daemon::api;
use common::world::{World, turn_completed, until, world};
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
async fn workspace_binding_resets_the_session_and_keeps_memory() {
    let mut w = world();
    let mut client = connect(&serve(&w).await).await;
    let project = w.home.path().join("projet");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("AGENTS.md"), "Règle du projet rattaché.").unwrap();
    let bind = |workdir: String| Command::SetWorkspace {
        agent: w.agent.id.clone(),
        workdir,
    };

    // Aucun Worker ne doit conserver le contexte du workspace précédent.
    assert!(matches!(
        ask(&mut client, 1, bind(project.to_string_lossy().into())).await,
        Reply::Error { .. }
    ));
    w.supervisor.retire(&w.worker.id).await.unwrap();
    for path in [
        "relatif".to_string(),
        project.join("absent").to_string_lossy().into(),
        project.join("AGENTS.md").to_string_lossy().into(),
    ] {
        assert!(matches!(
            ask(&mut client, 2, bind(path)).await,
            Reply::Error { .. }
        ));
    }

    w.supervisor.send(&w.agent.id, "bonjour").await.unwrap();
    turn_completed(&mut w.events, &w.agent.id).await;
    let old_session = w
        .supervisor
        .record(&w.agent.id)
        .unwrap()
        .session_id
        .unwrap();
    let memory = w.supervisor.memory().dir(&w.agent.id).join("index.md");
    std::fs::write(&memory, "Mémoire conservée du projet.").unwrap();
    match ask(&mut client, 3, bind(project.to_string_lossy().into())).await {
        Reply::Agent { agent } => {
            assert_eq!(agent.record.workdir, project);
            assert!(agent.record.session_id.is_none());
            assert!(!agent.running);
        }
        other => panic!("rattachement attendu : {other:?}"),
    }
    w.supervisor.send(&w.agent.id, "encore").await.unwrap();
    turn_completed(&mut w.events, &w.agent.id).await;
    let args = std::fs::read_to_string(project.join(".fake-args")).unwrap();
    assert!(args.contains("Règle du projet rattaché."));
    assert!(args.contains("Mémoire conservée du projet."));
    assert!(
        !args.contains("--disallowedTools"),
        "l'Agent peut exécuter directement"
    );
    assert!(!args.contains(&old_session));
    assert!(args.contains("temps ET des tokens"));
    for agent in [&w.assistant.id, &w.worker.id] {
        assert!(matches!(
            ask(
                &mut client,
                4,
                Command::SetWorkspace {
                    agent: agent.clone(),
                    workdir: project.to_string_lossy().into()
                }
            )
            .await,
            Reply::Error { .. }
        ));
    }
    w.supervisor.shutdown().await;
}

#[tokio::test]
async fn workspace_binding_does_not_interrupt_a_busy_agent() {
    let w = world();
    w.supervisor.retire(&w.worker.id).await.unwrap();
    let project = w.home.path().join("projet");
    std::fs::create_dir(&project).unwrap();
    w.supervisor.send(&w.agent.id, "SLOW").await.unwrap();
    let before = w.supervisor.record(&w.agent.id).unwrap();
    assert!(
        w.supervisor
            .set_workspace(&w.agent.id, project.to_str().unwrap())
            .await
            .is_err()
    );
    assert_eq!(w.supervisor.record(&w.agent.id).unwrap(), before);
    assert!(w.supervisor.inspect(&w.agent.id).unwrap().busy);
    w.supervisor.shutdown().await;
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

#[tokio::test]
async fn a_surface_talks_to_its_agent_through_the_console() {
    let mut w = world();
    let surface = w
        .supervisor
        .registry()
        .add_surface(
            &w.agent.id,
            argos_core::SurfaceKind::Cockpit,
            "Lecteur PDF",
            "<p>pdf</p>",
        )
        .unwrap();
    let mut client = connect(&serve(&w).await).await;

    match ask(
        &mut client,
        1,
        Command::Surfaces {
            agent: w.agent.id.clone(),
        },
    )
    .await
    {
        Reply::Surfaces { surfaces } => assert_eq!(surfaces, vec![surface.clone()]),
        other => panic!("surfaces attendues, reçu {other:?}"),
    }
    match ask(
        &mut client,
        2,
        Command::Surface {
            surface: surface.id.clone(),
        },
    )
    .await
    {
        Reply::Surface { surface: doc } => assert_eq!(doc.html, "<p>pdf</p>"),
        other => panic!("surface attendue, reçu {other:?}"),
    }

    let reply = ask(
        &mut client,
        3,
        Command::SurfaceSend {
            surface: surface.id.clone(),
            text: "page suivante".into(),
        },
    )
    .await;
    assert_eq!(
        reply,
        Reply::Done {
            affected: vec![w.agent.id.clone()]
        }
    );
    // L'input est publié avant la réponse : on le lit sur le flux du superviseur.
    let expected = format!(
        "[Depuis la surface « Lecteur PDF » ({})]\npage suivante",
        surface.id
    );
    let agent = w.agent.id.clone();
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent: a, text, .. } if *a == agent && *text == expected)
    })
    .await;

    let reply = ask(
        &mut client,
        4,
        Command::SurfaceEmit {
            surface: surface.id.clone(),
            name: "page".into(),
            payload: serde_json::json!({"n": 3}),
        },
    )
    .await;
    assert!(matches!(reply, Reply::Done { .. }));

    let reply = ask(
        &mut client,
        5,
        Command::SurfaceDelete {
            surface: surface.id.clone(),
        },
    )
    .await;
    assert!(matches!(reply, Reply::Done { .. }));
    assert!(
        w.supervisor
            .registry()
            .surfaces(&w.agent.id)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_user_changes_the_model_of_an_agent() {
    let w = world();
    let mut client = connect(&serve(&w).await).await;
    let agent = w.agent.id.clone();

    let Reply::Models { profiles, models } = ask(&mut client, 1, Command::Models).await else {
        panic!("profils attendus");
    };
    assert!(profiles.iter().any(|profile| profile.name == "léger"));
    assert!(models.iter().any(|model| model.model == "vendor/model"));
    assert!(models.iter().any(|model| model.model == "other/model"));

    // L'agent tourne : il est parké, puis reprend sur le nouveau modèle.
    w.supervisor.send(&agent, "bonjour").await.unwrap();
    assert!(w.supervisor.inspect(&agent).unwrap().running);
    let session = w.supervisor.record(&agent).unwrap().session_id;
    let reply = ask(
        &mut client,
        2,
        Command::SetModel {
            agent: agent.clone(),
            engine: EngineKind::Claude,
            model: "claude-haiku-5-5".into(),
        },
    )
    .await;
    let Reply::Agent { agent: status } = reply else {
        panic!("{reply:?}");
    };
    assert!(!status.running);
    assert_eq!(status.record.model.as_deref(), Some("claude-haiku-5-5"));
    // Le modèle est dans la chaîne de « léger » : ses replis s'appliquent.
    assert_eq!(status.record.profile.as_deref(), Some("léger"));
    // Même moteur : la conversation continue.
    assert_eq!(status.record.session_id, session);

    // Changer de moteur ouvre une session neuve ; un modèle hors profil n'a pas de profil.
    let Reply::Agent { agent: status } = ask(
        &mut client,
        3,
        Command::SetModel {
            agent: agent.clone(),
            engine: EngineKind::Codex,
            model: "gpt-autre".into(),
        },
    )
    .await
    else {
        panic!("agent attendu");
    };
    assert_eq!(status.record.engine, EngineKind::Codex);
    assert_eq!(status.record.session_id, None);
    assert_eq!(status.record.profile, None);

    // Jamais sur un Worker (D2).
    let refused = ask(
        &mut client,
        4,
        Command::SetModel {
            agent: w.worker.id.clone(),
            engine: EngineKind::Claude,
            model: "claude-haiku-5-5".into(),
        },
    )
    .await;
    assert!(matches!(refused, Reply::Error { .. }), "{refused:?}");
}
