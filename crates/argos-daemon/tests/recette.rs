//! Recette (T10), avec de vrais moteurs et des modèles bon marché. On parle à
//! l'Assistant en texte, puis on vérifie ce que la hiérarchie a réellement fait.
//! Consomme des tokens : `cargo test -p argos-daemon --test recette -- --ignored --nocapture`.

use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentId, AgentRecord, Charter, EngineKind, Level, TaskState, TurnOutcome};
use argos_daemon::policy::ModelPolicy;
use argos_daemon::registry::{NewAgent, Registry};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

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

/// Un daemon réel (moteurs réels, serveur MCP) avec son Assistant sur Haiku.
struct Stage {
    home: TempDir,
    supervisor: Supervisor,
    assistant: AgentRecord,
    events: broadcast::Receiver<DaemonEvent>,
    names: HashMap<AgentId, String>,
}

impl Stage {
    async fn new() -> Self {
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
        let events = supervisor.subscribe();
        let names = HashMap::from([(assistant.id.clone(), "Assistant".to_string())]);
        Self {
            home,
            supervisor,
            assistant,
            events,
            names,
        }
    }

    /// Un Agent créé d'avance, comme si l'Assistant l'avait créé un autre jour.
    fn agent(&mut self, name: &str, scope: &str) -> AgentRecord {
        let agent = self
            .supervisor
            .create(NewAgent {
                parent: self.assistant.id.clone(),
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: Some("claude-haiku-5-5".into()),
                profile: Some("standard".into()),
                workdir: self.home.path().join(name.to_lowercase()),
                charter: Some(Charter {
                    name: name.into(),
                    scope: scope.into(),
                }),
            })
            .unwrap();
        self.names.insert(agent.id.clone(), format!("Agent {name}"));
        agent
    }

    /// Envoie la demande de l'utilisateur, affiche le fil et attend que l'Assistant ait
    /// traité le rapport de mission. Rend son dernier mot et tous les `input` vus.
    async fn ask(&mut self, request: &str) -> (String, Vec<(AgentId, Origin, String)>) {
        self.supervisor
            .send(&self.assistant.id, request)
            .await
            .unwrap();
        let assistant = self.assistant.id.clone();
        let (mut report_received, mut final_word, mut inputs) = (false, String::new(), vec![]);
        let outcome = tokio::time::timeout(LIMIT, async {
            loop {
                let event = match self.events.recv().await {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(error) => panic!("flux d'événements : {error}"),
                };
                match &event {
                    DaemonEvent::AgentUpdated { agent } => {
                        let record = &agent.record;
                        let label = match (&record.charter, record.level) {
                            (Some(charter), _) => format!("Agent {}", charter.name),
                            (None, Level::Worker) => format!("Worker {}", &record.id.as_str()[..8]),
                            (None, _) => "Assistant".into(),
                        };
                        self.names.insert(record.id.clone(), label);
                    }
                    DaemonEvent::Input { agent, from, text } => {
                        let sender = match from {
                            Origin::User => "utilisateur".into(),
                            Origin::Argos => "Argos".into(),
                            Origin::Agent { agent } => name(&self.names, agent),
                        };
                        println!(
                            "→ {} reçoit de {sender} : {}",
                            name(&self.names, agent),
                            one_line(text)
                        );
                        if *agent == assistant && text.starts_with("[Rapport sur la mission") {
                            report_received = true;
                        }
                        inputs.push((agent.clone(), from.clone(), text.clone()));
                    }
                    DaemonEvent::Message { agent, text } => {
                        println!("← {} : {}", name(&self.names, agent), one_line(text));
                        if *agent == assistant && report_received {
                            final_word.push_str(text);
                        }
                    }
                    DaemonEvent::Tool { agent, name: tool } => {
                        println!("  {} utilise {tool}", name(&self.names, agent));
                    }
                    DaemonEvent::Notice { text, .. } => println!("! {text}"),
                    DaemonEvent::TurnCompleted { agent, outcome, .. } => {
                        if *agent == assistant && report_received {
                            return outcome.clone();
                        }
                    }
                    DaemonEvent::Exited { .. } => {}
                }
            }
        })
        .await
        .expect("l'Assistant a traité le rapport de mission dans le délai");
        assert_eq!(outcome, TurnOutcome::Completed);
        println!("\nDernier mot de l'Assistant : {final_word}");
        (final_word, inputs)
    }

