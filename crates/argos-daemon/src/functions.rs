//! Exécution HTTP bornée des fonctions publiées : aucun modèle, aucune commande système.

use argos_core::{FunctionRequest, PublishedFunction};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::time::Duration;

pub const MAX_BYTES: usize = 64 * 1024;

pub fn validate_definition(function: &PublishedFunction) -> Result<(), String> {
    if !identifier(&function.name) || function.name.len() > 64 {
        return Err("nom de fonction invalide (snake_case, 64 caractères au plus)".into());
    }
    if function.description.trim().is_empty() || function.description.len() > 2_000 {
        return Err("description requise (2 000 octets au plus)".into());
    }
    if serde_json::to_vec(function)
        .map_err(|_| "définition illisible")?
        .len()
        > MAX_BYTES
    {
        return Err("définition trop grosse (64 Kio au plus)".into());
    }
    validate_schema(&function.parameters)?;
    validate_request(&function.request)?;
    let properties = function.parameters["properties"].as_object().unwrap();
    validate_template(&function.request.url, properties, false)?;
    for value in function.request.headers.values() {
        validate_template(value, properties, true)?;
    }
    if let Some(body) = &function.request.body {
        validate_body(body, properties)?;
    }
    Ok(())
}

/// Sous-ensemble volontairement étroit de JSON Schema ; les contraintes inconnues sont refusées.
fn validate_schema(schema: &Value) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or("parameters doit être un objet JSON Schema")?;
    allowed_keys(
        object,
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
        ],
    )?;
    if object.get("type").and_then(Value::as_str) != Some("object")
        || object.get("additionalProperties") != Some(&Value::Bool(false))
    {
        return Err("parameters exige type=object et additionalProperties=false".into());
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("properties doit être un objet")?;
    if properties.len() > 64 {
        return Err("64 paramètres au plus".into());
    }
    for (name, property) in properties {
        if !identifier(name) || name.len() > 64 {
            return Err("nom de paramètre invalide".into());
        }
        let property = property.as_object().ok_or("schéma de paramètre invalide")?;
        allowed_keys(property, &["type", "enum", "description"])?;
        let kind = property.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "string" | "boolean" | "integer" | "number") {
            return Err("types acceptés : string, boolean, integer, number".into());
        }
        if let Some(values) = property.get("enum") {
            let values = values.as_array().ok_or("enum doit être un tableau")?;
            if values.is_empty() || !values.iter().all(|value| matches_type(kind, value)) {
                return Err("enum doit contenir des valeurs du type déclaré".into());
            }
        }
    }
    if let Some(required) = object.get("required") {
        let required = required.as_array().ok_or("required doit être un tableau")?;
        let mut seen = HashSet::new();
        for name in required {
            let name = name.as_str().ok_or("paramètre requis invalide")?;
            if !properties.contains_key(name) || !seen.insert(name) {
                return Err("paramètre requis inconnu ou répété".into());
            }
        }
    }
    Ok(())
}

fn allowed_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("contrainte JSON Schema non prise en charge".into());
    }
    if object
        .get("description")
        .is_some_and(|value| !value.is_string())
    {
        return Err("description de paramètre invalide".into());
    }
    Ok(())
}

pub fn validate_args(schema: &Value, arguments: &Value) -> Result<(), String> {
    validate_schema(schema)?;
    if serde_json::to_vec(arguments)
        .map_err(|_| "arguments illisibles")?
        .len()
        > MAX_BYTES
    {
        return Err("arguments trop gros (64 Kio au plus)".into());
    }
    let arguments = arguments
        .as_object()
        .ok_or("les arguments doivent former un objet")?;
    let properties = schema["properties"].as_object().unwrap();
    if let Some(required) = schema["required"].as_array() {
        for name in required {
            if !arguments.contains_key(name.as_str().unwrap()) {
                return Err(format!(
                    "paramètre requis absent : {}",
                    name.as_str().unwrap()
                ));
            }
        }
    }
    for (name, value) in arguments {
        let property = properties
            .get(name)
            .ok_or_else(|| format!("paramètre inconnu : {name}"))?;
        if !matches_type(property["type"].as_str().unwrap(), value) {
            return Err(format!("type invalide pour le paramètre {name}"));
        }
        if property["enum"]
            .as_array()
            .is_some_and(|values| !values.contains(value))
        {
            return Err(format!("valeur non autorisée pour le paramètre {name}"));
        }
    }
    Ok(())
}

fn matches_type(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    }
}

fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn validate_request(request: &FunctionRequest) -> Result<(), String> {
    if !matches!(
        request.method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
    ) {
        return Err("méthode HTTP non prise en charge".into());
    }
    checked_url(&request.url)?;
    if !(100..=30_000).contains(&request.timeout_ms) {
        return Err("timeout_ms doit être compris entre 100 et 30 000".into());
    }
    if serde_json::to_vec(request)
        .map_err(|_| "requête illisible")?
        .len()
        > MAX_BYTES
    {
        return Err("requête trop grosse (64 Kio au plus)".into());
    }
    for (name, value) in &request.headers {
        let name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| "nom d'en-tête HTTP invalide")?;
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "upgrade"
                | "expect"
                | "proxy-authorization"
        ) {
            return Err("en-tête de transport HTTP interdit".into());
        }
        HeaderValue::from_str(value).map_err(|_| "valeur d'en-tête HTTP invalide")?;
    }
    Ok(())
}

/// Les substitutions ne peuvent changer ni le protocole, ni l'hôte, ni le port.
fn checked_url(text: &str) -> Result<reqwest::Url, String> {
    let scheme = text.find("://").ok_or("URL HTTP(S) absolue requise")? + 3;
    let authority_end = text[scheme..]
        .find(['/', '?', '#'])
        .map_or(text.len(), |index| scheme + index);
    let authority = &text[..authority_end];
    if authority.contains("{{")
        || authority.contains("}}")
        || text.contains('\\')
        || text.chars().any(char::is_control)
    {
        return Err("le protocole, l'hôte et le port doivent être fixes".into());
    }
    let mut sample = String::new();
    for (literal, name) in template_parts(text)? {
        sample.push_str(literal);
        if let Some(name) = name {
            if name.starts_with("env:") {
                return Err("les secrets d'environnement sont réservés aux en-têtes".into());
            }
            sample.push_str("argosvalue");
        }
    }
    // URL normalise les segments '..' ; les refuser avant son parsing garde le chemin choisi.
    percent_decoded(&sample[authority_end..])?;
    let path = sample[authority_end..].split('?').next().unwrap();
    for segment in path.split('/') {
        let decoded = percent_decoded(segment)?;
        if decoded == b"." || decoded == b".." {
            return Err("segment de traversée interdit dans le chemin URL".into());
        }
    }
    let url = reqwest::Url::parse(&sample).map_err(|_| "URL HTTP invalide")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("URL HTTP(S) requise, sans identifiants ni fragment".into());
    }
    Ok(url)
}

fn percent_decoded(text: &str) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let digit = |byte: Option<u8>| byte.and_then(|byte| (byte as char).to_digit(16));
            let high = digit(bytes.next()).ok_or("encodage percent invalide dans l'URL")?;
            let low = digit(bytes.next()).ok_or("encodage percent invalide dans l'URL")?;
            decoded.push((high * 16 + low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    Ok(decoded)
}

fn expand_url(text: &str, arguments: &Value) -> Result<reqwest::Url, String> {
    use std::fmt::Write;
    let destination = checked_url(text)?;
    let mut result = String::new();
    for (literal, name) in template_parts(text)? {
        result.push_str(literal);
        if let Some(name) = name {
            let value = arguments
                .get(name)
                .ok_or("paramètre du modèle URL absent")?;
            if value.is_object() || value.is_array() || value.is_null() {
                return Err("seuls les paramètres scalaires sont acceptés dans l'URL".into());
            }
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            for byte in value.bytes() {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                    result.push(byte as char);
                } else {
                    write!(result, "%{byte:02X}").unwrap();
                }
            }
        }
        if result.len() > MAX_BYTES {
            return Err("URL développée trop grosse (64 Kio au plus)".into());
        }
    }
    let url = checked_url(&result)?;
    if destination.origin() != url.origin() {
        return Err("la destination HTTP de la fonction ne peut pas changer".into());
    }
    Ok(url)
}

/// Parcourt un modèle sans interpréter les valeurs substituées comme d'autres modèles.
fn template_parts(text: &str) -> Result<Vec<(&str, Option<&str>)>, String> {
    let mut parts = Vec::new();
    let mut tail = text;
    while let Some(start) = tail.find("{{") {
        let literal = &tail[..start];
        if literal.contains("}}") {
            return Err("modèle de paramètre mal fermé".into());
        }
        let after = &tail[start + 2..];
        let end = after.find("}}").ok_or("modèle de paramètre non fermé")?;
        let name = &after[..end];
        if name.contains('{') || name.contains('}') || name.is_empty() {
            return Err("modèle de paramètre invalide".into());
        }
        parts.push((literal, Some(name)));
        tail = &after[end + 2..];
    }
    if tail.contains("}}") {
        return Err("modèle de paramètre mal fermé".into());
    }
    parts.push((tail, None));
    Ok(parts)
}

