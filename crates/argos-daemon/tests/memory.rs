//! Mémoire des agents de bout en bout, sur faux CLI : outils, digest au réveil, journal,
//! compaction contrôlée. Aucun token.

mod common;

use argos_core::protocol::DaemonEvent;
use common::world::{World, is_message, poll, turn_completed, until, world};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Appelle un outil MCP au nom d'un agent ; rend son texte et s'il s'agit d'une erreur.
async fn call(
    w: &World,
    agent: &argos_core::AgentId,
    name: &str,
    arguments: Value,
) -> (String, bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(argos_daemon::mcp::serve(listener, w.supervisor.clone()));
    let token = w.supervisor.mcp_token(agent);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).to_string();
    let mut stream = TcpStream::connect(&address).await.unwrap();
    let request = format!(
        "POST /mcp/{token} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let reply: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    let result = &reply["result"];
    (
        result["content"][0]["text"].as_str().unwrap().into(),
        result["isError"].as_bool().unwrap(),
    )
}

/// Arguments reçus par le faux CLI lancé dans ce répertoire de travail.
fn args_in(w: &World, workdir: &str) -> Vec<String> {
    std::fs::read_to_string(w.home.path().join(workdir).join(".fake-args"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

/// Ce qui suit `--append-system-prompt` dans les arguments reçus. Le faux CLI les écrit
/// un par ligne, et le digest est multiligne : on prend tout le texte qui suit.
fn instructions(w: &World, workdir: &str) -> Option<String> {
    let raw = std::fs::read_to_string(w.home.path().join(workdir).join(".fake-args")).unwrap();
    raw.split_once("--append-system-prompt\n")
        .map(|(_, rest)| rest.to_string())
}

#[tokio::test]
async fn an_agent_writes_reads_and_searches_its_memory() {
    let w = world();
    let agent = w.agent.id.clone();
    let (_, error) = call(
        &w,
        &agent,
        "memory_write",
        json!({"file": "index.md", "content": "Je gère la maison."}),
    )
    .await;
    assert!(!error);
    let (_, error) = call(
        &w,
        &agent,
        "memory_write",
        json!({"file": "notes/salon.md", "content": "Pont Hue"}),
    )
    .await;
    assert!(!error);
    let (read, _) = call(&w, &agent, "memory_read", json!({"file": "index.md"})).await;
    assert_eq!(read, "Je gère la maison.\n");
    let (listed, _) = call(&w, &agent, "memory_read", json!({})).await;
    assert!(listed.contains("notes/salon.md"));
    let (found, _) = call(&w, &agent, "memory_search", json!({"query": "hue"})).await;
    assert!(found.starts_with("notes/salon.md:1"), "{found}");

    let (refusal, error) = call(
        &w,
        &agent,
        "memory_write",
        json!({"file": "../evasion.md", "content": "x"}),
    )
    .await;
    assert!(error, "{refusal}");
    // Un Worker n'a pas de mémoire.
    let (_, error) = call(&w, &w.worker.id, "memory_read", json!({})).await;
    assert!(error);
}

#[tokio::test]
async fn waking_an_agent_hands_it_its_memory_digest() {
    let mut w = world();
    let agent = w.agent.id.clone();
    w.supervisor.memory().ensure(&agent).unwrap();
    w.supervisor
        .memory()
        .write(&agent, "index.md", "Je gère la maison.", false)
        .unwrap();

    w.supervisor.send(&agent, "bonjour").await.unwrap();
    turn_completed(&mut w.events, &agent).await;

    let digest = instructions(&w, "agent").expect("digest dans le prompt système");
    // Le contrat de l'Agent vient d'abord, la mémoire ensuite.
    let contract_at = digest
        .find("## Ton rôle : l'Agent « Domotique »")
        .expect("contrat");
    let memory_at = digest.find("## Ta mémoire").expect("mémoire");
    assert!(contract_at < memory_at);
    assert!(digest.contains("Je gère la maison."));
    assert!(digest.contains("Rôle : Agent « Domotique »"));
    assert!(
        digest.contains(w.worker.id.as_str()),
        "ses Workers figurent dans l'état"
    );
    let state = w.supervisor.memory().read(&agent, "state.md").unwrap();
    assert!(state.contains("## Ton board"));
}

#[tokio::test]
async fn a_worker_wakes_without_memory() {
    let mut w = world();
    let worker = w.worker.id.clone();
    w.supervisor.send(&worker, "bonjour").await.unwrap();
    turn_completed(&mut w.events, &worker).await;
    assert!(!w.supervisor.memory().dir(&worker).exists());
}

#[tokio::test]
async fn missions_are_logged_in_both_journals() {
    let w = world();
    let (_, error) = call(
        &w,
        &w.assistant.id,
        "assign_mission",
        json!({"agent": "Domotique", "mission": "refaire l'éclairage"}),
    )
    .await;
    assert!(!error);
    let memory = w.supervisor.memory();
    let agent_journal = memory.read(&w.agent.id, "journal.md").unwrap();
    assert!(
        agent_journal.contains("reçue : refaire l'éclairage"),
        "{agent_journal}"
    );
    let assistant_journal = memory.read(&w.assistant.id, "journal.md").unwrap();
    assert!(
        assistant_journal.contains("confiée à l'Agent « Domotique »"),
        "{assistant_journal}"
    );
}

#[tokio::test]
async fn a_nearly_full_context_is_saved_to_memory_then_renewed() {
    let mut w = world();
    let agent = w.agent.id.clone();
    w.supervisor.send(&agent, "FULL").await.unwrap();
    let first_session = w
        .supervisor
        .inspect(&agent)
        .unwrap()
        .record
        .session_id
        .unwrap();

    // Argos demande à l'agent d'écrire sa mémoire…
    turn_completed(&mut w.events, &agent).await;
    until(&mut w.events, |event| is_message(event, &agent, "reply 2")).await;
    // …puis, ce tour fini, range la session et l'oublie.
    poll(|| {
        let record = w.supervisor.inspect(&agent).ok()?.record;
        record.session_id.is_none().then_some(())
    })
    .await;
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::AgentUpdated { agent: a } if a.record.id == agent && !a.running)
    })
    .await;
    let journal = w.supervisor.memory().read(&agent, "journal.md").unwrap();
    assert!(journal.contains("contexte compacté"), "{journal}");

    // Le réveil suivant ouvre une session neuve, qui repart de la mémoire.
    w.supervisor.send(&agent, "bonjour").await.unwrap();
    turn_completed(&mut w.events, &agent).await;
    let args = args_in(&w, "agent");
    assert!(!args.contains(&"--resume".to_string()));
    let renewed = w
        .supervisor
        .inspect(&agent)
        .unwrap()
        .record
        .session_id
        .unwrap();
    assert_ne!(renewed, first_session);
}
