//! Serveur HTTP des surfaces face à un superviseur sur faux CLI : aucun token.

mod common;

use argos_core::SurfaceKind;
use argos_core::protocol::DaemonEvent;
use argos_daemon::surface_web;
use common::world::{World, until, world};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Lance le serveur des surfaces sur un port libre ; rend `127.0.0.1:port` et le registre
/// des appareils.
async fn serve_with_devices(world: &World) -> (String, surface_web::Devices) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let devices = surface_web::Devices::open(world.home.path()).unwrap();
    tokio::spawn(surface_web::serve(
        listener,
        world.supervisor.clone(),
        devices.clone(),
        Some(surface_web::Links {
            base: "http://mac.local:7890".into(),
        }),
    ));
    (address.to_string(), devices)
}

async fn serve(world: &World) -> String {
    serve_with_devices(world).await.0
}

/// Un autre appareil : le `Host` nomme le Mac sur le réseau, pas la boucle locale.
const REMOTE: &str = "mac.local:7890";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) Safari/604.1";

fn no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Le secret posé en cookie par une réponse.
fn set_cookie(response: &reqwest::Response) -> String {
    let set = response.headers()["set-cookie"].to_str().unwrap();
    assert!(set.contains("HttpOnly"), "{set}");
    set.split(';').next().unwrap().to_string()
}

fn cockpit(world: &World) -> argos_core::SurfaceMeta {
    world
        .supervisor
        .registry()
        .add_surface(
            &world.agent.id,
            SurfaceKind::Cockpit,
            "Compteur",
            "<button>+1</button>",
        )
        .unwrap()
}

async fn receive(client: &mut Client) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("aucun message dans le délai")
            .expect("connexion fermée")
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

/// Lignes du flux reçues jusqu'à en trouver une qui satisfait `wanted`.
async fn feed_until(client: &mut Client, wanted: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let message = receive(client).await;
        if message["type"] != "feed" {
            continue;
        }
        for item in message["items"].as_array().unwrap() {
            seen.push(item.clone());
            if wanted(item) {
                return seen;
            }
        }
    }
}

