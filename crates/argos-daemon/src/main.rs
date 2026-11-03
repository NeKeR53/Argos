use argos_core::{AgentRecord, EngineKind, Level};
use argos_daemon::config::{Config, load_or_create_token};
use argos_daemon::peer::Peers;
use argos_daemon::registry::{Registry, RegistryError};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use argos_daemon::{api, events, mcp, surface_web};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::net::TcpListener;

type Failure = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let home = argos_home()?;
    let mode = std::env::args().nth(1);
    if matches!(mode.as_deref(), Some("version" | "--version")) {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    std::fs::create_dir_all(&home)?;
    match mode.as_deref() {
        // Un seul daemon depuis D48 : `node` reste accepté pour les services déjà posés.
        None | Some("hub" | "node") => hub(home).await,
        Some("service") => service(home),
        Some("update") => update(home).await,
        Some("config") => config(home),
        Some("setup") => setup(home),
        Some(other) => Err(format!(
            "mode inconnu : {other} (hub, node, service, update, config, setup ou version)"
        )
        .into()),
    }
}

/// `update [check]` : cherche une version plus récente ; `update apply` l'installe (D46).
async fn update(home: PathBuf) -> Result<(), Failure> {
    use argos_daemon::update;
    let args: Vec<String> = std::env::args().skip(2).collect();
    match args.first().map(String::as_str) {
        None | Some("check") => {
            let current = env!("CARGO_PKG_VERSION");
            match update::find().await? {
                Some(candidate) => {
                    println!(
                        "Argos {} est disponible (installé : {current}).",
                        candidate.version
                    );
                    println!("{}", candidate.page);
                    if !candidate.notes.trim().is_empty() {
                        println!("\n{}", candidate.notes.trim());
                    }
                    println!("\nPour l'installer : argos-daemon update apply");
                }
                None => println!("Argos {current} est à jour."),
            }
            Ok(())
        }
        Some("apply") => {
            let restart_pid = match args.iter().position(|arg| arg == "--restart-pid") {
                Some(index) => Some(
                    args.get(index + 1)
                        .and_then(|pid| pid.parse().ok())
                        .ok_or("--restart-pid attend un numéro de process")?,
                ),
                None => None,
            };
            // L'avancement, issue comprise, est déjà écrit au fil de l'installation.
            update::apply(&home, update::ApplyOptions { restart_pid }).await?;
            Ok(())
        }
        _ => Err("usage : argos-daemon update [check|apply]".into()),
    }
}

/// `config init` : crée ou migre `config.toml` ; `config set <clé> <valeur>` : change une
/// valeur en gardant les commentaires.
fn config(home: PathBuf) -> Result<(), Failure> {
    use argos_daemon::config::{self, Init};
    let path = home.join("config.toml");
    let args: Vec<String> = std::env::args().skip(2).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["init"] => {
            match config::init(&path)? {
                Init::Created => println!("configuration créée : {}", path.display()),
                Init::UpToDate => println!("configuration à jour : {}", path.display()),
                Init::Migrated { changes, backup } => {
                    println!("configuration migrée : {}", path.display());
                    for change in changes {
                        println!("- {change}");
                    }
                    println!("ancienne version gardée dans {}", backup.display());
                }
            }
            Ok(())
        }
        ["set", key, value] => {
            config::set(&path, key, value)?;
            println!("{key} = {value}");
            Ok(())
        }
        ["path"] => {
            println!("{}", path.display());
            Ok(())
        }
        _ => Err("usage : argos-daemon config <init|set <clé> <valeur>|path>".into()),
    }
}

/// `setup prompt` : la consigne que l'installeur confie à `claude` ou `codex` (D46).
fn setup(home: PathBuf) -> Result<(), Failure> {
    use argos_daemon::update::Layout;
    if std::env::args().nth(2).as_deref() != Some("prompt") {
        return Err("usage : argos-daemon setup prompt".into());
    }
    print!("{}", argos_daemon::setup::prompt(&home, &Layout::detect()?));
    Ok(())
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
    let path = service::service_path(std::env::var_os("PATH"), &user_home());
    let text = match kind.as_deref() {
        Some("systemd") => service::systemd_unit(&binary, &home, &path, &mode),
        Some("launchd") => service::launchd_plist(&binary, &home, &path, &mode),
        _ => return Err("usage : argos-daemon service <systemd|launchd> [hub|node]".into()),
    };
    print!("{text}");
    Ok(())
}

