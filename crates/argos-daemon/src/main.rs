use argos_core::{AgentRecord, Level};
use argos_daemon::registry::{Registry, RegistryError};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = argos_home()?;
    std::fs::create_dir_all(&home)?;
    let registry = Registry::open(&home.join("argos.db"))?;
    let report = registry.rehydrate()?;

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
    if let Some(assistant) = registry.assistant()? {
        print_tree(&registry, &assistant)?;
    }
    Ok(())
}

/// Affiche un agent puis ses descendants, chacun sous son parent.
fn print_tree(registry: &Registry, agent: &AgentRecord) -> Result<(), RegistryError> {
    println!("{}", describe(agent));
    for child in registry.children(&agent.id)? {
        print_tree(registry, &child)?;
    }
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
