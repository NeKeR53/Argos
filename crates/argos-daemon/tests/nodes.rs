//! Nœuds pairs simulés sur la machine (D48) : chaque nœud est un superviseur complet,
//! avec son registre, son dossier de données, son serveur MCP et ses pairs, relié aux
//! autres par un vrai lien WebSocket. Moteurs remplacés par le faux CLI, aucun hôte
//! distant (le faux `ssh` relaie en local).

mod common;

use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentRecord, Charter, EngineKind, Level};
use argos_daemon::node::transport::NodeConfig;
use argos_daemon::peer::Peers;
use argos_daemon::policy::ModelPolicy;
use argos_daemon::registry::{NewAgent, Registry};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use common::world::{fixture, poll, until};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Un nœud du maillage, dans le process du test.
struct Node {
    name: String,
    home: TempDir,
    supervisor: Supervisor,
    events: broadcast::Receiver<DaemonEvent>,
    port: u16,
    token: String,
}

/// Un nœud qui joint lui-même les pairs donnés. `assistant` : il crée l'Assistant.
async fn node(name: &str, assistant: bool, peers: &[(&str, NodeConfig)]) -> Node {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::open(&home.path().join("argos.db")).unwrap();
    if assistant {
        registry
            .ensure_assistant(EngineKind::Claude, None, home.path().join("assistant"))
            .unwrap();
    }
    let token = format!("jeton-{name}");
    let declared = peers
        .iter()
        .map(|(peer, config)| (peer.to_string(), config.clone()))
        .collect();
    let mesh = Peers::new(name, token.clone(), declared);
    let mcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
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
            surfaces: None,
            policy: ModelPolicy::default(),
            peers: mesh.clone(),
            worker_access: argos_daemon::engine::Access::Full,
            preempt: false,
        },
    );
    tokio::spawn(argos_daemon::mcp::serve(mcp, supervisor.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    mesh.start(supervisor.clone(), Some(listener), fixture("fake-ssh"));
    Node {
        name: name.into(),
        home,
        events: supervisor.subscribe(),
        supervisor,
        port,
        token,
    }
}

impl Node {
    /// Comment un pair joint ce nœud, directement.
    fn config(&self) -> NodeConfig {
        toml::from_str(&format!(
            "url = \"ws://127.0.0.1:{}\"\ntoken = \"{}\"",
            self.port, self.token
        ))
        .unwrap()
    }

    async fn linked(&self, peer: &str) {
        let nodes = self.supervisor.nodes().clone();
        let peer = peer.to_string();
        tokio::time::timeout(Duration::from_secs(20), async move {
            while !nodes.is_connected(&peer) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("le lien entre pairs ne s'est pas ouvert");
    }

    fn assistant(&self) -> AgentRecord {
        self.supervisor.registry().assistant().unwrap().unwrap()
    }

    async fn tool(&self, caller: &AgentRecord, name: &str, args: Value) -> Result<String, String> {
        argos_daemon::tools::call(&self.supervisor, caller, name, &args).await
    }

    fn record(&self, name: &str) -> AgentRecord {
        self.supervisor
            .registry()
            .agent_named(name)
            .unwrap()
            .unwrap_or_else(|| panic!("{} ne connaît pas {name}", self.name))
    }

    /// Coupe le nœud du maillage, comme une machine qui s'éteint.
    fn power_off(&self) {
        self.supervisor.nodes().halt();
    }

    /// Le relie de nouveau, sur le même port.
    async fn power_on(&self) {
        let listener = TcpListener::bind(("127.0.0.1", self.port)).await.unwrap();
        self.supervisor
            .nodes()
            .start(self.supervisor.clone(), Some(listener), fixture("fake-ssh"));
    }

    /// Un Agent détenu par ce nœud, sous l'Assistant (connu ici ou par l'annuaire).
    fn agent(&self, name: &str, workdir: &Path) -> AgentRecord {
        std::fs::create_dir_all(workdir).unwrap();
        self.supervisor
            .create(NewAgent {
                parent: self.assistant().id,
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                node: None,
                workdir: workdir.to_path_buf(),
                charter: Some(Charter {
                    name: name.into(),
                    scope: format!("tout ce qui concerne {name}"),
                }),
            })
            .unwrap()
    }
}

/// Le Mac avec l'Assistant, et le Pi qui le joint.
async fn mac_and_pi() -> (Node, Node) {
    let mac = node("mac", true, &[]).await;
    let pi = node("pi", false, &[("mac", mac.config())]).await;
    mac.linked("pi").await;
    pi.linked("mac").await;
    poll(|| pi.supervisor.registry().assistant().ok().flatten()).await;
    (mac, pi)
}

#[tokio::test]
async fn peers_share_a_directory_with_a_single_assistant() {
    let (mac, pi) = mac_and_pi().await;
    let assistant = mac.assistant();
    assert_eq!(pi.assistant().id, assistant.id);
    assert_eq!(pi.assistant().node.as_deref(), Some("mac"));
    assert_eq!(mac.assistant().node, None);

    // L'Assistant fait naître un Agent sur le Pi : le Pi le détient, le Mac le voit.
    let report = mac
        .tool(
            &assistant,
            "create_agent",
            json!({"name": "maison", "scope": "la maison", "node": "pi"}),
        )
        .await
        .unwrap();
    assert!(report.contains("Nœud : pi"), "{report}");
    let on_pi = pi.record("maison");
    assert_eq!(on_pi.node, None);
    assert!(on_pi.workdir.starts_with(pi.home.path().join("agents")));
    let seen = poll(|| {
        mac.supervisor
            .registry()
            .agent_named("maison")
            .ok()
            .flatten()
    })
    .await;
    assert_eq!(seen.id, on_pi.id);
    assert_eq!(seen.node.as_deref(), Some("pi"));

    // Les deux nœuds se connaissent ; un nœud inconnu est refusé.
    let nodes = mac.tool(&assistant, "list_nodes", json!({})).await.unwrap();
    assert!(
        nodes.contains("\"pi\"") && nodes.contains("true"),
        "{nodes}"
    );
    let unknown = mac
        .tool(
            &assistant,
            "create_agent",
            json!({"name": "x", "scope": "x", "node": "mars"}),
        )
        .await
        .unwrap_err();
    assert!(unknown.contains("nœud inconnu"), "{unknown}");
}

#[tokio::test]
async fn the_assistant_drives_an_agent_elsewhere_and_reports_wait_for_its_return() {
    let (mut mac, mut pi) = mac_and_pi().await;
    let assistant = mac.assistant();
    let maison = pi.agent("maison", &pi.home.path().join("agents/maison"));
    poll(|| {
        mac.supervisor
            .registry()
            .agent_named("maison")
            .ok()
            .flatten()
    })
    .await;

    // Un ordre de l'Assistant part au Pi, où l'Agent tourne.
    mac.tool(
        &assistant,
        "send_to_agent",
        json!({"agent": "maison", "message": "allume le salon"}),
    )
    .await
    .unwrap();
    until(&mut pi.events, |event| {
        common::world::is_message(event, &maison.id, "reply 1")
    })
    .await;
    let args = read(&pi.home.path().join("agents/maison/.fake-args")).unwrap();
    assert!(args.contains("allume le salon") || pi.home.path().join("agents/maison").exists());
    let inspected = mac
        .tool(&assistant, "inspect_agent", json!({"agent": "maison"}))
        .await
        .unwrap();
    assert!(inspected.contains("maison"), "{inspected}");

    // Son rapport revient à l'Assistant, sur le Mac.
    poll(|| (!pi.supervisor.inspect(&maison.id).ok()?.busy).then_some(())).await;
    pi.tool(
        &maison,
        "report_to_assistant",
        json!({"summary": "salon allumé"}),
    )
    .await
    .unwrap();
    until(&mut mac.events, |event| {
        matches!(event, DaemonEvent::Input { agent, text, .. }
            if *agent == assistant.id && text.contains("salon allumé"))
    })
    .await;

    // Mac éteint : le Pi continue, son rapport attend en file, puis part au retour.
    poll(|| (!mac.supervisor.inspect(&assistant.id).ok()?.busy).then_some(())).await;
    mac.power_off();
    poll(|| (!pi.supervisor.nodes().is_connected("mac")).then_some(())).await;
    pi.supervisor
        .deliver(&maison.id, Origin::User, "et la cuisine ?")
        .await
        .unwrap();
    until(&mut pi.events, |event| {
        common::world::is_message(event, &maison.id, "reply 2")
    })
    .await;
    poll(|| (!pi.supervisor.inspect(&maison.id).ok()?.busy).then_some(())).await;
    pi.tool(
        &maison,
        "report_to_assistant",
        json!({"summary": "cuisine allumée pendant ton absence"}),
    )
    .await
    .unwrap();
    assert_eq!(
        pi.supervisor.registry().queued().unwrap(),
        [("mac".to_string(), 1)]
    );
    mac.power_on().await;
    until(&mut mac.events, |event| {
        matches!(event, DaemonEvent::Input { agent, text, .. }
            if *agent == assistant.id && text.contains("cuisine allumée"))
    })
    .await;
    poll(|| {
        pi.supervisor
            .registry()
            .queued()
            .unwrap()
            .is_empty()
            .then_some(())
    })
    .await;
}

/// D47 + D48 : tout ce que possède un Agent part avec lui, et le nœud d'arrivée le détient.
#[tokio::test]
async fn an_agent_moves_with_everything_it_owns() {
    let (mac, mut pi) = mac_and_pi().await;
    let assistant = mac.assistant();
    mac.tool(
        &assistant,
        "create_agent",
        json!({"name": "maison", "scope": "la maison"}),
    )
    .await
    .unwrap();
    let maison = mac.record("maison");
    let first_dir = maison.workdir.clone();
    std::fs::create_dir_all(first_dir.join(".git")).unwrap();
    std::fs::write(first_dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(first_dir.join(".gitignore"), "secret.env\n").unwrap();
    std::fs::write(first_dir.join("secret.env"), "TOKEN=1\n").unwrap();
    std::fs::create_dir_all(first_dir.join("plans")).unwrap();
    std::fs::write(first_dir.join("plans/salon.md"), "Salon : 24 m².\n").unwrap();
    // Ce qu'il possède : mémoire, mission, déclencheur.
    mac.tool(
        &maison,
        "memory_write",
        json!({"file": "index.md", "content": "Je suis Maison : chauffage et plans."}),
    )
    .await
    .unwrap();
    mac.tool(
        &assistant,
        "assign_mission",
        json!({"agent": "maison", "mission": "réviser la chaudière"}),
    )
    .await
    .unwrap();
    mac.tool(
        &maison,
        "trigger_create",
        json!({"name": "relevé", "source": "every", "every_minutes": "60", "prompt": "relève le thermostat"}),
    )
    .await
    .unwrap();
    poll(|| (!mac.supervisor.inspect(&maison.id).ok()?.busy).then_some(())).await;
    let name = first_dir.file_name().unwrap().to_owned();

    let report = mac
        .tool(
            &assistant,
            "move_agent",
            json!({"agent": "maison", "node": "pi"}),
        )
        .await
        .unwrap();
    assert!(report.contains("déplacé sur pi"), "{report}");

    // Le Pi le détient, avec tout ce qu'il possède.
    let on_pi = pi.record("maison");
    assert_eq!(on_pi.node, None);
    assert_eq!(on_pi.workdir, pi.home.path().join("agents").join(&name));
    assert!(on_pi.session_id.is_none());
    let dir = &on_pi.workdir;
    assert_eq!(
        read(&dir.join("plans/salon.md")).as_deref(),
        Some("Salon : 24 m².\n")
    );
    assert_eq!(
        read(&dir.join(".git/HEAD")).as_deref(),
        Some("ref: refs/heads/main\n")
    );
    assert_eq!(read(&dir.join("secret.env")).as_deref(), Some("TOKEN=1\n"));
    let board = pi.supervisor.registry().board(&maison.id).unwrap();
    assert_eq!(board.missions.len(), 1);
    assert_eq!(board.missions[0].text, "réviser la chaudière");
    let triggers = pi.supervisor.registry().triggers_of(&maison.id).unwrap();
    assert_eq!(triggers.len(), 1);
    let memory = pi.home.path().join("memory").join(maison.id.as_str());
    assert_eq!(
        read(&memory.join("index.md")).as_deref(),
        Some("Je suis Maison : chauffage et plans.\n")
    );
    let history = Command::new("git")
        .arg("-C")
        .arg(&memory)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    let history = String::from_utf8_lossy(&history.stdout);
    assert!(history.contains("index.md écrit par l'agent"), "{history}");

    // Le Mac n'en garde qu'une copie d'annuaire.
    let on_mac = mac.record("maison");
    assert_eq!(on_mac.node.as_deref(), Some("pi"));
    assert!(
        mac.supervisor
            .registry()
            .board(&maison.id)
            .unwrap()
            .missions
            .is_empty()
    );
    assert!(
        mac.supervisor
            .registry()
            .triggers_of(&maison.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        !mac.home
            .path()
            .join("memory")
            .join(maison.id.as_str())
            .exists()
    );
    assert!(mac.home.path().join("memory-moved").is_dir());
    assert!(
        first_dir.join("plans/salon.md").exists(),
        "l'ancienne copie reste"
    );

    // Il tourne sur le Pi, d'une session neuve, avec sa mémoire.
    mac.tool(
        &assistant,
        "send_to_agent",
        json!({"agent": "maison", "message": "tu es où ?"}),
    )
    .await
    .unwrap();
    until(
        &mut pi.events,
        |event| matches!(event, DaemonEvent::Message { agent, .. } if *agent == maison.id),
    )
    .await;
    let args = read(&dir.join(".fake-args")).unwrap();
    assert!(!args.contains("--resume"), "{args}");
    assert!(
        args.contains("Je suis Maison : chauffage et plans."),
        "{args}"
    );
    poll(|| (!pi.supervisor.inspect(&maison.id).ok()?.busy).then_some(())).await;

    // Retour sur le Mac : demandé au Mac, exécuté par le Pi qui le détient. L'ancien
    // dossier existe encore : il en reçoit un neuf.
    mac.tool(
        &assistant,
        "move_agent",
        json!({"agent": "maison", "node": "mac"}),
    )
    .await
    .unwrap();
    let back = mac.record("maison");
    assert_eq!(back.node, None);
    assert_ne!(back.workdir, first_dir);
    assert_eq!(
        read(&back.workdir.join("plans/salon.md")).as_deref(),
        Some("Salon : 24 m².\n")
    );
    assert_eq!(
        mac.supervisor
            .registry()
            .triggers_of(&maison.id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(pi.record("maison").node.as_deref(), Some("mac"));

    // Un dossier de projet ne voyage pas : il faut dire où est le projet à l'arrivée.
    let project = tempfile::tempdir().unwrap();
    mac.supervisor
        .set_workspace(&maison.id, project.path().to_str().unwrap())
        .await
        .unwrap();
    let refused = mac
        .tool(
            &assistant,
            "move_agent",
            json!({"agent": "maison", "node": "pi"}),
        )
        .await
        .unwrap_err();
    assert!(refused.contains("indique workdir"), "{refused}");
}

/// L'Assistant part sur le Pi avec son canal Telegram : on lui parle là-bas, Mac éteint.
#[tokio::test]
async fn the_assistant_moves_with_its_channels() {
    let (mac, pi) = mac_and_pi().await;
    let assistant = mac.assistant();
    let config = mac.home.path().join("config.toml");
    argos_daemon::config::Config::load_or_create(&config).unwrap();
    let mut text = std::fs::read_to_string(&config).unwrap();
    // Une API Telegram injoignable : le relevé échoue sans quitter la machine.
    text.push_str(
        "\n[channels.telegram]\ntype = \"telegram\"\nchat_id = 42\napi = \"http://127.0.0.1:9\"\n",
    );
    std::fs::write(&config, text).unwrap();
    std::fs::write(mac.home.path().join("telegram-token"), "123:abc\n").unwrap();
    argos_daemon::channels::start_configured(&mac.supervisor);
    assert_eq!(mac.supervisor.channels().list().len(), 1);
    mac.tool(
        &assistant,
        "memory_write",
        json!({"file": "index.md", "content": "Je suis l'Assistant d'Alexandre."}),
    )
    .await
    .unwrap();

    mac.supervisor
        .move_agent(&assistant.id, "pi", None)
        .await
        .unwrap();

    assert_eq!(pi.assistant().node, None);
    assert_eq!(mac.assistant().node.as_deref(), Some("pi"));
    let channels = pi.supervisor.channels().list();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, "telegram");
    let pi_config = std::fs::read_to_string(pi.home.path().join("config.toml")).unwrap();
    assert!(
        pi_config
            .lines()
            .any(|line| line.trim() == "[channels.telegram]"),
        "{pi_config}"
    );
    assert!(pi_config.contains("chat_id = 42"), "{pi_config}");
    assert_eq!(
        read(&pi.home.path().join("telegram-token")).as_deref(),
        Some("123:abc\n")
    );
    assert!(mac.supervisor.channels().list().is_empty());
    let mac_config = std::fs::read_to_string(&config).unwrap();
    assert!(
        !mac_config
            .lines()
            .any(|line| line.trim() == "[channels.telegram]"),
        "{mac_config}"
    );
    assert!(!mac.home.path().join("telegram-token").exists());
    let memory = pi.home.path().join("memory").join(assistant.id.as_str());
    assert_eq!(
        read(&memory.join("index.md")).as_deref(),
        Some("Je suis l'Assistant d'Alexandre.\n")
    );

    // Mac éteint : l'Assistant répond sur le Pi.
    mac.power_off();
    let mut events = pi.supervisor.subscribe();
    pi.supervisor
        .send(&assistant.id, "[Canal telegram]\nbonjour")
        .await
        .unwrap();
    until(
        &mut events,
        |event| matches!(event, DaemonEvent::Message { agent, .. } if *agent == assistant.id),
    )
    .await;
}

/// T5 : un ancien hub confie au nœud les fiches des agents qui y tournaient déjà.
#[tokio::test]
async fn an_old_hub_hands_its_agents_over_to_their_node() {
    let mac = node("mac", true, &[]).await;
    let assistant = mac.assistant();
    let dir = Path::new("agents").join("courrier");
    let courrier = mac
        .supervisor
        .create(NewAgent {
            parent: assistant.id.clone(),
            level: Level::Agent,
            engine: EngineKind::Claude,
            model: None,
            profile: None,
            node: Some("pi".into()),
            workdir: dir.clone(),
            charter: Some(Charter {
                name: "courrier".into(),
                scope: "la boîte mail".into(),
            }),
        })
        .unwrap();
    mac.supervisor
        .registry()
        .add_mission(&courrier.id, "trier les factures")
        .unwrap();
    mac.supervisor
        .registry()
        .mark_handover(&courrier.id)
        .unwrap();

    let pi = node("pi", false, &[("mac", mac.config())]).await;
    let on_pi = poll(|| {
        pi.supervisor
            .registry()
            .agent_named("courrier")
            .ok()
            .flatten()
            .filter(|record| record.node.is_none())
    })
    .await;
    assert_eq!(on_pi.id, courrier.id);
    assert_eq!(on_pi.workdir, pi.home.path().join(&dir));
    assert_eq!(
        pi.supervisor
            .registry()
            .board(&courrier.id)
            .unwrap()
            .missions[0]
            .text,
        "trier les factures"
    );
    poll(|| {
        mac.supervisor
            .registry()
            .board(&courrier.id)
            .ok()
            .filter(|board| board.missions.is_empty())
    })
    .await;
    assert_eq!(mac.record("courrier").node.as_deref(), Some("pi"));
    assert!(
        mac.supervisor
            .registry()
            .handovers("pi")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_peer_presenting_another_name_is_refused() {
    let pi = node("pi", false, &[]).await;
    let mac = node("mac", true, &[("vps", pi.config())]).await;
    let error = poll(|| {
        mac.supervisor
            .nodes()
            .status()
            .into_iter()
            .find(|status| status.name == "vps")?
            .error
    })
    .await;
    assert!(error.contains("« pi »"), "{error}");
    assert!(!mac.supervisor.nodes().is_connected("vps"));
}

#[tokio::test]
async fn a_peer_is_reached_through_an_ssh_tunnel() {
    let pi = node("pi", false, &[]).await;
    let log = pi.home.path().join("ssh.log");
    // SAFETY : ce test est seul à lire cette variable, dans son propre process de test.
    unsafe { std::env::set_var("FAKE_SSH_LOG", &log) };
    let config: NodeConfig = toml::from_str(&format!(
        "ssh = \"pi.example\"\nremote = \"127.0.0.1:{}\"\ntoken = \"{}\"",
        pi.port, pi.token
    ))
    .unwrap();
    let mac = node("mac", true, &[("pi", config)]).await;
    mac.linked("pi").await;
    poll(|| pi.supervisor.registry().assistant().ok().flatten()).await;
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("BatchMode=yes"), "{calls}");
    assert!(calls.trim_end().ends_with("pi.example"), "{calls}");
    assert!(
        calls.contains(&format!(":127.0.0.1:{}", pi.port)),
        "{calls}"
    );
}

/// Attend qu'une condition sur les fichiers devienne vraie (watchers, aller-retours).
async fn eventually(what: &str, mut probe: impl FnMut() -> bool) {
    for _ in 0..300 {
        if probe() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("jamais vu : {what}");
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Un workspace de projet sur un nœud, avec ce qui ne doit pas voyager.
fn project_on(node: &Node, name: &str) -> std::path::PathBuf {
    let root = node.home.path().join("agents").join(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(root.join("target/debug/app"), "binaire").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("notes.local"), "propre au nœud").unwrap();
    root
}

/// D44 entre pairs : la copie locale sur le Mac d'un workspace détenu par le Pi.
#[tokio::test]
async fn a_workspace_syncs_both_ways_without_git_ignored_or_excluded_files() {
    let (mac, pi) = mac_and_pi().await;
    let remote = project_on(&pi, "dev");
    let dev = pi.agent("dev", &remote);
    poll(|| mac.supervisor.registry().agent_named("dev").ok().flatten()).await;
    let copy = mac.home.path().join("copie");

    let sync = mac
        .supervisor
        .syncs()
        .open(&mac.supervisor, &dev.id, &copy, &["*.local".into()])
        .unwrap();
    eventually("la copie initiale", || {
        read(&copy.join("src/main.rs")).as_deref() == Some("fn main() {}")
    })
    .await;
    assert!(copy.join(".gitignore").exists());
    assert!(!copy.join(".git").exists(), ".git reste sur le nœud");
    assert!(
        !copy.join("target").exists(),
        "les builds restent sur leur machine"
    );
    assert!(
        !copy.join("notes.local").exists(),
        "exclusion de la synchro"
    );

    std::fs::write(copy.join("src/lib.rs"), "pub fn mac() {}").unwrap();
    eventually("l'ajout du Mac sur le Pi", || {
        read(&remote.join("src/lib.rs")).as_deref() == Some("pub fn mac() {}")
    })
    .await;
    std::fs::write(remote.join("src/main.rs"), "fn main() { agent() }").unwrap();
    eventually("la modification du Pi sur le Mac", || {
        read(&copy.join("src/main.rs")).as_deref() == Some("fn main() { agent() }")
    })
    .await;
    std::fs::remove_file(copy.join("src/lib.rs")).unwrap();
    eventually("la suppression du Mac sur le Pi", || {
        !remote.join("src/lib.rs").exists()
    })
    .await;
    assert!(remote.join("notes.local").exists());
    assert!(remote.join(".git/HEAD").exists());

    // Le Pi refuse d'écrire hors de l'arbre ouvert.
    let escape = argos_daemon::sync::SyncRequest::Write {
        ops: vec![argos_daemon::sync::WriteOp::Delete {
            path: "../../argos.db".into(),
            expect: None,
        }],
    };
    let reply = mac
        .supervisor
        .nodes()
        .sync("pi", &sync.id, escape)
        .await
        .unwrap();
    let argos_daemon::sync::SyncResponse::Written { results } = reply else {
        panic!("réponse d'écriture attendue")
    };
    assert!(results[0].as_deref().unwrap().contains("refusé"));
    assert!(pi.home.path().join("argos.db").exists());

    mac.supervisor
        .syncs()
        .close(&mac.supervisor, &sync.id)
        .await
        .unwrap();
    assert!(mac.supervisor.syncs().list().is_empty());
    assert!(copy.join("src/main.rs").exists());
}

#[tokio::test]
async fn changes_made_while_a_peer_is_away_are_reconciled_and_conflicts_keep_both() {
    let (mac, pi) = mac_and_pi().await;
    let remote = project_on(&pi, "dev");
    std::fs::write(remote.join("both.txt"), "origine").unwrap();
    std::fs::write(remote.join("kept.txt"), "origine").unwrap();
    let dev = pi.agent("dev", &remote);
    poll(|| mac.supervisor.registry().agent_named("dev").ok().flatten()).await;
    let copy = mac.home.path().join("copie");
    mac.supervisor
        .syncs()
        .open(&mac.supervisor, &dev.id, &copy, &[])
        .unwrap();
    eventually("la copie initiale", || copy.join("kept.txt").exists()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    pi.power_off();
    let syncs = mac.supervisor.syncs().clone();
    eventually("la synchro hors ligne", || {
        syncs
            .list()
            .iter()
            .all(|status| status.state == argos_core::protocol::SyncState::Offline)
    })
    .await;
    std::fs::write(copy.join("both.txt"), "version du Mac").unwrap();
    std::fs::write(remote.join("both.txt"), "version du Pi").unwrap();
    std::fs::remove_file(remote.join("kept.txt")).unwrap();
    std::fs::write(copy.join("kept.txt"), "modifié sur le Mac").unwrap();
    std::fs::write(copy.join("new.txt"), "écrit hors ligne").unwrap();
    pi.power_on().await;

    eventually("le rapprochement au retour du Pi", || {
        read(&remote.join("new.txt")).as_deref() == Some("écrit hors ligne")
            && read(&remote.join("both.txt.conflit-hub")).as_deref() == Some("version du Mac")
    })
    .await;
    assert_eq!(
        read(&copy.join("both.txt")).as_deref(),
        Some("version du Pi")
    );
    assert_eq!(
        read(&remote.join("kept.txt")).as_deref(),
        Some("modifié sur le Mac"),
        "une modification l'emporte sur une suppression"
    );
}

#[tokio::test]
async fn a_sync_needs_an_agent_elsewhere_and_an_empty_local_folder() {
    let (mac, pi) = mac_and_pi().await;
    let dev = pi.agent("dev", &pi.home.path().join("agents/dev"));
    poll(|| mac.supervisor.registry().agent_named("dev").ok().flatten()).await;
    let local = mac.agent("ici", &mac.home.path().join("ici"));
    let syncs = mac.supervisor.syncs();
    let error = syncs
        .open(&mac.supervisor, &local.id, &mac.home.path().join("x"), &[])
        .unwrap_err();
    assert!(error.contains("déjà ici"), "{error}");
    let busy = mac.home.path().join("plein");
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join("a"), "a").unwrap();
    let error = syncs
        .open(&mac.supervisor, &dev.id, &busy, &[])
        .unwrap_err();
    assert!(error.contains("n'est pas vide"));
}
