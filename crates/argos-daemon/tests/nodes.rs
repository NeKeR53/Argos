//! Multi-nœud simulé sur la machine (D32) : un vrai `argos-daemon node` dans un process à
//! part, avec son propre `ARGOS_HOME` et ses ports, sur faux CLI. Aucun hôte distant.

mod common;

use argos_core::protocol::DaemonEvent;
use argos_core::{AgentRecord, AgentState, Charter, EngineKind, Level};
use argos_daemon::node::Nodes;
use argos_daemon::node::transport::NodeConfig;
use argos_daemon::policy::ModelPolicy;
use argos_daemon::registry::{NewAgent, Registry};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use common::world::{fixture, poll, until};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

/// Un nœud simulé : le daemon en mode nœud, dans son process.
struct Node {
    home: TempDir,
    listen: u16,
    mcp: u16,
    process: Option<Child>,
}

impl Node {
    fn start(name: &str) -> Node {
        let home = tempfile::tempdir().unwrap();
        let (listen, mcp) = (free_port(), free_port());
        std::fs::write(
            home.path().join("node.toml"),
            format!(
                "[node]\nname = \"{name}\"\nlisten = \"127.0.0.1:{listen}\"\nmcp_listen = \"127.0.0.1:{mcp}\"\n"
            ),
        )
        .unwrap();
        let mut node = Node {
            home,
            listen,
            mcp,
            process: None,
        };
        node.restart();
        node
    }

    /// (Re)lance le process du nœud, sur les mêmes ports et le même dossier.
    fn restart(&mut self) {
        let process = Command::new(env!("CARGO_BIN_EXE_argos-daemon"))
            .arg("node")
            .env("ARGOS_HOME", self.home.path())
            .env("ARGOS_CLAUDE_BIN", fixture("fake-claude"))
            .env("ARGOS_CODEX_BIN", fixture("fake-codex"))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        self.process = Some(process);
        wait_listening(self.listen);
    }

