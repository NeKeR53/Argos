//! Outils MCP d'Argos, par niveau (D15). Chaque appel est vérifié : un agent n'agit que
//! sur ses enfants directs (D13). Les comptes rendus et escalades arrivent au parent
//! comme un nouveau message, qui le réveille au besoin : personne n'attend personne.
//! Le board est tenu ici de façon déterministe : un Worker part sur une tâche du board
//! (D17) et son compte rendu met la tâche à jour.

use crate::registry::{NewAgent, Position, TaskUpdate};
use crate::supervisor::Supervisor;
use argos_core::protocol::AgentStatus;
use argos_core::{
    AgentId, AgentRecord, AgentState, Charter, EngineKind, Level, MissionState, TaskState,
};
use serde_json::{Value, json};
use std::fmt::Display;

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// Les outils d'un niveau, tels qu'annoncés par `tools/list`.
pub fn tools_for(level: Level) -> Vec<Tool> {
    match level {
        Level::Assistant => vec![
            tool(
                "list_agents",
                "Liste tes Agents : nom, périmètre, moteur, état. Consulte-la avant d'en créer un : réutilise l'Agent dont le périmètre correspond.",
                &[],
                &[],
            ),
            tool(
                "create_agent",
                "Crée un Agent durable, dédié à un projet ou un domaine. Son nom est unique parmi tes Agents.",
                &[
                    ("name", "Nom court et parlant."),
                    (
                        "scope",
                        "Périmètre : le projet, domaine ou tâche récurrente dont il a la charge.",
                    ),
                    ("engine", "« claude » ou « codex ». Par défaut, le tien."),
                    (
                        "model",
                        "Modèle du moteur. Par défaut, le tien si le moteur est le même.",
                    ),
                    (
                        "workdir",
                        "Répertoire de travail. Par défaut, un dossier dédié.",
                    ),
                ],
                &["name", "scope"],
            ),
            tool(
                "assign_mission",
                "Confie une mission à l'un de tes Agents : elle s'inscrit sur son board et il est réveillé pour la découper en tâches. Son compte rendu te reviendra plus tard.",
                &[
                    ("agent", AGENT_REF),
                    ("mission", "La mission, complète et autonome."),
                ],
                &["agent", "mission"],
            ),
            tool(
                "send_to_agent",
                "Envoie un simple message à l'un de tes Agents (réponse à sa question, précision) ; il est réveillé s'il est parké. Pour un nouveau travail, préfère assign_mission.",
                &[("agent", AGENT_REF), ("message", "Le message.")],
                &["agent", "message"],
            ),
            tool(
                "inspect_agent",
                "État d'un de tes Agents : son board (missions, tâches) et ses Workers.",
                &[("agent", AGENT_REF)],
                &["agent"],
            ),
            tool(
                "prioritize_mission",
                "Change la priorité d'une mission sur le board de son Agent.",
                &[
                    ("mission", "Identifiant de la mission (m-…)."),
                    ("position", POSITION),
                ],
                &["mission", "position"],
            ),
            tool(
                "cancel_mission",
                "Annule une mission : ses tâches non terminées sont annulées et ses Workers arrêtés.",
                &[("mission", "Identifiant de la mission (m-…).")],
                &["mission"],
            ),
            tool(
                "interrupt_agent",
                "Interrompt le tour en cours d'un Agent ; sa session continue.",
                &[("agent", AGENT_REF)],
                &["agent"],
            ),
            tool(
                "park_agent",
                "Met un Agent au repos (process arrêté proprement) ; un message le réveillera.",
                &[("agent", AGENT_REF)],
                &["agent"],
            ),
            tool(
                "stop_agent",
                "Arrêt dur d'un Agent et de tous ses Workers.",
                &[("agent", AGENT_REF)],
                &["agent"],
            ),
            tool(
                "retire_agent",
                "Retire définitivement un Agent et ses Workers.",
                &[("agent", AGENT_REF)],
                &["agent"],
            ),
        ],
        Level::Agent => vec![
            tool(
                "board",
                "Ton board : missions ouvertes et tâches, dans l'ordre de priorité.",
                &[],
                &[],
            ),
            tool(
                "add_task",
                "Inscrit une tâche à faire sur ton board. Découpe toute mission en tâches avant de lancer des Workers.",
                &[
                    ("title", "Ce qu'il faut faire, en une ligne."),
                    (
                        "detail",
                        "Tout ce que le Worker devra savoir : il ne connaît rien d'autre.",
                    ),
                    (
                        "mission",
                        "Identifiant de la mission (m-…) à laquelle elle appartient.",
                    ),
                ],
                &["title"],
            ),
            tool(
                "update_task",
                "Modifie une tâche de ton board (état, titre, détail, résultat).",
                &[
                    ("task", TASK_REF),
                    ("state", "Nouvel état."),
                    ("title", "Nouveau titre."),
                    ("detail", "Nouveau détail."),
                    ("result", "Résultat ou note."),
                ],
                &["task"],
            ),
            tool(
                "reorder_task",
                "Change la priorité d'une tâche sur ton board.",
                &[("task", TASK_REF), ("position", POSITION)],
                &["task", "position"],
            ),
            tool(
                "spawn_worker",
                "Lance un Worker éphémère sur UNE tâche de ton board (à faire, bloquée ou interrompue). Il reçoit son titre et son détail, travaille dans ton répertoire et te rendra compte par un rapport.",
                &[
                    ("task", TASK_REF),
                    ("engine", "« claude » ou « codex ». Par défaut, le tien."),
                    (
                        "model",
                        "Modèle du moteur. Par défaut, le tien si le moteur est le même.",
                    ),
                ],
                &["task"],
            ),
            tool("list_workers", "Liste tes Workers et leur état.", &[], &[]),
            tool(
                "send_to_worker",
                "Envoie un message à l'un de tes Workers.",
                &[("worker", WORKER_REF), ("message", "Le message.")],
                &["worker", "message"],
            ),
            tool(
                "inspect_worker",
                "État d'un de tes Workers et de sa tâche.",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "interrupt_worker",
                "Interrompt le tour en cours d'un Worker.",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "park_worker",
                "Met un Worker en pause (process arrêté proprement) ; sa tâche reste en cours.",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "stop_worker",
                "Arrêt dur d'un Worker ; sa tâche reste en cours.",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "retire_worker",
                "Retire définitivement un Worker, une fois sa tâche finie ou abandonnée. Une tâche encore en cours passe « interrompue ».",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "report_to_assistant",
                "Rends compte à l'Assistant : très court, l'essentiel et ce qui demande une décision. Avec « mission » et « outcome: done », la mission est close.",
                &[
                    ("summary", "Le compte rendu."),
                    ("mission", "Identifiant de la mission (m-…) concernée."),
                    ("outcome", "Où en est la mission."),
                ],
                &["summary"],
            ),
            tool(
                "escalate_to_user",
                "Demande l'aide de l'utilisateur (information manquante, décision qui lui revient). L'Assistant le préviendra.",
                &[("question", "La question, avec le contexte nécessaire.")],
                &["question"],
            ),
        ],
        Level::Worker => vec![
            tool(
                "report_to_agent",
                "Rends compte de ta tâche à ton Agent. « outcome: done » la marque terminée, « blocked » bloquée.",
                &[
                    ("summary", "Le résultat, ou ce qui bloque."),
                    ("outcome", "Où en est ta tâche."),
                ],
                &["summary"],
            ),
            tool(
                "escalate_to_agent",
                "Pose à ton Agent une question qui te bloque.",
                &[("question", "La question, avec le contexte nécessaire.")],
                &["question"],
            ),
        ],
    }
}