/// Mode nœud (D32) : exécute les sessions que le hub lui confie.
async fn hub(home: PathBuf) -> Result<(), Failure> {
    let config = Config::load_or_create(&home.join("config.toml"))?;
    let identity = config.identity(&home);
    let token_path = home.join("token");
    let token = load_or_create_token(&token_path)?;
    // Jeton que les pairs présentent pour se relier à ce nœud (D48).
    let peer_token_path = home.join("node-token");
    let peer_token = load_or_create_token(&peer_token_path)?;
    let api_tokens = vec![token.clone(), peer_token.clone()];
    let registry = Registry::open(&home.join("argos.db"))?;
    let report = registry.rehydrate()?;
    // Un seul Assistant dans le maillage : un nœud ne le crée que s'il en a le droit et
    // n'en connaît aucun ; sinon il le reçoit de l'annuaire d'un pair.
    if identity.assistant {
        registry.ensure_assistant(
            config.assistant.engine,
            config.assistant.model.clone(),
            home.join("assistant"),
        )?;
    }

    println!(
        "argos-daemon {} (protocole v{}) — nœud « {} », données dans {}",
        env!("CARGO_PKG_VERSION"),
        argos_core::PROTOCOL_VERSION,
        identity.name,
        home.display()
    );
    println!(
        "réhydratation : {} agent(s) parké(s) pour reprise, {} worker(s) perdu(s)",
        report.parked.len(),
        report.lost_workers.len()
    );
    match registry.assistant()? {
        Some(assistant) => print_tree(&registry, &assistant)?,
        None => println!("pas encore d'Assistant connu : il viendra de l'annuaire d'un pair"),
    }

    let peers = Peers::new(identity.name.clone(), peer_token, config.peers.clone());
    let peer_listener = TcpListener::bind(identity.listen).await?;
    let mcp_listener = TcpListener::bind(config.mcp.listen).await?;
    let mcp_url = format!("http://{}", mcp_listener.local_addr()?);
    let surfaces_listener = TcpListener::bind(config.surfaces.listen).await?;
    let devices = surface_web::Devices::open(&home)?;
    let surfaces_address = surfaces_listener.local_addr()?;
    let surfaces = surface_links(&config.surfaces, surfaces_address);
    let pairing = surfaces.shared().then(|| surfaces.clone());
    let supervisor = Supervisor::new(
        registry,
        SupervisorSettings {
            home: home.clone(),
            user_codex_home: user_home().join(".codex"),
            programs: program_overrides(),
            mcp_url: Some(mcp_url.clone()),
            surfaces: Some(surfaces.clone()),
            policy: config.models.clone(),
            peers: peers.clone(),
            worker_access: config.workers.access,
            preempt: config.board.preempt,
        },
    );
    supervisor.syncs().start(&supervisor);
    println!(
        "pairs sur ws://{}/?token=… (jeton dans {})",
        peer_listener.local_addr()?,
        peer_token_path.display()
    );
    peers.start(supervisor.clone(), Some(peer_listener), ssh_program());
    for peer in config.peers.keys() {
        println!("pair déclaré : {peer}");
    }
    let listener = TcpListener::bind(config.api.listen).await?;
    println!(
        "API WebSocket sur ws://{}/?token=… (jeton dans {})",
        listener.local_addr()?,
        token_path.display()
    );
    println!("serveur MCP des agents sur {mcp_url}/mcp/<jeton de session>");
    println!(
        "surfaces dans le navigateur sur http://127.0.0.1:{}/ (associer un autre appareil : \
         QR code sur cette page), {}/ (appareils associés)",
        surfaces_address.port(),
        surfaces.base
    );

    tokio::spawn(events::Events::new(supervisor.clone()).run());
    // Les canaux de discussion vivent sur le nœud de l'Assistant, et le suivent.
    argos_daemon::channels::start_configured(&supervisor);

    tokio::select! {
        served = api::serve(listener, supervisor.clone(), api_tokens) => served?,
        served = mcp::serve(mcp_listener, supervisor.clone()) => served?,
        served = surface_web::serve(surfaces_listener, supervisor.clone(), devices, pairing) => served?,
        _ = stop_requested() => println!("arrêt demandé : parking des sessions en cours"),
    }
    supervisor.shutdown().await;
    Ok(())
}

fn surface_links(
    config: &argos_daemon::config::SurfacesConfig,
    address: std::net::SocketAddr,
) -> surface_web::Links {
    if address.ip().is_loopback() {
        return surface_web::Links {
            base: format!("http://{address}"),
        };
    }
    let base = config.public_url.clone().unwrap_or_else(|| {
        let host = local_hostname().unwrap_or_else(|| "127.0.0.1".into());
        format!("http://{host}:{}", address.port())
    });
    surface_web::Links {
        base: base.trim_end_matches('/').into(),
    }
}

/// `<nom>.local` sur macOS (`scutil --get LocalHostName`), sinon le nom d'hôte.
fn local_hostname() -> Option<String> {
    let read = |program: &str, args: &[&str]| {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        let name = String::from_utf8(output.stdout).ok()?.trim().to_string();
        (output.status.success() && !name.is_empty()).then_some(name)
    };
    if let Some(name) = read("scutil", &["--get", "LocalHostName"]) {
        return Some(format!("{name}.local"));
    }
    read("hostname", &[])
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

/// Ctrl-C, ou SIGTERM : ce qu'envoient `systemctl stop`, `launchctl kickstart -k` et une
/// mise à jour qui relance le service. Sans lui, le daemon meurt sans parker ses sessions
/// et laisse ses tunnels SSH orphelins.
async fn stop_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
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
