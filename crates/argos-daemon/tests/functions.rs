//! Fonctions publiées, appelées directement par MCP et depuis les surfaces, sans LLM.

mod common;

use argos_core::protocol::{Command, Reply, Request, ServerMessage};
use argos_core::{Charter, EngineKind, Level, SurfaceKind};
use argos_daemon::{api, registry::NewAgent};
use axum::{Json, Router, routing::post};
use common::call_tool;
use common::world::{World, world};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const TOKEN: &str = "fonctions-test";
type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Endpoint {
    url: String,
    seen: Arc<Mutex<Vec<Value>>>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn endpoint(slow: bool) -> Endpoint {
    let seen = Arc::new(Mutex::new(vec![]));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (captured, started, ready) = (seen.clone(), entered.clone(), release.clone());
    let router = Router::new().route(
        "/light",
        post(move |Json(body): Json<Value>| {
            let (captured, started, ready) = (captured.clone(), started.clone(), ready.clone());
            async move {
                captured.lock().unwrap().push(body.clone());
                started.notify_one();
                if slow {
                    ready.notified().await;
                }
                Json(body)
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Endpoint {
        url: format!("http://{address}/light"),
        seen,
        entered,
        release,
        task,
    }
}

fn definition(url: &str) -> Value {
    json!({
        "name": "set_light",
        "description": "Change l'état d'une lampe.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "state": {"type": "boolean"}
            },
            "required": ["name", "state"],
            "additionalProperties": false
        },
        "request": {
            "method": "POST", "url": url,
            "headers": {"X-Private": "private-definition-marker"},
            "body": {"entity_id": "{{name}}", "on": "{{state}}"},
            "timeout_ms": 10000
        }
    })
}

async fn publish(w: &World, endpoint: &Endpoint) {
    let (message, error) = call_tool(
        w,
        &w.agent.id,
        "function_publish",
        definition(&endpoint.url),
    )
    .await;
    assert!(!error, "{message}");
}

fn assert_parked(w: &World) {
    for agent in [&w.assistant.id, &w.agent.id, &w.worker.id] {
        let status = w.supervisor.inspect(agent).unwrap();
        assert!(!status.running, "{agent} a été réveillé");
        assert!(!status.busy);
        assert!(status.record.session_id.is_none());
    }
}

async fn surface_client(w: &World) -> Client {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(api::serve(listener, w.supervisor.clone(), TOKEN.into()));
    let (mut client, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/?token={TOKEN}"))
            .await
            .unwrap();
    assert!(matches!(
        receive(&mut client).await,
        ServerMessage::Hello { .. }
    ));
    client
}

async fn receive(client: &mut Client) -> ServerMessage {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("réponse dans le délai")
            .expect("connexion ouverte")
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

async fn ask(client: &mut Client, id: u64, command: Command) -> Reply {
    client
        .send(Message::text(
            serde_json::to_string(&Request { id, command }).unwrap(),
        ))
        .await
        .unwrap();
    loop {
        if let ServerMessage::Reply { id: got, reply } = receive(client).await
            && got == id
        {
            return reply;
        }
    }
}

#[tokio::test]
async fn published_functions_are_discovered_and_called_without_waking_an_agent() {
    let w = world();
    let server = endpoint(false).await;
    publish(&w, &server).await;
    // Ni les détails HTTP ni les secrets ne font partie du catalogue visible aux pairs.
    let (catalogue, error) = call_tool(&w, &w.assistant.id, "function_list", json!({})).await;
    assert!(!error, "{catalogue}");
    let catalogue: Value = serde_json::from_str(&catalogue).unwrap();
    assert_eq!(catalogue[0]["name"], "set_light");
    assert_eq!(catalogue[0]["agent"], json!(w.agent.id));
    assert!(catalogue[0].get("request").is_none());
    assert!(!catalogue.to_string().contains(&server.url));
    assert!(!catalogue.to_string().contains("private-definition-marker"));

    let name = "salon\"},\"intrus\":true,\"x\":\"\n\\";
    let (result, error) = call_tool(
        &w,
        &w.assistant.id,
        "function_call",
        json!({"agent": "Domotique", "function": "set_light", "arguments": {"name": name, "state": true}}),
    )
    .await;
    assert!(!error, "{result}");
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result, json!({"entity_id": name, "on": true}));
    assert_eq!(*server.seen.lock().unwrap(), vec![result]);
    assert_parked(&w);
}

#[tokio::test]
async fn only_the_owner_publishes_and_deletes_and_workers_cannot_call() {
    let w = world();
    let server = endpoint(false).await;
    for caller in [&w.assistant.id, &w.worker.id] {
        let (message, error) =
            call_tool(&w, caller, "function_publish", definition(&server.url)).await;
        assert!(error, "{message}");
    }
    let mut own = definition(&server.url);
    own["agent"] = json!(w.assistant.id);
    let (message, error) = call_tool(&w, &w.agent.id, "function_publish", own).await;
    // Un champ de propriétaire fourni par l'appelant ne doit jamais permettre de publier
    // au nom de quelqu'un d'autre ; il peut être refusé ou ignoré.
    if error {
        publish(&w, &server).await;
    } else {
        assert!(!message.is_empty());
    }
    assert!(w.supervisor.registry().functions(&w.assistant.id).is_err());
    assert_eq!(
        w.supervisor
            .registry()
            .functions(&w.agent.id)
            .unwrap()
            .len(),
        1
    );
    let (message, error) = call_tool(
        &w,
        &w.worker.id,
        "function_call",
        json!({"agent": "Domotique", "function": "set_light", "arguments": {"name": "salon", "state": true}}),
    )
    .await;
    assert!(error, "{message}");
    let (message, error) = call_tool(
        &w,
        &w.assistant.id,
        "function_delete",
        json!({"function": "set_light"}),
    )
    .await;
    assert!(error, "{message}");
    assert!(server.seen.lock().unwrap().is_empty());

    let mut updated = definition(&server.url);
    updated["description"] = json!("Nouvelle version");
    let (message, error) = call_tool(&w, &w.agent.id, "function_publish", updated).await;
    assert!(!error, "{message}");
    let definitions = w.supervisor.registry().functions(&w.agent.id).unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].description, "Nouvelle version");
    let (message, error) = call_tool(
        &w,
        &w.agent.id,
        "function_delete",
        json!({"function": "set_light"}),
    )
    .await;
    assert!(!error, "{message}");
    assert!(
        w.supervisor
            .registry()
            .functions(&w.agent.id)
            .unwrap()
            .is_empty()
    );
    assert_parked(&w);
}

#[tokio::test]
async fn invalid_arguments_missing_functions_and_retired_agents_make_no_http_request() {
    let w = world();
    let server = endpoint(false).await;
    publish(&w, &server).await;
    for (owner, function, arguments) in [
        (
            "Domotique",
            "set_light",
            json!({"name": "salon", "state": "on"}),
        ),
        ("Domotique", "set_light", json!({"name": "salon"})),
        (
            "Domotique",
            "set_light",
            json!({"name": "salon", "state": true, "extra": "x"}),
        ),
        (
            "Domotique",
            "absente",
            json!({"name": "salon", "state": true}),
        ),
        (
            "Inconnu",
            "set_light",
            json!({"name": "salon", "state": true}),
        ),
    ] {
        let (message, error) = call_tool(
            &w,
            &w.assistant.id,
            "function_call",
            json!({"agent": owner, "function": function, "arguments": arguments}),
        )
        .await;
        assert!(error, "{message}");
    }
    w.supervisor.retire(&w.agent.id).await.unwrap();
    let result = w
        .supervisor
        .call_function(
            &w.assistant.id,
            &w.agent.id,
            "set_light",
            json!({"name": "salon", "state": true}),
        )
        .await;
    assert!(result.is_err());
    let (catalogue, error) = call_tool(&w, &w.assistant.id, "function_list", json!({})).await;
    assert!(!error, "{catalogue}");
    assert_eq!(
        serde_json::from_str::<Value>(&catalogue).unwrap(),
        json!([])
    );
    assert!(server.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_surface_calls_only_its_owners_functions_and_receives_json() {
    let w = world();
    let server = endpoint(false).await;
    publish(&w, &server).await;
    let own = w
        .supervisor
        .registry()
        .add_surface(
            &w.agent.id,
            SurfaceKind::Cockpit,
            "Lumières",
            "<button>Salon</button>",
        )
        .unwrap();
    let other = w
        .supervisor
        .create(NewAgent {
            parent: w.assistant.id.clone(),
            level: Level::Agent,
            engine: EngineKind::Claude,
            model: None,
            profile: None,
            node: None,
            workdir: w.home.path().join("mail"),
            charter: Some(Charter {
                name: "Mail".into(),
                scope: "les courriels".into(),
            }),
        })
        .unwrap();
    let foreign = w
        .supervisor
        .registry()
        .add_surface(&other.id, SurfaceKind::Cockpit, "Mail", "<p>Mail</p>")
        .unwrap();
    let mut client = surface_client(&w).await;
    let reply = ask(
        &mut client,
        1,
        Command::SurfaceCall {
            surface: own.id.clone(),
            function: "set_light".into(),
            arguments: json!({"name": "salon", "state": false}),
        },
    )
    .await;
    assert_eq!(
        reply,
        Reply::FunctionResult {
            result: json!({"entity_id": "salon", "on": false})
        }
    );
    for surface in [foreign.id, "surface-inconnue".into()] {
        assert!(matches!(
            ask(
                &mut client,
                2,
                Command::SurfaceCall {
                    surface,
                    function: "set_light".into(),
                    arguments: json!({"name": "salon", "state": true}),
                }
            )
            .await,
            Reply::Error { .. }
        ));
    }
    w.supervisor.registry().delete_surface(&own.id).unwrap();
    assert!(matches!(
        ask(
            &mut client,
            3,
            Command::SurfaceCall {
                surface: own.id,
                function: "set_light".into(),
                arguments: json!({"name": "salon", "state": true}),
            }
        )
        .await,
        Reply::Error { .. }
    ));
    assert_eq!(server.seen.lock().unwrap().len(), 1);
    assert_parked(&w);
    assert!(!w.supervisor.inspect(&other.id).unwrap().running);
}

#[tokio::test]
async fn stop_and_panic_cancel_direct_calls_even_when_the_owner_is_parked() {
    for panic in [false, true] {
        let w = world();
        let server = endpoint(true).await;
        publish(&w, &server).await;
        let supervisor = w.supervisor.clone();
        let (caller, owner) = (w.assistant.id.clone(), w.agent.id.clone());
        let call = tokio::spawn(async move {
            supervisor
                .call_function(
                    &caller,
                    &owner,
                    "set_light",
                    json!({"name": "salon", "state": true}),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), server.entered.notified())
            .await
            .unwrap();
        if panic {
            w.supervisor.panic_stop().await.unwrap();
        } else {
            w.supervisor.stop(&w.agent.id).await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err(), "l'appel devait être annulé : {result:?}");
        assert_parked(&w);
        // L'arrêt annule l'appel en cours, sans supprimer la définition persistante.
        let fast = endpoint(false).await;
        publish(&w, &fast).await;
        assert_eq!(
            w.supervisor
                .call_function(
                    &w.assistant.id,
                    &w.agent.id,
                    "set_light",
                    json!({"name": "salon", "state": false}),
                )
                .await
                .unwrap(),
            json!({"entity_id": "salon", "on": false})
        );
        // Libère le handler serveur après annulation pour ne pas garder de tâche en attente.
        server.release.notify_one();
    }
}

#[tokio::test]
async fn a_peer_discovers_and_calls_a_published_function_by_owner_id() {
    let w = world();
    let server = endpoint(false).await;
    publish(&w, &server).await;
    let peer = w
        .supervisor
        .create(NewAgent {
            parent: w.assistant.id.clone(),
            level: Level::Agent,
            engine: EngineKind::Claude,
            model: None,
            profile: None,
            node: None,
            workdir: w.home.path().join("routine"),
            charter: Some(Charter {
                name: "Routine".into(),
                scope: "les routines quotidiennes".into(),
            }),
        })
        .unwrap();
    let (catalogue, error) =
        call_tool(&w, &peer.id, "function_list", json!({"agent": "Domotique"})).await;
    assert!(!error, "{catalogue}");
    assert_eq!(
        serde_json::from_str::<Value>(&catalogue).unwrap()[0]["name"],
        "set_light"
    );
    let (result, error) = call_tool(
        &w, &peer.id, "function_call",
        json!({"agent": w.agent.id, "function": "set_light", "arguments": {"name": "salon", "state": true}}),
    ).await;
    assert!(!error, "{result}");
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap(),
        json!({"entity_id": "salon", "on": true})
    );
    assert_parked(&w);
    assert!(!w.supervisor.inspect(&peer.id).unwrap().running);
}