const AGENT_REF: &str = "Identifiant ou nom de l'Agent.";
const WORKER_REF: &str = "Identifiant du Worker.";
const TASK_REF: &str = "Identifiant de la tâche (t-…).";
const POSITION: &str = "« top », « bottom » ou un rang (1 = en tête).";

fn tool(
    name: &'static str,
    description: &'static str,
    properties: &[(&str, &str)],
    required: &[&str],
) -> Tool {
    let properties: serde_json::Map<String, Value> = properties
        .iter()
        .map(|(key, about)| {
            let choices: &[&str] = match *key {
                "engine" => &["claude", "codex"],
                "outcome" => &["progress", "done", "blocked"],
                "state" => &["pending", "in_progress", "blocked", "done", "cancelled"],
                _ => &[],
            };
            let mut property = json!({"type": "string", "description": about});
            if !choices.is_empty() {
                property["enum"] = json!(choices);
            }
            (key.to_string(), property)
        })
        .collect();
    Tool {
        name,
        description,
        schema: json!({"type": "object", "properties": properties, "required": required}),
    }
}

/// Exécute un outil pour `caller`. `Err` porte un message destiné à l'agent.
pub async fn call(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let target_key = match caller.level {
        Level::Assistant => "agent",
        _ => "worker",
    };
    match (caller.level, name) {
        (Level::Assistant, "list_agents") | (Level::Agent, "list_workers") => {
            listed(supervisor.children(&caller.id).map_err(said)?)
        }
        (Level::Assistant, "create_agent") => create_agent(supervisor, caller, args),
        (Level::Assistant, "assign_mission") => assign_mission(supervisor, caller, args).await,
        (Level::Assistant, "prioritize_mission") => {
            let mission = own_mission(supervisor, caller, text(args, "mission")?)?;
            supervisor
                .registry()
                .reposition_mission(&mission, position(args)?)
                .map_err(said)?;
            Ok(format!("Mission {mission} déplacée."))
        }
        (Level::Assistant, "cancel_mission") => cancel_mission(supervisor, caller, args).await,
        (Level::Agent, "board") => {
            let board = supervisor.registry().board(&caller.id).map_err(said)?;
            Ok(json!(board).to_string())
        }
        (Level::Agent, "add_task") => {
            let task = supervisor
                .registry()
                .add_task(
                    &caller.id,
                    text(args, "title")?,
                    optional(args, "detail"),
                    optional(args, "mission"),
                )
                .map_err(said)?;
            Ok(format!("Tâche {} inscrite : {}.", task.id, task.title))
        }
        (Level::Agent, "update_task") => {
            let task = own_task(supervisor, caller, text(args, "task")?)?;
            let state = optional(args, "state")
                .map(|state| state.parse::<TaskState>())
                .transpose()
                .map_err(said)?;
            let update = TaskUpdate {
                state,
                title: optional(args, "title").map(String::from),
                detail: optional(args, "detail").map(String::from),
                result: optional(args, "result").map(String::from),
            };
            let task = supervisor
                .registry()
                .update_task(&task, update)
                .map_err(said)?;
            Ok(format!("Tâche {} : {}.", task.id, task.state))
        }
        (Level::Agent, "reorder_task") => {
            let task = own_task(supervisor, caller, text(args, "task")?)?;
            supervisor
                .registry()
                .reposition_task(&task, position(args)?)
                .map_err(said)?;
            Ok(format!("Tâche {task} déplacée."))
        }
        (Level::Agent, "spawn_worker") => spawn_worker(supervisor, caller, args).await,
        (Level::Assistant, "send_to_agent") | (Level::Agent, "send_to_worker") => {
            let target = child(supervisor, caller, text(args, target_key)?)?;
            supervisor
                .send(&target.id, text(args, "message")?)
                .await
                .map_err(said)?;
            Ok(format!("Transmis à {}.", label(&target)))
        }
        (Level::Assistant, "inspect_agent") | (Level::Agent, "inspect_worker") => {
            let target = child(supervisor, caller, text(args, target_key)?)?;
            inspect(supervisor, &target)
        }
        (Level::Assistant, "interrupt_agent") | (Level::Agent, "interrupt_worker") => {
            let target = child(supervisor, caller, text(args, target_key)?)?;
            supervisor.interrupt(&target.id).await.map_err(said)?;
            Ok(format!("{} interrompu.", label(&target)))
        }
        (Level::Assistant, "park_agent") | (Level::Agent, "park_worker") => {
            let target = child(supervisor, caller, text(args, target_key)?)?;
            supervisor.park(&target.id).await.map_err(said)?;
            Ok(format!("{} au repos.", label(&target)))
        }
        (Level::Assistant, "stop_agent") | (Level::Agent, "stop_worker") => {
            let target = child(supervisor, caller, text(args, target_key)?)?;
            let stopped = supervisor.stop(&target.id).await.map_err(said)?;
            Ok(format!(
                "{} arrêté ({} process).",
                label(&target),
                stopped.len()
            ))
        }
        (Level::Assistant, "retire_agent") | (Level::Agent, "retire_worker") => {
            let target = child(supervisor, caller, text(args, target_key)?)?;
            supervisor.retire(&target.id).await.map_err(said)?;
            Ok(format!("{} retiré.", label(&target)))
        }
        (Level::Agent, "report_to_assistant") => {
            let summary = text(args, "summary")?;
            let mission = optional(args, "mission");
            if let Some(mission) = mission
                && optional(args, "outcome") == Some("done")
            {
                let mission = supervisor.registry().mission(mission).map_err(said)?;
                if mission.agent != caller.id {
                    return Err(format!("la mission {} n'est pas la tienne", mission.id));
                }
                supervisor
                    .registry()
                    .set_mission_state(&mission.id, MissionState::Done)
                    .map_err(said)?;
            }
            let kind = match mission {
                Some(mission) => format!("Rapport sur la mission {mission}"),
                None => "Rapport".into(),
            };
            deliver(supervisor, caller, &kind, summary).await
        }
        (Level::Worker, "report_to_agent") => report_task(supervisor, caller, args).await,
        (Level::Agent, "escalate_to_user") => {
            deliver(
                supervisor,
                caller,
                "Escalade pour l'utilisateur",
                text(args, "question")?,
            )
            .await
        }
        (Level::Worker, "escalate_to_agent") => {
            deliver(supervisor, caller, "Question", text(args, "question")?).await
        }
        _ => Err(format!("outil inconnu pour un {} : {name}", caller.level)),
    }
}

