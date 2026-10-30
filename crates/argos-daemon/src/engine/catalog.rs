//! Découverte des modèles auprès des harnais, sans lancer de tour.
use super::{EngineError, process};
use argos_core::{EngineKind, protocol::ModelRef};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, time::Duration};

pub async fn discover(programs: &HashMap<EngineKind, PathBuf>) -> Vec<ModelRef> {
    let (claude, codex) = tokio::join!(
        list(EngineKind::Claude, programs),
        list(EngineKind::Codex, programs)
    );
    let mut seen = std::collections::HashSet::new();
    [claude, codex]
        .into_iter()
        .flat_map(|result| match result {
            Ok(models) => models,
            Err(error) => {
                eprintln!("Catalogue de modèles indisponible : {error}");
                vec![]
            }
        })
        // Un même modèle peut être proposé sous plusieurs alias : une seule ligne.
        .filter(|model| seen.insert((model.engine, model.model.clone())))
        .collect()
}

async fn list(
    engine: EngineKind,
    programs: &HashMap<EngineKind, PathBuf>,
) -> Result<Vec<ModelRef>, EngineError> {
    let mut command = tokio::process::Command::new(
        programs
            .get(&engine)
            .cloned()
            .unwrap_or_else(|| engine.binary().into()),
    );
    match engine {
        EngineKind::Claude => {
            command.args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--setting-sources",
                "project",
                "--strict-mcp-config",
                "--settings",
                r#"{"autoMemoryEnabled":false}"#,
            ]);
        }
        EngineKind::Codex => {
            command.arg("app-server");
        }
    }
    let mut p = process::spawn(command)?;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        match engine {
            EngineKind::Claude => {
                process::write_json(&mut p.stdin, &json!({"type":"control_request","request_id":"models","request":{"subtype":"initialize"}})).await?;
                loop {
                    let line = p.read_line_within(Duration::from_secs(15)).await?;
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v["type"] == "control_response" && v["response"]["request_id"] == "models" {
                        return Ok(parse_models(engine, &v["response"]["response"]["models"]));
                    }
                }
            }
            EngineKind::Codex => {
                process::write_json(&mut p.stdin, &json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"argos","version":env!("CARGO_PKG_VERSION")},"capabilities":null}})).await?;
                loop { let line = p.read_line_within(Duration::from_secs(15)).await?; if serde_json::from_str::<Value>(&line).is_ok_and(|v| v["id"] == 1) { break; } }
                process::write_json(&mut p.stdin, &json!({"method":"initialized"})).await?;
                let mut cursor: Option<String> = None;
                let mut all = Vec::new();
                loop {
                    process::write_json(&mut p.stdin, &json!({"id":2,"method":"model/list","params":{"cursor":cursor}})).await?;
                    loop {
                        let line = p.read_line_within(Duration::from_secs(15)).await?;
                        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                        if v["id"] == 2 {
                            if !v["error"].is_null() { return Err(EngineError::Protocol(v["error"].to_string())); }
                            all.extend(parse_models(engine, &v["result"]["data"]));
                            cursor = v["result"]["nextCursor"].as_str().map(str::to_owned);
                            break;
                        }
                    }
                    if cursor.is_none() { return Ok(all); }
                }
            }
        }
    }).await.unwrap_or_else(|_| Err(EngineError::Protocol("délai de découverte dépassé".into())));
    p.tree.kill();
    let _ = p.child.wait().await;
    result
}

fn parse_models(engine: EngineKind, value: &Value) -> Vec<ModelRef> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let model = match engine {
                EngineKind::Claude => item["resolvedModel"]
                    .as_str()
                    .or_else(|| item["value"].as_str()),
                EngineKind::Codex => item["model"].as_str(),
            }?;
            // Les modèles cachés du harnais ne sont pas proposés.
            let hidden = item["hidden"].as_bool().unwrap_or(false);
            (!model.is_empty() && !hidden).then(|| ModelRef {
                engine,
                model: model.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_third_party_model_ids() {
        let items = json!([{"value":"custom","resolvedModel":"vendor/model-v2"}]);
        assert_eq!(
            parse_models(EngineKind::Claude, &items)[0].model,
            "vendor/model-v2"
        );
        assert_eq!(
            parse_models(EngineKind::Codex, &json!([{"model":"third-party/model"}]))[0].model,
            "third-party/model"
        );
    }
}
