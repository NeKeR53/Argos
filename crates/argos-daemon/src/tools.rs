//! Outils MCP d'Argos, par niveau (D15). Chaque appel est vérifié : un agent n'agit que
//! sur ses enfants directs (D13). Les comptes rendus et escalades arrivent au parent
//! comme un nouveau message, qui le réveille au besoin : personne n'attend personne.
//! Le board est tenu ici de façon déterministe : un Worker part sur une tâche du board
//! (D17) et son compte rendu met la tâche à jour.

use crate::policy::ModelPolicy;
use crate::registry::{NewAgent, NewTrigger, Position, TaskUpdate};
use crate::supervisor::{Supervisor, SupervisorError};
use argos_core::protocol::{AgentStatus, Origin};
use argos_core::{
    AgentId, AgentRecord, AgentState, Charter, Level, MissionState, TaskState, TriggerKind,
};
use serde_json::{Value, json};
use std::fmt::Display;

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// Les outils d'un niveau, tels qu'annoncés par `tools/list`. L'argument `profile` y
/// énumère les profils de la politique de modèles, avec leur usage (D22).
pub fn tools_for(level: Level, policy: &ModelPolicy) -> Vec<Tool> {
    let mut tools = level_tools(level);
    if level.has_memory() {
        tools.extend(memory_tools());
        tools.extend(surface_tools());
        tools.extend(trigger_tools());
    }
    let profiles: Vec<&String> = policy.profiles.keys().collect();
    for tool in &mut tools {
        if let Some(profile) = tool.schema["properties"].get_mut("profile") {
            profile["enum"] = json!(profiles);
            profile["description"] = json!(format!(
                "Profil de modèle selon le travail : {}. Par défaut « {} ».",
                policy.catalogue(),
                policy.default_profile
            ));
        }
    }
    tools
}

/// Déclencheurs de l'Assistant et des Agents (D29–D31) : ce qui les réveille sans qu'on
/// leur parle.
fn trigger_tools() -> Vec<Tool> {
    vec![
        tool(
            "trigger_create",
            "Crée un déclencheur : à chaque événement, sa consigne te sera remise (ou à l'un de tes Agents, pour l'Assistant), même au repos. Sources : every (toutes les N minutes), cron (expression à 5 champs, heure locale), watch (nouveau fichier dans un dossier), webhook (appel HTTP externe, par exemple une automatisation Home Assistant), switch (interrupteur que l'utilisateur actionne).",
            &[
                ("name", "Nom court."),
                ("source", "every, cron, watch, webhook ou switch."),
                ("prompt", "La consigne à exécuter à chaque déclenchement."),
                (
                    "every_minutes",
                    "Pour every : l'intervalle en minutes (1 au moins).",
                ),
                (
                    "cron",
                    "Pour cron : l'expression, par exemple « 0 8 * * 1-5 ».",
                ),
                (
                    "path",
                    "Pour watch : le dossier à surveiller (chemin absolu).",
                ),
                (
                    "target",
                    "Pour l'Assistant : l'Agent qui reçoit les événements. Par défaut, toi.",
                ),
            ],
            &["name", "source", "prompt"],
        ),
        tool(
            "trigger_list",
            "Liste les déclencheurs dont tu es propriétaire ou cible.",
            &[],
            &[],
        ),
        tool(
            "trigger_set",
            "Active (on) ou suspend (off) un de tes déclencheurs.",
            &[("trigger", TRIGGER_REF), ("status", "on ou off.")],
            &["trigger", "status"],
        ),
        tool(
            "trigger_delete",
            "Supprime un de tes déclencheurs.",
            &[("trigger", TRIGGER_REF)],
            &["trigger"],
        ),
    ]
}

/// Surfaces de l'Assistant et des Agents (D26–D28) : pages HTML hébergées par l'app.
fn surface_tools() -> Vec<Tool> {
    vec![
        tool(
            "surface_create",
            "Crée une page HTML pour l'utilisateur. Un cockpit est vivant (tu le mets à jour, tu lui pousses des données, il t'envoie ses actions) ; un artifact est un résultat figé (synthèse, graphiques). La page tourne dans un bac à sable sans réseau : tout doit être dans le HTML (CSS et JS en ligne, images en data:). Le SDK window.argos y est déjà chargé.",
            &[
                ("kind", "cockpit ou artifact."),
                ("title", "Titre court, affiché dans l'onglet."),
                ("html", "La page complète : HTML, CSS et JS en ligne."),
            ],
            &["kind", "title", "html"],
        ),
        tool(
            "surface_update",
            "Remplace le code et/ou le titre d'une de tes surfaces ; la page se recharge chez l'utilisateur.",
            &[
                ("surface", SURFACE_REF),
                ("html", "Le nouveau code complet."),
                ("title", "Le nouveau titre."),
            ],
            &["surface"],
        ),
        tool(
            "surface_push",
            "Pousse des données vers un de tes cockpits ouverts, sans le recharger : la page les reçoit par argos.on('data', …).",
            &[("surface", SURFACE_REF), ("data", "Les données, en JSON.")],
            &["surface", "data"],
        ),
        tool("surface_list", "Liste tes surfaces.", &[], &[]),
        tool(
            "surface_delete",
            "Supprime une de tes surfaces.",
            &[("surface", SURFACE_REF)],
            &["surface"],
        ),
    ]
}

