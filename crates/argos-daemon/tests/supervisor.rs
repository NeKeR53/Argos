//! Plan de contrôle du superviseur face aux faux CLI (tests/fixtures) : aucun token.

mod common;

use argos_core::protocol::DaemonEvent;
use argos_core::{AgentId, AgentState, TurnOutcome};
use argos_daemon::supervisor::SupervisorError;
use common::world::{is_message, poll, turn_completed, until, world};

#[tokio::test]
async fn sending_to_a_parked_agent_wakes_it() {
    let mut w = world();
    let id = w.assistant.id.clone();
    w.supervisor.send(&id, "bonjour").await.unwrap();

    // Ce que l'agent reçoit est publié, pour que la console le montre.
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, text } if *agent == id && text == "bonjour")
    })
    .await;
    until(&mut w.events, |event| is_message(event, &id, "reply 1")).await;
    assert_eq!(
        turn_completed(&mut w.events, &id).await,
        TurnOutcome::Completed
    );
    let status = w.supervisor.inspect(&id).unwrap();
    assert_eq!(status.record.state, AgentState::Active);
    assert!(status.running);
    assert!(!status.busy);
    assert!(status.record.session_id.is_some());
}

#[tokio::test]
async fn a_parked_agent_resumes_its_own_session() {
    let mut w = world();
    let id = w.agent.id.clone();
    w.supervisor.send(&id, "bonjour").await.unwrap();
    turn_completed(&mut w.events, &id).await;

    w.supervisor.park(&id).await.unwrap();
    let parked = w.supervisor.inspect(&id).unwrap();
    assert_eq!(parked.record.state, AgentState::Parked);
    assert!(!parked.running);
    let session = parked.record.session_id.unwrap();

    w.supervisor.send(&id, "encore").await.unwrap();
    until(&mut w.events, |event| is_message(event, &id, "reply 1")).await;
    let args = std::fs::read_to_string(w.home.path().join("agent/.fake-args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--resume", session.as_str()])
    );
}

#[tokio::test]
async fn interrupt_reaches_the_running_turn() {
    let mut w = world();
    let id = w.assistant.id.clone();
    w.supervisor.send(&id, "SLOW").await.unwrap();
    w.supervisor.interrupt(&id).await.unwrap();
    assert_eq!(
        turn_completed(&mut w.events, &id).await,
        TurnOutcome::Interrupted
    );
}

#[tokio::test]
async fn stop_kills_the_whole_subtree() {
    let mut w = world();
    let (agent, worker) = (w.agent.id.clone(), w.worker.id.clone());
    w.supervisor.send(&agent, "HANG").await.unwrap();
    w.supervisor.send(&worker, "SLOW").await.unwrap();
    let pid_file = w.home.path().join("agent/.child-pid");
    let child: i32 = poll(|| std::fs::read_to_string(&pid_file).ok()?.trim().parse().ok()).await;

    let stopped = w.supervisor.stop(&agent).await.unwrap();

    assert_eq!(stopped, vec![agent.clone(), worker.clone()]);
    for id in [&agent, &worker] {
        let status = w.supervisor.inspect(id).unwrap();
        assert_eq!(
            (status.record.state, status.running),
            (AgentState::Parked, false)
        );
    }
    // L'outil lancé par l'Agent est mort avec lui. SAFETY : le signal 0 teste l'existence.
    poll(|| (unsafe { libc::kill(child, 0) } != 0).then_some(())).await;
    until(
        &mut w.events,
        |event| matches!(event, DaemonEvent::Exited { agent: a, code: None } if *a == worker),
    )
    .await;
}

#[tokio::test]
async fn retire_stops_and_retires_the_subtree() {
    let w = world();
    let (agent, worker) = (w.agent.id.clone(), w.worker.id.clone());
    w.supervisor.send(&worker, "SLOW").await.unwrap();

    let retired = w.supervisor.retire(&agent).await.unwrap();

    assert_eq!(retired, vec![agent.clone(), worker.clone()]);
    assert!(!w.supervisor.inspect(&worker).unwrap().running);
    let tree: Vec<AgentId> = w
        .supervisor
        .tree()
        .unwrap()
        .into_iter()
        .map(|status| status.record.id)
        .collect();
    assert_eq!(tree, vec![w.assistant.id.clone()]);
    assert!(matches!(
        w.supervisor.send(&agent, "bonjour").await,
        Err(SupervisorError::Retired(_))
    ));
}

#[tokio::test]
async fn panic_stop_stops_everything() {
    let w = world();
    for id in [&w.assistant.id, &w.agent.id, &w.worker.id] {
        w.supervisor.send(id, "SLOW").await.unwrap();
    }

    let mut stopped = w.supervisor.panic_stop().await.unwrap();
    stopped.sort();

    let mut all = vec![
        w.assistant.id.clone(),
        w.agent.id.clone(),
        w.worker.id.clone(),
    ];
    all.sort();
    assert_eq!(stopped, all);
    assert!(
        w.supervisor
            .tree()
            .unwrap()
            .iter()
            .all(|status| !status.running)
    );
}

#[tokio::test]
async fn a_crashed_session_goes_back_to_parked() {
    let mut w = world();
    let id = w.agent.id.clone();
    w.supervisor.send(&id, "CRASH").await.unwrap();

    until(
        &mut w.events,
        |event| matches!(event, DaemonEvent::Exited { agent, code: Some(3) } if *agent == id),
    )
    .await;
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::AgentUpdated { agent } if agent.record.id == id && !agent.running)
    })
    .await;
    assert_eq!(
        w.supervisor.inspect(&id).unwrap().record.state,
        AgentState::Parked
    );

    // Il se réveille au message suivant.
    w.supervisor.send(&id, "bonjour").await.unwrap();
    assert_eq!(
        turn_completed(&mut w.events, &id).await,
        TurnOutcome::Completed
    );
}

#[tokio::test]
async fn shutdown_parks_every_running_session() {
    let mut w = world();
    for id in [&w.assistant.id, &w.worker.id] {
        w.supervisor.send(id, "bonjour").await.unwrap();
        turn_completed(&mut w.events, id).await;
    }
    w.supervisor.shutdown().await;
    assert!(
        w.supervisor
            .tree()
            .unwrap()
            .iter()
            .all(|status| !status.running)
    );
    assert_eq!(
        w.supervisor.inspect(&w.worker.id).unwrap().record.state,
        AgentState::Parked
    );
}

#[tokio::test]
async fn unknown_agents_are_reported() {
    let w = world();
    let ghost = AgentId::new("fantome");
    assert!(w.supervisor.inspect(&ghost).is_err());
    assert!(w.supervisor.send(&ghost, "x").await.is_err());
    assert!(w.supervisor.stop(&ghost).await.is_err());
}
