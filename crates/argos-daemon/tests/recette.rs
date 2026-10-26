//! Recette de la phase 1 (T10), avec de vrais moteurs et des modèles bon marché. On parle
//! à l'Assistant en texte ; il crée un Agent, l'Agent planifie et lance un Worker, le
//! Worker fait le travail, les comptes rendus remontent, puis le panic stop arrête tout.
//! Consomme des tokens : `cargo test -p argos-daemon --test recette -- --ignored --nocapture`.

use argos_core::protocol::DaemonEvent;
use argos_core::{AgentId, EngineKind, Level, TaskState};
use argos_daemon::policy::ModelPolicy;
use argos_daemon::registry::Registry;
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;

const LIMIT: Duration = Duration::from_secs(900);

/// Tous les profils sur des modèles bon marché, avec un repli sur l'autre moteur.
fn cheap_policy() -> ModelPolicy {
    let toml = r#"
        default_profile = "standard"
        [profiles.code]
        description = "Écrire ou modifier du code."
        chain = [{ engine = "claude", model = "claude-haiku-5-5" }, { engine = "codex", model = "gpt-6-luna" }]
        [profiles.standard]
        description = "Travail courant."
        chain = [{ engine = "claude", model = "claude-haiku-5-5" }, { engine = "codex", model = "gpt-6-luna" }]
        ["profiles"."léger"]
        description = "Tâches mécaniques : fichiers, conversions."
        chain = [{ engine = "codex", model = "gpt-6-luna" }, { engine = "claude", model = "claude-haiku-5-5" }]
    "#;
    toml::from_str(toml).unwrap()
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn talking_to_the_assistant_gets_the_work_done_through_the_hierarchy() {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::open_in_memory().unwrap();
    let assistant = registry
        .ensure_assistant(
            EngineKind::Claude,
            Some("claude-haiku-5-5".into()),
            home.path().join("assistant"),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_url = format!("http://{}", listener.local_addr().unwrap());
    let supervisor = Supervisor::new(
        registry,
        SupervisorSettings {
            home: home.path().to_path_buf(),
            user_codex_home: PathBuf::from(std::env::var("HOME").unwrap()).join(".codex"),
            programs: HashMap::new(),
            mcp_url: Some(mcp_url),
            policy: cheap_policy(),
        },
    );
    tokio::spawn(argos_daemon::mcp::serve(listener, supervisor.clone()));
    let mut events = supervisor.subscribe();

    supervisor
        .send(
            &assistant.id,
            "Crée un Agent « Notes », dont le périmètre est la tenue de mes notes, et confie-lui \
             cette mission : écrire dans son répertoire de travail un fichier bonjour.txt contenant \
             exactement « bonjour Argos ». Préviens-moi quand c'est fait.",
        )
        .await
        .unwrap();

    // Le fil complet, jusqu'à ce que l'Assistant ait traité le rapport de la mission.
    let mut names: HashMap<AgentId, String> =
        HashMap::from([(assistant.id.clone(), "Assistant".into())]);
    let mut report_received = false;
    let mut final_word = String::new();
    let outcome = tokio::time::timeout(LIMIT, async {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(error) => panic!("flux d'événements : {error}"),
            };
            match &event {
                DaemonEvent::AgentUpdated { agent } => {
                    let label = match &agent.record.charter {
                        Some(charter) => format!("Agent {}", charter.name),
                        None if agent.record.level == Level::Worker => {
                            format!("Worker {}", &agent.record.id.as_str()[..8])
                        }
                        None => "Assistant".into(),
                    };
                    names.insert(agent.record.id.clone(), label);
                }
                DaemonEvent::Input { agent, text } => {
                    println!("→ {} reçoit : {}", name(&names, agent), one_line(text));
                    if *agent == assistant.id && text.starts_with("[Rapport sur la mission") {
                        report_received = true;
                    }
                }
                DaemonEvent::Message { agent, text } => {
                    println!("← {} : {}", name(&names, agent), one_line(text));
                    if *agent == assistant.id && report_received {
                        final_word.push_str(text);
                    }
                }
                DaemonEvent::Tool { agent, name: tool } => {
                    println!("  {} utilise {tool}", name(&names, agent));
                }
                DaemonEvent::Notice { text, .. } => println!("! {text}"),
                DaemonEvent::TurnCompleted {
                    agent,
                    outcome,
                    usage,
                } => {
                    let cost = usage.and_then(|usage| usage.cost_usd).unwrap_or(0.0);
                    println!(
                        "  fin de tour de {} : {outcome:?} ({cost:.4} $)",
                        name(&names, agent)
                    );
                    if *agent == assistant.id && report_received {
                        return outcome.clone();
                    }
                }
                DaemonEvent::Exited { .. } => {}
            }
        }
    })
    .await
    .expect("l'Assistant a traité le rapport de mission dans le délai");

    println!("\nDernier mot de l'Assistant : {final_word}");
    assert_eq!(outcome, argos_core::TurnOutcome::Completed);

    // L'Agent existe, sa mission est close, une tâche est faite par un Worker.
    let notes = supervisor
        .registry()
        .agent_named("Notes")
        .unwrap()
        .expect("l'Agent « Notes » a été créé");
    {
        let registry = supervisor.registry();
        let board = registry.board(&notes.id).unwrap();
        assert!(
            board.missions.is_empty(),
            "plus aucune mission ouverte : {board:?}"
        );
        let workers = registry.children(&notes.id).unwrap();
        println!("Workers de Notes encore vivants : {}", workers.len());
    }
    let file = std::fs::read_to_string(notes.workdir.join("bonjour.txt"))
        .expect("bonjour.txt écrit dans le répertoire de l'Agent");
    assert!(file.contains("bonjour Argos"), "contenu : {file:?}");
    // Le journal de l'Agent garde la trace du travail délégué : tâche faite par un
    // Worker, puis mission close.
    let journal = supervisor.memory().read(&notes.id, "journal.md").unwrap();
    assert!(
        journal.contains(&format!("{}", TaskState::Done)),
        "tâche faite au journal : {journal}"
    );
    assert!(
        journal.contains("close"),
        "mission close au journal : {journal}"
    );

    // L'arbre est visible ; le panic stop arrête tout.
    let tree = supervisor.tree().unwrap();
    println!(
        "Arbre : {}",
        tree.iter()
            .map(|status| format!(
                "{} [{}{}]",
                name(&names, &status.record.id),
                status.record.state,
                if status.running { ", en cours" } else { "" }
            ))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    supervisor.panic_stop().await.unwrap();
    assert!(
        supervisor
            .tree()
            .unwrap()
            .iter()
            .all(|status| !status.running)
    );
}

fn name(names: &HashMap<AgentId, String>, agent: &AgentId) -> String {
    names
        .get(agent)
        .cloned()
        .unwrap_or_else(|| agent.as_str()[..8].to_string())
}

fn one_line(text: &str) -> String {
    let line = text.replace('\n', " ⏎ ");
    if line.chars().count() > 220 {
        line.chars().take(220).collect::<String>() + "…"
    } else {
        line
    }
}