    fn kill(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.home.path().join("node-token"))
            .unwrap()
            .trim()
            .into()
    }

    fn config(&self) -> NodeConfig {
        toml::from_str(&format!(
            "url = \"ws://127.0.0.1:{}\"\ntoken = \"{}\"",
            self.listen,
            self.token()
        ))
        .unwrap()
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Un hub sur faux CLI, avec son serveur MCP, connecté aux nœuds donnés.
struct Hub {
    home: TempDir,
    supervisor: Supervisor,
    events: broadcast::Receiver<DaemonEvent>,
    assistant: AgentRecord,
}

async fn hub(nodes: BTreeMap<String, NodeConfig>) -> Hub {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::open_in_memory().unwrap();
    let assistant = registry
        .ensure_assistant(EngineKind::Claude, None, home.path().join("assistant"))
        .unwrap();
    let mcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let nodes = Nodes::new(nodes);
    let supervisor = Supervisor::new(
        registry,
        SupervisorSettings {
            home: home.path().to_path_buf(),
            user_codex_home: home.path().join("user-codex"),
            programs: HashMap::from([
                (EngineKind::Claude, fixture("fake-claude")),
                (EngineKind::Codex, fixture("fake-codex")),
            ]),
            mcp_url: Some(format!("http://{}", mcp.local_addr().unwrap())),
            policy: ModelPolicy::default(),
            nodes: nodes.clone(),
        },
    );
    tokio::spawn(argos_daemon::mcp::serve(mcp, supervisor.clone()));
    nodes.connect(supervisor.clone(), fixture("fake-ssh"));
    Hub {
        home,
        events: supervisor.subscribe(),
        supervisor,
        assistant,
    }
}

impl Hub {
    /// Un Agent placé sur `node`, dans `agents/<name>` du nœud.
    fn agent_on(&self, node: &str, name: &str) -> AgentRecord {
        self.supervisor
            .create(NewAgent {
                parent: self.assistant.id.clone(),
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                node: Some(node.into()),
                workdir: Path::new("agents").join(name),
                charter: Some(Charter {
                    name: name.into(),
                    scope: format!("tout ce qui concerne {name}"),
                }),
            })
            .unwrap()
    }

    async fn connected(&self, node: &str) {
        let nodes = self.supervisor.nodes().clone();
        let node = node.to_string();
        tokio::time::timeout(Duration::from_secs(20), async move {
            while !nodes
                .status()
                .iter()
                .any(|status| status.name == node && status.connected)
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("le hub ne s'est pas connecté au nœud");
    }
}

fn one(name: &str, node: &Node) -> BTreeMap<String, NodeConfig> {
    BTreeMap::from([(name.to_string(), node.config())])
}

#[tokio::test]
async fn an_agent_placed_on_a_node_runs_there_and_reaches_the_hub_tools() {
    let node = Node::start("pi");
    let mut hub = hub(one("pi", &node)).await;
    hub.connected("pi").await;
    let mail = hub.agent_on("pi", "mail");

    hub.supervisor.send(&mail.id, "bonjour").await.unwrap();
    until(&mut hub.events, |event| {
        common::world::is_message(event, &mail.id, "reply 1")
    })
    .await;

    // Le process tourne dans le dossier du nœud, pas sur le hub.
    let args = std::fs::read_to_string(node.home.path().join("agents/mail/.fake-args")).unwrap();
    // Ses outils passent par le mandataire MCP du nœud.
    let proxy = format!("http://127.0.0.1:{}/mcp/", node.mcp);
    assert!(args.contains(&proxy), "{args}");
    assert!(
        args.contains("--disallowedTools"),
        "un Agent ne modifie rien : {args}"
    );
    assert!(hub.supervisor.inspect(&mail.id).unwrap().running);

    // Le mandataire relaie au hub, qui reconnaît l'agent à son jeton.
    let token = hub.supervisor.mcp_token(&mail.id);
    let listed = mcp(
        node.mcp,
        &token,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    let names: Vec<&str> = listed.1["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"spawn_worker"), "{names:?}");
    let added = mcp(
        node.mcp,
        &token,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "add_task", "arguments": {"title": "trier la boîte"}}}),
    )
    .await;
    assert_eq!(added.1["result"]["isError"], false, "{}", added.1);
    let board = hub.supervisor.registry().board(&mail.id).unwrap();
    assert_eq!(board.tasks[0].title, "trier la boîte");

    // Un jeton inconnu du hub : refusé comme en local.
    assert_eq!(
        mcp(
            node.mcp,
            "inconnu",
            json!({"jsonrpc": "2.0", "id": 3, "method": "ping"})
        )
        .await
        .0,
        404
    );

    hub.supervisor.park(&mail.id).await.unwrap();
    assert_eq!(
        hub.supervisor.record(&mail.id).unwrap().state,
        AgentState::Parked
    );
}

#[tokio::test]
async fn a_worker_follows_its_agent_to_its_node() {
    let node = Node::start("pi");
    let mut hub = hub(one("pi", &node)).await;
    hub.connected("pi").await;
    let mail = hub.agent_on("pi", "mail");
    let token = hub.supervisor.mcp_token(&mail.id);
    let (_, task) = mcp(
        node.mcp,
        &token,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "add_task", "arguments": {"title": "compter"}}}),
    )
    .await;
    assert_eq!(task["result"]["isError"], false, "{task}");
    let task_id = hub.supervisor.registry().board(&mail.id).unwrap().tasks[0]
        .id
        .clone();
    let (_, spawned) = mcp(
        node.mcp,
        &token,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "spawn_worker", "arguments": {"task": task_id, "profile": "léger"}}}),
    )
    .await;
    assert_eq!(spawned["result"]["isError"], false, "{spawned}");

    let worker = poll(|| hub.supervisor.children(&mail.id).unwrap().pop()).await;
    assert_eq!(worker.record.node.as_deref(), Some("pi"));
    let worker_id = worker.record.id.clone();
    until(
        &mut hub.events,
        |event| matches!(event, DaemonEvent::TurnCompleted { agent, .. } if *agent == worker_id),
    )
    .await;
    // Le Worker (Codex) tourne dans le dossier de son Agent, avec un CODEX_HOME du nœud.
    assert!(
        node.home
            .path()
            .join("codex")
            .join(worker_id.as_str())
            .exists()
    );
}