    /// L'arbre est visible et le panic stop arrête tout.
    async fn panic_stop(&self) {
        let tree = self.supervisor.tree().unwrap();
        println!(
            "Arbre : {}",
            tree.iter()
                .map(|status| format!(
                    "{} [{}]",
                    name(&self.names, &status.record.id),
                    status.record.state
                ))
                .collect::<Vec<_>>()
                .join(" · ")
        );
        self.supervisor.panic_stop().await.unwrap();
        assert!(
            self.supervisor
                .tree()
                .unwrap()
                .iter()
                .all(|status| !status.running)
        );
    }
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn talking_to_the_assistant_gets_the_work_done_through_the_hierarchy() {
    let mut stage = Stage::new().await;
    stage
        .ask(
            "Crée un Agent « Notes », dont le périmètre est la tenue de mes notes, et confie-lui \
             cette mission : écrire dans son répertoire de travail un fichier bonjour.txt contenant \
             exactement « bonjour Argos ». Préviens-moi quand c'est fait.",
        )
        .await;

    // L'Agent existe, sa mission est close, le fichier est écrit par un Worker.
    let notes = stage
        .supervisor
        .registry()
        .agent_named("Notes")
        .unwrap()
        .expect("l'Agent « Notes » a été créé");
    let board = stage.supervisor.registry().board(&notes.id).unwrap();
    assert!(
        board.missions.is_empty(),
        "plus aucune mission ouverte : {board:?}"
    );
    let file = std::fs::read_to_string(notes.workdir.join("bonjour.txt"))
        .expect("bonjour.txt écrit dans le répertoire de l'Agent");
    assert!(file.contains("bonjour Argos"), "contenu : {file:?}");
    // Le journal de l'Agent garde la trace du travail délégué : tâche faite par un
    // Worker, puis mission close.
    let journal = stage
        .supervisor
        .memory()
        .read(&notes.id, "journal.md")
        .unwrap();
    assert!(
        journal.contains(&format!("{}", TaskState::Done)),
        "tâche faite au journal : {journal}"
    );
    assert!(
        journal.contains("close"),
        "mission close au journal : {journal}"
    );
    stage.panic_stop().await;
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn agents_ask_each_other_what_belongs_to_their_domain() {
    let mut stage = Stage::new().await;
    let mail = stage.agent("Mail", "ma boîte mail et mes codes d'accès reçus par mail");
    stage.agent("Notes", "la tenue de mes notes");
    // Ce que seul l'Agent Mail sait : il l'a noté dans sa mémoire.
    let memory = stage.supervisor.memory();
    memory.ensure(&mail.id).unwrap();
    memory
        .write(
            &mail.id,
            "index.md",
            "Je gère la boîte mail. Code du portail reçu par mail le 20 octobre : 4271.",
            false,
        )
        .unwrap();

    stage
        .ask(
            "Demande à l'Agent Notes d'écrire dans son répertoire de travail un fichier portail.txt \
             contenant le code du portail. Ce code, c'est l'Agent Mail qui le connaît : Notes doit \
             le lui demander. Préviens-moi quand c'est fait.",
        )
        .await;

    let (notes, mail) = {
        let registry = stage.supervisor.registry();
        (
            registry.agent_named("Notes").unwrap().unwrap(),
            registry.agent_named("Mail").unwrap().unwrap(),
        )
    };
    let file = std::fs::read_to_string(notes.workdir.join("portail.txt"))
        .expect("portail.txt écrit dans le répertoire de Notes");
    assert!(file.contains("4271"), "contenu : {file:?}");
    // La question est passée directement de Notes à Mail, et la réponse dans l'autre sens.
    let journal = stage
        .supervisor
        .memory()
        .read(&mail.id, "journal.md")
        .unwrap();
    assert!(
        journal.contains("question de l'Agent « Notes »"),
        "journal de Mail : {journal}"
    );
    let journal = stage
        .supervisor
        .memory()
        .read(&notes.id, "journal.md")
        .unwrap();
    assert!(
        journal.contains("information de l'Agent « Mail »"),
        "journal de Notes : {journal}"
    );
    stage.panic_stop().await;
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
