//! Adaptateurs de moteur face à de faux CLI (tests/fixtures) : aucun token consommé.

mod common;

use argos_core::EngineKind;
use argos_daemon::engine::{EngineError, EngineEvent, Session, SessionSpec, TurnOutcome};
use common::message;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

struct Fake {
    dir: TempDir,
    spec: SessionSpec,
}

impl Fake {
    fn new(engine: EngineKind) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = match engine {
            EngineKind::Claude => "fake-claude",
            EngineKind::Codex => "fake-codex",
        };
        let mut spec = SessionSpec::new(engine, dir.path());
        spec.program = Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(script),
        );
        spec.codex_home = Some(dir.path().join("codex-home"));
        Self { dir, spec }
    }

    /// Arguments (et CODEX_HOME pour Codex) reçus par le faux CLI.
    fn received(&self) -> Vec<String> {
        read_lines(&self.dir.path().join(".fake-args"))
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

const LIMIT: Duration = Duration::from_secs(10);

async fn next(session: &mut Session) -> EngineEvent {
    common::next(session, LIMIT).await
}

async fn turn(session: &mut Session) -> (Vec<EngineEvent>, TurnOutcome) {
    common::turn(session, LIMIT).await
}

async fn exit_code(session: &mut Session) -> Option<i32> {
    common::exit_code(session, LIMIT).await
}

#[tokio::test]
async fn claude_answers_several_turns_in_one_process() {
    let fake = Fake::new(EngineKind::Claude);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();

    session.send("bonjour").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 1")], TurnOutcome::Completed)
    );
    session.send("encore").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 2")], TurnOutcome::Completed)
    );
}

#[tokio::test]
async fn claude_runs_isolated_with_a_known_session_id() {
    let mut fake = Fake::new(EngineKind::Claude);
    fake.spec.model = Some("haiku".into());
    fake.spec.instructions = Some("Tu es un Worker.".into());
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.send("bonjour").unwrap();
    turn(&mut session).await;

    let args = fake.received();
    let pair = |flag: &str, value: &str| args.windows(2).any(|w| w == [flag, value]);
    assert!(pair("--setting-sources", "project"));
    assert!(args.contains(&"--strict-mcp-config".to_string()));
    assert!(pair("--session-id", session.session_id()));
    assert!(pair("--model", "haiku"));
    assert!(pair("--append-system-prompt", "Tu es un Worker."));
}

#[tokio::test]
async fn claude_resume_reuses_the_session_id() {
    let mut fake = Fake::new(EngineKind::Claude);
    fake.spec.resume = Some("session-parquee".into());
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    assert_eq!(session.session_id(), "session-parquee");
    session.send("bonjour").unwrap();
    turn(&mut session).await;

    let args = fake.received();
    assert!(
        args.windows(2)
            .any(|w| w == ["--resume", "session-parquee"])
    );
    assert!(!args.contains(&"--session-id".to_string()));
}

#[tokio::test]
async fn claude_interrupt_ends_the_turn_and_keeps_the_session() {
    let fake = Fake::new(EngineKind::Claude);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();

    session.send("SLOW").unwrap();
    session.interrupt().unwrap();
    assert_eq!(turn(&mut session).await.1, TurnOutcome::Interrupted);

    session.send("bonjour").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 2")], TurnOutcome::Completed)
    );
}

#[tokio::test]
async fn claude_reports_tool_activity_and_usage() {
    let fake = Fake::new(EngineKind::Claude);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.send("TOOL").unwrap();

    assert_eq!(
        next(&mut session).await,
        EngineEvent::ToolActivity {
            name: "Bash".into()
        }
    );
    assert_eq!(next(&mut session).await, message("reply 1"));
    match next(&mut session).await {
        EngineEvent::TurnCompleted {
            usage: Some(usage), ..
        } => {
            assert_eq!((usage.input_tokens, usage.output_tokens), (13, 2));
            assert_eq!(usage.cost_usd, Some(0.001));
        }
        other => panic!("fin de tour attendue, reçu {other:?}"),
    }
}

#[tokio::test]
async fn park_ends_the_process_cleanly() {
    let fake = Fake::new(EngineKind::Claude);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.send("bonjour").unwrap();
    turn(&mut session).await;

    session.park().unwrap();
    assert_eq!(exit_code(&mut session).await, Some(0));
    assert!(session.next_event().await.is_none());
    assert!(matches!(
        session.send("trop tard"),
        Err(EngineError::Closed)
    ));
}

