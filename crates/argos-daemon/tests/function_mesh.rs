//! Fonctions HTTP sur un vrai nœud local, sans lancer de session de modèle.

use argos_core::{AgentId, FunctionRequest};
use argos_daemon::node::{FromNode, NODE_PROTOCOL, ToNode, runner};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

async fn socket() -> (
    tempfile::TempDir,
    tokio::task::JoinHandle<std::io::Result<()>>,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) {
    let home = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let settings = runner::RunnerSettings {
        name: "pi".into(),
        home: home.path().to_path_buf(),
        user_codex_home: home.path().join("user-codex"),
        programs: HashMap::new(),
    };
    let task = tokio::spawn(runner::serve(listener, mcp, settings, "test".into()));
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/?token=test"))
        .await
        .unwrap();
    let hello: FromNode =
        serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert!(matches!(
        hello,
        FromNode::Hello {
            protocol: NODE_PROTOCOL,
            ..
        }
    ));
    (home, task, socket)
}

fn request(address: std::net::SocketAddr) -> FunctionRequest {
    serde_json::from_value(
        json!({"method":"POST", "url":format!("http://{address}/light"),
        "body":{"state":"{{state}}"}}),
    )
    .unwrap()
}

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    order: ToNode,
) {
    socket
        .send(Message::text(serde_json::to_string(&order).unwrap()))
        .await
        .unwrap();
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await.unwrap();
        assert!(count > 0, "requête HTTP interrompue");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn a_remote_function_runs_without_an_agent_process() {
    let (home, runner, mut socket) = socket().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let http = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let received = read_request(&mut stream).await;
        assert!(received.contains("POST /light"));
        assert!(received.contains("\"state\":true"));
        let body = r#"{"on":true}"#;
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    send(
        &mut socket,
        ToNode::Function {
            call: 7,
            agent: AgentId::new("parked-agent"),
            request: request(address),
            arguments: json!({"state":true}),
        },
    )
    .await;
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: FromNode = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert!(
        matches!(reply, FromNode::FunctionReply { call:7, result:Ok(result) } if result == json!({"on":true}))
    );
    http.await.unwrap();
    assert!(
        !home.path().join("agents").exists(),
        "aucune session à préparer"
    );
    socket.close(None).await.unwrap();
    runner.abort();
}

#[tokio::test]
async fn cancellation_and_disconnect_close_the_remote_http_call() {
    for disconnect in [false, true] {
        let (_home, runner, mut socket) = socket().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (started, received) = tokio::sync::oneshot::channel();
        let http = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            let _ = started.send(());
            // La requête ne reçoit aucune réponse : seul l'abandon ferme la connexion.
            let mut byte = [0; 1];
            loop {
                if stream.read(&mut byte).await.unwrap() == 0 {
                    break;
                }
            }
        });
        let mut definition = request(address);
        definition.method = "GET".into();
        definition.body = None;
        send(
            &mut socket,
            ToNode::Function {
                call: 8,
                agent: AgentId::new("parked-agent"),
                request: definition,
                arguments: json!({}),
            },
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), received)
            .await
            .unwrap()
            .unwrap();
        if disconnect {
            socket.close(None).await.unwrap();
        } else {
            send(&mut socket, ToNode::CancelFunction { call: 8 }).await;
        }
        tokio::time::timeout(Duration::from_secs(5), http)
            .await
            .unwrap()
            .unwrap();
        runner.abort();
    }
}
