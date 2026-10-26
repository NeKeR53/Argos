//! Moteur d'événements de bout en bout, sur faux CLI : périodiques, dossier surveillé,
//! webhook, interrupteur, création par MCP. Aucun token.

mod common;

use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentId, Trigger, TriggerKind};
use argos_daemon::events::{self, Events};
use argos_daemon::registry::NewTrigger;
use common::call_tool;
use common::world::{World, until, world};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn add(w: &World, kind: TriggerKind, spec: &str) -> Trigger {
    w.supervisor
        .registry()
        .add_trigger(NewTrigger {
            owner: w.assistant.id.clone(),
            target: w.agent.id.clone(),
            name: "relevé".into(),
            kind,
            spec: spec.into(),
            prompt: "Relève les capteurs.".into(),
        })
        .unwrap()
}

/// Attend la consigne du déclencheur chez sa cible ; rend le texte reçu.
async fn delivered(w: &mut World, trigger: &Trigger) -> String {
    let (target, id) = (trigger.target.clone(), trigger.id.clone());
    match until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, from: Origin::Argos, text } if *agent == target && text.contains(&id))
    })
    .await
    {
        DaemonEvent::Input { text, .. } => text,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_due_interval_wakes_its_target_once() {
    let mut w = world();
    let trigger = add(&w, TriggerKind::Every, "600");
    let engine = Events::new(w.supervisor.clone());

    engine.tick(trigger.created_at + 10);
    engine.tick(trigger.created_at + 600);
    let text = delivered(&mut w, &trigger).await;
    assert!(text.starts_with(&format!("[Déclencheur « relevé » ({}, every)]", trigger.id)));
    assert!(text.contains("Relève les capteurs."));

    // La référence a avancé : pas de second déclenchement dans la même période.
    let fired = w.supervisor.registry().trigger(&trigger.id).unwrap();
    assert!(fired.last_fired.is_some());
    assert!(!events::is_due(&fired, fired.last_fired.unwrap() + 10));
}

#[tokio::test]
async fn a_cron_fires_and_a_suspended_trigger_does_not() {
    let mut w = world();
    let cron = add(&w, TriggerKind::Cron, "* * * * *");
    let paused = add(&w, TriggerKind::Every, "60");
    w.supervisor
        .registry()
        .set_trigger_enabled(&paused.id, false)
        .unwrap();
    let engine = Events::new(w.supervisor.clone());

    engine.tick(cron.created_at + 120);
    delivered(&mut w, &cron).await;
    assert_eq!(
        w.supervisor
            .registry()
            .trigger(&paused.id)
            .unwrap()
            .last_fired,
        None
    );
}

#[tokio::test]
async fn a_new_file_in_a_watched_folder_fires_with_its_path() {
    let mut w = world();
    let folder = tempfile::tempdir().unwrap();
    let folder_path = folder.path().canonicalize().unwrap();
    let trigger = add(&w, TriggerKind::Watch, folder_path.to_str().unwrap());
    let engine = Events::new(w.supervisor.clone());
    engine.tick(trigger.created_at);
    // Laisse la surveillance s'ouvrir avant de déposer le fichier.
    tokio::time::sleep(Duration::from_millis(500)).await;

    std::fs::write(folder_path.join("facture.pdf"), b"%PDF").unwrap();
    let text = delivered(&mut w, &trigger).await;
    assert!(
        text.contains("Nouveau fichier :") && text.contains("facture.pdf"),
        "{text}"
    );
}

/// POST HTTP minimal ; rend le code.
async fn post(address: &str, path: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_webhook_needs_its_secret_and_passes_its_body() {
    let mut w = world();
    let trigger = add(&w, TriggerKind::Webhook, "");
    let secret = trigger.secret.clone().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(argos_daemon::mcp::serve(listener, w.supervisor.clone()));

    assert_eq!(
        post(&address, &format!("/hooks/{}/faux", trigger.id), "x").await,
        404
    );
    assert_eq!(post(&address, "/hooks/tr-inconnu/x", "x").await, 404);
    assert_eq!(
        post(
            &address,
            &format!("/hooks/{}/{secret}", trigger.id),
            "porte du garage ouverte"
        )
        .await,
        202
    );
    let text = delivered(&mut w, &trigger).await;
    assert!(text.contains("Reçu :\nporte du garage ouverte"), "{text}");
}

#[tokio::test]
async fn agents_create_triggers_for_themselves_only() {
    let w = world();
    let (created, error) = call_tool(
        &w,
        &w.agent.id,
        "trigger_create",
        json!({"name": "relevé", "source": "every", "every_minutes": "15", "prompt": "Relève."}),
    )
    .await;
    assert!(!error, "{created}");
    let mine = w.supervisor.registry().triggers_of(&w.agent.id).unwrap();
    assert_eq!(
        (mine[0].spec.as_str(), &mine[0].target),
        ("900", &w.agent.id)
    );

    let (refusal, error) = call_tool(
        &w,
        &w.agent.id,
        "trigger_create",
        json!({"name": "x", "source": "switch", "prompt": "x", "target": w.assistant.id.as_str()}),
    )
    .await;
    assert!(error, "{refusal}");

    // L'Assistant cible l'un de ses Agents ; un webhook rend son URL.
    let (created, error) = call_tool(
        &w,
        &w.assistant.id,
        "trigger_create",
        json!({"name": "garage", "source": "webhook", "prompt": "Préviens-moi.", "target": "Domotique"}),
    )
    .await;
    assert!(!error, "{created}");
    assert!(created.contains("/hooks/tr-"), "{created}");
    // Seul le propriétaire suspend ou supprime.
    let hook = w
        .supervisor
        .registry()
        .triggers_of(&w.agent.id)
        .unwrap()
        .into_iter()
        .find(|trigger| trigger.kind == TriggerKind::Webhook)
        .unwrap();
    let (_, error) = call_tool(
        &w,
        &w.agent.id,
        "trigger_delete",
        json!({"trigger": hook.id}),
    )
    .await;
    assert!(error);
    let (_, error) = call_tool(
        &w,
        &w.assistant.id,
        "trigger_set",
        json!({"trigger": hook.id, "status": "off"}),
    )
    .await;
    assert!(!error);
}

#[tokio::test]
async fn the_user_flips_a_switch_through_the_api() {
    let mut w = world();
    let trigger = add(&w, TriggerKind::Switch, "");
    let target: AgentId = trigger.target.clone();
    events::fire(
        &w.supervisor,
        &trigger,
        Some("Actionné par l'utilisateur.".into()),
    );
    let text = delivered(&mut w, &trigger).await;
    assert!(text.ends_with("Actionné par l'utilisateur."));
    assert_eq!(target, w.agent.id);
}