fn env_name(name: &str) -> Option<&str> {
    let name = name.strip_prefix("env:")?;
    (!name.is_empty()
        && (name.as_bytes()[0].is_ascii_alphabetic() || name.starts_with('_'))
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then_some(name)
}

fn validate_template(
    text: &str,
    properties: &Map<String, Value>,
    allow_env: bool,
) -> Result<(), String> {
    for (_, name) in template_parts(text)? {
        if let Some(name) = name {
            if properties.contains_key(name) {
                continue;
            }
            if allow_env && env_name(name).is_some() {
                continue;
            }
            return Err(
                "le modèle référence un paramètre inconnu ou un secret hors en-tête".into(),
            );
        }
    }
    Ok(())
}

fn validate_body(body: &Value, properties: &Map<String, Value>) -> Result<(), String> {
    match body {
        Value::String(text) => validate_template(text, properties, false),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| validate_body(value, properties)),
        Value::Object(values) => {
            for (key, value) in values {
                if key.contains("{{") || key.contains("}}") {
                    return Err("les clés du corps JSON doivent être fixes".into());
                }
                validate_body(value, properties)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn expand_text(
    text: &str,
    arguments: &Value,
    secrets: &mut Vec<String>,
    allow_env: bool,
) -> Result<String, String> {
    let mut result = String::new();
    for (literal, name) in template_parts(text)? {
        result.push_str(literal);
        if let Some(name) = name {
            if let Some(value) = arguments.get(name) {
                if value.is_object() || value.is_array() || value.is_null() {
                    return Err("seuls les paramètres scalaires sont acceptés".into());
                }
                match value.as_str() {
                    Some(text) => result.push_str(text),
                    None => result.push_str(&value.to_string()),
                }
            } else if let Some(name) = env_name(name).filter(|_| allow_env) {
                let secret = std::env::var(name).map_err(|_| "secret d'environnement absent")?;
                if secret.is_empty() {
                    return Err("secret d'environnement vide".into());
                }
                result.push_str(&secret);
                secrets.push(secret);
            } else {
                return Err("paramètre du modèle absent".into());
            }
        }
        if result.len() > MAX_BYTES {
            return Err("requête développée trop grosse (64 Kio au plus)".into());
        }
    }
    Ok(result)
}

fn expand_body(body: &Value, arguments: &Value) -> Result<Value, String> {
    let mut remaining = MAX_BYTES;
    expand_body_bounded(body, arguments, &mut remaining)
}

fn spend(remaining: &mut usize, bytes: usize) -> Result<(), String> {
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or("requête développée trop grosse (64 Kio au plus)")?;
    Ok(())
}

/// Compte les octets JSON, échappements compris, sans allouer la valeur sérialisée.
fn spend_json(remaining: &mut usize, value: &impl serde::Serialize) -> Result<(), String> {
    struct Budget<'a>(&'a mut usize);
    impl std::io::Write for Budget<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            spend(self.0, bytes.len()).map_err(std::io::Error::other)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(remaining), value)
        .map_err(|_| "requête développée trop grosse (64 Kio au plus)".into())
}

fn expand_body_bounded(
    body: &Value,
    arguments: &Value,
    remaining: &mut usize,
) -> Result<Value, String> {
    Ok(match body {
        Value::String(text) => {
            let parts = template_parts(text)?;
            if parts.len() == 2 && parts[0].0.is_empty() && parts[1] == ("", None) {
                let name = parts[0].1.unwrap();
                let value = arguments.get(name).ok_or("paramètre du modèle absent")?;
                spend_json(remaining, value)?;
                value.clone()
            } else {
                let text = expand_text(text, arguments, &mut Vec::new(), false)?;
                spend_json(remaining, &text)?;
                Value::String(text)
            }
        }
        Value::Array(values) => {
            spend(remaining, 2 + values.len().saturating_sub(1))?;
            Value::Array(
                values
                    .iter()
                    .map(|value| expand_body_bounded(value, arguments, remaining))
                    .collect::<Result<_, _>>()?,
            )
        }
        Value::Object(values) => {
            spend(remaining, 2 + values.len().saturating_sub(1))?;
            Value::Object(
                values
                    .iter()
                    .map(|(key, value)| {
                        spend_json(remaining, key)?;
                        spend(remaining, 1)?;
                        let value = expand_body_bounded(value, arguments, remaining)?;
                        Ok((key.clone(), value))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        value => {
            spend_json(remaining, value)?;
            value.clone()
        }
    })
}

fn redact(value: Value, secrets: &[String]) -> Value {
    let clean = |text: String| {
        secrets
            .iter()
            .fold(text, |text, secret| text.replace(secret, "[redacted]"))
    };
    match value {
        Value::String(text) => Value::String(clean(text)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| redact(value, secrets))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (clean(key), redact(value, secrets)))
                .collect(),
        ),
        value => {
            let serialized = value.to_string();
            if secrets.contains(&serialized) {
                Value::String("[redacted]".into())
            } else {
                value
            }
        }
    }
}

pub async fn execute(request: &FunctionRequest, arguments: &Value) -> Result<Value, String> {
    validate_request(request)?;
    if !arguments.is_object()
        || serde_json::to_vec(arguments)
            .map_err(|_| "arguments illisibles")?
            .len()
            > MAX_BYTES
    {
        return Err("arguments invalides ou trop gros".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_millis(request.timeout_ms))
        .build()
        .map_err(|_| "création du client HTTP impossible")?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|_| "méthode HTTP invalide")?;
    let url = expand_url(&request.url, arguments)?;
    let mut bytes = url.as_str().len();
    let mut outgoing = client.request(method, url);
    let mut secrets = Vec::new();
    for (name, value) in &request.headers {
        let value = expand_text(value, arguments, &mut secrets, true)?;
        bytes += name.len() + value.len();
        if bytes > MAX_BYTES {
            return Err("requête développée trop grosse (64 Kio au plus)".into());
        }
        let value = HeaderValue::from_str(&value).map_err(|_| "en-tête HTTP développé invalide")?;
        outgoing = outgoing.header(name, value);
    }
    if let Some(body) = &request.body {
        let body = expand_body(body, arguments)?;
        let body = serde_json::to_vec(&body).map_err(|_| "corps JSON invalide")?;
        bytes += body.len();
        outgoing = outgoing
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    if bytes > MAX_BYTES {
        return Err("requête développée trop grosse (64 Kio au plus)".into());
    }
    let mut response = outgoing
        .send()
        .await
        .map_err(|_| "échec ou délai dépassé de la requête HTTP")?;
    if !response.status().is_success() {
        return Err(format!(
            "la fonction a reçu HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err("réponse trop grosse (64 Kio au plus)".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "lecture de la réponse HTTP impossible")?
    {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err("réponse trop grosse (64 Kio au plus)".into());
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    let value = serde_json::from_slice(&body).map_err(|_| "la réponse HTTP n'est pas du JSON")?;
    let value = redact(value, &secrets);
    if serde_json::to_vec(&value)
        .map_err(|_| "réponse JSON invalide")?
        .len()
        > MAX_BYTES
    {
        return Err("résultat trop gros (64 Kio au plus)".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argos_core::AgentId;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn definition() -> PublishedFunction {
        PublishedFunction {
            agent: AgentId::new("domotique"),
            name: "set_light".into(),
            description: "Change la lumière du salon.".into(),
            parameters: json!({
                "type":"object", "additionalProperties":false,
                "properties":{
                    "name":{"type":"string", "enum":["salon", "bureau"]},
                    "state":{"type":"boolean"}
                }, "required":["name", "state"]
            }),
            request: FunctionRequest {
                method: "POST".into(),
                url: "http://127.0.0.1:8123/api/light".into(),
                headers: Default::default(),
                body: Some(json!({"name":"{{name}}", "state":"{{state}}"})),
                timeout_ms: 1_000,
            },
        }
    }

    async fn mock(
        status: u16,
        body: String,
        delay: Duration,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/light", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            tokio::time::sleep(delay).await;
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            String::from_utf8(request).unwrap()
        });
        (url, task)
    }

    #[test]
    fn definitions_and_arguments_are_strict() {
        let function = definition();
        validate_definition(&function).unwrap();
        validate_args(&function.parameters, &json!({"name":"salon", "state":true})).unwrap();
        for args in [
            json!({"name":"salon"}),
            json!({"name":"inconnue", "state":true}),
            json!({"name":"salon", "state":"true"}),
            json!({"name":"salon", "state":true, "extra":1}),
        ] {
            assert!(validate_args(&function.parameters, &args).is_err());
        }
        let mut bad = function.clone();
        bad.parameters["properties"]["state"]["minimum"] = json!(1);
        assert!(validate_definition(&bad).is_err());
        bad = function.clone();
        bad.request.url = "http://{{name}}/light".into();
        assert!(validate_definition(&bad).is_err());
        bad = function.clone();
        bad.request.body = Some(json!({"secret":"{{env:PATH}}"}));
        assert!(validate_definition(&bad).is_err());
        bad = function.clone();
        bad.request.body = Some(json!({"name":"{{missing}}"}));
        assert!(validate_definition(&bad).is_err());
        bad = function.clone();
        bad.request
            .headers
            .insert("Host".into(), "other.host".into());
        assert!(validate_definition(&bad).is_err());
        bad = function.clone();
        bad.request.timeout_ms = 30_001;
        assert!(validate_definition(&bad).is_err());
        bad = function;
        bad.request.body = Some(json!("x".repeat(MAX_BYTES)));
        assert!(validate_definition(&bad).is_err());
    }

    #[test]
    fn scalar_substitution_keeps_types_and_does_not_reparse_arguments() {
        let args = json!({"name":"bureau \"nord\" {{env:PATH}}", "state":false});
        let expanded = expand_body(
            &json!({"state":"{{state}}", "label":"Lumière {{name}}"}),
            &args,
        )
        .unwrap();
        assert_eq!(
            expanded,
            json!({"state":false, "label":"Lumière bureau \"nord\" {{env:PATH}}"})
        );
        assert!(template_parts("{{state}").is_err());
        assert!(template_parts("state}}").is_err());
    }

    #[test]
    fn repeated_arguments_are_bounded_during_rendering_including_json_escaping() {
        let body = Value::Array(vec![json!("{{large}}"); 5_000]);
        assert!(serde_json::to_vec(&body).unwrap().len() < MAX_BYTES);
        let arguments = json!({"large":"x".repeat(MAX_BYTES / 2)});
        assert!(
            expand_body(&body, &arguments)
                .unwrap_err()
                .contains("trop grosse")
        );

        let body = json!({"clé":["{{value}}", true, 123, null]});
        let arguments = json!({"value":"\n\"\u{0001}é"});
        let expected = json!({"clé":["\n\"\u{0001}é", true, 123, null]});
        let size = serde_json::to_vec(&expected).unwrap().len();
        let mut remaining = size;
        assert_eq!(
            expand_body_bounded(&body, &arguments, &mut remaining).unwrap(),
            expected
        );
        assert_eq!(remaining, 0);
        assert!(expand_body_bounded(&body, &arguments, &mut (size - 1)).is_err());
    }

    #[test]
    fn secrets_echoed_as_numeric_or_boolean_scalars_are_redacted() {
        let secrets = vec!["1234".into(), "true".into(), "1.5".into()];
        assert_eq!(
            redact(
                json!({"values":[1234, true, 1.5, false, 5], "1234":"PIN 1234"}),
                &secrets
            ),
            json!({"values":["[redacted]", "[redacted]", "[redacted]", false, 5], "[redacted]":"PIN [redacted]"})
        );
    }

    #[tokio::test]
    async fn cumulative_expanded_headers_are_rejected_before_http() {
        let mut request = definition().request;
        request.url = "http://127.0.0.1:1/no-request".into();
        request.body = None;
        request.headers.insert("X-One".into(), "{{large}}".into());
        request.headers.insert("X-Two".into(), "{{large}}".into());
        let error = execute(&request, &json!({"large":"x".repeat(40_000)}))
            .await
            .unwrap_err();
        assert!(error.contains("trop grosse"), "{error}");
    }

    #[test]
    fn url_parameters_stay_in_path_and_query_with_a_fixed_authority() {
        let url = expand_url(
            "http://localhost:8123/api/{{name}}?state={{state}}",
            &json!({
                "name":"../?#é", "state":"on&admin=true"
            }),
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "http://localhost:8123/api/..%2F%3F%23%C3%A9?state=on%26admin%3Dtrue"
        );
        assert_eq!(
            expand_url("http://localhost/api/{{name}}", &json!({"name":"%2e%2e"}))
                .unwrap()
                .path(),
            "/api/%252e%252e"
        );
        for text in [
            "http://{{name}}/api",
            "{{name}}://localhost/api",
            "http://localhost:{{name}}/api",
            "http://localhost/api/%2e%2e/admin",
            "http://localhost/api/%2E./admin",
            "http://localhost/api/%zz",
            "http://localhost/api?value=%x",
            "http://localhost/{{env:PATH}}",
        ] {
            assert!(checked_url(text).is_err(), "{text}");
        }
        for name in [".", ".."] {
            assert!(expand_url("http://localhost/api/{{name}}", &json!({"name":name})).is_err());
        }
        let mut function = definition();
        function.request.url = "http://localhost/api/{{unknown}}".into();
        assert!(validate_definition(&function).is_err());
        function.request.url = "http://localhost/api/{{name}}?state={{state}}".into();
        validate_definition(&function).unwrap();
    }

    #[tokio::test]
    async fn home_assistant_service_path_is_parameterized() {
        let (url, received) = mock(200, "[]".into(), Duration::ZERO).await;
        let mut function = definition();
        function.parameters["properties"]["state"] = json!({"type":"string", "enum":["on", "off"]});
        function.request.url = url.replace("/light", "/api/services/light/turn_{{state}}");
        function.request.body = Some(json!({"entity_id":"light.{{name}}"}));
        validate_definition(&function).unwrap();
        let arguments = json!({"name":"salon", "state":"on"});
        validate_args(&function.parameters, &arguments).unwrap();
        assert_eq!(
            execute(&function.request, &arguments).await.unwrap(),
            json!([])
        );
        let request = received.await.unwrap();
        assert!(request.starts_with("POST /api/services/light/turn_on HTTP/1.1\r\n"));
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap(),
            json!({"entity_id":"light.salon"})
        );
    }

    #[tokio::test]
    async fn request_executes_with_typed_arguments_and_json_result() {
        let (url, received) = mock(200, "{\"ok\":true}".into(), Duration::ZERO).await;
        let mut function = definition();
        function.request.url = url;
        let result = execute(&function.request, &json!({"name":"salon", "state":true}))
            .await
            .unwrap();
        assert_eq!(result, json!({"ok":true}));
        let request = received.await.unwrap();
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap(),
            json!({"name":"salon", "state":true})
        );
    }

    #[tokio::test]
    async fn errors_and_echoed_responses_do_not_disclose_secrets() {
        let secret = std::env::var("PATH").unwrap();
        let (url, received) = mock(200, json!({"echo":secret}).to_string(), Duration::ZERO).await;
        let mut request = definition().request;
        request.url = url;
        request
            .headers
            .insert("Authorization".into(), "Bearer {{env:PATH}}".into());
        let result = execute(&request, &json!({"name":"salon", "state":false}))
            .await
            .unwrap();
        assert_eq!(result, json!({"echo":"[redacted]"}));
        assert!(
            received
                .await
                .unwrap()
                .contains(&format!("Bearer {secret}"))
        );

        let (url, received) = mock(401, secret.clone(), Duration::ZERO).await;
        request.url = url;
        let error = execute(&request, &json!({"name":"salon", "state":false}))
            .await
            .unwrap_err();
        assert!(error.contains("401"));
        assert!(!error.contains(&secret));
        received.await.unwrap();
    }

    #[tokio::test]
    async fn non_json_redirect_large_response_and_timeout_fail() {
        let mut request = definition().request;
        for (status, body, message) in [
            (200, "not json".into(), "JSON"),
            (302, "{}".into(), "302"),
            (200, "x".repeat(MAX_BYTES + 1), "trop grosse"),
        ] {
            let (url, received) = mock(status, body, Duration::ZERO).await;
            request.url = url;
            let error = execute(&request, &json!({"name":"salon", "state":false}))
                .await
                .unwrap_err();
            assert!(error.contains(message), "{error}");
            received.await.unwrap();
        }
        let (url, received) = mock(200, "{}".into(), Duration::from_millis(300)).await;
        request.url = url;
        request.timeout_ms = 100;
        assert!(
            execute(&request, &json!({"name":"salon", "state":false}))
                .await
                .unwrap_err()
                .contains("délai")
        );
        received.abort();
    }

    #[tokio::test]
    async fn empty_success_returns_null() {
        let (url, received) = mock(204, String::new(), Duration::ZERO).await;
        let mut request = definition().request;
        request.url = url;
        assert_eq!(
            execute(&request, &json!({"name":"salon", "state":true}))
                .await
                .unwrap(),
            Value::Null
        );
        received.await.unwrap();
    }
}
