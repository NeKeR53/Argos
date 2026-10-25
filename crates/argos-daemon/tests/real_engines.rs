//! Les vrais moteurs, avec des modèles bon marché. Consomment des tokens, donc ignorés
//! par défaut : `cargo test -p argos-daemon --test real_engines -- --ignored`.

mod common;

use argos_core::protocol::DaemonEvent;
use argos_core::{Charter, EngineKind, Level};
use argos_daemon::engine::{Session, SessionSpec, TurnOutcome, prepare_codex_home};
use argos_daemon::registry::{NewAgent, Registry};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use common::said;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;

const LIMIT: Duration = Duration::from_secs(180);

/// Le cycle de vie complet d'un agent : mémoire entre deux tours, interruption,
/// parking, puis reprise dans un nouveau process qui se souvient.
async fn full_lifecycle(mut spec: SessionSpec) {
    let mut session = Session::spawn(spec.clone()).await.unwrap();

    session
        .send("Retiens le nombre 42. Réponds seulement : ok")
        .unwrap();
    assert_eq!(
        common::turn(&mut session, LIMIT).await.1,
        TurnOutcome::Completed
    );

    session
        .send("Écris les nombres de 1 à 400, un par ligne, sans commentaire.")
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    session.interrupt().unwrap();
    assert_eq!(
        common::turn(&mut session, LIMIT).await.1,
        TurnOutcome::Interrupted
    );

    // Un parking réussi : le process sort de lui-même, sans être tué. Son code ne dit
    // rien de plus (claude rend le statut du dernier tour, ici interrompu).
    let session_id = session.session_id().to_string();
    session.park().unwrap();
    assert!(common::exit_code(&mut session, LIMIT).await.is_some());

    spec.resume = Some(session_id.clone());
    let mut resumed = Session::spawn(spec).await.unwrap();
    assert_eq!(resumed.session_id(), session_id);
    resumed
        .send("Quel nombre devais-tu retenir ? Réponds seulement par le nombre.")
        .unwrap();
    let (events, outcome) = common::turn(&mut resumed, LIMIT).await;
    assert_eq!(outcome, TurnOutcome::Completed);
    assert!(said(&events).contains("42"), "réponse : {events:?}");
    resumed.park().unwrap();
}

/// Un Assistant sur ce moteur, avec un Agent « Domotique » et le serveur MCP d'Argos :
/// il doit appeler `list_agents` et en tirer le bon nombre.
async fn calls_argos_tools(engine: EngineKind, model: &str) {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::open_in_memory().unwrap();
    let assistant = registry
        .ensure_assistant(engine, Some(model.into()), home.path().join("assistant"))
        .unwrap();
    registry
        .create(NewAgent {
            parent: assistant.id.clone(),
            level: Level::Agent,
            engine,
            model: Some(model.into()),
            workdir: home.path().join("domotique"),
            charter: Some(Charter {
                name: "Domotique".into(),
                scope: "la maison".into(),
            }),
        })
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_url = format!("http://{}", listener.local_addr().unwrap());
    let user_codex_home = PathBuf::from(std::env::var("HOME").unwrap()).join(".codex");
    let supervisor = Supervisor::new(
        registry,
        SupervisorSettings {
            home: home.path().to_path_buf(),
            user_codex_home,
            programs: HashMap::new(),
            mcp_url: Some(mcp_url),
        },
    );
    tokio::spawn(argos_daemon::mcp::serve(listener, supervisor.clone()));
    let mut events = supervisor.subscribe();

    supervisor
        .send(
            &assistant.id,
            "Appelle l'outil list_agents (serveur MCP argos), puis réponds seulement par le nombre d'Agents.",
        )
        .await
        .unwrap();

    let (mut tools, mut said) = (vec![], String::new());
    let outcome = tokio::time::timeout(LIMIT, async {
        loop {
            match events.recv().await.unwrap() {
                DaemonEvent::Tool { name, .. } => tools.push(name),
                DaemonEvent::Message { text, .. } => said.push_str(&text),
                DaemonEvent::TurnCompleted { outcome, .. } => return outcome,
                _ => {}
            }
        }
    })
    .await
    .expect("tour terminé dans le délai");
    supervisor.shutdown().await;

    assert_eq!(outcome, TurnOutcome::Completed);
    assert!(
        tools.iter().any(|name| name.contains("list_agents")),
        "outils appelés : {tools:?}"
    );
    assert!(said.contains('1'), "réponse : {said}");
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn real_claude_calls_argos_tools() {
    calls_argos_tools(EngineKind::Claude, "haiku").await;
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn real_codex_calls_argos_tools() {
    calls_argos_tools(EngineKind::Codex, "gpt-6-luna").await;
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn real_claude_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = SessionSpec::new(EngineKind::Claude, dir.path());
    spec.model = Some("haiku".into());
    full_lifecycle(spec).await;
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn real_codex_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("codex-home");
    let user_home = PathBuf::from(std::env::var("HOME").unwrap()).join(".codex");
    prepare_codex_home(&home, &user_home).unwrap();

    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut spec = SessionSpec::new(EngineKind::Codex, work);
    spec.model = Some("gpt-6-luna".into());
    spec.codex_home = Some(home);
    full_lifecycle(spec).await;
}
