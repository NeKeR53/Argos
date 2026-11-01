//! Serveur HTTP local des surfaces : chacune s'ouvre aussi dans un navigateur, à
//! `http://127.0.0.1:7890/surfaces/<id>` (D27–D28).
//!
//! La page servie n'est qu'un hôte : elle place le code de l'agent dans le même iframe bac
//! à sable que la console (`allow-scripts` seul, CSP sans réseau, SDK injecté) et relaie
//! le pont `postMessage` par un WebSocket propre à la surface. Le SDK ne voit aucune
//! différence. Ce WebSocket ne fait que ce que fait le pont de la console : `send`,
//! `emit`, `call`, et le flux de la page.
//!
//! Sans authentification (choix de l'utilisateur) : tout ce qui tourne sur la machine peut
//! lire les surfaces et parler aux agents par leur pont. Pour qu'un site web ouvert dans
//! le navigateur ne le puisse pas, l'en-tête `Host` doit nommer la boucle locale (contre
//! le DNS rebinding), un WebSocket ouvert par une page doit venir de cette origine, et les
//! pages refusent d'être encadrées.

use crate::supervisor::Supervisor;
use argos_core::protocol::DaemonEvent;
use argos_core::{AgentId, Level, SurfaceDoc, SurfaceMeta};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

/// Lignes du flux gardées par surface, comme la console.
const FEED_LIMIT: usize = 500;
/// Bornes des messages acceptés d'une page, celles du pont de la console (`bridge.ts`).
const MAX_TEXT: usize = 64 * 1024;
const MAX_EVENT_NAME: usize = 100;
const MAX_PAYLOAD: usize = 64 * 1024;

/// Politique de l'iframe, identique à celle de la console (`srcdoc.ts`).
const SURFACE_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; \
style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; \
connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; \
form-action 'none'; base-uri 'none'";

const MARKDOWN: &str = include_str!("../../../apps/gui/src/surface/markdown.js");
const SDK: &str = include_str!("../../../apps/gui/src/surface/sdk.js");
const HOST_SCRIPT: &str = include_str!("surface_web/host.js");
const STYLE: &str = include_str!("surface_web/style.css");

pub async fn serve(listener: TcpListener, supervisor: Supervisor) -> std::io::Result<()> {
    let feeds = Feeds::default();
    tokio::spawn(feeds.clone().follow(supervisor.clone()));
    axum::serve(listener, router(supervisor, feeds)).await
}

pub fn router(supervisor: Supervisor, feeds: Feeds) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/surfaces/{id}", get(page))
        .route("/surfaces/{id}/ws", get(socket))
        .layer(middleware::from_fn(local_only))
        .with_state(Web { supervisor, feeds })
}

#[derive(Clone)]
struct Web {
    supervisor: Supervisor,
    feeds: Feeds,
}

// --- Flux des pages ---------------------------------------------------------------

/// Une ligne du flux d'une surface, au format du SDK (`FeedItem` de la console).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeedItem {
    Sent { text: String },
    Message { text: String },
    Busy { busy: bool },
    Data { data: Value },
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub seq: u64,
    #[serde(flatten)]
    pub item: FeedItem,
}

#[derive(Debug, Clone)]
enum Update {
    Feed { surface: String, entry: Entry },
    Changed { surface: SurfaceMeta, deleted: bool },
}

/// Flux de chaque surface depuis le démarrage du daemon, rejoué à chaque chargement de
/// page : une page rechargée ou ouverte plus tard retrouve la conversation.
#[derive(Clone)]
pub struct Feeds {
    inner: Arc<Mutex<FeedState>>,
    updates: broadcast::Sender<Update>,
}

#[derive(Default)]
struct FeedState {
    seq: u64,
    surfaces: HashMap<String, VecDeque<Entry>>,
}

impl Default for Feeds {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            updates: broadcast::channel(1024).0,
        }
    }
}