/// Mémoire persistante de l'Assistant et des Agents (D23–D24).
fn memory_tools() -> Vec<Tool> {
    vec![
        tool(
            "memory_read",
            "Lit un fichier de ta mémoire (index.md, journal.md, state.md, notes/<nom>.md). Sans argument, liste les fichiers.",
            &[("file", "Le fichier à lire.")],
            &[],
        ),
        tool(
            "memory_write",
            "Écrit dans ta mémoire : index.md (l'essentiel, court), journal.md (ce qui s'est passé), notes/<nom>.md (le détail). Chaque écriture est historisée.",
            &[
                ("file", "index.md, journal.md ou notes/<nom>.md."),
                ("content", "Le contenu."),
                (
                    "mode",
                    "Remplacer le fichier ou ajouter à la fin. Par défaut : remplacer.",
                ),
            ],
            &["file", "content"],
        ),
        tool(
            "memory_search",
            "Cherche un mot ou une expression dans toute ta mémoire.",
            &[("query", "Ce qu'il faut chercher.")],
            &["query"],
        ),
    ]
}

fn level_tools(level: Level) -> Vec<Tool> {
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
                    ("profile", "Profil de modèle."),
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
                &[("task", TASK_REF), ("profile", "Profil de modèle.")],
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
            tool(
                "list_peers",
                "Liste les autres Agents (nom, périmètre, état) : ceux à qui tu peux poser une question.",
                &[],
                &[],
            ),
            tool(
                "ask_agent",
                "Pose une question à un autre Agent, sur son domaine (ex. demander à l'Agent « Mail » un courriel). Jamais un ordre. Sa réponse t'arrivera plus tard comme un message.",
                &[
                    ("agent", PEER_REF),
                    ("question", "La question, avec le contexte nécessaire."),
                ],
                &["agent", "question"],
            ),
            tool(
                "tell_agent",
                "Transmet une information à un autre Agent, ou répond à sa question. Jamais un ordre.",
                &[
                    ("agent", PEER_REF),
                    ("message", "L'information ou la réponse."),
                ],
                &["agent", "message"],
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
const TRIGGER_REF: &str = "Identifiant du déclencheur (tr-…).";
const SURFACE_REF: &str = "Identifiant de la surface (s-…).";
const PEER_REF: &str = "Nom (ou identifiant) de l'autre Agent.";
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
                "outcome" => &["progress", "done", "blocked"],
                "mode" => &["replace", "append"],
                "kind" => &["cockpit", "artifact"],
                "source" => &["every", "cron", "watch", "webhook", "switch"],
                "status" => &["on", "off"],
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
            send_as(supervisor, caller, &target.id, text(args, "message")?)
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
            journal(
                supervisor,
                &caller.id,
                &format!("{} retiré", label(&target)),
            );
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
                journal(
                    supervisor,
                    &caller.id,
                    &format!("mission {} close : {summary}", mission.id),
                );
                if let Some(parent) = &caller.parent {
                    journal(
                        supervisor,
                        parent,
                        &format!(
                            "mission {} close par {} : {summary}",
                            mission.id,
                            label(caller)
                        ),
                    );
                }
            }
            let kind = match mission {
                Some(mission) => format!("Rapport sur la mission {mission}"),
                None => "Rapport".into(),
            };
            deliver(supervisor, caller, &kind, summary).await
        }
        (Level::Worker, "report_to_agent") => report_task(supervisor, caller, args).await,
        (Level::Agent, "list_peers") => {
            let peers = match &caller.parent {
                Some(assistant) => supervisor.children(assistant).map_err(said)?,
                None => vec![],
            };
            listed(
                peers
                    .into_iter()
                    .filter(|peer| peer.record.id != caller.id)
                    .collect(),
            )
        }
        (Level::Agent, "ask_agent") => {
            lateral(
                supervisor,
                caller,
                "Question",
                text(args, "question")?,
                args,
            )
            .await
        }
        (Level::Agent, "tell_agent") => {
            lateral(
                supervisor,
                caller,
                "Information",
                text(args, "message")?,
                args,
            )
            .await
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
        (Level::Assistant | Level::Agent, "memory_read") => {
            let memory = supervisor.memory();
            memory.ensure(&caller.id).map_err(said)?;
            match optional(args, "file") {
                Some(file) => memory.read(&caller.id, file),
                None => Ok(memory.list(&caller.id).map_err(said)?.join("\n")),
            }
        }
        (Level::Assistant | Level::Agent, "memory_write") => {
            let memory = supervisor.memory();
            memory.ensure(&caller.id).map_err(said)?;
            let file = text(args, "file")?;
            let append = optional(args, "mode") == Some("append");
            memory.write(&caller.id, file, text(args, "content")?, append)?;
            Ok(format!("{file} mis à jour."))
        }
        (Level::Assistant | Level::Agent, "trigger_create") => {
            create_trigger(supervisor, caller, args)
        }
        (Level::Assistant | Level::Agent, "trigger_list") => {
            let triggers = supervisor
                .registry()
                .triggers_of(&caller.id)
                .map_err(said)?;
            if triggers.is_empty() {
                return Ok("Aucun déclencheur.".into());
            }
            Ok(json!(triggers).to_string())
        }
        (Level::Assistant | Level::Agent, "trigger_set") => {
            let id = own_trigger(supervisor, caller, text(args, "trigger")?)?;
            let enabled = match text(args, "status")? {
                "on" => true,
                "off" => false,
                other => return Err(format!("état « {other} » : on ou off")),
            };
            supervisor
                .registry()
                .set_trigger_enabled(&id, enabled)
                .map_err(said)?;
            Ok(format!(
                "Déclencheur {id} {}.",
                if enabled { "actif" } else { "suspendu" }
            ))
        }
        (Level::Assistant | Level::Agent, "trigger_delete") => {
            let id = own_trigger(supervisor, caller, text(args, "trigger")?)?;
            supervisor.registry().delete_trigger(&id).map_err(said)?;
            Ok(format!("Déclencheur {id} supprimé."))
        }
        (Level::Assistant | Level::Agent, "surface_create") => {
            let kind = text(args, "kind")?.parse().map_err(said)?;
            let html = raw(args, "html")?;
            let surface = supervisor
                .registry()
                .add_surface(&caller.id, kind, text(args, "title")?, html)
                .map_err(said)?;
            let id = surface.id.clone();
            supervisor.publish_surface(surface, false);
            Ok(format!(
                "Surface {id} créée ; l'utilisateur la voit dans un onglet."
            ))
        }
        (Level::Assistant | Level::Agent, "surface_update") => {
            let id = own_surface(supervisor, caller, text(args, "surface")?)?;
            let html = args["html"].as_str().filter(|html| !html.trim().is_empty());
            let surface = supervisor
                .registry()
                .update_surface(&id, html, optional(args, "title"))
                .map_err(said)?;
            let version = surface.version;
            supervisor.publish_surface(surface, false);
            Ok(format!("Surface {id} mise à jour (version {version})."))
        }
        (Level::Assistant | Level::Agent, "surface_push") => {
            let id = own_surface(supervisor, caller, text(args, "surface")?)?;
            let data = raw(args, "data")?;
            // Du JSON si possible ; sinon le texte tel quel.
            let data = serde_json::from_str(data).unwrap_or_else(|_| Value::from(data));
            supervisor.push_surface(&id, data);
            Ok(format!("Données poussées vers {id}."))
        }
        (Level::Assistant | Level::Agent, "surface_list") => {
            let surfaces = supervisor.registry().surfaces(&caller.id).map_err(said)?;
            if surfaces.is_empty() {
                return Ok("Aucune surface pour l'instant.".into());
            }
            Ok(json!(surfaces).to_string())
        }
        (Level::Assistant | Level::Agent, "surface_delete") => {
            let id = own_surface(supervisor, caller, text(args, "surface")?)?;
            let surface = supervisor.registry().delete_surface(&id).map_err(said)?;
            supervisor.publish_surface(surface, true);
            Ok(format!("Surface {id} supprimée."))
        }
        (Level::Assistant | Level::Agent, "memory_search") => {
            let found = supervisor
                .memory()
                .search(&caller.id, text(args, "query")?)
                .map_err(said)?;
            if found.is_empty() {
                return Ok("Rien trouvé.".into());
            }
            Ok(found
                .iter()
                .take(50)
                .map(|hit| format!("{}:{} {}", hit.file, hit.line, hit.text))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        _ => Err(format!("outil inconnu pour un {} : {name}", caller.level)),
    }
}

/// Ajoute un fait au journal d'un agent, au nom du daemon (D23). Appelé seulement pour
/// l'Assistant et les Agents ; une erreur d'écriture n'empêche pas l'action elle-même.
fn journal(supervisor: &Supervisor, agent: &AgentId, fact: &str) {
    let mut fact = fact.replace('\n', " ");
    if fact.chars().count() > 300 {
        fact = fact.chars().take(300).collect::<String>() + "…";
    }
    let memory = supervisor.memory();
    let _ = memory.ensure(agent).and_then(|()| memory.log(agent, &fact));
}

fn create_agent(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let name = text(args, "name")?;
    let (profile, choice) = supervisor
        .policy()
        .resolve(optional(args, "profile"), Level::Agent)?;
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
            engine: choice.engine,
            model: Some(choice.model),
            profile: Some(profile),
            workdir,
            charter: Some(Charter {
                name: name.into(),
                scope: text(args, "scope")?.into(),
            }),
        })
        .map_err(said)?;
    journal(
        supervisor,
        &caller.id,
        &format!("{} créé : {}", label(&agent), text(args, "scope")?),
    );
    Ok(format!(
        "Agent « {name} » créé ({}), au repos jusqu'à sa première mission. Répertoire : {}.",
        agent.id,
        agent.workdir.display()
    ))
}

