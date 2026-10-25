//! Outils MCP d'Argos, par niveau (D15). Chaque appel est vérifié : un agent n'agit que
//! sur ses enfants directs (D13). Les comptes rendus et escalades arrivent au parent
//! comme un nouveau message, qui le réveille au besoin : personne n'attend personne.

use crate::registry::NewAgent;
use crate::supervisor::Supervisor;
use argos_core::protocol::AgentStatus;
use argos_core::{AgentId, AgentRecord, AgentState, Charter, EngineKind, Level};
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
                "send_to_agent",
                "Confie une mission ou un message à l'un de tes Agents ; il est réveillé s'il est parké. Sa réponse te reviendra plus tard sous forme de rapport.",
                &[
                    ("agent", AGENT_REF),
                    ("message", "La mission ou le message."),
                ],
                &["agent", "message"],
            ),
            tool(
                "inspect_agent",
                "État d'un de tes Agents et de ses Workers.",
                &[("agent", AGENT_REF)],
                &["agent"],
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
                "spawn_worker",
                "Lance un Worker éphémère sur UNE tâche. Il travaille dans ton répertoire et te rendra compte par un rapport.",
                &[
                    (
                        "task",
                        "La tâche, complète et autonome : le Worker ne connaît rien d'autre.",
                    ),
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
                "État d'un de tes Workers.",
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
                "Met un Worker en pause (process arrêté proprement).",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "stop_worker",
                "Arrêt dur d'un Worker.",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "retire_worker",
                "Retire définitivement un Worker, une fois sa tâche finie ou abandonnée.",
                &[("worker", WORKER_REF)],
                &["worker"],
            ),
            tool(
                "report_to_assistant",
                "Rends compte à l'Assistant : très court, l'essentiel et ce qui demande une décision.",
                &[("summary", "Le compte rendu.")],
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
                "Rends compte de ta tâche à ton Agent : le résultat, ou ce qui bloque.",
                &[("summary", "Le compte rendu.")],
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

fn tool(
    name: &'static str,
    description: &'static str,
    properties: &[(&str, &str)],
    required: &[&str],
) -> Tool {
    let properties: serde_json::Map<String, Value> = properties
        .iter()
        .map(|(key, about)| {
            let property = match *key {
                "engine" => {
                    json!({"type": "string", "enum": ["claude", "codex"], "description": about})
                }
                _ => json!({"type": "string", "description": about}),
            };
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
            let mut state = summary(&supervisor.inspect(&target.id).map_err(said)?);
            if target.level == Level::Agent {
                let workers = supervisor.children(&target.id).map_err(said)?;
                state["workers"] = workers.iter().map(summary).collect();
            }
            Ok(state.to_string())
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
        (Level::Agent, "report_to_assistant") | (Level::Worker, "report_to_agent") => {
            deliver(supervisor, caller, "Rapport", text(args, "summary")?).await
        }
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

async fn spawn_worker(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let task = text(args, "task")?;
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
    supervisor.send(&worker.id, task).await.map_err(said)?;
    Ok(format!(
        "Worker {} lancé sur la tâche. Il te rendra compte par un rapport.",
        worker.id
    ))
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
        assert!(!names(Level::Assistant).contains(&"spawn_worker"));
        assert!(names(Level::Agent).contains(&"spawn_worker"));
        assert!(names(Level::Agent).contains(&"escalate_to_user"));
        assert_eq!(
            names(Level::Worker),
            vec!["report_to_agent", "escalate_to_agent"]
        );
    }

    #[test]
    fn schemas_declare_their_required_arguments() {
        let create = tools_for(Level::Assistant)
            .into_iter()
            .find(|t| t.name == "create_agent")
            .unwrap();
        assert_eq!(create.schema["required"], json!(["name", "scope"]));
        assert_eq!(
            create.schema["properties"]["engine"]["enum"],
            json!(["claude", "codex"])
        );
    }

    #[test]
    fn slugs_are_safe_directory_names() {
        assert_eq!(slug("Home Assistant (Pi)"), "home-assistant--pi");
        assert_eq!(slug("!!!"), "agent");
    }
}