impl Feeds {
    /// Suit les événements du daemon jusqu'à son arrêt.
    pub async fn follow(self, supervisor: Supervisor) {
        let mut events = supervisor.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => self.on_event(&supervisor, event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    fn on_event(&self, supervisor: &Supervisor, event: DaemonEvent) {
        match event {
            DaemonEvent::Input { agent, text, .. } => {
                // Le chat d'une surface montre ce qui en est parti, d'où qu'on l'ait envoyé.
                if let Some((surface, sent)) = sent_from_surface(&text) {
                    self.push(&surface, FeedItem::Sent { text: sent.into() });
                }
                self.push_agent(supervisor, &agent, FeedItem::Busy { busy: true });
            }
            DaemonEvent::Message { agent, text } => {
                self.push_agent(supervisor, &agent, FeedItem::Message { text });
            }
            DaemonEvent::TurnCompleted { agent, .. } | DaemonEvent::Exited { agent, .. } => {
                self.push_agent(supervisor, &agent, FeedItem::Busy { busy: false });
            }
            DaemonEvent::SurfaceData { surface, data } => {
                self.push(&surface, FeedItem::Data { data });
            }
            DaemonEvent::SurfaceUpdated { surface, deleted } => {
                if deleted {
                    locked(&self.inner).surfaces.remove(&surface.id);
                }
                let _ = self.updates.send(Update::Changed { surface, deleted });
            }
            _ => {}
        }
    }

    fn push_agent(&self, supervisor: &Supervisor, agent: &AgentId, item: FeedItem) {
        let surfaces = supervisor.registry().surfaces(agent).unwrap_or_default();
        for surface in surfaces {
            self.push(&surface.id, item.clone());
        }
    }

    pub fn push(&self, surface: &str, item: FeedItem) {
        let entry = {
            let mut state = locked(&self.inner);
            state.seq += 1;
            let entry = Entry {
                seq: state.seq,
                item,
            };
            let feed = state.surfaces.entry(surface.into()).or_default();
            feed.push_back(entry.clone());
            if feed.len() > FEED_LIMIT {
                feed.pop_front();
            }
            entry
        };
        let _ = self.updates.send(Update::Feed {
            surface: surface.into(),
            entry,
        });
    }

    pub fn history(&self, surface: &str) -> Vec<Entry> {
        locked(&self.inner)
            .surfaces
            .get(surface)
            .map(|feed| feed.iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ce qu'une surface envoie à son agent, préfixé pour qu'il sache d'où cela vient.
pub fn surface_message(meta: &SurfaceMeta, text: &str) -> String {
    format!(
        "[Depuis la surface « {} » ({})]\n{text}",
        meta.title, meta.id
    )
}

/// Un événement d'une surface, tel que son agent le reçoit.
pub fn surface_event(meta: &SurfaceMeta, name: &str, payload: &Value) -> String {
    format!(
        "[Événement « {name} » de la surface « {} » ({})]\n{payload}",
        meta.title, meta.id
    )
}

/// Surface et texte d'un message parti d'une surface (voir [`surface_message`]).
fn sent_from_surface(text: &str) -> Option<(String, &str)> {
    let (head, body) = text.split_once('\n')?;
    let head = head
        .strip_prefix("[Depuis la surface « ")?
        .strip_suffix(")]")?;
    let (_, id) = head.rsplit_once(" (")?;
    id.starts_with("s-").then(|| (id.to_string(), body))
}

// --- Garde de la boucle locale -----------------------------------------------------

/// Refuse un `Host` qui ne nomme pas la boucle locale et un WebSocket ouvert par une page
/// d'une autre origine ; interdit d'encadrer les pages.
async fn local_only(request: Request, next: Next) -> Response {
    let Some(host) = local_host(request.headers()) else {
        return (
            StatusCode::FORBIDDEN,
            "hôte refusé : utiliser 127.0.0.1 ou localhost",
        )
            .into_response();
    };
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return (StatusCode::FORBIDDEN, "origine refusée").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// L'en-tête `Host` s'il nomme la boucle locale (`127.0.0.1`, `localhost`, `[::1]`).
fn local_host(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]").then(|| host.to_string())
}

/// Politique des pages hôtes. L'iframe `srcdoc` en hérite et y ajoute la sienne, plus
/// stricte : seule la page hôte ouvre son WebSocket.
fn host_csp(host: &str) -> String {
    format!(
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
         img-src data: blob:; font-src data:; media-src data: blob:; \
         connect-src 'self' ws://{host}; frame-ancestors 'none'; form-action 'none'; \
         base-uri 'none'"
    )
}

fn html_page(headers: &HeaderMap, body: String) -> Response {
    let host = local_host(headers).unwrap_or_default();
    let mut response = Html(body).into_response();
    if let Ok(value) = HeaderValue::from_str(&host_csp(&host)) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    response
}

// --- Pages --------------------------------------------------------------------------

/// Les surfaces de l'Assistant et des Agents, par agent.
async fn index(State(web): State<Web>, headers: HeaderMap) -> Response {
    let agents = match web.supervisor.tree() {
        Ok(agents) => agents,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let mut sections = String::new();
    for agent in agents
        .iter()
        .filter(|agent| agent.record.level != Level::Worker)
    {
        let surfaces = web
            .supervisor
            .registry()
            .surfaces(&agent.record.id)
            .unwrap_or_default();
        if surfaces.is_empty() {
            continue;
        }
        let name = agent_name(&agent.record);
        sections.push_str(&format!("<section><h2>{}</h2><ul>", escape_html(&name)));
        for surface in surfaces {
            sections.push_str(&format!(
                "<li><a href=\"/surfaces/{id}\">{title}</a> <span class=\"kind\" data-kind=\"{kind}\">{kind}</span> <span class=\"muted\">{id} · v{version}</span></li>",
                id = escape_html(&surface.id),
                title = escape_html(&surface.title),
                kind = surface.kind,
                version = surface.version,
            ));
        }
        sections.push_str("</ul></section>");
    }
    if sections.is_empty() {
        sections = "<p class=\"muted\">Aucune surface pour l'instant.</p>".into();
    }
    html_page(
        &headers,
        format!(
            "<!doctype html>\n<html lang=\"fr\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>Surfaces · Argos</title><style>{STYLE}</style></head>\
             <body class=\"index\"><main><h1>Surfaces</h1>{sections}</main></body></html>"
        ),
    )
}

fn agent_name(record: &argos_core::AgentRecord) -> String {
    match (&record.charter, record.level) {
        (Some(charter), _) => charter.name.clone(),
        (None, Level::Assistant) => "Assistant".into(),
        (None, _) => record.id.to_string(),
    }
}

/// Page hôte d'une surface : barre de titre, iframe bac à sable, pont vers le WebSocket.
async fn page(State(web): State<Web>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let found = web.supervisor.registry().surface_meta(&id);
    let meta = match found {
        Ok(meta) => meta,
        Err(_) => return (StatusCode::NOT_FOUND, "surface inconnue").into_response(),
    };
    html_page(
        &headers,
        format!(
            "<!doctype html>\n<html lang=\"fr\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>{title}</title><style>{STYLE}</style></head>\
             <body class=\"host\"><header class=\"bar\"><a href=\"/\" class=\"home\">Surfaces</a>\
             <span class=\"kind\" data-kind=\"{kind}\">{kind}</span>\
             <span class=\"title\">{title}</span><span class=\"muted\" id=\"version\">{id} · v{version}</span>\
             <span class=\"spacer\"></span><span class=\"status muted\" id=\"status\" role=\"status\"></span></header>\
             <main id=\"frame\"><p class=\"muted loading\">Chargement de la surface…</p></main>\
             <script>window.__argosHost = {config};</script><script>\n{HOST_SCRIPT}\n</script></body></html>",
            title = escape_html(&meta.title),
            kind = meta.kind,
            id = escape_html(&meta.id),
            version = meta.version,
            config = script_json(&json!({ "surface": meta.id })),
        ),
    )
}

/// Page de la surface pour l'attribut `srcdoc` de l'iframe, comme `buildSrcdoc` dans la
/// console : CSP sans aucun accès réseau, puis le SDK, puis le HTML de l'agent tel quel.
pub fn srcdoc(doc: &SurfaceDoc) -> String {
    let surface = json!({
        "id": doc.meta.id,
        "title": doc.meta.title,
        "kind": doc.meta.kind,
        "version": doc.meta.version,
    });
    [
        "<!doctype html>",
        "<html lang=\"fr\">",
        "<head>",
        "<meta charset=\"utf-8\">",
        &format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{SURFACE_CSP}\">"),
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
        &format!("<title>{}</title>", escape_html(&doc.meta.title)),
        &format!(
            "<script>window.__argosSurface = {};</script>",
            script_json(&surface)
        ),
        &format!("<script>\n{MARKDOWN}\n</script>"),
        &format!("<script>\n{SDK}\n</script>"),
        "</head>",
        "<body>",
        &doc.html,
        "</body>",
        "</html>",
    ]
    .join("\n")
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// JSON sûr dans un `<script>` : un titre contenant `</script>` ne ferme pas le script.
fn script_json(value: &Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

// --- Pont ---------------------------------------------------------------------------

/// Ce qu'une page hôte relaie de sa surface (les messages du SDK, déjà filtrés par elle,
/// revérifiés ici).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FromPage {
    Send {
        text: String,
    },
    Emit {
        name: String,
        payload: Value,
    },
    Call {
        id: u64,
        name: String,
        arguments: Map<String, Value>,
    },
}

async fn socket(
    State(web): State<Web>,
    Path(id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if web.supervisor.registry().surface_meta(&id).is_err() {
        return (StatusCode::NOT_FOUND, "surface inconnue").into_response();
    }
    upgrade
        .max_message_size(4 * MAX_PAYLOAD)
        .on_upgrade(move |socket| bridge(web, id, socket))
}

/// Une page hôte connectée : la page et son flux en entier, puis au fil de l'eau ; ses
/// envois, événements et appels vers l'agent.
async fn bridge(web: Web, surface: String, mut socket: WebSocket) {
    // Abonné avant de lire l'historique : rien ne se perd entre les deux, la page écarte
    // ce qu'elle a déjà par son `seq`.
    let mut updates = web.feeds.updates.subscribe();
    let (outbox, mut queued) = mpsc::unbounded_channel::<Value>();
    let Some(page) = page_message(&web.supervisor, &surface) else {
        return;
    };
    // Verrou du registre relâché avant `inspect`, qui le reprend.
    let owner = web.supervisor.registry().surface_meta(&surface).ok();
    let busy = owner
        .and_then(|meta| web.supervisor.inspect(&meta.agent).ok())
        .is_some_and(|agent| agent.busy);
    let _ = outbox.send(page);
    let _ = outbox.send(json!({
        "type": "feed",
        "items": web.feeds.history(&surface),
        "busy": busy,
    }));

    loop {
        let outgoing = tokio::select! {
            queued = queued.recv() => match queued {
                Some(message) => message,
                None => break,
            },
            update = updates.recv() => match update {
                Ok(Update::Feed { surface: target, entry }) if target == surface => {
                    json!({ "type": "feed", "items": [entry] })
                }
                Ok(Update::Changed { surface: meta, deleted }) if meta.id == surface => {
                    if deleted {
                        json!({ "type": "deleted" })
                    } else {
                        match page_message(&web.supervisor, &surface) {
                            Some(page) => page,
                            None => continue,
                        }
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = socket.recv() => match frame {
                Some(Ok(Message::Text(text))) => {
                    receive(&web, &surface, text.as_str(), &outbox);
                    continue;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            },
        };
        if socket
            .send(Message::text(outgoing.to_string()))
            .await
            .is_err()
        {
            break;
        }
    }
}

fn page_message(supervisor: &Supervisor, surface: &str) -> Option<Value> {
    let doc = supervisor.registry().surface(surface).ok()?;
    Some(json!({
        "type": "page",
        "surface": doc.meta,
        "srcdoc": srcdoc(&doc),
    }))
}

/// Un message de la page, borné comme au pont de la console ; les envois et appels
/// partent dans leur tâche pour ne pas retarder le flux.
fn receive(web: &Web, surface: &str, text: &str, outbox: &mpsc::UnboundedSender<Value>) {
    let Ok(message) = serde_json::from_str::<FromPage>(text) else {
        return;
    };
    let supervisor = web.supervisor.clone();
    let surface = surface.to_string();
    match message {
        FromPage::Send { text } => {
            if text.trim().is_empty() || text.len() > MAX_TEXT {
                return;
            }
            tokio::spawn(async move {
                let _ = surface_send(&supervisor, &surface, &text).await;
            });
        }
        FromPage::Emit { name, payload } => {
            if name.trim().is_empty()
                || name.chars().count() > MAX_EVENT_NAME
                || payload.to_string().len() > MAX_PAYLOAD
            {
                return;
            }
            tokio::spawn(async move {
                let _ = surface_emit(&supervisor, &surface, &name, &payload).await;
            });
        }
        FromPage::Call {
            id,
            name,
            arguments,
        } => {
            let arguments = Value::Object(arguments);
            if arguments.to_string().len() > MAX_PAYLOAD {
                let _ = outbox.send(json!({
                    "type": "call_result",
                    "id": id,
                    "error": "arguments trop volumineux (64 Kio au plus)",
                }));
                return;
            }
            let outbox = outbox.clone();
            tokio::spawn(async move {
                let reply = match surface_call(&supervisor, &surface, &name, arguments).await {
                    Ok(result) => json!({ "type": "call_result", "id": id, "result": result }),
                    Err(error) => json!({ "type": "call_result", "id": id, "error": error }),
                };
                let _ = outbox.send(reply);
            });
        }
    }
}

/// Texte remis à l'agent de la surface, au nom de l'utilisateur. Rend l'agent concerné.
pub async fn surface_send(
    supervisor: &Supervisor,
    surface: &str,
    text: &str,
) -> Result<AgentId, String> {
    let meta = supervisor
        .registry()
        .surface_meta(surface)
        .map_err(|error| error.to_string())?;
    supervisor
        .deliver(
            &meta.agent,
            argos_core::protocol::Origin::User,
            surface_message(&meta, text),
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(meta.agent)
}

/// Événement remis à l'agent de la surface, au nom de l'utilisateur.
pub async fn surface_emit(
    supervisor: &Supervisor,
    surface: &str,
    name: &str,
    payload: &Value,
) -> Result<AgentId, String> {
    let meta = supervisor
        .registry()
        .surface_meta(surface)
        .map_err(|error| error.to_string())?;
    supervisor
        .deliver(
            &meta.agent,
            argos_core::protocol::Origin::User,
            surface_event(&meta, name, payload),
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(meta.agent)
}

/// Appel d'une fonction du propriétaire de la surface, et d'aucune autre (D41).
pub async fn surface_call(
    supervisor: &Supervisor,
    surface: &str,
    function: &str,
    arguments: Value,
) -> Result<Value, String> {
    let meta = supervisor
        .registry()
        .surface_meta(surface)
        .map_err(|error| error.to_string())?;
    supervisor
        .call_function(&meta.agent, &meta.agent, function, arguments)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_sent_from_a_surface_is_recognized() {
        let meta = SurfaceMeta {
            id: "s-abc".into(),
            agent: AgentId::new("a-1"),
            kind: argos_core::SurfaceKind::Cockpit,
            title: "Tableau (v2)".into(),
            version: 1,
            created_at: 0,
            updated_at: 0,
        };
        let text = surface_message(&meta, "bonjour\nà toi");
        assert_eq!(
            sent_from_surface(&text),
            Some(("s-abc".to_string(), "bonjour\nà toi"))
        );
        assert_eq!(sent_from_surface("[Depuis la console]\nbonjour"), None);
    }

    #[test]
    fn only_loopback_hosts_are_served() {
        let host = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(value).unwrap());
            local_host(&headers)
        };
        assert_eq!(host("127.0.0.1:7890").as_deref(), Some("127.0.0.1:7890"));
        assert_eq!(host("localhost:7890").as_deref(), Some("localhost:7890"));
        assert_eq!(host("[::1]:7890").as_deref(), Some("[::1]:7890"));
        assert_eq!(host("evil.example:7890"), None);
        assert_eq!(host("127.0.0.1.evil.example"), None);
    }

    #[test]
    fn the_title_cannot_close_the_script() {
        let doc = SurfaceDoc {
            meta: SurfaceMeta {
                id: "s-1".into(),
                agent: AgentId::new("a-1"),
                kind: argos_core::SurfaceKind::Artifact,
                title: "</script><b>".into(),
                version: 3,
                created_at: 0,
                updated_at: 0,
            },
            html: "<p>salut</p>".into(),
        };
        let page = srcdoc(&doc);
        assert!(page.contains("connect-src 'none'"));
        assert!(page.contains("\\u003c/script\\u003e"));
        assert!(
            page.contains("window.argos")
                || page.contains("Object.defineProperty(window, \"argos\"")
        );
        assert!(page.ends_with("<p>salut</p>\n</body>\n</html>"));
    }
}