#[tokio::test]
async fn losing_a_node_parks_its_agents_who_resume_once_it_is_back() {
    let mut node = Node::start("pi");
    let mut hub = hub(one("pi", &node)).await;
    hub.connected("pi").await;
    let mail = hub.agent_on("pi", "mail");
    hub.supervisor.send(&mail.id, "bonjour").await.unwrap();
    until(&mut hub.events, |event| {
        common::world::is_message(event, &mail.id, "reply 1")
    })
    .await;
    let session = hub.supervisor.record(&mail.id).unwrap().session_id.unwrap();

    node.kill();
    until(
        &mut hub.events,
        |event| matches!(event, DaemonEvent::Exited { agent, .. } if *agent == mail.id),
    )
    .await;
    poll(|| (!hub.supervisor.inspect(&mail.id).unwrap().running).then_some(())).await;
    assert_eq!(
        hub.supervisor.record(&mail.id).unwrap().state,
        AgentState::Parked
    );
    // Nœud absent : le message est refusé, l'agent ne se perd pas.
    let refused = hub
        .supervisor
        .send(&mail.id, "tu es là ?")
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("injoignable"), "{refused}");
    let status = hub.supervisor.nodes().status();
    assert!(!status[0].connected);

    node.restart();
    hub.connected("pi").await;
    hub.supervisor.send(&mail.id, "re-bonjour").await.unwrap();
    until(&mut hub.events, |event| {
        common::world::is_message(event, &mail.id, "reply 1")
    })
    .await;
    let args = std::fs::read_to_string(node.home.path().join("agents/mail/.fake-args")).unwrap();
    assert!(
        args.contains("--resume") && args.contains(&session),
        "{args}"
    );
}

#[tokio::test]
async fn the_assistant_places_agents_only_on_declared_nodes() {
    let node = Node::start("pi");
    let hub = hub(one("pi", &node)).await;
    hub.connected("pi").await;
    let call = |name: &'static str, args: Value| {
        let supervisor = hub.supervisor.clone();
        let assistant = hub.assistant.clone();
        async move { argos_daemon::tools::call(&supervisor, &assistant, name, &args).await }
    };

    let nodes = call("list_nodes", json!({})).await.unwrap();
    assert!(
        nodes.contains("\"pi\"") && nodes.contains("\"connected\":true"),
        "{nodes}"
    );
    let refused = call(
        "create_agent",
        json!({"name": "Maison", "scope": "la domotique", "node": "grenier"}),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("nœud inconnu"), "{refused}");
    let created = call(
        "create_agent",
        json!({"name": "Maison", "scope": "la domotique", "node": "pi"}),
    )
    .await
    .unwrap();
    assert!(created.contains("Nœud : pi"), "{created}");
    let maison = hub
        .supervisor
        .registry()
        .agent_named("Maison")
        .unwrap()
        .unwrap();
    assert_eq!(maison.node.as_deref(), Some("pi"));
    // Sur un nœud, le répertoire par défaut part du dossier de ce nœud.
    assert!(maison.workdir.is_relative() && maison.workdir.starts_with("agents"));
}

#[tokio::test]
async fn a_node_presenting_another_name_is_refused() {
    let node = Node::start("pi");
    let hub = hub(one("vps", &node)).await;
    let error = poll(|| hub.supervisor.nodes().status()[0].error.clone()).await;
    assert!(error.contains("« pi »"), "{error}");
    assert!(!hub.supervisor.nodes().status()[0].connected);
}

