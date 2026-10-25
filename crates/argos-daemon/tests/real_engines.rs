//! Les vrais moteurs, avec des modèles bon marché. Consomment des tokens, donc ignorés
//! par défaut : `cargo test -p argos-daemon --test real_engines -- --ignored`.

mod common;

use argos_core::EngineKind;
use argos_daemon::engine::{Session, SessionSpec, TurnOutcome, prepare_codex_home};
use common::said;
use std::path::PathBuf;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(180);

/// Le cycle de vie complet d'un agent : mémoire entre deux tours, interruption,
/// parking, puis reprise dans un nouveau process qui se souvient.
async fn full_lifecycle(mut spec: SessionSpec) {
    let mut session = Session::spawn(spec.clone()).await.unwrap();

    session
        .send("Retiens le nombre 42. Réponds seulement : ok")
        .unwrap();
    assert_eq!(
        common::turn(&mut session, LIMIT).await.1,
        TurnOutcome::Completed
    );

    session
        .send("Écris les nombres de 1 à 400, un par ligne, sans commentaire.")
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    session.interrupt().unwrap();
    assert_eq!(
        common::turn(&mut session, LIMIT).await.1,
        TurnOutcome::Interrupted
    );

    // Un parking réussi : le process sort de lui-même, sans être tué. Son code ne dit
    // rien de plus (claude rend le statut du dernier tour, ici interrompu).
    let session_id = session.session_id().to_string();
    session.park().unwrap();
    assert!(common::exit_code(&mut session, LIMIT).await.is_some());

    spec.resume = Some(session_id.clone());
    let mut resumed = Session::spawn(spec).await.unwrap();
    assert_eq!(resumed.session_id(), session_id);
    resumed
        .send("Quel nombre devais-tu retenir ? Réponds seulement par le nombre.")
        .unwrap();
    let (events, outcome) = common::turn(&mut resumed, LIMIT).await;
    assert_eq!(outcome, TurnOutcome::Completed);
    assert!(said(&events).contains("42"), "réponse : {events:?}");
    resumed.park().unwrap();
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn real_claude_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = SessionSpec::new(EngineKind::Claude, dir.path());
    spec.model = Some("haiku".into());
    full_lifecycle(spec).await;
}

#[tokio::test]
#[ignore = "consomme des tokens"]
async fn real_codex_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("codex-home");
    let user_home = PathBuf::from(std::env::var("HOME").unwrap()).join(".codex");
    prepare_codex_home(&home, &user_home).unwrap();

    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut spec = SessionSpec::new(EngineKind::Codex, work);
    spec.model = Some("gpt-6-luna".into());
    spec.codex_home = Some(home);
    full_lifecycle(spec).await;
}