fn create_agent(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let name = text(args, "name")?;
    let (engine, model) = engine_and_model(caller, args)?;
    let workdir = match optional(args, "workdir") {
        Some(dir) => dir.into(),
        None => supervisor.agents_dir().join(format!(
            "{}-{}",
            slug(name),
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        )),
    };
    let agent = supervisor
        .create(NewAgent {
            parent: caller.id.clone(),
            level: Level::Agent,
            engine,
            model,
            workdir,
            charter: Some(Charter {
                name: name.into(),
                scope: text(args, "scope")?.into(),
            }),
        })
        .map_err(said)?;
    Ok(format!(
        "Agent « {name} » créé ({}), au repos jusqu'à sa première mission. Répertoire : {}.",
        agent.id,
        agent.workdir.display()
    ))
}

async fn assign_mission(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let target = child(supervisor, caller, text(args, "agent")?)?;
    let mission = supervisor
        .registry()
        .add_mission(&target.id, text(args, "mission")?)
        .map_err(said)?;
    supervisor
        .send(
            &target.id,
            format!("[Mission {} de l'Assistant]\n{}", mission.id, mission.text),
        )
        .await
        .map_err(said)?;
    Ok(format!(
        "Mission {} confiée à {}.",
        mission.id,
        label(&target)
    ))
}

