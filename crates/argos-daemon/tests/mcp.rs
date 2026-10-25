//! Serveur MCP et outils par niveau, face à un superviseur sur faux CLI : aucun token.

mod common;

use argos_core::{AgentId, Level};
use common::world::{World, is_message, until, world};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Lance le serveur MCP sur un port libre ; rend son adresse.
async fn serve(world: &World) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(argos_daemon::mcp::serve(listener, world.supervisor.clone()));
    address
}

/// POST HTTP/1.1 minimal ; rend le code et le corps.
async fn post(address: &str, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, body.to_string())
}

struct Client {
    address: String,
    token: String,
}

impl Client {
    fn new(world: &World, address: &str, agent: &AgentId) -> Self {
        Self {
            address: address.into(),
            token: world.supervisor.mcp_token(agent),
        }
    }

    async fn rpc(&self, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let (status, body) = post(
            &self.address,
            &format!("/mcp/{}", self.token),
            &body.to_string(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        serde_json::from_str(&body).unwrap()
    }

    async fn tools(&self) -> Vec<String> {
        let reply = self.rpc("tools/list", json!({})).await;
        reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Appelle un outil ; rend son texte et s'il s'agit d'une erreur.
    async fn call(&self, name: &str, arguments: Value) -> (String, bool) {
        let reply = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        let result = &reply["result"];
        (
            result["content"][0]["text"].as_str().unwrap().to_string(),
            result["isError"].as_bool().unwrap(),
        )
    }
}

#[tokio::test]
async fn an_unknown_token_is_refused() {
    let w = world();
    let address = serve(&w).await;
    let (status, _) = post(
        &address,
        "/mcp/faux",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn initialize_echoes_the_protocol_and_offers_tools() {
    let w = world();
    let address = serve(&w).await;
    let client = Client::new(&w, &address, &w.assistant.id);
    let reply = client
        .rpc("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}))
        .await;
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(reply["result"]["capabilities"], json!({"tools": {}}));

    // Une notification n'appelle pas de réponse.
    let (status, body) = post(
        &address,
        &format!("/mcp/{}", client.token),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    assert_eq!((status, body.as_str()), (202, ""));
}

#[tokio::test]
async fn each_caller_sees_the_tools_of_its_level() {
    let w = world();
    let address = serve(&w).await;
    let assistant = Client::new(&w, &address, &w.assistant.id).tools().await;
    let agent = Client::new(&w, &address, &w.agent.id).tools().await;
    let worker = Client::new(&w, &address, &w.worker.id).tools().await;

    assert!(assistant.contains(&"create_agent".to_string()));
    assert!(agent.contains(&"spawn_worker".to_string()));
    assert_eq!(worker, vec!["report_to_agent", "escalate_to_agent"]);
}

#[tokio::test]
async fn the_assistant_creates_and_lists_agents() {
    let w = world();
    let address = serve(&w).await;
    let assistant = Client::new(&w, &address, &w.assistant.id);

    let (created, error) = assistant
        .call(
            "create_agent",
            json!({"name": "Compta", "scope": "les factures"}),
        )
        .await;
    assert!(!error, "{created}");
    let compta = w
        .supervisor
        .registry()
        .agent_named("compta")
        .unwrap()
        .unwrap();
    assert_eq!(compta.level, Level::Agent);
    assert_eq!(compta.engine, w.assistant.engine);
    assert!(compta.workdir.starts_with(w.home.path().join("agents")));

    let (listed, _) = assistant.call("list_agents", json!({})).await;
    let names: Vec<String> = serde_json::from_str::<Vec<Value>>(&listed)
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["Domotique", "Compta"]);

    let (duplicate, error) = assistant
        .call("create_agent", json!({"name": "compta", "scope": "encore"}))
        .await;
    assert!(error, "{duplicate}");
}

#[tokio::test]
async fn a_mission_wakes_the_agent_named_by_the_assistant() {
    let mut w = world();
    let address = serve(&w).await;
    let assistant = Client::new(&w, &address, &w.assistant.id);
    let (_, error) = assistant
        .call(
            "send_to_agent",
            json!({"agent": "Domotique", "message": "allume le salon"}),
        )
        .await;
    assert!(!error);
    let agent = w.agent.id.clone();
    until(&mut w.events, |event| is_message(event, &agent, "reply 1")).await;
}

#[tokio::test]
async fn an_agent_spawns_a_worker_on_a_task() {
    let mut w = world();
    let address = serve(&w).await;
    let agent = Client::new(&w, &address, &w.agent.id);

    let (spawned, error) = agent
        .call("spawn_worker", json!({"task": "liste les ampoules"}))
        .await;
    assert!(!error, "{spawned}");
    let workers = w.supervisor.children(&w.agent.id).unwrap();
    assert_eq!(workers.len(), 2, "le Worker du monde de test et le nouveau");
    let new = workers.last().unwrap().record.clone();
    assert_eq!(new.workdir, w.agent.workdir);
    until(&mut w.events, |event| is_message(event, &new.id, "reply 1")).await;
}

#[tokio::test]
async fn a_worker_report_wakes_its_agent() {
    let mut w = world();
    let address = serve(&w).await;
    let worker = Client::new(&w, &address, &w.worker.id);
    let (_, error) = worker
        .call("report_to_agent", json!({"summary": "3 ampoules"}))
        .await;
    assert!(!error);
    let agent = w.agent.id.clone();
    until(&mut w.events, |event| is_message(event, &agent, "reply 1")).await;
}

#[tokio::test]
async fn nobody_acts_outside_its_own_children() {
    let w = world();
    let address = serve(&w).await;
    let assistant = Client::new(&w, &address, &w.assistant.id);
    let agent = Client::new(&w, &address, &w.agent.id);

    // L'Assistant ne touche pas un Worker, même en passant son id.
    let (_, error) = assistant
        .call("stop_agent", json!({"agent": w.worker.id.as_str()}))
        .await;
    assert!(error);
    // Ni l'outil d'un autre niveau.
    let (_, error) = assistant.call("spawn_worker", json!({"task": "x"})).await;
    assert!(error);
    // Un Agent ne touche pas le Worker d'un autre Agent.
    let (_, created) = assistant
        .call(
            "create_agent",
            json!({"name": "Compta", "scope": "les factures"}),
        )
        .await;
    assert!(!created);
    let compta = w
        .supervisor
        .registry()
        .agent_named("Compta")
        .unwrap()
        .unwrap();
    let other = Client::new(&w, &address, &compta.id);
    let (_, error) = other
        .call("stop_worker", json!({"worker": w.worker.id.as_str()}))
        .await;
    assert!(error);
    // Il agit bien sur le sien.
    let (_, error) = agent
        .call("inspect_worker", json!({"worker": w.worker.id.as_str()}))
        .await;
    assert!(!error);
}

#[tokio::test]
async fn tokens_die_with_their_session() {
    let mut w = world();
    let address = serve(&w).await;
    let id = w.agent.id.clone();
    let client = Client::new(&w, &address, &id);
    assert_eq!(client.tools().await.len(), 10);

    w.supervisor.send(&id, "bonjour").await.unwrap();
    until(&mut w.events, |event| is_message(event, &id, "reply 1")).await;
    w.supervisor.park(&id).await.unwrap();

    let (status, _) = post(
        &address,
        &format!("/mcp/{}", client.token),
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    )
    .await;
    assert_eq!(status, 404);
}
