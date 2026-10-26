//! Client HTTP/1.1 minimal pour parler à `whisper-server` sur la boucle locale :
//! une requête par connexion, corps multipart construit à la main. Cela évite d'embarquer
//! une pile HTTP complète pour deux appels vers 127.0.0.1.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Taille maximale d'une réponse acceptée : un texte JSON, jamais gros.
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

/// Corps `multipart/form-data`.
pub(crate) struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    /// Frontière aléatoire : elle ne doit pas apparaître dans les données binaires.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_default();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self::with_boundary(format!(
            "argos-voice-{nanos:016x}{:08x}{count:08x}",
            std::process::id()
        ))
    }

    pub fn with_boundary(boundary: String) -> Self {
        Self {
            boundary,
            body: Vec::new(),
        }
    }

    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.part_header(name, None, None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn file(mut self, name: &str, filename: &str, content_type: &str, data: &[u8]) -> Self {
        self.part_header(name, Some(filename), Some(content_type));
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    fn part_header(&mut self, name: &str, filename: Option<&str>, content_type: Option<&str>) {
        let mut header = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{name}\"",
            self.boundary
        );
        if let Some(filename) = filename {
            header.push_str(&format!("; filename=\"{filename}\""));
        }
        header.push_str("\r\n");
        if let Some(content_type) = content_type {
            header.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        header.push_str("\r\n");
        self.body.extend_from_slice(header.as_bytes());
    }

    /// Rend l'en-tête `Content-Type` et le corps complet.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Envoie une requête et lit la réponse jusqu'à la fermeture (`Connection: close`).
pub(crate) async fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<(&str, &[u8])>,
) -> io::Result<Response> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some((content_type, data)) = body {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            data.len()
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if let Some((_, data)) = body {
        stream.write_all(data).await?;
    }
    stream.flush().await?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() > MAX_RESPONSE {
            return Err(invalid("réponse trop volumineuse"));
        }
        // On s'arrête dès que le corps annoncé est complet, sans attendre la fermeture.
        if let Ok(Some(response)) = parse_response(&raw, false) {
            return Ok(response);
        }
    }
    parse_response(&raw, true)?.ok_or_else(|| invalid("réponse tronquée"))
}

/// Analyse une réponse brute. `Ok(None)` : il manque encore des octets. `eof` indique
/// que la connexion est fermée, ce qui termine un corps sans longueur annoncée.
pub(crate) fn parse_response(raw: &[u8], eof: bool) -> io::Result<Option<Response>> {
    let Some(head_end) = find(raw, b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&raw[..head_end]).map_err(|_| invalid("en-tête non UTF-8"))?;
    let rest = &raw[head_end + 4..];
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| invalid("ligne de statut illisible"))?;
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| invalid("Content-Length illisible"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        }
    }
    if chunked {
        return Ok(decode_chunked(rest)?.map(|body| Response { status, body }));
    }
    let body = match content_length {
        Some(length) if rest.len() >= length => Some(rest[..length].to_vec()),
        Some(_) => None,
        // Sans longueur, le corps court jusqu'à la fermeture de la connexion.
        None => eof.then(|| rest.to_vec()),
    };
    Ok(body.map(|body| Response { status, body }))
}

fn decode_chunked(mut data: &[u8]) -> io::Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = find(data, b"\r\n") else {
            return Ok(None);
        };
        let size_line = std::str::from_utf8(&data[..line_end])
            .map_err(|_| invalid("taille de bloc illisible"))?;
        let size_hex = size_line.split(';').next().unwrap_or_default().trim();
        let size =
            usize::from_str_radix(size_hex, 16).map_err(|_| invalid("taille de bloc illisible"))?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        if data.len() < size + 2 {
            return Ok(None);
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_body_is_byte_exact() {
        let (content_type, body) = Multipart::with_boundary("XYZ".into())
            .text("language", "fr")
            .file("file", "audio.wav", "audio/wav", b"RIFF\x00\x01")
            .finish();
        assert_eq!(content_type, "multipart/form-data; boundary=XYZ");
        let expected: &[u8] = b"--XYZ\r\n\
Content-Disposition: form-data; name=\"language\"\r\n\
\r\n\
fr\r\n\
--XYZ\r\n\
Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
Content-Type: audio/wav\r\n\
\r\n\
RIFF\x00\x01\r\n\
--XYZ--\r\n";
        assert_eq!(body, expected);
    }

    #[test]
    fn boundaries_differ_between_bodies() {
        let (a, _) = Multipart::new().finish();
        let (b, _) = Multipart::new().finish();
        assert_ne!(a, b);
    }

    #[test]
    fn a_response_with_content_length_is_parsed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"text\":\"a\"}\n";
        assert_eq!(
            parse_response(raw, false).unwrap(),
            Some(Response {
                status: 200,
                body: b"{\"text\":\"a\"}\n".to_vec()
            })
        );
        // Corps incomplet : il faut lire la suite.
        assert_eq!(parse_response(&raw[..raw.len() - 3], false).unwrap(), None);
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-", false).unwrap(),
            None
        );
    }

    #[test]
    fn a_body_without_length_ends_with_the_connection() {
        let raw = b"HTTP/1.1 200 OK\r\nServer: x\r\n\r\nhello";
        assert_eq!(parse_response(raw, false).unwrap(), None);
        assert_eq!(parse_response(raw, true).unwrap().unwrap().body, b"hello");
    }

    #[test]
    fn a_chunked_response_is_decoded() {
        let raw = b"HTTP/1.1 503 Service Unavailable\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nbusy\r\n3;x=1\r\n!!!\r\n0\r\n\r\n";
        let response = parse_response(raw, false).unwrap().unwrap();
        assert_eq!(response.status, 503);
        assert_eq!(response.body, b"busy!!!");
        assert_eq!(parse_response(&raw[..raw.len() - 7], false).unwrap(), None);
    }

    #[test]
    fn a_garbage_status_line_is_an_error() {
        assert!(parse_response(b"bonjour\r\n\r\n", false).is_err());
    }

    #[tokio::test]
    async fn a_request_round_trips_through_a_local_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let mut chunk = [0u8; 1024];
            while find(&received, b"ping").is_none() {
                let read = socket.read(&mut chunk).await.unwrap();
                received.extend_from_slice(&chunk[..read]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\npong")
                .await
                .unwrap();
            String::from_utf8(received).unwrap()
        });
        let response = request(addr, "POST", "/inference", Some(("text/plain", b"ping")))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"pong");
        let received = server.await.unwrap();
        assert!(received.starts_with("POST /inference HTTP/1.1\r\n"));
        assert!(received.contains("Content-Length: 4\r\n"));
        assert!(received.contains("Connection: close\r\n"));
    }
}