#[tokio::test]
async fn the_index_and_the_host_page_list_and_frame_the_surface() {
    let w = world();
    let surface = cockpit(&w);
    let address = serve(&w).await;
    let http = reqwest::Client::new();

    let index = http.get(format!("http://{address}/")).send().await.unwrap();
    assert_eq!(index.status(), 200);
    let index = index.text().await.unwrap();
    assert!(index.contains("Domotique") && index.contains(&format!("/surfaces/{}", surface.id)));
    // Sur la machine elle-même : le QR code d'association et la liste des appareils.
    assert!(index.contains("Associer un appareil") && index.contains("<svg"));
    assert!(index.contains("Aucun appareil associé"));
    // Installable sur l'écran d'accueil.
    assert!(index.contains("rel=\"manifest\"") && index.contains("apple-touch-icon"));

    let page = http
        .get(format!("http://{address}/surfaces/{}", surface.id))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let csp = page.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(page.headers()["x-frame-options"], "DENY");
    assert!(page.text().await.unwrap().contains("allow-scripts"));

    let missing = http
        .get(format!("http://{address}/surfaces/s-inconnue"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn a_foreign_host_or_origin_is_refused() {
    let w = world();
    let surface = cockpit(&w);
    let address = serve(&w).await;

    // DNS rebinding : un nom étranger qui résout vers 127.0.0.1.
    let rebound = reqwest::Client::new()
        .get(format!("http://{address}/"))
        .header("host", "attaquant.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), 401);

    // Une page d'un autre site ne peut pas ouvrir le pont.
    let mut request = format!("ws://{address}/surfaces/{}/ws", surface.id)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", "https://attaquant.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());

    // Depuis la page hôte, même origine : accepté.
    let mut request = format!("ws://{address}/surfaces/{}/ws", surface.id)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_ok());
}

#[tokio::test]
async fn the_bridge_relays_the_page_and_its_agent() {
    let mut w = world();
    let surface = cockpit(&w);
    let address = serve(&w).await;
    let (mut client, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/surfaces/{}/ws", surface.id))
            .await
            .unwrap();

    // La page, dans le même bac à sable que la console, puis le flux.
    let page = receive(&mut client).await;
    assert_eq!(page["type"], "page");
    let srcdoc = page["srcdoc"].as_str().unwrap();
    assert!(
        srcdoc.contains("connect-src 'none'")
            && srcdoc.ends_with("<button>+1</button>\n</body>\n</html>")
    );
    let feed = receive(&mut client).await;
    assert_eq!(feed["type"], "feed");
    assert_eq!(feed["busy"], false);

    // Un envoi depuis la page arrive à l'agent, préfixé, et revient dans le chat.
    client
        .send(Message::text(
            json!({"type": "send", "text": "+1"}).to_string(),
        ))
        .await
        .unwrap();
    let expected = format!("[Depuis la surface « Compteur » ({})]\n+1", surface.id);
    let agent = w.agent.id.clone();
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent: a, text, .. } if *a == agent && *text == expected)
    })
    .await;
    let seen = feed_until(&mut client, |item| {
        item["type"] == "busy" && item["busy"] == false
    })
    .await;
    let kinds: Vec<&str> = seen
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        seen[0],
        json!({"seq": seen[0]["seq"], "type": "sent", "text": "+1"})
    );
    assert!(kinds.contains(&"message"), "{seen:?}");

    // Ce que l'agent pousse arrive à la page.
    w.supervisor.push_surface(&surface.id, json!({"valeur": 1}));
    let seen = feed_until(&mut client, |item| item["type"] == "data").await;
    assert_eq!(seen.last().unwrap()["data"], json!({"valeur": 1}));

    // Une page ouverte plus tard retrouve le flux.
    let (mut late, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/surfaces/{}/ws", surface.id))
            .await
            .unwrap();
    assert_eq!(receive(&mut late).await["type"], "page");
    let history = receive(&mut late).await;
    let items = history["items"].as_array().unwrap();
    assert_eq!(items.first().unwrap()["type"], "sent");
    assert_eq!(items.last().unwrap()["type"], "data");

    // Un appel de fonction inconnue répond une erreur, sans réveiller l'agent.
    client
        .send(Message::text(
            json!({"type": "call", "id": 7, "name": "absente", "arguments": {}}).to_string(),
        ))
        .await
        .unwrap();
    let reply = loop {
        let message = receive(&mut client).await;
        if message["type"] == "call_result" {
            break message;
        }
    };
    assert_eq!(reply["id"], 7);
    assert!(reply["error"].is_string(), "{reply}");

    // Nouvelle version : la page est renvoyée ; suppression : la page le sait.
    let updated = w
        .supervisor
        .registry()
        .update_surface(&surface.id, Some("<p>v2</p>"), None)
        .unwrap();
    w.supervisor.publish_surface(updated, false);
    let page = loop {
        let message = receive(&mut client).await;
        if message["type"] == "page" {
            break message;
        }
    };
    assert_eq!(page["surface"]["version"], 2);
    let deleted = w.supervisor.registry().delete_surface(&surface.id).unwrap();
    w.supervisor.publish_surface(deleted, true);
    loop {
        if receive(&mut client).await["type"] == "deleted" {
            break;
        }
    }
}