fn create_trigger(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    args: &Value,
) -> Result<String, String> {
    let kind: TriggerKind = text(args, "source")?.parse().map_err(said)?;
    let target = match optional(args, "target") {
        None => caller.id.clone(),
        Some(reference) if caller.level == Level::Assistant => {
            if reference == caller.id.as_str() {
                caller.id.clone()
            } else {
                child(supervisor, caller, reference)?.id
            }
        }
        Some(reference) if reference == caller.id.as_str() => caller.id.clone(),
        Some(_) => return Err("un Agent ne crée de déclencheur que pour lui-même".into()),
    };
    let spec = match kind {
        TriggerKind::Every => {
            let minutes: u64 = text(args, "every_minutes")?
                .parse()
                .map_err(|_| "every_minutes : un nombre entier de minutes".to_string())?;
            (minutes * 60).to_string()
        }
        TriggerKind::Cron => text(args, "cron")?.to_string(),
        TriggerKind::Watch => text(args, "path")?.to_string(),
        TriggerKind::Switch | TriggerKind::Webhook => String::new(),
    };
    let trigger = supervisor
        .registry()
        .add_trigger(NewTrigger {
            owner: caller.id.clone(),
            target,
            name: text(args, "name")?.into(),
            kind,
            spec,
            prompt: text(args, "prompt")?.into(),
        })
        .map_err(said)?;
    journal(
        supervisor,
        &caller.id,
        &format!(
            "déclencheur « {} » ({}, {}) créé",
            trigger.name, trigger.id, trigger.kind
        ),
    );
    let mut done = format!("Déclencheur {} « {} » actif.", trigger.id, trigger.name);
    if let Some(secret) = &trigger.secret {
        let base = supervisor.mcp_url().unwrap_or("http://<daemon>");
        done.push_str(&format!(
            " URL à appeler (POST, le corps accompagne la consigne) : {base}/hooks/{}/{secret}",
            trigger.id
        ));
    }
    Ok(done)
}