#[tokio::test]
async fn a_crash_is_reported_with_its_stderr() {
    let fake = Fake::new(EngineKind::Claude);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.send("CRASH").unwrap();

    match next(&mut session).await {
        EngineEvent::Exited { code, stderr_tail } => {
            assert_eq!(code, Some(3));
            assert_eq!(stderr_tail, vec!["panne simulée".to_string()]);
        }
        other => panic!("sortie attendue, reçu {other:?}"),
    }
}

#[tokio::test]
async fn kill_stops_the_whole_process_group() {
    let fake = Fake::new(EngineKind::Claude);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.send("HANG").unwrap();

    let pid_file = fake.dir.path().join(".child-pid");
    let child: i32 = poll(|| std::fs::read_to_string(&pid_file).ok()?.trim().parse().ok()).await;
    session.kill();

    assert_eq!(exit_code(&mut session).await, None);
    // L'outil lancé par l'agent (ici `sleep`) est tué avec lui.
    // SAFETY : kill avec le signal 0 ne fait que tester l'existence du process.
    poll(|| (unsafe { libc::kill(child, 0) } != 0).then_some(())).await;
}

/// Répète `probe` jusqu'à ce qu'il rende une valeur (5 s au plus).
async fn poll<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    for _ in 0..100 {
        if let Some(value) = probe() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition jamais remplie");
}

#[tokio::test]
async fn codex_answers_several_turns_on_one_thread() {
    let fake = Fake::new(EngineKind::Codex);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    assert_eq!(session.session_id(), "thread-new");

    session.send("bonjour").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 1")], TurnOutcome::Completed)
    );
    session.send("encore").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 2")], TurnOutcome::Completed)
    );
}

#[tokio::test]
async fn codex_runs_app_server_with_the_isolated_home() {
    let fake = Fake::new(EngineKind::Codex);
    let _session = Session::spawn(fake.spec.clone()).await.unwrap();

    let home = fake.spec.codex_home.as_ref().unwrap();
    assert_eq!(
        fake.received(),
        vec![
            "app-server".to_string(),
            format!("CODEX_HOME={}", home.display())
        ]
    );
}

#[tokio::test]
async fn codex_requires_an_isolated_home() {
    let mut fake = Fake::new(EngineKind::Codex);
    fake.spec.codex_home = None;
    assert!(matches!(
        Session::spawn(fake.spec.clone()).await,
        Err(EngineError::Config(_))
    ));
}

#[tokio::test]
async fn codex_resume_attaches_to_the_existing_thread() {
    let mut fake = Fake::new(EngineKind::Codex);
    fake.spec.resume = Some("thread-parque".into());
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    assert_eq!(session.session_id(), "thread-parque");
    session.send("bonjour").unwrap();
    assert_eq!(turn(&mut session).await.1, TurnOutcome::Completed);
}

#[tokio::test]
async fn codex_interrupt_ends_the_turn_and_keeps_the_thread() {
    let fake = Fake::new(EngineKind::Codex);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();

    // Interruption immédiate : l'id du tour n'est pas encore connu, elle doit partir quand même.
    session.send("SLOW").unwrap();
    session.interrupt().unwrap();
    assert_eq!(turn(&mut session).await.1, TurnOutcome::Interrupted);

    session.send("bonjour").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 2")], TurnOutcome::Completed)
    );
}

#[tokio::test]
async fn codex_answers_server_requests_instead_of_hanging() {
    let fake = Fake::new(EngineKind::Codex);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.send("ASK").unwrap();
    assert_eq!(
        turn(&mut session).await,
        (vec![message("reply 1")], TurnOutcome::Completed)
    );
}

#[tokio::test]
async fn codex_reports_failures_and_tool_activity() {
    let fake = Fake::new(EngineKind::Codex);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();

    session.send("TOOL").unwrap();
    let (events, _) = turn(&mut session).await;
    assert_eq!(
        events,
        vec![
            EngineEvent::ToolActivity {
                name: "shell".into()
            },
            message("reply 1")
        ]
    );

    session.send("FAIL").unwrap();
    assert_eq!(
        turn(&mut session).await.1,
        TurnOutcome::Failed {
            message: "quota dépassé".into()
        }
    );
}

#[tokio::test]
async fn codex_park_ends_the_process_cleanly() {
    let fake = Fake::new(EngineKind::Codex);
    let mut session = Session::spawn(fake.spec.clone()).await.unwrap();
    session.park().unwrap();
    assert_eq!(exit_code(&mut session).await, Some(0));
}
