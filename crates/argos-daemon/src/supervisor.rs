//! Superviseur : relie le registre aux sessions vivantes et applique le plan de contrôle
//! (D14) — réveil, interruption, parking, stop en cascade, retrait, panic stop. Tout y est
//! déterministe : il exécute des ordres, il ne décide rien.
//!
//! Règle de cohérence : celui qui retire une session de son emplacement met le registre à
//! jour. Une session morte en retard ne peut donc pas écraser l'état d'une session plus
//! récente du même agent.

use crate::engine::{
    EngineError, EngineEvent, EventStream, McpServer, Session, SessionControl, SessionSpec,
    prepare_codex_home,
};
use crate::registry::{NewAgent, Registry, RegistryError};
use argos_core::protocol::{AgentStatus, DaemonEvent};
use argos_core::{AgentId, AgentRecord, AgentState, EngineKind};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, broadcast, watch};

/// Délai laissé à un process pour sortir après un parking ou un kill.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct SupervisorSettings {
    /// `$ARGOS_HOME` : les `CODEX_HOME` isolés y sont créés.
    pub home: PathBuf,
    /// `~/.codex` de l'utilisateur, d'où vient l'authentification Codex.
    pub user_codex_home: PathBuf,
    /// Binaires à lancer à la place de `claude` / `codex` (tests).
    pub programs: HashMap<EngineKind, PathBuf>,
    /// Adresse du serveur MCP d'Argos (`http://127.0.0.1:7879`). Sans elle, les agents
    /// tournent sans les outils d'Argos.
    pub mcp_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("préparation de l'agent : {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} est retiré")]
    Retired(AgentId),
    #[error("{0} est en cours d'arrêt")]
    Stopping(AgentId),
}

pub type Result<T> = std::result::Result<T, SupervisorError>;

#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Mutex<Registry>,
    /// Un verrou par agent : réveil, parking et stop d'un même agent ne se chevauchent
    /// pas, sans bloquer les autres agents.
    slots: Mutex<HashMap<AgentId, Arc<AsyncMutex<Option<Live>>>>>,
    /// Agents dont le process tourne, avec leur drapeau « tour en cours ». Lisible sans
    /// attendre le verrou d'un agent (qui peut être pris par un démarrage lent).
    running: Mutex<HashMap<AgentId, Arc<AtomicBool>>>,
    /// Agents en cours de stop : aucun réveil tant que le stop n'est pas fini (sinon le
    /// rapport tardif d'un Worker relancerait l'Agent qu'on arrête).
    halted: Mutex<HashSet<AgentId>>,
    /// Jetons MCP des sessions en cours → agent appelant.
    tokens: Mutex<HashMap<String, AgentId>>,
    events: broadcast::Sender<DaemonEvent>,
    generations: AtomicU64,
    settings: SupervisorSettings,
}

/// La session en cours d'un agent.
struct Live {
    generation: u64,
    control: SessionControl,
    exited: watch::Receiver<bool>,
}