#[tokio::test]
async fn an_ssh_node_is_reached_through_the_tunnel() {
    let node = Node::start("pi");
    let log = node.home.path().join("ssh.log");
    // SAFETY : ce test est seul à lire cette variable, dans son propre process de test.
    unsafe { std::env::set_var("FAKE_SSH_LOG", &log) };
    let config: NodeConfig = toml::from_str(&format!(
        "ssh = \"pi.example\"\nremote = \"127.0.0.1:{}\"\ntoken = \"{}\"",
        node.listen,
        node.token()
    ))
    .unwrap();
    let mut hub = hub(BTreeMap::from([("pi".to_string(), config)])).await;
    hub.connected("pi").await;
    let mail = hub.agent_on("pi", "mail");
    hub.supervisor.send(&mail.id, "bonjour").await.unwrap();
    until(&mut hub.events, |event| {
        common::world::is_message(event, &mail.id, "reply 1")
    })
    .await;
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("BatchMode=yes"), "{calls}");
    assert!(calls.trim_end().ends_with("pi.example"), "{calls}");
    assert!(
        calls.contains(&format!(":127.0.0.1:{}", node.listen)),
        "{calls}"
    );
}

/// Requête MCP au mandataire d'un nœud ; rend le statut HTTP et le corps JSON.
async fn mcp(port: u16, token: &str, body: Value) -> (u16, Value) {
    let body = body.to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "POST /mcp/{token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_listening(port: u16) {
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("le nœud n'écoute pas sur {port}");
}

#[tokio::test]
async fn an_agents_memory_lives_on_its_node_and_is_mirrored_on_the_hub() {
    let mut node = Node::start("pi");
    let mut hub = hub(one("pi", &node)).await;
    hub.connected("pi").await;
    let mail = hub.agent_on("pi", "mail");
    let on_node = node.home.path().join("memory").join(mail.id.as_str());
    let mirror = hub.home.path().join("memory").join(mail.id.as_str());

    // Le réveil écrit l'état structuré dans la mémoire du nœud.
    hub.supervisor.send(&mail.id, "bonjour").await.unwrap();
    until(&mut hub.events, |event| {
        common::world::is_message(event, &mail.id, "reply 1")
    })
    .await;
    let state = std::fs::read_to_string(on_node.join("state.md")).unwrap();
    assert!(state.contains("Nœud : pi"), "{state}");
    assert!(
        on_node.join(".git").exists(),
        "l'historique vit sur le nœud"
    );

    // L'agent écrit par ses outils : sur le nœud, puis copié sur le hub.
    let call = |name: &'static str, args: Value| {
        let supervisor = hub.supervisor.clone();
        let mail = mail.clone();
        async move { argos_daemon::tools::call(&supervisor, &mail, name, &args).await }
    };
    call(
        "memory_write",
        json!({"file": "notes/expéditeurs.md", "content": "La banque écrit le lundi."}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(on_node.join("notes/expéditeurs.md")).unwrap(),
        "La banque écrit le lundi.\n"
    );
    poll(|| std::fs::read_to_string(mirror.join("notes/expéditeurs.md")).ok()).await;
    assert!(mirror.join("state.md").exists());
    assert!(
        !mirror.join(".git").exists(),
        "le miroir n'a pas d'historique"
    );
    let found = call("memory_search", json!({"query": "banque"}))
        .await
        .unwrap();
    assert!(found.contains("notes/expéditeurs.md:1"), "{found}");

    // Nœud absent : la lecture vient du miroir, l'écriture est refusée.
    node.kill();
    poll(|| (!hub.supervisor.nodes().status()[0].connected).then_some(())).await;
    let read = call("memory_read", json!({"file": "notes/expéditeurs.md"}))
        .await
        .unwrap();
    assert_eq!(read, "La banque écrit le lundi.\n");
    let refused = call("memory_write", json!({"file": "index.md", "content": "x"}))
        .await
        .unwrap_err();
    assert!(refused.contains("injoignable"), "{refused}");

    // Ce qui a changé sur le nœud pendant l'absence revient au miroir à la reconnexion.
    std::fs::write(on_node.join("notes/expéditeurs.md"), "Changé sur place.\n").unwrap();
    node.restart();
    hub.connected("pi").await;
    poll(|| {
        let mirrored = std::fs::read_to_string(mirror.join("notes/expéditeurs.md")).ok()?;
        (mirrored == "Changé sur place.\n").then_some(())
    })
    .await;
}
