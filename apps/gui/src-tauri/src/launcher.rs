//! Démarrage du daemon au lancement de l'app : s'il n'écoute pas, l'app le démarre, par
//! son service (launchd, systemd) quand il en a un, sinon en process détaché qui survit à
//! l'app. Le front se connecte ensuite comme d'habitude, en réessayant (`connection.ts`).

use crate::daemon;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const DAEMON: &str = if cfg!(windows) {
    "argos-daemon.exe"
} else {
    "argos-daemon"
};
/// Libellé du service quand `install.toml` n'en donne pas (celui de `install.sh`).
const DEFAULT_LABEL: &str = "argos-hub";

/// Complète le `PATH` d'une app ouverte depuis le Finder ou le Dock, qui ne reçoit que
/// `/usr/bin:/bin:/usr/sbin:/sbin` : sans ça, `whisper-server` (Homebrew), `claude` ou
/// `argos-daemon` restent introuvables. Les dossiers existants manquants sont ajoutés à
/// la fin, l'ordre de l'utilisateur gardé. À appeler avant tout autre thread.
#[cfg(unix)]
pub fn complete_path() {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let extra = [
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/opt/homebrew/sbin")),
        Some(PathBuf::from("/usr/local/bin")),
        home.as_ref().map(|home| home.join(".local/bin")),
        home.as_ref().map(|home| home.join(".cargo/bin")),
    ];
    let before = dirs.len();
    for dir in extra.into_iter().flatten() {
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if dirs.len() > before
        && let Ok(path) = std::env::join_paths(dirs)
    {
        // SAFETY: appelé en tête de `run`, avant que l'app ne lance le moindre thread.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// Démarre le daemon s'il n'écoute pas déjà, sans bloquer le lancement de l'app.
pub fn ensure_running() {
    std::thread::spawn(|| {
        if let Err(error) = start_if_absent() {
            eprintln!("daemon non démarré : {error}");
        }
    });
}

fn start_if_absent() -> Result<(), String> {
    let home = daemon::argos_home()?;
    if listening(daemon::api_address_in(&home)?) {
        return Ok(());
    }
    let binary = find_daemon();
    // Le service tourne sur le `ARGOS_HOME` par défaut : avec un autre (essais), on lance
    // le daemon nous-mêmes plutôt que de réveiller celui de tous les jours.
    if std::env::var_os("ARGOS_HOME").is_none() {
        let label = binary
            .as_deref()
            .and_then(service_label)
            .unwrap_or_else(|| DEFAULT_LABEL.into());
        if start_service(&label) {
            eprintln!("daemon démarré par son service ({label})");
            return Ok(());
        }
    }
    let binary = binary.ok_or_else(|| format!("{DAEMON} introuvable"))?;
    spawn_detached(&binary, &home)?;
    eprintln!("daemon démarré : {}", binary.display());
    Ok(())
}

fn listening(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok()
}

/// Le binaire du daemon : `ARGOS_DAEMON_BIN`, puis à côté de l'app (archive décompressée,
/// `target/` en développement), puis aux emplacements de l'installeur, puis le `PATH`.
fn find_daemon() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ARGOS_DAEMON_BIN") {
        return Some(path.into());
    }
    let mut places = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        places.push(dir.join(DAEMON));
        // `…/Argos.app/Contents/MacOS/argos` : le daemon est à côté du bundle.
        if let Some(beside_bundle) = dir.ancestors().nth(3) {
            places.push(beside_bundle.join(DAEMON));
        }
    }
    // L'AppImage s'exécute depuis un montage : son vrai dossier est dans `APPIMAGE`.
    if let Some(appimage) = std::env::var_os("APPIMAGE")
        && let Some(dir) = Path::new(&appimage).parent()
    {
        places.push(dir.join(DAEMON));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        places.push(
            home.join("Library/Application Support/Argos/bin")
                .join(DAEMON),
        );
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        places.push(data.join("argos").join(DAEMON));
    }
    if let Some(path) = std::env::var_os("PATH") {
        places.extend(std::env::split_paths(&path).map(|dir| dir.join(DAEMON)));
    }
    places.into_iter().find(|place| place.is_file())
}

/// Libellé du service lu dans `install.toml`, à côté du daemon.
fn service_label(binary: &Path) -> Option<String> {
    let text = std::fs::read_to_string(binary.parent()?.join("install.toml")).ok()?;
    let manifest: toml::Table = toml::from_str(&text).ok()?;
    manifest.get("label")?.as_str().map(str::to_owned)
}

#[cfg(target_os = "macos")]
fn start_service(label: &str) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let plist = Path::new(&home).join(format!("Library/LaunchAgents/{label}.plist"));
    if !plist.is_file() {
        return false;
    }
    let uid = current_uid();
    let target = format!("gui/{uid}/{label}");
    // Déjà chargé : on le relance ; sinon (déchargé à la main), on le recharge.
    succeeds(Command::new("launchctl").args(["kickstart", &target]))
        || succeeds(
            Command::new("launchctl")
                .args(["bootstrap", &format!("gui/{uid}")])
                .arg(&plist),
        )
}

/// Identifiant de l'utilisateur, par `id -u` (pas de `libc` dans les dépendances).
#[cfg(target_os = "macos")]
fn current_uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|uid| uid.trim().to_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn start_service(label: &str) -> bool {
    let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    else {
        return false;
    };
    config
        .join(format!("systemd/user/{label}.service"))
        .is_file()
        && succeeds(Command::new("systemctl").args(["--user", "start", label]))
}

/// Pas de service sous Windows (D46) : le daemon est lancé en process.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn start_service(_label: &str) -> bool {
    false
}

fn succeeds(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// `argos-daemon hub` hors du groupe de process de l'app : quitter l'app ne l'arrête pas.
/// Sa sortie va dans `$ARGOS_HOME/daemon.log`.
fn spawn_detached(binary: &Path, home: &Path) -> Result<(), String> {
    std::fs::create_dir_all(home).map_err(|error| format!("{}: {error}", home.display()))?;
    let log_path = home.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("{}: {error}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .map_err(|error| format!("{}: {error}", log_path.display()))?;
    let mut command = Command::new(binary);
    command
        .arg("hub")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(drop)
        .map_err(|error| format!("{}: {error}", binary.display()))
}