impl Supervisor {
    pub fn new(registry: Registry, settings: SupervisorSettings) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                registry: Mutex::new(registry),
                slots: Mutex::default(),
                running: Mutex::default(),
                halted: Mutex::default(),
                tokens: Mutex::default(),
                events,
                generations: AtomicU64::new(0),
                settings,
            }),
        }
    }

    /// Flux de tous les événements des agents.
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.inner.events.subscribe()
    }

    /// Accès direct au registre. Ne pas garder le verrou pendant un `await`.
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        locked(&self.inner.registry)
    }

    pub fn tree(&self) -> Result<Vec<AgentStatus>> {
        let agents = self.registry().tree()?;
        Ok(agents
            .into_iter()
            .map(|record| self.status(record))
            .collect())
    }

    pub fn inspect(&self, id: &AgentId) -> Result<AgentStatus> {
        Ok(self.status(self.record(id)?))
    }

    pub fn record(&self, id: &AgentId) -> Result<AgentRecord> {
        self.registry()
            .get(id)?
            .ok_or_else(|| RegistryError::NotFound(id.clone()).into())
    }

    /// Enfants non retirés d'un agent, avec leur état d'exécution.
    pub fn children(&self, id: &AgentId) -> Result<Vec<AgentStatus>> {
        let children = self.registry().children(id)?;
        Ok(children
            .into_iter()
            .map(|record| self.status(record))
            .collect())
    }

    /// Crée un agent dans le registre (il commence parké) et le signale.
    pub fn create(&self, new: NewAgent) -> Result<AgentRecord> {
        let record = self.registry().create(new)?;
        self.publish_status(&record.id);
        Ok(record)
    }

    /// Dossier où ranger le répertoire de travail d'un nouvel Agent.
    pub fn agents_dir(&self) -> PathBuf {
        self.inner.settings.home.join("agents")
    }

    /// Émet un jeton MCP pour `id`, valable jusqu'à la fin de sa session en cours.
    pub fn mcp_token(&self, id: &AgentId) -> String {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        locked(&self.inner.tokens).insert(token.clone(), id.clone());
        token
    }

    /// L'agent auquel ce jeton MCP a été donné.
    pub fn agent_for_token(&self, token: &str) -> Option<AgentId> {
        locked(&self.inner.tokens).get(token).cloned()
    }

    /// Message à un agent. Un agent parké est d'abord réveillé : sa session reprend.
    pub async fn send(&self, id: &AgentId, text: impl Into<String>) -> Result<()> {
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        if live.is_none() {
            *live = Some(self.wake(id).await?);
        }
        if let Some(busy) = locked(&self.inner.running).get(id) {
            busy.store(true, Ordering::SeqCst);
        }
        live.as_ref()
            .expect("session démarrée juste au-dessus")
            .control
            .send(text)?;
        self.publish_status(id);
        Ok(())
    }

    /// Interrompt le tour en cours, s'il y en a un.
    pub async fn interrupt(&self, id: &AgentId) -> Result<()> {
        self.record(id)?;
        let slot = self.slot(id);
        let live = slot.lock().await;
        if let Some(session) = live.as_ref() {
            session.control.interrupt()?;
        }
        Ok(())
    }

    /// Arrêt propre ; rend la main une fois le process sorti.
    pub async fn park(&self, id: &AgentId) -> Result<()> {
        self.record(id)?;
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        if let Some(session) = live.take() {
            let _ = session.control.park();
            if !exited(session.exited.clone()).await {
                session.control.kill();
                exited(session.exited).await;
            }
            self.settle(id);
        }
        Ok(())
    }

    /// Arrêt dur de l'agent et de tout son sous-arbre ; rend la main une fois les process
    /// morts. Rend les agents dont un process a été arrêté.
    pub async fn stop(&self, id: &AgentId) -> Result<Vec<AgentId>> {
        self.record(id)?;
        // Le parent d'abord : arrêté, il ne peut plus relancer de Worker.
        let mut targets = vec![id.clone()];
        targets.extend(self.registry().descendants(id)?);
        locked(&self.inner.halted).extend(targets.iter().cloned());
        let mut stopped = vec![];
        for target in &targets {
            if self.kill(target).await {
                stopped.push(target.clone());
            }
        }
        let mut halted = locked(&self.inner.halted);
        for target in &targets {
            halted.remove(target);
        }
        Ok(stopped)
    }

    /// Retrait définitif de l'agent et de son sous-arbre, process arrêtés d'abord.
    pub async fn retire(&self, id: &AgentId) -> Result<Vec<AgentId>> {
        self.stop(id).await?;
        let retired = self.registry().retire(id)?;
        for id in &retired {
            self.publish_status(id);
        }
        Ok(retired)
    }

    /// Arrêt dur de tout l'arbre (D16).
    pub async fn panic_stop(&self) -> Result<Vec<AgentId>> {
        let Some(assistant) = self.registry().assistant()? else {
            return Ok(vec![]);
        };
        self.stop(&assistant.id).await
    }

    /// Arrêt du daemon : parke toutes les sessions en cours.
    pub async fn shutdown(&self) {
        let ids: Vec<AgentId> = locked(&self.inner.running).keys().cloned().collect();
        futures_util::future::join_all(ids.iter().map(|id| self.park(id))).await;
    }

    /// Lance ou reprend la session d'un agent. Appelé avec le verrou de l'agent tenu.
    async fn wake(&self, id: &AgentId) -> Result<Live> {
        let record = self.record(id)?;
        if record.state == AgentState::Retired {
            return Err(SupervisorError::Retired(id.clone()));
        }
        {
            let halted = locked(&self.inner.halted);
            if halted.contains(id) || record.parent.as_ref().is_some_and(|p| halted.contains(p)) {
                return Err(SupervisorError::Stopping(id.clone()));
            }
        }
        let spawned = match self.spec_for(&record) {
            Ok(spec) => Session::spawn(spec).await.map_err(SupervisorError::from),
            Err(error) => Err(error),
        };
        let (control, events) = match spawned {
            Ok(session) => session.split(),
            Err(error) => {
                self.revoke_tokens(id);
                return Err(error);
            }
        };
        let registered = {
            let registry = self.registry();
            registry
                .set_session(id, control.session_id())
                .and_then(|()| registry.set_state(id, AgentState::Active))
        };
        if let Err(error) = registered {
            control.kill();
            self.revoke_tokens(id);
            return Err(error.into());
        }

        let generation = self.inner.generations.fetch_add(1, Ordering::SeqCst) + 1;
        let busy = Arc::new(AtomicBool::new(false));
        locked(&self.inner.running).insert(id.clone(), Arc::clone(&busy));
        let (exit_tx, exit_rx) = watch::channel(false);
        tokio::spawn(
            self.clone()
                .pump(id.clone(), generation, events, busy, exit_tx),
        );
        self.publish_status(id);
        Ok(Live {
            generation,
            control,
            exited: exit_rx,
        })
    }

    /// Relaie les événements d'une session jusqu'à sa fin.
    async fn pump(
        self,
        id: AgentId,
        generation: u64,
        mut events: EventStream,
        busy: Arc<AtomicBool>,
        exit: watch::Sender<bool>,
    ) {
        while let Some(event) = events.next_event().await {
            let agent = id.clone();
            match event {
                EngineEvent::AgentMessage { text } => {
                    self.publish(DaemonEvent::Message { agent, text })
                }
                EngineEvent::ToolActivity { name } => {
                    self.publish(DaemonEvent::Tool { agent, name })
                }
                EngineEvent::TurnCompleted { outcome, usage } => {
                    busy.store(false, Ordering::SeqCst);
                    self.publish(DaemonEvent::TurnCompleted {
                        agent,
                        outcome,
                        usage,
                    });
                    self.publish_status(&id);
                }
                EngineEvent::Exited { code, stderr_tail } => {
                    // D'abord prévenir qui attend cette sortie (parking, stop) : il tient
                    // le verrou de l'agent.
                    let _ = exit.send(true);
                    self.publish(DaemonEvent::Exited { agent, code });
                    let slot = self.slot(&id);
                    let mut live = slot.lock().await;
                    // Personne n'a repris la session : c'est une sortie imprévue (panne).
                    if live
                        .as_ref()
                        .is_some_and(|session| session.generation == generation)
                    {
                        *live = None;
                        drop(live);
                        eprintln!(
                            "agent {id} : process sorti de lui-même (code {code:?}) : {}",
                            stderr_tail.join(" | ")
                        );
                        self.settle(&id);
                    }
                }
            }
        }
    }

    /// Tue la session d'un agent s'il en a une ; rend `true` dans ce cas.
    async fn kill(&self, id: &AgentId) -> bool {
        let slot = self.slot(id);
        let mut live = slot.lock().await;
        let Some(session) = live.take() else {
            return false;
        };
        session.control.kill();
        exited(session.exited).await;
        self.settle(id);
        true
    }

    /// La session d'un agent a disparu : il redevient parké (sauf s'il est retiré).
    fn settle(&self, id: &AgentId) {
        locked(&self.inner.running).remove(id);
        self.revoke_tokens(id);
        // Échoue seulement pour un agent retiré, qui doit le rester.
        let _ = self.registry().set_state(id, AgentState::Parked);
        self.publish_status(id);
    }

    fn spec_for(&self, record: &AgentRecord) -> Result<SessionSpec> {
        let settings = &self.inner.settings;
        std::fs::create_dir_all(&record.workdir)?;
        let mut spec = SessionSpec::new(record.engine, &record.workdir);
        spec.model = record.model.clone();
        spec.resume = record.session_id.clone();
        spec.program = settings.programs.get(&record.engine).cloned();
        if let Some(base) = &settings.mcp_url {
            spec.mcp_servers = vec![McpServer {
                name: "argos".into(),
                url: format!("{base}/mcp/{}", self.mcp_token(&record.id)),
            }];
        }
        if record.engine == EngineKind::Codex {
            let home = settings.home.join("codex").join(record.id.as_str());
            prepare_codex_home(&home, &settings.user_codex_home)?;
            spec.codex_home = Some(home);
        }
        Ok(spec)
    }

    fn revoke_tokens(&self, id: &AgentId) {
        locked(&self.inner.tokens).retain(|_, holder| holder != id);
    }

    fn slot(&self, id: &AgentId) -> Arc<AsyncMutex<Option<Live>>> {
        Arc::clone(locked(&self.inner.slots).entry(id.clone()).or_default())
    }

    fn status(&self, record: AgentRecord) -> AgentStatus {
        let busy = locked(&self.inner.running).get(&record.id).cloned();
        AgentStatus {
            running: busy.is_some(),
            busy: busy.is_some_and(|busy| busy.load(Ordering::SeqCst)),
            record,
        }
    }

    fn publish(&self, event: DaemonEvent) {
        // Sans abonné, l'événement est simplement perdu.
        let _ = self.inner.events.send(event);
    }

    fn publish_status(&self, id: &AgentId) {
        if let Ok(agent) = self.inspect(id) {
            self.publish(DaemonEvent::AgentUpdated { agent });
        }
    }
}

/// Attend la sortie signalée par la tâche de la session ; `false` après le délai.
async fn exited(mut signal: watch::Receiver<bool>) -> bool {
    // Un émetteur disparu signifie aussi que la session est finie.
    tokio::time::timeout(EXIT_TIMEOUT, signal.wait_for(|exited| *exited))
        .await
        .is_ok()
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("verrou empoisonné")
}