/// Un déclencheur dont `caller` est propriétaire.
fn own_trigger(supervisor: &Supervisor, caller: &AgentRecord, id: &str) -> Result<String, String> {
    let trigger = supervisor.registry().trigger(id).map_err(said)?;
    if trigger.owner != caller.id {
        return Err(format!("le déclencheur {id} n'est pas le tien"));
    }
    Ok(trigger.id)
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
    journal(
        supervisor,
        &caller.id,
        &format!(
            "mission {} confiée à {} : {}",
            mission.id,
            label(&target),
            mission.text
        ),
    );
    journal(
        supervisor,
        &target.id,
        &format!("mission {} reçue : {}", mission.id, mission.text),
    );
    send_as(
        supervisor,
        caller,
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
    journal(supervisor, &caller.id, &format!("mission {id} annulée"));
    journal(
        supervisor,
        &mission.agent,
        &format!("mission {id} annulée par l'Assistant"),
    );
    send_as(
        supervisor,
        caller,
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
    let (profile, choice) = supervisor
        .policy()
        .resolve(optional(args, "profile"), Level::Worker)?;
    let worker = supervisor
        .create(NewAgent {
            parent: caller.id.clone(),
            level: Level::Worker,
            engine: choice.engine,
            model: Some(choice.model),
            profile: Some(profile),
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
    send_as(supervisor, caller, &worker.id, brief)
        .await
        .map_err(said)?;
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
            if let (Some(state), Some(parent)) = (state, &caller.parent) {
                journal(
                    supervisor,
                    parent,
                    &format!("tâche {} {state} : {summary}", task.id),
                );
            }
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

/// Remet un message au nom de l'agent appelant : l'expéditeur suit le message.
async fn send_as(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    to: &AgentId,
    text: impl Into<String>,
) -> Result<(), SupervisorError> {
    let from = Origin::Agent {
        agent: caller.id.clone(),
    };
    supervisor.deliver(to, from, text).await
}

/// Dialogue latéral entre Agents (D40) : une question ou une information, jamais un
/// ordre. Le pair est désigné par son nom ; le daemon l'achemine où qu'il tourne.
async fn lateral(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    kind: &str,
    body: &str,
    args: &Value,
) -> Result<String, String> {
    let peer = peer(supervisor, caller, text(args, "agent")?)?;
    if !supervisor.allow_lateral(&caller.id, &peer.id) {
        return Err(format!(
            "trop d'échanges avec {} ces dernières minutes : attends, ou passe par l'Assistant",
            label(&peer)
        ));
    }
    let hint = match kind {
        "Question" => "\n(Réponds-lui avec tell_agent si tu as la réponse.)",
        _ => "",
    };
    send_as(
        supervisor,
        caller,
        &peer.id,
        format!("[{kind} de {}]\n{body}{hint}", label(caller)),
    )
    .await
    .map_err(said)?;
    let what = kind.to_lowercase();
    journal(
        supervisor,
        &caller.id,
        &format!("{what} à {} : {body}", label(&peer)),
    );
    journal(
        supervisor,
        &peer.id,
        &format!("{what} de {} : {body}", label(caller)),
    );
    Ok(format!("Transmis à {}.", label(&peer)))
}

/// Un autre Agent vivant, désigné par son nom ou son id.
fn peer(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    reference: &str,
) -> Result<AgentRecord, String> {
    let found = match supervisor.record(&AgentId::new(reference)) {
        Ok(record) => Some(record),
        Err(_) => supervisor.registry().agent_named(reference).map_err(said)?,
    };
    let peer = found.ok_or_else(|| format!("aucun Agent « {reference} »"))?;
    if peer.level != Level::Agent {
        return Err(format!(
            "« {reference} » n'est pas un Agent : le dialogue latéral se fait entre Agents"
        ));
    }
    if peer.id == caller.id {
        return Err("c'est toi".into());
    }
    if peer.state == AgentState::Retired {
        return Err(format!("{} est retiré", label(&peer)));
    }
    Ok(peer)
}

/// Remonte un compte rendu ou une question au parent, comme un nouveau message.
async fn deliver(
    supervisor: &Supervisor,
    caller: &AgentRecord,
    kind: &str,
    body: &str,
) -> Result<String, String> {
    let parent = caller.parent.as_ref().ok_or("tu n'as pas de parent")?;
    send_as(
        supervisor,
        caller,
        parent,
        format!("[{kind} de {}]\n{body}", label(caller)),
    )
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

/// Une surface de `caller`.
fn own_surface(supervisor: &Supervisor, caller: &AgentRecord, id: &str) -> Result<String, String> {
    let surface = supervisor.registry().surface_meta(id).map_err(said)?;
    if surface.agent != caller.id {
        return Err(format!("la surface {id} n'est pas la tienne"));
    }
    Ok(surface.id)
}

/// Une tâche du board de `caller`.
fn own_task(supervisor: &Supervisor, caller: &AgentRecord, id: &str) -> Result<String, String> {
    let task = supervisor.registry().task(id).map_err(said)?;
    if task.agent != caller.id {
        return Err(format!("la tâche {id} n'est pas sur ton board"));
    }
    Ok(task.id)
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

/// Argument texte non vide, sans le rogner (code HTML, JSON).
fn raw<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("argument « {key} » manquant"))
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
        let names = |level| -> Vec<&str> {
            tools_for(level, &ModelPolicy::default())
                .iter()
                .map(|t| t.name)
                .collect()
        };
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
            tools_for(level, &ModelPolicy::default())
                .into_iter()
                .find(|t| t.name == name)
                .unwrap()
        };
        let create = find(Level::Assistant, "create_agent");
        assert_eq!(create.schema["required"], json!(["name", "scope"]));
        assert_eq!(
            create.schema["properties"]["profile"]["enum"],
            json!(["code", "léger", "standard"])
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
