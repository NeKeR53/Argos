//! Adaptateur Claude Code : `claude -p` en stream-json, un process par session,
//! plusieurs tours sur stdin, interruption par `control_request`.

use super::process::{self, Spawned};
use super::{
    Command, EngineError, EngineEvent, Events, Session, SessionSpec, TurnOutcome, Usage, forward,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

pub(super) async fn spawn(spec: SessionSpec) -> Result<Session, EngineError> {
    // Argos choisit l'identifiant : la session est connue avant même le premier tour.
    let session_id = spec
        .resume
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut command = tokio::process::Command::new(spec.program());
    command
        .args(arguments(&spec, &session_id))
        .current_dir(&spec.cwd);
    let process = process::spawn(command)?;
    let pgid = process.pgid;
    Ok(Session::launch(
        session_id,
        pgid,
        move |commands, events| drive(process, commands, events),
    ))
}

fn arguments(spec: &SessionSpec, session_id: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        // Isolation : ni les réglages, plugins, CLAUDE.md ni MCP globaux de l'utilisateur.
        // `--safe-mode` isolerait mieux mais désactive aussi les MCP passés par Argos.
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        // La mémoire est celle d'Argos (D23) : pas de mémoire automatique concurrente.
        "--settings",
        r#"{"autoMemoryEnabled": false}"#,
        // Provisoire, en attendant les garde-fous (D34).
        "--permission-mode",
        "acceptEdits",
    ]
    .map(String::from)
    .into();
    match &spec.resume {
        Some(id) => args.extend(["--resume".into(), id.clone()]),
        None => args.extend(["--session-id".into(), session_id.into()]),
    }
    if let Some(model) = &spec.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(instructions) = &spec.instructions {
        args.extend(["--append-system-prompt".into(), instructions.clone()]);
    }
    if !spec.mcp_servers.is_empty() {
        let servers: serde_json::Map<String, Value> = spec
            .mcp_servers
            .iter()
            .map(|server| {
                (
                    server.name.clone(),
                    json!({"type": "http", "url": server.url}),
                )
            })
            .collect();
        args.extend([
            "--mcp-config".into(),
            json!({"mcpServers": servers}).to_string(),
        ]);
        // En mode -p, un outil non autorisé est refusé : ceux d'Argos le sont d'office.
        for server in &spec.mcp_servers {
            args.extend(["--allowedTools".into(), format!("mcp__{}", server.name)]);
        }
    }
    args
}

async fn drive(
    mut process: Spawned,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: Events,
) -> EngineEvent {
    let mut state = State::default();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let message = match command {
                    Some(Command::Send(text)) => user_message(&text),
                    Some(Command::Interrupt) => state.interrupt_request(),
                    Some(Command::Park) | None => break,
                };
                // Une écriture ratée signifie que le process est mort : la fin de stdout suit.
                let _ = process::write_json(&mut process.stdin, &message).await;
            }
            line = process.stdout.next_line() => match line {
                Ok(Some(line)) => forward(&events, state.handle_line(&line)),
                _ => break,
            },
        }
    }
    // Plus aucune commande n'est lue : les refuser plutôt que les perdre en silence.
    commands.close();
    process
        .finish(|line| forward(&events, state.handle_line(line)))
        .await
}

fn user_message(text: &str) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": text}})
}

#[derive(Default)]
struct State {
    interrupting: bool,
    requests: u64,
}

impl State {
    fn interrupt_request(&mut self) -> Value {
        self.interrupting = true;
        self.requests += 1;
        json!({
            "type": "control_request",
            "request_id": format!("argos-{}", self.requests),
            "request": {"subtype": "interrupt"},
        })
    }

    fn handle_line(&mut self, line: &str) -> Vec<EngineEvent> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        match message["type"].as_str() {
            Some("assistant") => assistant_events(&message),
            Some("result") => vec![self.turn_completed(&message)],
            _ => vec![],
        }
    }

    fn turn_completed(&mut self, result: &Value) -> EngineEvent {
        let outcome = if result["subtype"] == "success" {
            TurnOutcome::Completed
        } else if self.interrupting {
            TurnOutcome::Interrupted
        } else {
            let message = result["result"]
                .as_str()
                .or(result["subtype"].as_str())
                .unwrap_or("le tour a échoué");
            TurnOutcome::Failed {
                message: message.into(),
            }
        };
        self.interrupting = false;
        let usage = &result["usage"];
        let tokens = |key: &str| usage[key].as_u64().unwrap_or(0);
        EngineEvent::TurnCompleted {
            outcome,
            usage: Some(Usage {
                input_tokens: tokens("input_tokens")
                    + tokens("cache_creation_input_tokens")
                    + tokens("cache_read_input_tokens"),
                output_tokens: tokens("output_tokens"),
                cost_usd: result["total_cost_usd"].as_f64(),
                // Un seul modèle par session : sa fenêtre est celle de la session.
                context_window: result["modelUsage"]
                    .as_object()
                    .and_then(|models| models.values().find_map(|m| m["contextWindow"].as_u64())),
            }),
        }
    }
}

