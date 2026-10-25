//! Accès au daemon local : URL de son API WebSocket (lue dans `$ARGOS_HOME`, comme le
//! fait le daemon) et sonde qui distingue un daemon absent d'un jeton refusé, ce que
//! le WebSocket du navigateur ne sait pas faire.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Adresse par défaut de l'API, identique à celle de la configuration du daemon.
const DEFAULT_LISTEN: &str = "127.0.0.1:7878";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Seule la section `[api]` intéresse la GUI ; le reste du fichier est ignoré.
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    api: ApiSection,
}

#[derive(Debug, Default, Deserialize)]
struct ApiSection {
    listen: Option<String>,
}

/// Répertoire de données du daemon : `$ARGOS_HOME`, sinon `~/.argos`.
pub fn argos_home() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("ARGOS_HOME") {
        return Ok(dir.into());
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".argos"))
        .ok_or_else(|| "ni ARGOS_HOME ni HOME ne sont définis".into())
}

/// URL complète de l'API (`ws://<listen>/?token=<jeton>`) d'après les fichiers du daemon.
pub fn url_in(home: &Path) -> Result<String, String> {
    let token_path = home.join("token");
    let token = std::fs::read_to_string(&token_path).map_err(|error| {
        format!(
            "jeton illisible ({}) : le daemon a-t-il déjà démarré ? {error}",
            token_path.display()
        )
    })?;
    // Pas de config : le daemon n'a jamais démarré ici, ou elle a été supprimée.
    let config = std::fs::read_to_string(home.join("config.toml")).ok();
    url_from(config.as_deref(), &token)
}

/// Assemble l'URL à partir du contenu de `config.toml` (s'il existe) et du jeton.
fn url_from(config: Option<&str>, token: &str) -> Result<String, String> {
    let parsed: ConfigFile = match config {
        Some(text) => {
            toml::from_str(text).map_err(|error| format!("config.toml illisible : {error}"))?
        }
        None => ConfigFile::default(),
    };
    let listen = parsed.api.listen.as_deref().unwrap_or(DEFAULT_LISTEN);
    let address: SocketAddr = listen
        .parse()
        .map_err(|_| format!("adresse [api] listen invalide : {listen:?}"))?;
    let token = token.trim();
    if token.is_empty() {
        return Err("le fichier token est vide".into());
    }
    // Le jeton du daemon est hexadécimal : rien à échapper dans l'URL.
    Ok(format!("ws://{address}/?token={token}"))
}

/// Ce que le daemon répond à une poignée de main WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Probe {
    /// La connexion serait acceptée.
    Accepted,
    /// Le daemon écoute mais refuse le jeton.
    Refused,
    /// Personne n'écoute, ou la réponse n'est pas celle d'un daemon Argos.
    Unreachable,
}

/// Tente une poignée de main sur `url` (`ws://hôte:port/chemin`) et lit le statut HTTP.
/// La connexion est fermée aussitôt : le daemon voit un client qui part.
pub fn probe(url: &str) -> Probe {
    probe_handshake(url).unwrap_or(Probe::Unreachable)
}

fn probe_handshake(url: &str) -> Option<Probe> {
    let rest = url.strip_prefix("ws://")?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, "/"),
    };
    let address = authority.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&address, PROBE_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT)).ok()?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line).ok()?;
    Some(classify(&status_line))
}

/// `HTTP/1.1 401 Unauthorized` → refusé, `101` → accepté.
fn classify(status_line: &str) -> Probe {
    match status_line.split_whitespace().nth(1) {
        Some("101") => Probe::Accepted,
        Some("401") => Probe::Refused,
        _ => Probe::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_uses_the_configured_address_and_the_token() {
        let config = "[assistant]\nengine = \"claude\"\n\n[api]\nlisten = \"127.0.0.1:9000\"\n";
        assert_eq!(
            url_from(Some(config), "abc123\n").unwrap(),
            "ws://127.0.0.1:9000/?token=abc123"
        );
    }

    #[test]
    fn a_missing_address_falls_back_to_the_default() {
        assert_eq!(
            url_from(Some("[assistant]\nengine = \"codex\"\n"), "t").unwrap(),
            "ws://127.0.0.1:7878/?token=t"
        );
        assert_eq!(url_from(None, "t").unwrap(), "ws://127.0.0.1:7878/?token=t");
    }

    #[test]
    fn broken_inputs_are_reported() {
        assert!(url_from(Some("[api]\nlisten = 3"), "t").is_err());
        assert!(url_from(Some("[api]\nlisten = \"pas une adresse\""), "t").is_err());
        assert!(url_from(None, "  \n").is_err());
    }

    #[test]
    fn the_status_line_tells_a_refused_token_apart() {
        assert_eq!(
            classify("HTTP/1.1 101 Switching Protocols\r\n"),
            Probe::Accepted
        );
        assert_eq!(classify("HTTP/1.1 401 Unauthorized\r\n"), Probe::Refused);
        assert_eq!(classify(""), Probe::Unreachable);
        assert_eq!(classify("SSH-2.0-OpenSSH_9.9\r\n"), Probe::Unreachable);
    }

    #[test]
    fn nobody_listening_is_unreachable() {
        // Port pris puis libéré : plus personne n'y écoute.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert_eq!(
            probe(&format!("ws://127.0.0.1:{port}/?token=t")),
            Probe::Unreachable
        );
        assert_eq!(probe("http://127.0.0.1:1/"), Probe::Unreachable);
    }

    /// Faux daemon : lit la poignée de main, vérifie le chemin, répond `status_line`.
    fn answering(status_line: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            assert_eq!(request, "GET /?token=t HTTP/1.1\r\n");
            (&stream).write_all(status_line.as_bytes()).unwrap();
        });
        format!("ws://127.0.0.1:{port}/?token=t")
    }

    #[test]
    fn the_probe_reads_the_daemon_answer() {
        let refused = answering("HTTP/1.1 401 Unauthorized\r\n\r\njeton absent ou invalide");
        assert_eq!(probe(&refused), Probe::Refused);
        let accepted = answering("HTTP/1.1 101 Switching Protocols\r\n\r\n");
        assert_eq!(probe(&accepted), Probe::Accepted);
    }
}