async fn cancel_mission(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let id = own_mission(supervisor, caller, text(args, "mission")?)?;
    let mission = supervisor.registry().mission(&id).map_err(said)?;
    let workers = supervisor
        .registry()
        .set_mission_state(&id, MissionState::Cancelled)
        .map_err(said)?;
    for worker in &workers {
        supervisor.stop(worker).await.map_err(said)?;
    }
    supervisor
        .send(
            &mission.agent,
            format!("[Mission {id} annulée par l'Assistant]"),
        )
        .await
        .map_err(said)?;
    Ok(format!(
        "Mission {id} annulée ({} Worker(s) arrêté(s)).",
        workers.len()
    ))
}

async fn spawn_worker(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let task_id = own_task(supervisor, caller, text(args, "task")?)?;
    let task = supervisor.registry().task(&task_id).map_err(said)?;
    if !matches!(
        task.state,
        TaskState::Pending | TaskState::Blocked | TaskState::Interrupted
    ) {
        return Err(format!("la tâche {task_id} est {}", task.state));
    }
    let (engine, model) = engine_and_model(caller, args)?;
    let worker = supervisor
        .create(NewAgent {
            parent: caller.id.clone(),
            level: Level::Worker,
            engine,
            model,
            workdir: caller.workdir.clone(),
            charter: None,
        })
        .map_err(said)?;
    supervisor
        .registry()
        .assign_task(&task_id, &worker.id)
        .map_err(said)?;

    let mut brief = format!(
        "[Tâche {task_id} confiée par {}]\n{}",
        label(caller),
        task.title
    );
    if let Some(detail) = &task.detail {
        brief.push_str(&format!("\n\n{detail}"));
    }
    if let Some(mission) = &task.mission {
        let mission = supervisor.registry().mission(mission).map_err(said)?;
        brief.push_str(&format!(
            "\n\nContexte (mission {}) : {}",
            mission.id, mission.text
        ));
    }
    supervisor.send(&worker.id, brief).await.map_err(said)?;
    Ok(format!(
        "Worker {} lancé sur la tâche {task_id}. Il te rendra compte par un rapport.",
        worker.id
    ))
}

