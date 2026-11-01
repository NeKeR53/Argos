//! Un superviseur sur faux CLI : un Assistant et un Agent sur Claude, un Worker sur Codex.

use argos_core::protocol::DaemonEvent;
use argos_core::{AgentId, AgentRecord, Charter, EngineKind, Level, TurnOutcome};
use argos_daemon::policy::ModelPolicy;
use argos_daemon::registry::{NewAgent, Registry};
use argos_daemon::supervisor::{Supervisor, SupervisorSettings};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::broadcast;

pub struct World {
    pub home: TempDir,
    pub supervisor: Supervisor,
    pub events: broadcast::Receiver<DaemonEvent>,
    pub assistant: AgentRecord,
    pub agent: AgentRecord,
    pub worker: AgentRecord,
}

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Tous les agents commencent parkés.
pub fn world() -> World {
    world_with(ModelPolicy::default())
}

/// Le même monde, avec une politique de modèles donnée.
pub fn world_with(policy: ModelPolicy) -> World {
    world_custom(policy, false, None)
}

/// Le même monde, surfaces ouvertes aux autres appareils sur `http://mac.local:7890`.
#[allow(dead_code)]
pub fn world_sharing_surfaces() -> World {
    let links = argos_daemon::surface_web::Links {
        base: "http://mac.local:7890".into(),
    };
    world_custom(ModelPolicy::default(), false, Some(links))
}

/// Le même monde, préemption des Workers active (D14).
#[allow(dead_code)]
pub fn world_preempting() -> World {
    world_custom(ModelPolicy::default(), true, None)
}

fn world_custom(
    policy: ModelPolicy,
    preempt: bool,
    surfaces: Option<argos_daemon::surface_web::Links>,
) -> World {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::open_in_memory().unwrap();
    let assistant = registry
        .ensure_assistant(EngineKind::Claude, None, home.path().join("assistant"))
        .unwrap();
    let agent = registry
        .create(NewAgent {
            profile: None,
            node: None,
            parent: assistant.id.clone(),
            level: Level::Agent,
            engine: EngineKind::Claude,
            model: None,
            workdir: home.path().join("agent"),
            charter: Some(Charter {
                name: "Domotique".into(),
                scope: "la maison".into(),
            }),
        })
        .unwrap();
    let worker = registry
        .create(NewAgent {
            profile: None,
            node: None,
            parent: agent.id.clone(),
            level: Level::Worker,
            engine: EngineKind::Codex,
            model: None,
            workdir: home.path().join("worker"),
            charter: None,
        })
        .unwrap();
    let settings = SupervisorSettings {
        home: home.path().to_path_buf(),
        user_codex_home: home.path().join("user-codex"),
        programs: HashMap::from([
            (EngineKind::Claude, fixture("fake-claude")),
            (EngineKind::Codex, fixture("fake-codex")),
        ]),
        mcp_url: None,
        surfaces,
        policy,
        nodes: Default::default(),
        worker_access: Default::default(),
        preempt,
    };
    let supervisor = Supervisor::new(registry, settings);
    let events = supervisor.subscribe();
    World {
        home,
        supervisor,
        events,
        assistant,
        agent,
        worker,
    }
}

/// Prochain événement qui satisfait `wanted` (10 s au plus).
pub async fn until(
    events: &mut broadcast::Receiver<DaemonEvent>,
    wanted: impl Fn(&DaemonEvent) -> bool,
) -> DaemonEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("flux d'événements");
            if wanted(&event) {
                return event;
            }
        }
    })
    .await
    .expect("événement attendu jamais reçu")
}

pub async fn turn_completed(
    events: &mut broadcast::Receiver<DaemonEvent>,
    agent: &AgentId,
) -> TurnOutcome {
    let event = until(
        events,
        |event| matches!(event, DaemonEvent::TurnCompleted { agent: a, .. } if a == agent),
    )
    .await;
    match event {
        DaemonEvent::TurnCompleted { outcome, .. } => outcome,
        _ => unreachable!(),
    }
}

pub fn is_message(event: &DaemonEvent, agent: &AgentId, text: &str) -> bool {
    matches!(event, DaemonEvent::Message { agent: a, text: t } if a == agent && t == text)
}

/// Répète `probe` jusqu'à ce qu'il rende une valeur (5 s au plus).
pub async fn poll<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    for _ in 0..100 {
        if let Some(value) = probe() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition jamais remplie");
}
