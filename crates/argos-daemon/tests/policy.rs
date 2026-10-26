//! Politique de modèles de bout en bout, sur faux CLI : profils, repli sur quota épuisé,
//! plafonds. Aucun token.

mod common;

use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentRecord, EngineKind, Level};
use argos_daemon::policy::ModelPolicy;
use argos_daemon::registry::NewAgent;
use argos_daemon::supervisor::SupervisorError;
use common::world::{World, is_message, until, world, world_with};

/// Un Worker du profil `code`, sur le premier maillon de sa chaîne (Claude Opus).
fn code_worker(w: &World) -> AgentRecord {
    let (profile, choice) = w
        .supervisor
        .policy()
        .resolve(Some("code"), Level::Worker)
        .unwrap();
    w.supervisor
        .create(NewAgent {
            parent: w.agent.id.clone(),
            level: Level::Worker,
            engine: choice.engine,
            model: Some(choice.model),
            profile: Some(profile),
            workdir: w.home.path().join("code-worker"),
            charter: None,
        })
        .unwrap()
}

fn is_notice(event: &DaemonEvent, about: &AgentRecord, words: &str) -> bool {
    matches!(event, DaemonEvent::Notice { agent: Some(agent), text } if *agent == about.id && text.contains(words))
}

#[tokio::test]
async fn an_exhausted_quota_moves_a_worker_down_its_chain_and_resumes_its_task() {
    let mut w = world();
    let worker = code_worker(&w);
    let task = w
        .supervisor
        .registry()
        .add_task(&w.agent.id, "QUOTA ampoules", Some("trois pièces"), None)
        .unwrap();
    w.supervisor
        .registry()
        .assign_task(&task.id, &worker.id)
        .unwrap();

    w.supervisor
        .send(&worker.id, "QUOTA ampoules")
        .await
        .unwrap();

    until(&mut w.events, |event| {
        is_notice(event, &worker, "vers codex gpt-6.1-sol")
    })
    .await;
    let moved = w.supervisor.inspect(&worker.id).unwrap().record;
    assert_eq!(
        (moved.engine, moved.model.as_deref()),
        (EngineKind::Codex, Some("gpt-6.1-sol"))
    );
    // Nouveau moteur : session neuve, relancée sur la tâche. Son titre contient QUOTA :
    // le faux Codex échoue à son tour, ce qui mène au bout de la chaîne.
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, from: Origin::Argos, text } if *agent == worker.id && text.contains(&task.id))
    })
    .await;
    // Bout de chaîne : plus de repli, l'utilisateur est prévenu.
    until(&mut w.events, |event| {
        is_notice(event, &worker, "aucun repli prévu")
    })
    .await;
}

#[tokio::test]
async fn an_agent_without_a_profile_has_no_fallback() {
    let mut w = world();
    let agent = w.agent.clone();
    w.supervisor.send(&agent.id, "QUOTA").await.unwrap();
    until(&mut w.events, |event| {
        is_notice(event, &agent, "aucun repli prévu")
    })
    .await;
    assert_eq!(
        w.supervisor.inspect(&agent.id).unwrap().record.engine,
        EngineKind::Claude
    );
}

#[tokio::test]
async fn a_model_cap_refuses_one_session_too_many() {
    let mut policy = ModelPolicy::default();
    policy.caps.max_parallel.insert("claude-opus-5-5".into(), 1);
    let mut w = world_with(policy);
    let first = code_worker(&w);
    let second = code_worker(&w);

    w.supervisor.send(&first.id, "bonjour").await.unwrap();
    until(&mut w.events, |event| {
        is_message(event, &first.id, "reply 1")
    })
    .await;
    assert!(matches!(
        w.supervisor.send(&second.id, "bonjour").await,
        Err(SupervisorError::Cap(_))
    ));

    // La place libérée, il démarre.
    w.supervisor.park(&first.id).await.unwrap();
    w.supervisor.send(&second.id, "bonjour").await.unwrap();
}

#[tokio::test]
async fn workers_are_kept_off_forbidden_models() {
    let mut policy = ModelPolicy::default();
    policy
        .caps
        .forbidden_for_workers
        .push("claude-opus-5-5".into());
    let w = world_with(policy);
    let worker = code_worker(&w);
    assert_eq!(worker.engine, EngineKind::Codex);
    assert_eq!(worker.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(worker.profile.as_deref(), Some("code"));
}