#[tokio::test]
async fn another_device_is_paired_once_then_keeps_its_own_secret() {
    let w = world();
    let surface = cockpit(&w);
    let (address, devices) = serve_with_devices(&w).await;
    let key = devices.key();
    let http = no_redirect();
    let get = |path: String| {
        http.get(format!("http://{address}{path}"))
            .header("host", REMOTE)
            .header("user-agent", IPHONE)
    };

    let refused = get(format!("/surfaces/{}", surface.id))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);
    assert!(refused.text().await.unwrap().contains("action=\"/pair\""));
    let wrong = get(format!("/surfaces/{}?k=autre", surface.id))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // Le lien du QR code : un secret propre à l'appareil, la clé quitte l'adresse.
    let link = get(format!("/surfaces/{}?k={key}", surface.id))
        .send()
        .await
        .unwrap();
    assert_eq!(link.status(), 303);
    assert_eq!(
        link.headers()["location"],
        format!("/surfaces/{}", surface.id)
    );
    let cookie = set_cookie(&link);
    assert!(cookie.starts_with("argos_surfaces=") && !cookie.contains(&key));
    assert_eq!(devices.list().len(), 1);
    assert_eq!(devices.list()[0].name, "iPhone · Safari");

    let page = get(format!("/surfaces/{}", surface.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let index = get("/".into())
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    // Le QR code et le code, qui portent la clé, ne sont montrés que sur la machine.
    let index = index.text().await.unwrap();
    assert!(!index.contains("Associer un appareil") && !index.contains(&devices.code()));
    // Le manifeste, demandé avec le cookie ; les icônes, sans.
    let manifest = get(format!("/surfaces/{}/manifest.webmanifest", surface.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(manifest.status(), 200);
    let manifest: Value = manifest.json().await.unwrap();
    assert_eq!(manifest["name"], "Compteur");
    assert_eq!(manifest["start_url"], format!("/surfaces/{}", surface.id));
    assert_eq!(
        get("/icon-180.png".into()).send().await.unwrap().status(),
        200
    );

    // Le pont, depuis la page de cet appareil.
    let bridge = |cookie: Option<&str>| {
        let mut request = format!("ws://{address}/surfaces/{}/ws", surface.id)
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert("host", REMOTE.parse().unwrap());
        headers.insert("origin", format!("http://{REMOTE}").parse().unwrap());
        if let Some(cookie) = cookie {
            headers.insert("cookie", cookie.parse().unwrap());
        }
        request
    };
    let (mut client, _) = tokio_tungstenite::connect_async(bridge(Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(receive(&mut client).await["type"], "page");
    // Sans cookie, même avec la bonne origine : refusé.
    assert!(
        tokio_tungstenite::connect_async(bridge(None))
            .await
            .is_err()
    );

    // La clé seule en cookie (avant l'association par appareil) devient un appareil.
    let legacy = get(format!("/surfaces/{}", surface.id))
        .header("cookie", format!("argos_surfaces={key}"))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 303);
    assert_ne!(set_cookie(&legacy), format!("argos_surfaces={key}"));
    assert_eq!(devices.list().len(), 2);
}

#[tokio::test]
async fn a_device_pairs_with_the_short_code_and_is_revoked_from_the_mac() {
    let w = world();
    let surface = cockpit(&w);
    let (address, devices) = serve_with_devices(&w).await;
    let http = no_redirect();
    let pair = |code: String, origin: Option<&str>| {
        let mut request = http
            .post(format!("http://{address}/pair"))
            .header("host", REMOTE)
            .header("user-agent", IPHONE)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("code={code}&next=%2Fsurfaces%2F{}", surface.id));
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        request
    };
    let origin = format!("http://{REMOTE}");

    let wrong = pair("AAAA-AAAA".into(), Some(&origin))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    assert!(wrong.text().await.unwrap().contains("Code incorrect"));
    // Sans origine, ou depuis un autre site : refusé avant tout essai.
    let code = devices.code();
    assert_eq!(pair(code.clone(), None).send().await.unwrap().status(), 403);
    let foreign = pair(code.clone(), Some("http://attaquant.example"));
    assert_eq!(foreign.send().await.unwrap().status(), 403);

    let paired = pair(code.to_lowercase(), Some(&origin))
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 303);
    assert_eq!(
        paired.headers()["location"],
        format!("/surfaces/{}", surface.id)
    );
    let cookie = set_cookie(&paired);
    let open = || {
        http.get(format!("http://{address}/surfaces/{}", surface.id))
            .header("host", REMOTE)
            .header("cookie", &cookie)
            .send()
    };
    assert_eq!(open().await.unwrap().status(), 200);

    // Un autre appareil ne peut pas révoquer ; la machine, si.
    let id = devices.list()[0].id.clone();
    let remote = http
        .post(format!("http://{address}/devices/{id}/revoke"))
        .header("host", REMOTE)
        .header("origin", &origin)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(remote.status(), 403);
    let index = http.get(format!("http://{address}/")).send().await.unwrap();
    assert!(index.text().await.unwrap().contains("iPhone · Safari"));
    let revoked = http
        .post(format!("http://{address}/devices/{id}/revoke"))
        .header("origin", format!("http://{address}"))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 303);
    assert_eq!(open().await.unwrap().status(), 401);

    // Renouveler : l'ancien code ne sert plus.
    let renewed = http
        .post(format!("http://{address}/pairing/renew"))
        .header("origin", format!("http://{address}"))
        .send()
        .await
        .unwrap();
    assert_eq!(renewed.status(), 303);
    assert_ne!(devices.code(), code);
}
