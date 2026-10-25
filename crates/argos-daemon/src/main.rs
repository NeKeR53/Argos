use argos_core::{AgentRecord, EngineKind, Level};
use argos_daemon::config::{Config, load_or_create_token};
use argos_daemon::registry::{Registry, RegistryError};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use argos_daemon::{api, mcp};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = argos_home()?;
    std::fs::create_dir_all(&home)?;
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

    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mcp_listener = TcpListener::bind(config.mcp.listen).await?;
    let mcp_url = format!("http://{}", mcp_listener.local_addr()?);
    let supervisor = Supervisor::new(
        registry,
        SupervisorSettings {
            home: home.clone(),
            user_codex_home: user_home.join(".codex"),
            programs: program_overrides(),
            mcp_url: Some(mcp_url.clone()),
        },
    );
    let listener = TcpListener::bind(config.api.listen).await?;
    println!(
        "API WebSocket sur ws://{}/?token=… (jeton dans {})",
        listener.local_addr()?,
        token_path.display()
    );
    println!("serveur MCP des agents sur {mcp_url}/mcp/<jeton de session>");

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
        .map(|home| PathBuf::from(home).join(".argos"))
        .ok_or("ni ARGOS_HOME ni HOME ne sont définis")
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
