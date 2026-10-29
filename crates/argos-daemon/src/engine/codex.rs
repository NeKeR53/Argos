//! Adaptateur Codex : `codex app-server` (JSON-RPC sur stdio), un process par
//! session, un thread par agent, interruption par `turn/interrupt`.

use super::process::{self, Spawned};
use super::{
    Access, Command, EngineError, EngineEvent, Events, McpServer, Session, SessionSpec,
    TurnOutcome, Usage, forward,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
use std::{fs, io};
use tokio::sync::mpsc;

/// Délai maximal de chaque réponse pendant l'initialisation.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// Prépare un `CODEX_HOME` isolé : il partage seulement l'authentification de
/// l'utilisateur, sans sa config, ses MCP ni ses consignes globales. Idempotent.
pub fn prepare_codex_home(dir: &Path, user_codex_home: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let auth = dir.join("auth.json");
    if fs::symlink_metadata(&auth).is_err() {
        std::os::unix::fs::symlink(user_codex_home.join("auth.json"), &auth)?;
    }
    let config = dir.join("config.toml");
    if !config.exists() {
        fs::write(&config, "")?;
    }
    Ok(())
}

/// Réécrit la config du `CODEX_HOME` isolé : seulement les serveurs MCP de la session
/// (leur URL porte un jeton qui change à chaque session).
fn write_config(
    home: &Path,
    servers: &[McpServer],
    external: &[crate::capabilities::ExternalMcp],
) -> io::Result<()> {
    let mut table = toml::Table::new();
    if !servers.is_empty() || !external.is_empty() {
        let mut servers: toml::Table = servers
            .iter()
            .map(|server| {
                let mut entry = toml::Table::new();
                entry.insert("url".into(), server.url.clone().into());
                // Sinon, avec `approvalPolicy: never`, chaque appel serait refusé faute
                // d'approbation : les outils donnés par Argos sont approuvés d'office.
                entry.insert("default_tools_approval_mode".into(), "approve".into());
                (server.name.clone(), entry.into())
            })
            .collect();
        for server in external {
            use crate::capabilities::Transport;
            let mut entry = toml::Table::new();
            match &server.transport {
                Transport::Http { url } => {
                    entry.insert("url".into(), url.clone().into());
                }
                Transport::Stdio { command, args } => {
                    entry.insert("command".into(), command.clone().into());
                    entry.insert(
                        "args".into(),
                        toml::Value::Array(args.iter().cloned().map(Into::into).collect()),
                    );
                }
            }
            entry.insert("default_tools_approval_mode".into(), "approve".into());
            servers.insert(server.name.clone(), entry.into());
        }
        table.insert("mcp_servers".into(), servers.into());
    }
    fs::create_dir_all(home)?;
    fs::write(home.join("config.toml"), table.to_string())
}

pub(super) async fn spawn(spec: SessionSpec) -> Result<Session, EngineError> {
    let home = spec
        .codex_home
        .as_ref()
        .ok_or_else(|| EngineError::Config("une session Codex exige un CODEX_HOME isolé".into()))?;
    write_config(home, &spec.mcp_servers, &spec.capabilities.mcp)?;
    let mut command = tokio::process::Command::new(spec.program());
    command
        .arg("app-server")
        .env("CODEX_HOME", home)
        .current_dir(&spec.cwd);
    let mut process = process::spawn(command)?;
    let mut state = State::default();

    let client = json!({"name": "argos", "title": null, "version": env!("CARGO_PKG_VERSION")});
    call(
        &mut process,
        &mut state,
        "initialize",
        json!({"clientInfo": client, "capabilities": null}),
    )
    .await?;
    process::write_json(&mut process.stdin, &json!({"method": "initialized"})).await?;

    let mut params = json!({
        "cwd": spec.cwd.to_string_lossy(),
        "model": spec.model,
        "developerInstructions": spec.instructions,
        // Jamais de demande d'approbation : personne n'est là pour répondre.
        "approvalPolicy": "never",
        "sandbox": match spec.access {
            Access::ReadOnly => "read-only",
            Access::Workspace => "workspace-write",
            Access::Full => "danger-full-access",
        },
    });
    let method = match &spec.resume {
        Some(thread_id) => {
            params["threadId"] = json!(thread_id);
            params["excludeTurns"] = json!(true);
            "thread/resume"
        }
        None => "thread/start",
    };
    let result = call(&mut process, &mut state, method, params).await?;
    state.thread_id = result["thread"]["id"]
        .as_str()
        .ok_or_else(|| EngineError::Protocol(format!("{method} sans thread.id")))?
        .to_string();

    let pgid = process.pgid;
    let session_id = state.thread_id.clone();
    Ok(Session::launch(
        session_id,
        Some(pgid),
        move |commands, events| drive(process, state, commands, events),
    ))
}

/// Envoie une requête pendant l'initialisation et attend sa réponse.
async fn call(
    process: &mut Spawned,
    state: &mut State,
    method: &str,
    params: Value,
) -> Result<Value, EngineError> {
    let (id, request) = state.request(method, params);
    process::write_json(&mut process.stdin, &request).await?;
    loop {
        let line = process.read_line_within(HANDSHAKE_TIMEOUT).await?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("method").is_none() && message["id"] == json!(id) {
            return match message.get("error") {
                Some(error) => Err(EngineError::Protocol(format!(
                    "{method} : {}",
                    error["message"].as_str().unwrap_or("refusé")
                ))),
                None => Ok(message["result"].clone()),
            };
        }
        if let (_, Some(reply)) = state.handle_line(&line) {
            process::write_json(&mut process.stdin, &reply).await?;
        }
    }
}

async fn drive(
    mut process: Spawned,
    mut state: State,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: Events,
) -> EngineEvent {
    loop {
        tokio::select! {
            command = commands.recv() => {
                let message = match command {
                    Some(Command::Send(text)) => Some(state.turn_start(&text)),
                    Some(Command::Interrupt) => state.interrupt(),
                    Some(Command::Park | Command::Kill) | None => break,
                };
                if let Some(message) = message {
                    // Une écriture ratée signifie que le process est mort : la fin de stdout suit.
                    let _ = process::write_json(&mut process.stdin, &message).await;
                }
            }
            line = process.stdout.next_line() => match line {
                Ok(Some(line)) => {
                    let (batch, reply) = state.handle_line(&line);
                    forward(&events, batch);
                    if let Some(reply) = reply {
                        let _ = process::write_json(&mut process.stdin, &reply).await;
                    }
                }
                _ => break,
            },
        }
    }
    // Plus aucune commande n'est lue : les refuser plutôt que les perdre en silence.
    commands.close();
    process
        .finish(|line| forward(&events, state.handle_line(line).0))
        .await
}

#[derive(Default)]
struct State {
    thread_id: String,
    next_id: u64,
    /// Requêtes `turn/start` sans réponse : leur réponse donne l'id du tour.
    turn_starts: HashSet<u64>,
    current_turn: Option<String>,
    /// Interruption demandée avant que le serveur ait donné l'id du tour.
    interrupt_wanted: bool,
    last_usage: Option<Usage>,
}

impl State {
    fn request(&mut self, method: &str, params: Value) -> (u64, Value) {
        self.next_id += 1;
        let id = self.next_id;
        (id, json!({"id": id, "method": method, "params": params}))
    }

    fn turn_start(&mut self, text: &str) -> Value {
        let input = json!([{"type": "text", "text": text, "text_elements": []}]);
        let params = json!({"threadId": self.thread_id, "input": input});
        let (id, request) = self.request("turn/start", params);
        self.turn_starts.insert(id);
        request
    }

    /// Requête d'interruption du tour en cours. Si le tour vient d'être demandé et que
    /// son id n'est pas encore connu, l'interruption part dès qu'il l'est.
    fn interrupt(&mut self) -> Option<Value> {
        let Some(turn_id) = self.current_turn.clone() else {
            self.interrupt_wanted = !self.turn_starts.is_empty();
            return None;
        };
        let params = json!({"threadId": self.thread_id, "turnId": turn_id});
        Some(self.request("turn/interrupt", params).1)
    }

    fn deferred_interrupt(&mut self) -> Option<Value> {
        if self.interrupt_wanted && self.current_turn.is_some() {
            self.interrupt_wanted = false;
            return self.interrupt();
        }
        None
    }

    /// Traite un message du serveur : événements produits et éventuel message à écrire.
    fn handle_line(&mut self, line: &str) -> (Vec<EngineEvent>, Option<Value>) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return (vec![], None);
        };
        match (message["method"].as_str(), message.get("id")) {
            // Requête du serveur (approbation, saisie…) : Argos n'en traite aucune et le
            // dit, plutôt que de laisser le tour bloqué en attente.
            (Some(method), Some(id)) => {
                let error = json!({"code": -32601, "message": format!("{method} non pris en charge par Argos")});
                (vec![], Some(json!({"id": id, "error": error})))
            }
            (Some(method), None) => {
                let events = self.notification(method, &message["params"]);
                (events, self.deferred_interrupt())
            }
            (None, Some(id)) => {
                let events = self.response(id, &message);
                (events, self.deferred_interrupt())
            }
            (None, None) => (vec![], None),
        }
    }

    fn notification(&mut self, method: &str, params: &Value) -> Vec<EngineEvent> {
        match method {
            "turn/started" => {
                self.current_turn = params["turn"]["id"].as_str().map(String::from);
                vec![]
            }
            "item/started" => tool_detail(&params["item"], false).into_iter().collect(),
            "item/completed" if tool_name(&params["item"]).is_some() => {
                tool_detail(&params["item"], true).into_iter().collect()
            }
            "item/agentMessage/delta" => params["delta"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| vec![EngineEvent::AgentDelta { text: text.into() }])
                .unwrap_or_default(),
            "item/completed" if params["item"]["type"] == "agentMessage" => {
                let text = params["item"]["text"].as_str().unwrap_or_default();
                vec![EngineEvent::AgentMessage { text: text.into() }]
            }
            "thread/tokenUsage/updated" => {
                let last = &params["tokenUsage"]["last"];
                self.last_usage = Some(Usage {
                    input_tokens: last["inputTokens"].as_u64().unwrap_or(0),
                    output_tokens: last["outputTokens"].as_u64().unwrap_or(0),
                    cost_usd: None,
                    context_window: params["tokenUsage"]["modelContextWindow"].as_u64(),
                });
                vec![]
            }
            "turn/completed" => {
                self.current_turn = None;
                self.interrupt_wanted = false;
                let turn = &params["turn"];
                let outcome = match turn["status"].as_str() {
                    Some("completed") => TurnOutcome::Completed,
                    Some("interrupted") => TurnOutcome::Interrupted,
                    _ => TurnOutcome::Failed {
                        message: turn["error"]["message"]
                            .as_str()
                            .unwrap_or("le tour a échoué")
                            .into(),
                    },
                };
                let mut events = vec![];
                let code = turn["error"]["codexErrorInfo"].as_str();
                if let Some(
                    code @ ("usageLimitExceeded" | "rateLimitExceeded" | "sessionBudgetExceeded"),
                ) = code
                {
                    events.push(EngineEvent::RateLimited {
                        message: format!("quota Codex épuisé ({code})"),
                    });
                }
                events.push(EngineEvent::TurnCompleted {
                    outcome,
                    usage: self.last_usage.take(),
                });
                events
            }
            _ => vec![],
        }
    }

    fn response(&mut self, id: &Value, message: &Value) -> Vec<EngineEvent> {
        let Some(id) = id.as_u64() else {
            return vec![];
        };
        if !self.turn_starts.remove(&id) {
            return vec![];
        }
        if let Some(error) = message.get("error") {
            let message = error["message"].as_str().unwrap_or("turn/start refusé");
            return vec![EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Failed {
                    message: message.into(),
                },
                usage: None,
            }];
        }
        if self.current_turn.is_none() {
            self.current_turn = message["result"]["turn"]["id"].as_str().map(String::from);
        }
        vec![]
    }
}

