//! Dialogue latéral entre Agents (D40), sur faux CLI : questions et informations
//! acheminées par nom, expéditeur visible, journaux, refus et garde-fou. Aucun token.

mod common;

use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentRecord, Charter, EngineKind, Level};
use argos_daemon::registry::NewAgent;
use common::call_tool;
use common::world::{World, is_message, until, world};
use serde_json::{Value, json};

/// Un second Agent, « Mail », à côté de « Domotique ».
fn mail(w: &World) -> AgentRecord {
    w.supervisor
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
                scope: "ma boîte mail".into(),
            }),
        })
        .unwrap()
}

#[tokio::test]
async fn an_agent_sees_its_peers_but_not_itself() {
    let w = world();
    mail(&w);
    let (peers, error) = call_tool(&w, &w.agent.id, "list_peers", json!({})).await;
    assert!(!error, "{peers}");
    let names: Vec<String> = serde_json::from_str::<Vec<Value>>(&peers)
        .unwrap()
        .iter()
        .map(|peer| peer["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["Mail"]);
}

#[tokio::test]
async fn a_question_reaches_the_peer_named_with_its_sender() {
    let mut w = world();
    let mail = mail(&w);
    let (sent, error) = call_tool(
        &w,
        &w.agent.id,
        "ask_agent",
        json!({"agent": "mail", "question": "Quel est le dernier mail du syndic ?"}),
    )
    .await;
    assert!(!error, "{sent}");

    let event = until(
        &mut w.events,
        |event| matches!(event, DaemonEvent::Input { agent, .. } if *agent == mail.id),
    )
    .await;
    let DaemonEvent::Input { from, text, .. } = event else {
        unreachable!()
    };
    assert_eq!(
        from,
        Origin::Agent {
            agent: w.agent.id.clone()
        }
    );
    assert!(
        text.starts_with("[Question de l'Agent « Domotique »"),
        "{text}"
    );
    assert!(text.contains("dernier mail du syndic"));
    // Le pair est réveillé et traite la question.
    until(&mut w.events, |event| {
        is_message(event, &mail.id, "reply 1")
    })
    .await;

    let memory = w.supervisor.memory();
    let asked = memory.read(&w.agent.id, "journal.md").unwrap();
    assert!(asked.contains("question à l'Agent « Mail »"), "{asked}");
    let received = memory.read(&mail.id, "journal.md").unwrap();
    assert!(
        received.contains("question de l'Agent « Domotique »"),
        "{received}"
    );
}

#[tokio::test]
async fn an_answer_goes_back_with_tell_agent() {
    let mut w = world();
    let mail = mail(&w);
    let (_, error) = call_tool(
        &w,
        &mail.id,
        "tell_agent",
        json!({"agent": "Domotique", "message": "Le syndic passe jeudi."}),
    )
    .await;
    assert!(!error);
    let agent = w.agent.id.clone();
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent: a, text, .. } if *a == agent && text.starts_with("[Information de l'Agent « Mail »"))
    })
    .await;
}

#[tokio::test]
async fn the_dialogue_stays_between_living_agents() {
    let w = world();
    let mail = mail(&w);
    for target in [
        w.worker.id.as_str(),
        w.assistant.id.as_str(),
        "Domotique",
        "inconnu",
    ] {
        let (refusal, error) = call_tool(
            &w,
            &w.agent.id,
            "ask_agent",
            json!({"agent": target, "question": "?"}),
        )
        .await;
        assert!(error, "{target} : {refusal}");
    }
    w.supervisor.retire(&mail.id).await.unwrap();
    let (_, error) = call_tool(
        &w,
        &w.agent.id,
        "ask_agent",
        json!({"agent": mail.id.as_str(), "question": "?"}),
    )
    .await;
    assert!(error, "un Agent retiré ne reçoit plus rien");

    // Ni l'Assistant ni un Worker n'ont ces outils.
    for caller in [&w.assistant.id, &w.worker.id] {
        let (_, error) = call_tool(
            &w,
            caller,
            "ask_agent",
            json!({"agent": "Domotique", "question": "?"}),
        )
        .await;
        assert!(error);
    }
}

#[tokio::test]
async fn an_endless_exchange_is_cut_off() {
    let w = world();
    mail(&w);
    for _ in 0..20 {
        assert!(w.supervisor.allow_lateral(&w.agent.id, &w.assistant.id));
    }
    assert!(
        !w.supervisor.allow_lateral(&w.assistant.id, &w.agent.id),
        "la paire compte dans les deux sens"
    );

    let mail = w
        .supervisor
        .registry()
        .agent_named("Mail")
        .unwrap()
        .unwrap();
    for _ in 0..20 {
        assert!(w.supervisor.allow_lateral(&w.agent.id, &mail.id));
    }
    let (refusal, error) = call_tool(
        &w,
        &mail.id,
        "tell_agent",
        json!({"agent": "Domotique", "message": "encore"}),
    )
    .await;
    assert!(error);
    assert!(refusal.contains("trop d'échanges"), "{refusal}");
}
