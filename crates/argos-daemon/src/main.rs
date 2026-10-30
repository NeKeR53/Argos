use argos_core::{AgentRecord, EngineKind, Level};
use argos_daemon::config::{Config, NodeModeConfig, load_or_create_token};
use argos_daemon::node::Nodes;
use argos_daemon::node::runner::{self, RunnerSettings};
use argos_daemon::registry::{Registry, RegistryError};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use argos_daemon::{api, events, mcp};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::net::TcpListener;

type Failure = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let home = argos_home()?;
    std::fs::create_dir_all(&home)?;
    match std::env::args().nth(1).as_deref() {
        None | Some("hub") => hub(home).await,
        Some("node") => node(home).await,
        Some("service") => service(home),
        Some(other) => Err(format!("mode inconnu : {other} (hub, node ou service)").into()),
    }
}

/// `service <systemd|launchd> [hub|node]` : écrit le fichier de service qui lance ce
/// binaire avec ce `ARGOS_HOME` (D33). Ne l'installe pas.
fn service(home: PathBuf) -> Result<(), Failure> {
    use argos_daemon::node::service;
    let kind = std::env::args().nth(2);
    let mode = std::env::args().nth(3).unwrap_or_else(|| "node".into());
    if mode != "hub" && mode != "node" {
        return Err(format!("mode inconnu : {mode} (hub ou node)").into());
    }
    let binary = std::env::current_exe()?;
    let text = match kind.as_deref() {
        Some("systemd") => service::systemd_unit(&binary, &home, &mode),
        Some("launchd") => service::launchd_plist(&binary, &home, &mode),
        _ => return Err("usage : argos-daemon service <systemd|launchd> [hub|node]".into()),
    };
    print!("{text}");
    Ok(())
}

/// Mode nœud (D32) : exécute les sessions que le hub lui confie.
async fn node(home: PathBuf) -> Result<(), Failure> {
    let config = NodeModeConfig::load_or_create(&home.join("node.toml"))?.node;
    let token_path = home.join("node-token");
    let token = load_or_create_token(&token_path)?;
    let listener = TcpListener::bind(config.listen).await?;
    let mcp_listener = TcpListener::bind(config.mcp_listen).await?;
    println!(
        "argos-daemon {} en mode nœud « {} » — hub attendu sur ws://{} (jeton dans {}), mandataire MCP sur {}",
        env!("CARGO_PKG_VERSION"),
        config.name,
        listener.local_addr()?,
        token_path.display(),
        mcp_listener.local_addr()?
    );
    let settings = RunnerSettings {
        name: config.name,
        home: home.clone(),
        user_codex_home: user_home().join(".codex"),
        programs: program_overrides(),
    };
    tokio::select! {
        served = runner::serve(listener, mcp_listener, settings, token) => served?,
        _ = tokio::signal::ctrl_c() => println!("arrêt du nœud"),
    }
    Ok(())
}

/// Mode hub : registre, Assistant, API, MCP, événements, connexions aux nœuds.
async fn hub(home: PathBuf) -> Result<(), Failure> {
    let config = Config::load_or_create(&home.join("config.toml"))?;
    let token_path = home.join("token");
    let token = load_or_create_token(&token_path)?;
    let registry = Registry::open(&home.join("argos.db"))?;
    let report = registry.rehydrate()?;
    let assistant = registry.ensure_assistant(
        config.assistant.engine,
        config.assistant.model.clone(),
        home.join("assistant"),
    )?;

    println!(
        "argos-daemon {} (protocole v{}) — données dans {}",
        env!("CARGO_PKG_VERSION"),
        argos_core::PROTOCOL_VERSION,
        home.display()
    );
    println!(
        "réhydratation : {} agent(s) parké(s) pour reprise, {} worker(s) perdu(s)",
        report.parked.len(),
        report.lost_workers.len()
    );
    print_tree(&registry, &assistant)?;

    let nodes = Nodes::new(config.nodes.clone());
    let mcp_listener = TcpListener::bind(config.mcp.listen).await?;
    let mcp_url = format!("http://{}", mcp_listener.local_addr()?);
    let supervisor = Supervisor::new(
        registry,
        SupervisorSettings {
            home: home.clone(),
            user_codex_home: user_home().join(".codex"),
            programs: program_overrides(),
            mcp_url: Some(mcp_url.clone()),
            policy: config.models.clone(),
            nodes: nodes.clone(),
            worker_access: config.workers.access,
        },
    );
    nodes.connect(supervisor.clone(), ssh_program());
    for node in config.nodes.keys() {
        println!("nœud déclaré : {node}");
    }
    let listener = TcpListener::bind(config.api.listen).await?;
    println!(
        "API WebSocket sur ws://{}/?token=… (jeton dans {})",
        listener.local_addr()?,
        token_path.display()
    );
    println!("serveur MCP des agents sur {mcp_url}/mcp/<jeton de session>");

    tokio::spawn(events::Events::new(supervisor.clone()).run());

    tokio::select! {
        served = api::serve(listener, supervisor.clone(), token) => served?,
        served = mcp::serve(mcp_listener, supervisor.clone()) => served?,
        _ = tokio::signal::ctrl_c() => println!("arrêt demandé : parking des sessions en cours"),
    }
    supervisor.shutdown().await;
    Ok(())
}

/// Répertoire de données du daemon : `$ARGOS_HOME`, sinon `~/.argos`.
fn argos_home() -> Result<PathBuf, &'static str> {
    if let Some(dir) = std::env::var_os("ARGOS_HOME") {
        return Ok(dir.into());
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".argos"))
        .ok_or("ni ARGOS_HOME ni HOME ne sont définis")
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Binaire des tunnels vers les nœuds : `ARGOS_SSH_BIN` (tests), sinon `ssh`.
fn ssh_program() -> PathBuf {
    std::env::var_os("ARGOS_SSH_BIN").map_or_else(|| "ssh".into(), PathBuf::from)
}

/// Binaires de remplacement des moteurs (`ARGOS_CLAUDE_BIN`, `ARGOS_CODEX_BIN`), par
/// exemple les faux CLI de tests/fixtures pour essayer la GUI sans consommer de tokens.
fn program_overrides() -> HashMap<EngineKind, PathBuf> {
    [
        (EngineKind::Claude, "ARGOS_CLAUDE_BIN"),
        (EngineKind::Codex, "ARGOS_CODEX_BIN"),
    ]
    .into_iter()
    .filter_map(|(engine, variable)| Some((engine, std::env::var_os(variable)?.into())))
    .collect()
}

/// Affiche un agent puis ses descendants, chacun sous son parent.
fn print_tree(registry: &Registry, agent: &AgentRecord) -> Result<(), RegistryError> {
    println!("{}", describe(agent));
    for child in registry.children(&agent.id)? {
        print_tree(registry, &child)?;
    }
    Ok(())
}

fn describe(agent: &AgentRecord) -> String {
    let indent = match agent.level {
        Level::Assistant => "",
        Level::Agent => "  ",
        Level::Worker => "    ",
    };
    let name = agent
        .charter
        .as_ref()
        .map_or(agent.id.as_str(), |charter| charter.name.as_str());
    format!(
        "{indent}{} {name} — {} {} [{}]",
        agent.level,
        agent.engine,
        agent.model.as_deref().unwrap_or("modèle par défaut"),
        agent.state
    )
}