fn assistant_events(message: &Value) -> Vec<EngineEvent> {
    let Some(blocks) = message["message"]["content"].as_array() else {
        return vec![];
    };
    blocks
        .iter()
        .filter_map(|block| match block["type"].as_str()? {
            "text" => block["text"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| EngineEvent::AgentMessage { text: text.into() }),
            "tool_use" => block["name"]
                .as_str()
                .map(|name| EngineEvent::ToolActivity { name: name.into() }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use argos_core::EngineKind;

    // Lignes de la forme observée avec claude 2.1.310 (docs/PHASE-1.md).
    const ASSISTANT_TEXT: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"42"}],"model":"claude-haiku-5-5","stop_reason":null},"parent_tool_use_id":null,"session_id":"a6ade44e"}"#;
    const ASSISTANT_THINKING: &str = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"","signature":"CAQS"}]},"session_id":"a6ade44e"}"#;
    const RESULT_SUCCESS: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"42","session_id":"a6ade44e","total_cost_usd":0.0076878,"usage":{"input_tokens":2,"cache_creation_input_tokens":38428,"cache_read_input_tokens":0,"output_tokens":4},"modelUsage":{"claude-haiku-5-5":{"inputTokens":2,"outputTokens":4,"contextWindow":1000000,"maxOutputTokens":128000}}}"#;
    const RESULT_INTERRUPTED: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"a6ade44e","usage":{"input_tokens":1,"output_tokens":0}}"#;

    #[test]
    fn assistant_text_becomes_an_agent_message() {
        let events = State::default().handle_line(ASSISTANT_TEXT);
        assert_eq!(
            events,
            vec![EngineEvent::AgentMessage { text: "42".into() }]
        );
    }

    #[test]
    fn thinking_blocks_and_unknown_lines_are_ignored() {
        let mut state = State::default();
        assert!(state.handle_line(ASSISTANT_THINKING).is_empty());
        assert!(
            state
                .handle_line(r#"{"type":"system","subtype":"init"}"#)
                .is_empty()
        );
        assert!(state.handle_line("pas du json").is_empty());
    }

    #[test]
    fn result_reports_usage_including_cache() {
        let events = State::default().handle_line(RESULT_SUCCESS);
        assert_eq!(
            events,
            vec![EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Completed,
                usage: Some(Usage {
                    input_tokens: 38430,
                    output_tokens: 4,
                    cost_usd: Some(0.0076878),
                    context_window: Some(1_000_000),
                }),
            }]
        );
    }

    #[test]
    fn an_error_result_is_an_interruption_only_after_an_interrupt_request() {
        let mut state = State::default();
        let failed = state.handle_line(RESULT_INTERRUPTED);
        assert!(matches!(
            &failed[0],
            EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Failed { .. },
                ..
            }
        ));

        state.interrupt_request();
        let interrupted = state.handle_line(RESULT_INTERRUPTED);
        assert!(matches!(
            &interrupted[0],
            EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
                ..
            }
        ));
        // Le drapeau ne survit pas au tour interrompu.
        let next = state.handle_line(RESULT_INTERRUPTED);
        assert!(matches!(
            &next[0],
            EngineEvent::TurnCompleted {
                outcome: TurnOutcome::Failed { .. },
                ..
            }
        ));
    }

    #[test]
    fn argos_mcp_servers_are_passed_and_allowed() {
        let mut spec = SessionSpec::new(EngineKind::Claude, "/tmp");
        spec.mcp_servers = vec![super::super::McpServer {
            name: "argos".into(),
            url: "http://127.0.0.1:7879/mcp/jeton".into(),
        }];
        let args = arguments(&spec, "id");
        let config = args
            .windows(2)
            .find(|w| w[0] == "--mcp-config")
            .map(|w| serde_json::from_str::<Value>(&w[1]).unwrap())
            .expect("--mcp-config");
        assert_eq!(
            config,
            json!({"mcpServers": {"argos": {"type": "http", "url": "http://127.0.0.1:7879/mcp/jeton"}}})
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["--allowedTools", "mcp__argos"])
        );
    }

    #[test]
    fn a_new_session_gets_its_id_and_a_resumed_one_keeps_it() {
        let mut spec = SessionSpec::new(EngineKind::Claude, "/tmp");
        let args = arguments(&spec, "nouvel-id");
        assert!(args.windows(2).any(|w| w == ["--session-id", "nouvel-id"]));

        spec.resume = Some("ancien-id".into());
        let args = arguments(&spec, "ancien-id");
        assert!(args.windows(2).any(|w| w == ["--resume", "ancien-id"]));
        assert!(!args.contains(&"--session-id".to_string()));
    }
}