/// Compte rendu d'un Worker : met sa tâche à jour, puis prévient son Agent.
async fn report_task(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let summary = text(args, "summary")?;
    let state = match optional(args, "outcome") {
        Some("done") => Some(TaskState::Done),
        Some("blocked") => Some(TaskState::Blocked),
        _ => None,
    };
    let task = supervisor
        .registry()
        .task_of_worker(&caller.id)
        .map_err(said)?;
    let kind = match &task {
        Some(task) => {
            supervisor
                .registry()
                .update_task(
                    &task.id,
                    TaskUpdate {
                        state,
                        result: Some(summary.into()),
                        ..TaskUpdate::default()
                    },
                )
                .map_err(said)?;
            match state {
                Some(state) => format!("Rapport sur la tâche {} ({state})", task.id),
                None => format!("Rapport sur la tâche {}", task.id),
            }
        }
        None => "Rapport".into(),
    };
    deliver(supervisor, caller, &kind, summary).await
}

fn inspect(supervisor: &Supervisor, target: &AgentRecord) -> Result<String, String> {
    let mut state = summary(&supervisor.inspect(&target.id).map_err(said)?);
    match target.level {
        Level::Agent => {
            let workers = supervisor.children(&target.id).map_err(said)?;
            state["workers"] = workers.iter().map(summary).collect();
            state["board"] = json!(supervisor.registry().board(&target.id).map_err(said)?);
        }
        Level::Worker => {
            let task = supervisor
                .registry()
                .task_of_worker(&target.id)
                .map_err(said)?;
            state["task"] = json!(task);
        }
        Level::Assistant => {}
    }
    Ok(state.to_string())
}

/// Remonte un compte rendu ou une question au parent, comme un nouveau message.
async fn deliver(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    kind: &str,
    body: &str,
) -> Result<String, String> {
    let parent = caller.parent.as_ref().ok_or("tu n'as pas de parent")?;
    supervisor
        .send(parent, format!("[{kind} de {}]\n{body}", label(caller)))
        .await
        .map_err(said)?;
    Ok("Transmis.".into())
}

/// Un enfant direct de `caller`, désigné par son id (ou son nom pour un Agent).
fn child(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    reference: &str,
) -> Result<AgentRecord, String> {
    let found = match supervisor.record(&AgentId::new(reference)) {
        Ok(record) => Some(record),
        Err(_) if caller.level == Level::Assistant => {
            supervisor.registry().agent_named(reference).map_err(said)?
        }
        Err(_) => None,
    };
    let record = found.ok_or_else(|| format!("aucun agent « {reference} »"))?;
    if record.parent.as_ref() != Some(&caller.id) {
        return Err(format!("« {reference} » n'est pas sous ton autorité"));
    }
    if record.state == AgentState::Retired {
        return Err(format!("{} est retiré", label(&record)));
    }
    Ok(record)
}