fn tool_detail(item: &Value, finished: bool) -> Option<EngineEvent> {
    let name = tool_name(item)?;
    let id = item["id"].as_str()?.to_owned();
    let input = item
        .get("command")
        .or_else(|| item.get("arguments"))
        .or_else(|| item.get("query"))
        .or_else(|| item.get("changes"))
        .filter(|v| !v.is_null())
        .map(tool_text);
    let output = item
        .get("aggregatedOutput")
        .or_else(|| item.get("result"))
        .or_else(|| item.get("error"))
        .filter(|v| !v.is_null())
        .map(tool_text);
    Some(EngineEvent::ToolDetail {
        id,
        name,
        input,
        output,
        finished,
    })
}

pub(super) fn tool_text(value: &Value) -> String {
    let text = value
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default());
    const MAX: usize = 32_768;
    if text.len() <= MAX {
        return text;
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (sortie tronquée)", &text[..end])
}

fn tool_name(item: &Value) -> Option<String> {
    match item["type"].as_str()? {
        "commandExecution" => Some("shell".into()),
        "fileChange" => Some("edit".into()),
        "webSearch" => Some("web_search".into()),
        "mcpToolCall" => Some(format!(
            "{}/{}",
            item["server"].as_str().unwrap_or("?"),
            item["tool"].as_str().unwrap_or("?")
        )),
        "dynamicToolCall" => item["tool"].as_str().map(String::from),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_detail_tracks_command_and_output() {
        let item = serde_json::json!({"id":"c1","type":"commandExecution","command":"pwd","aggregatedOutput":"/tmp"});
        assert_eq!(
            tool_detail(&item, true),
            Some(EngineEvent::ToolDetail {
                id: "c1".into(),
                name: "shell".into(),
                input: Some("pwd".into()),
                output: Some("/tmp".into()),
                finished: true
            })
        );
    }

    // Messages de la forme observée avec codex-cli 0.164.0 (docs/PHASE-1.md).
    const TURN_STARTED: &str = r#"{"method":"turn/started","params":{"threadId":"t1","turn":{"id":"turn-1","status":"inProgress"}}}"#;
    const AGENT_MESSAGE: &str = r#"{"method":"item/completed","params":{"threadId":"t1","turnId":"turn-1","completedAtMs":1,"item":{"type":"agentMessage","id":"m1","text":"42","phase":null}}}"#;
    const TOKEN_USAGE: &str = r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"t1","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":50,"outputTokens":3},"last":{"totalTokens":25,"inputTokens":20,"cachedInputTokens":7,"cacheWriteInputTokens":0,"outputTokens":5,"reasoningOutputTokens":0},"modelContextWindow":272000}}}"#;
    const TURN_COMPLETED: &str = r#"{"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"turn-1","status":"completed","error":null}}}"#;

    #[test]
    fn a_turn_produces_its_message_then_completion_with_usage() {
        let mut state = State::default();
        assert_eq!(state.handle_line(TURN_STARTED).0, vec![]);
        assert_eq!(state.current_turn.as_deref(), Some("turn-1"));
        assert_eq!(
            state.handle_line(AGENT_MESSAGE).0,
            vec![EngineEvent::AgentMessage { text: "42".into() }]
        );
        assert_eq!(state.handle_line(TOKEN_USAGE).0, vec![]);
        assert_eq!(
            state.handle_line(TURN_COMPLETED).0,
            vec![EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Completed,
                usage: Some(Usage {
                    input_tokens: 20,
                    output_tokens: 5,
                    cost_usd: None,
                    context_window: Some(272_000),
                }),
            }]
        );
        assert_eq!(state.current_turn, None);
    }

    #[test]
    fn failed_and_interrupted_turns_are_told_apart() {
        let mut state = State::default();
        let failed = r#"{"method":"turn/completed","params":{"turn":{"id":"x","status":"failed","error":{"message":"quota dépassé"}}}}"#;
        assert_eq!(
            state.handle_line(failed).0,
            vec![EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Failed {
                    message: "quota dépassé".into()
                },
                usage: None,
            }]
        );
        let interrupted =
            r#"{"method":"turn/completed","params":{"turn":{"id":"x","status":"interrupted"}}}"#;
        assert!(matches!(
            &state.handle_line(interrupted).0[0],
            EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
                ..
            }
        ));
    }

    #[test]
    fn message_deltas_become_agent_deltas() {
        let mut state = State::default();
        let delta = r#"{"method":"item/agentMessage/delta","params":{"threadId":"t1","turnId":"x","itemId":"m","delta":"bon"}}"#;
        assert_eq!(
            state.handle_line(delta).0,
            vec![EngineEvent::AgentDelta { text: "bon".into() }]
        );
    }

    #[test]
    fn an_exhausted_quota_is_reported_before_the_failed_turn() {
        let mut state = State::default();
        let limited = r#"{"method":"turn/completed","params":{"turn":{"id":"x","status":"failed","error":{"message":"limite atteinte","codexErrorInfo":"usageLimitExceeded"}}}}"#;
        let events = state.handle_line(limited).0;
        assert!(matches!(&events[0], EngineEvent::RateLimited { .. }));
        assert!(matches!(
            &events[1],
            EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Failed { .. },
                ..
            }
        ));
    }

    #[test]
    fn server_requests_are_answered_with_an_error() {
        let mut state = State::default();
        let (events, reply) =
            state.handle_line(r#"{"id":"srv-1","method":"execCommandApproval","params":{}}"#);
        assert!(events.is_empty());
        let reply = reply.expect("une réponse");
        assert_eq!(reply["id"], "srv-1");
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("execCommandApproval")
        );
    }

    fn turn_start_response(id: u64) -> String {
        format!(r#"{{"id":{id},"result":{{"turn":{{"id":"turn-9","status":"inProgress"}}}}}}"#)
    }

    #[test]
    fn the_turn_start_response_gives_the_turn_to_interrupt() {
        let mut state = State {
            thread_id: "t1".into(),
            ..State::default()
        };
        assert_eq!(state.interrupt(), None);
        let id = state.turn_start("bonjour")["id"].as_u64().unwrap();
        state.handle_line(&turn_start_response(id));
        let interrupt = state.interrupt().expect("un tour à interrompre");
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["params"]["threadId"], "t1");
        assert_eq!(interrupt["params"]["turnId"], "turn-9");
    }

    #[test]
    fn an_interrupt_sent_before_the_turn_is_known_leaves_once_it_is() {
        let mut state = State::default();
        let id = state.turn_start("bonjour")["id"].as_u64().unwrap();
        assert_eq!(state.interrupt(), None);

        let (_, deferred) = state.handle_line(&turn_start_response(id));
        let deferred = deferred.expect("l'interruption différée");
        assert_eq!(deferred["method"], "turn/interrupt");
        assert_eq!(deferred["params"]["turnId"], "turn-9");
        // Une seule fois.
        assert_eq!(state.handle_line(TURN_STARTED).1, None);
    }

    #[test]
    fn without_a_requested_turn_an_interrupt_is_not_kept_for_later() {
        let mut state = State::default();
        assert_eq!(state.interrupt(), None);
        assert_eq!(state.handle_line(TURN_STARTED).1, None);
    }

    #[test]
    fn a_refused_turn_start_fails_the_turn() {
        let mut state = State::default();
        let id = state.turn_start("bonjour")["id"].as_u64().unwrap();
        let events = state
            .handle_line(&format!(
                r#"{{"id":{id},"error":{{"message":"thread inconnu"}}}}"#
            ))
            .0;
        assert_eq!(
            events,
            vec![EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Failed {
                    message: "thread inconnu".into()
                },
                usage: None,
            }]
        );
    }

    #[test]
    fn tool_items_are_named() {
        let item = |json: &str| tool_name(&serde_json::from_str(json).unwrap());
        assert_eq!(
            item(r#"{"type":"commandExecution","command":"ls"}"#).as_deref(),
            Some("shell")
        );
        assert_eq!(
            item(r#"{"type":"mcpToolCall","server":"argos","tool":"spawn_worker"}"#).as_deref(),
            Some("argos/spawn_worker")
        );
        assert_eq!(item(r#"{"type":"reasoning"}"#), None);
    }

    #[test]
    fn the_config_holds_only_the_session_mcp_servers() {
        let home = tempfile::tempdir().unwrap();
        let server = McpServer {
            name: "argos".into(),
            url: "http://127.0.0.1:7879/mcp/jeton".into(),
        };
        write_config(home.path(), &[server], &[]).unwrap();
        let config: toml::Table = fs::read_to_string(home.path().join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            config["mcp_servers"]["argos"]["url"].as_str(),
            Some("http://127.0.0.1:7879/mcp/jeton")
        );
        assert_eq!(
            config["mcp_servers"]["argos"]["default_tools_approval_mode"].as_str(),
            Some("approve")
        );

        write_config(home.path(), &[], &[]).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join("config.toml")).unwrap(),
            ""
        );
    }

    #[test]
    fn prepare_codex_home_links_auth_only_and_is_idempotent() {
        let user = tempfile::tempdir().unwrap();
        fs::write(user.path().join("auth.json"), "{}").unwrap();
        fs::write(user.path().join("config.toml"), "model = \"x\"").unwrap();
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("codex");

        prepare_codex_home(&dir, user.path()).unwrap();
        prepare_codex_home(&dir, user.path()).unwrap();

        assert_eq!(
            fs::read_link(dir.join("auth.json")).unwrap(),
            user.path().join("auth.json")
        );
        assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), "");
    }
}
