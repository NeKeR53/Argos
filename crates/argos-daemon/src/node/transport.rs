//! Comment le hub atteint un nœud (D32) : directement (nœud simulé sur la même machine,
//! réseau privé) ou par un tunnel SSH qu'il ouvre lui-même vers le port local du nœud.
//! Le daemon du nœud n'écoute jamais que sur sa boucle locale.

use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

/// Délai laissé au tunnel SSH pour s'ouvrir.
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(20);

/// Un nœud déclaré dans la configuration du hub (`[nodes.<nom>]`). L'ajout d'un nœud
/// reste un geste humain (D33) : l'Assistant ne place des agents que sur ceux-là.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    /// Adresse WebSocket du nœud, sans tunnel : `ws://127.0.0.1:7880`.
    pub url: Option<String>,
    /// Ou bien l'hôte SSH (un nom de `~/.ssh/config`) à travers lequel l'atteindre.
    pub ssh: Option<String>,
    /// Adresse d'écoute du nœud, vue depuis l'hôte SSH.
    #[serde(default = "default_remote")]
    pub remote: SocketAddr,
    /// Jeton du nœud (son fichier `node-token`), en clair ou par un fichier.
    pub token: Option<String>,
    pub token_file: Option<PathBuf>,
}

fn default_remote() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7880))
}

impl NodeConfig {
    pub fn check(&self, name: &str) -> Result<(), String> {
        match (&self.url, &self.ssh) {
            (Some(_), Some(_)) => Err(format!("nœud {name} : url ou ssh, pas les deux")),
            (None, None) => Err(format!("nœud {name} : url ou ssh manquant")),
            _ if self.token.is_none() && self.token_file.is_none() => {
                Err(format!("nœud {name} : token ou token_file manquant"))
            }
            _ => Ok(()),
        }
    }

    pub fn token(&self) -> std::io::Result<String> {
        match (&self.token, &self.token_file) {
            (Some(token), _) => Ok(token.trim().to_string()),
            (None, Some(path)) => Ok(std::fs::read_to_string(path)?.trim().to_string()),
            (None, None) => Err(std::io::Error::other("aucun jeton")),
        }
    }
}

/// Une voie ouverte vers un nœud : l'adresse WebSocket à joindre, et le tunnel qui la
/// porte, fermé quand la voie est abandonnée.
pub struct Opened {
    pub url: String,
    pub tunnel: Option<Child>,
}

/// Ouvre la voie vers un nœud. `ssh` est le binaire à lancer (`ARGOS_SSH_BIN` pour les
/// tests, sinon `ssh`).
pub async fn open(config: &NodeConfig, ssh: &std::path::Path) -> Result<Opened, String> {
    if let Some(url) = &config.url {
        return Ok(Opened {
            url: url.clone(),
            tunnel: None,
        });
    }
    let host = config.ssh.as_deref().ok_or("ni url ni ssh")?;
    let port = free_port().await.map_err(|error| error.to_string())?;
    let mut command = Command::new(ssh);
    command
        .args(tunnel_arguments(host, port, config.remote))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut tunnel = command
        .spawn()
        .map_err(|error| format!("lancement de {} : {error}", ssh.display()))?;
    let deadline = tokio::time::Instant::now() + TUNNEL_TIMEOUT;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(Opened {
                url: format!("ws://127.0.0.1:{port}"),
                tunnel: Some(tunnel),
            });
        }
        if let Ok(Some(status)) = tunnel.try_wait() {
            return Err(format!("tunnel SSH vers {host} fermé ({status})"));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("tunnel SSH vers {host} : pas ouvert à temps"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `ssh -N -L <port>:<remote> <hôte>`, sans question interactive : un tunnel qui ne
/// peut pas s'ouvrir doit échouer, pas attendre un mot de passe.
pub fn tunnel_arguments(host: &str, port: u16, remote: SocketAddr) -> Vec<String> {
    [
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
    ]
    .into_iter()
    .map(String::from)
    .chain([format!("127.0.0.1:{port}:{remote}"), host.to_string()])
    .collect()
}

async fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_needs_one_way_in_and_a_token() {
        let node = |toml: &str| toml::from_str::<NodeConfig>(toml).unwrap();
        assert!(
            node("url = \"ws://127.0.0.1:7880\"\ntoken = \"x\"")
                .check("pi")
                .is_ok()
        );
        assert!(
            node("ssh = \"pi\"\ntoken_file = \"/t\"")
                .check("pi")
                .is_ok()
        );
        assert!(
            node("url = \"ws://a\"\nssh = \"pi\"\ntoken = \"x\"")
                .check("pi")
                .is_err()
        );
        assert!(node("token = \"x\"").check("pi").is_err());
        assert!(node("ssh = \"pi\"").check("pi").is_err());
        assert!(toml::from_str::<NodeConfig>("ssh = \"pi\"\nhost = \"x\"").is_err());
    }

    #[test]
    fn the_tunnel_forwards_a_loopback_port_without_prompting() {
        let args = tunnel_arguments("vps", 40000, default_remote());
        assert_eq!(args.first().map(String::as_str), Some("-N"));
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"ExitOnForwardFailure=yes".to_string()));
        assert_eq!(
            &args[args.len() - 3..],
            ["-L", "127.0.0.1:40000:127.0.0.1:7880", "vps"]
        );
    }
}