/// Une mission du board de l'un des Agents de `caller`.
fn own_mission(supervisor: &Supervisor, caller: &AgentRecord, id: &str) -> Result<String, String> {
    let mission = supervisor.registry().mission(id).map_err(said)?;
    child(supervisor, caller, mission.agent.as_str())?;
    Ok(mission.id)
}

/// Une tâche du board de `caller`.
fn own_task(supervisor: &Supervisor, caller: &AgentRecord, id: &str) -> Result<String, String> {
    let task = supervisor.registry().task(id).map_err(said)?;
    if task.agent != caller.id {
        return Err(format!("la tâche {id} n'est pas sur ton board"));
    }
    Ok(task.id)
}

/// Moteur et modèle demandés ; à défaut ceux de l'appelant (D21 : le parent choisit).
fn engine_and_model(
    caller: &AgentRecord,
    args: &Value,
) -> Result<(EngineKind, Option<String>), String> {
    let engine = match optional(args, "engine") {
        Some(engine) => engine.parse().map_err(said)?,
        None => caller.engine,
    };
    let model = match optional(args, "model") {
        Some(model) => Some(model.to_string()),
        None if engine == caller.engine => caller.model.clone(),
        None => None,
    };
    Ok((engine, model))
}

fn position(args: &Value) -> Result<Position, String> {
    let text = text(args, "position")?;
    Position::parse(text).ok_or_else(|| format!("position illisible : « {text} »"))
}

fn label(agent: &AgentRecord) -> String {
    match &agent.charter {
        Some(charter) => format!("l'Agent « {} » ({})", charter.name, agent.id),
        None => format!("{} {}", agent.level, agent.id),
    }
}

fn summary(status: &AgentStatus) -> Value {
    let record = &status.record;
    json!({
        "id": record.id,
        "level": record.level,
        "name": record.charter.as_ref().map(|charter| &charter.name),
        "scope": record.charter.as_ref().map(|charter| &charter.scope),
        "engine": record.engine,
        "model": record.model,
        "state": record.state,
        "running": status.running,
        "busy": status.busy,
    })
}

fn listed(statuses: Vec<AgentStatus>) -> Result<String, String> {
    if statuses.is_empty() {
        return Ok("Aucun pour l'instant.".into());
    }
    Ok(Value::from(statuses.iter().map(summary).collect::<Vec<_>>()).to_string())
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    optional(args, key).ok_or_else(|| format!("argument « {key} » manquant"))
}

fn optional<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn slug(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "agent".into()
    } else {
        slug
    }
}

fn said(error: impl Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_has_its_own_tools() {
        let names = |level| -> Vec<&str> { tools_for(level).iter().map(|t| t.name).collect() };
        assert!(names(Level::Assistant).contains(&"create_agent"));
        assert!(names(Level::Assistant).contains(&"assign_mission"));
        assert!(!names(Level::Assistant).contains(&"spawn_worker"));
        assert!(names(Level::Agent).contains(&"spawn_worker"));
        assert!(names(Level::Agent).contains(&"add_task"));
        assert!(names(Level::Agent).contains(&"escalate_to_user"));
        assert_eq!(
            names(Level::Worker),
            vec!["report_to_agent", "escalate_to_agent"]
        );
    }

    #[test]
    fn schemas_declare_their_required_arguments_and_choices() {
        let find = |level, name| {
            tools_for(level)
                .into_iter()
                .find(|t| t.name == name)
                .unwrap()
        };
        let create = find(Level::Assistant, "create_agent");
        assert_eq!(create.schema["required"], json!(["name", "scope"]));
        assert_eq!(
            create.schema["properties"]["engine"]["enum"],
            json!(["claude", "codex"])
        );
        let report = find(Level::Worker, "report_to_agent");
        assert_eq!(
            report.schema["properties"]["outcome"]["enum"],
            json!(["progress", "done", "blocked"])
        );
    }

    #[test]
    fn slugs_are_safe_directory_names() {
        assert_eq!(slug("Home Assistant (Pi)"), "home-assistant--pi");
        assert_eq!(slug("!!!"), "agent");
    }
}
