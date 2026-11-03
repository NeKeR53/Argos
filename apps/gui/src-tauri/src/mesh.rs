//! L'application reliée à tout le maillage (D48) : les autres nœuds sont ceux de
//! `[peers]` dans la config locale. Pour chacun, l'app ouvre elle-même un tunnel SSH vers
//! son API (`api`, par défaut `127.0.0.1:7878` vu depuis l'hôte) et s'y présente avec le
//! jeton du pair. Un nœud joint par `url` donne l'adresse de son API dans `api_url`.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Délai laissé au tunnel pour s'ouvrir.
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, Deserialize)]
struct ConfigFile {
    #[serde(default, alias = "nodes")]
    peers: BTreeMap<String, Peer>,
}

#[derive(Debug, Clone, Deserialize)]
struct Peer {
    ssh: Option<String>,
    /// API du nœud vue depuis son hôte SSH.
    api: Option<SocketAddr>,
    /// Ou bien l'adresse WebSocket directe de son API.
    api_url: Option<String>,
    token: Option<String>,
    token_file: Option<PathBuf>,
}

/// Les tunnels ouverts par l'app, par nœud ; fermés à sa sortie.
#[derive(Default)]
pub struct Tunnels {
    open: Mutex<HashMap<String, (Child, u16)>>,
}

impl Tunnels {
    /// Ferme tous les tunnels.
    pub fn shutdown(&self) {
        for (_, (mut child, _)) in locked(&self.open).drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn peers_in(home: &Path) -> BTreeMap<String, Peer> {
    std::fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<ConfigFile>(&text).ok())
        .unwrap_or_default()
        .peers
        .into_iter()
        .filter(|(_, peer)| peer.ssh.is_some() || peer.api_url.is_some())
        .collect()
}

/// Les autres nœuds que l'app peut joindre.
pub fn names(home: &Path) -> Vec<String> {
    peers_in(home).into_keys().collect()
}

/// L'URL de l'API d'un nœud, son tunnel ouvert au besoin.
pub fn endpoint(home: &Path, tunnels: &Tunnels, name: &str) -> Result<String, String> {
    let peer = peers_in(home)
        .remove(name)
        .ok_or_else(|| format!("nœud inconnu : {name}"))?;
    let token = match (&peer.token, &peer.token_file) {
        (Some(token), _) => token.trim().to_string(),
        (None, Some(path)) => std::fs::read_to_string(expand(path))
            .map_err(|error| format!("jeton du nœud {name} : {error}"))?
            .trim()
            .to_string(),
        (None, None) => return Err(format!("nœud {name} : token ou token_file manquant")),
    };
    if let Some(url) = &peer.api_url {
        return Ok(format!("{}/?token={token}", url.trim_end_matches('/')));
    }
    let host = peer.ssh.clone().expect("filtré plus haut");
    let api = peer
        .api
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 7878)));
    let port = tunnel(tunnels, name, &host, api)?;
    Ok(format!("ws://127.0.0.1:{port}/?token={token}"))
}

/// Le port local du tunnel vers ce nœud : réutilisé s'il tient encore, rouvert sinon.
fn tunnel(tunnels: &Tunnels, name: &str, host: &str, api: SocketAddr) -> Result<u16, String> {
    let mut open = locked(&tunnels.open);
    if let Some((child, port)) = open.get_mut(name) {
        if matches!(child.try_wait(), Ok(None)) {
            return Ok(*port);
        }
        open.remove(name);
    }
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map_err(|error| error.to_string())?
        .port();
    let mut child = Command::new("ssh")
        .args([
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-L",
            &format!("127.0.0.1:{port}:{api}"),
            host,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("ssh : {error}"))?;
    let started = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            open.insert(name.to_string(), (child, port));
            return Ok(port);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("tunnel SSH vers {host} fermé ({status})"));
        }
        if started.elapsed() > TUNNEL_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("tunnel SSH vers {host} : pas ouvert en 20 s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn expand(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_are_read_from_config_with_their_api() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("config.toml"),
            "[assistant]\nengine = \"claude\"\n[peers.hermes]\nssh = \"hermes-app\"\ntoken = \"t\"\n\
             [peers.vps]\nurl = \"ws://127.0.0.1:7880\"\napi_url = \"ws://127.0.0.1:9878\"\ntoken = \"u\"\n\
             [peers.muet]\nurl = \"ws://127.0.0.1:7881\"\ntoken = \"v\"\n",
        )
        .unwrap();
        assert_eq!(names(&dir), ["hermes", "vps"]);
        let tunnels = Tunnels::default();
        assert_eq!(
            endpoint(&dir, &tunnels, "vps").unwrap(),
            "ws://127.0.0.1:9878/?token=u"
        );
        assert!(endpoint(&dir, &tunnels, "muet").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("argos-mesh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
