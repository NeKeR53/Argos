//! Surfaces côté agents (D26–D28), sur faux CLI : création, versions, données poussées,
//! propriété. Aucun token.

mod common;

use argos_core::SurfaceKind;
use argos_core::protocol::DaemonEvent;
use common::call_tool;
use common::world::{until, world};
use serde_json::json;

#[tokio::test]
async fn an_agent_creates_updates_and_feeds_its_cockpit() {
    let mut w = world();
    let agent = w.agent.id.clone();
    let (created, error) = call_tool(
        &w,
        &agent,
        "surface_create",
        json!({"kind": "cockpit", "title": "Lecteur PDF", "html": "<p>v1</p>"}),
    )
    .await;
    assert!(!error, "{created}");
    let surface = w.supervisor.registry().surfaces(&agent).unwrap().remove(0);
    assert_eq!(
        (surface.kind, surface.title.as_str()),
        (SurfaceKind::Cockpit, "Lecteur PDF")
    );
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::SurfaceUpdated { surface: s, deleted: false } if s.id == surface.id && s.version == 1)
    })
    .await;

    let (_, error) = call_tool(
        &w,
        &agent,
        "surface_update",
        json!({"surface": surface.id, "html": "<p>v2</p>"}),
    )
    .await;
    assert!(!error);
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::SurfaceUpdated { surface: s, .. } if s.id == surface.id && s.version == 2)
    })
    .await;
    assert_eq!(
        w.supervisor.registry().surface(&surface.id).unwrap().html,
        "<p>v2</p>"
    );

    let (_, error) = call_tool(
        &w,
        &agent,
        "surface_push",
        json!({"surface": surface.id, "data": "{\"page\": 3}"}),
    )
    .await;
    assert!(!error);
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::SurfaceData { surface: s, data } if *s == surface.id && *data == json!({"page": 3}))
    })
    .await;
    // Ce qui n'est pas du JSON passe tel quel, en texte.
    let (_, error) = call_tool(
        &w,
        &agent,
        "surface_push",
        json!({"surface": surface.id, "data": "page 4"}),
    )
    .await;
    assert!(!error);
    until(
        &mut w.events,
        |event| matches!(event, DaemonEvent::SurfaceData { data, .. } if *data == json!("page 4")),
    )
    .await;

    let (listed, _) = call_tool(&w, &agent, "surface_list", json!({})).await;
    assert!(listed.contains(&surface.id));
    let (_, error) = call_tool(&w, &agent, "surface_delete", json!({"surface": surface.id})).await;
    assert!(!error);
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::SurfaceUpdated { deleted: true, .. })
    })
    .await;
}

#[tokio::test]
async fn a_surface_belongs_to_its_agent_alone() {
    let w = world();
    let (_, error) = call_tool(
        &w,
        &w.assistant.id,
        "surface_create",
        json!({"kind": "artifact", "title": "Synthèse", "html": "<h1>Synthèse</h1>"}),
    )
    .await;
    assert!(!error);
    let surface = w
        .supervisor
        .registry()
        .surfaces(&w.assistant.id)
        .unwrap()
        .remove(0);
    for (tool, args) in [
        (
            "surface_update",
            json!({"surface": surface.id, "html": "<p>volé</p>"}),
        ),
        ("surface_push", json!({"surface": surface.id, "data": "{}"})),
        ("surface_delete", json!({"surface": surface.id})),
    ] {
        let (_, error) = call_tool(&w, &w.agent.id, tool, args).await;
        assert!(error, "{tool} sur la surface d'un autre");
    }
    // Un Worker ne parle pas à l'utilisateur : pas de surface.
    let (_, error) = call_tool(
        &w,
        &w.worker.id,
        "surface_create",
        json!({"kind": "artifact", "title": "x", "html": "<p>x</p>"}),
    )
    .await;
    assert!(error);
}

#[tokio::test]
async fn an_agent_deploys_a_surface_for_another_and_both_can_edit_it() {
    let mut w = world();
    let builder = w.assistant.id.clone();
    let owner = w.agent.id.clone();
    let (reply, error) = call_tool(
        &w,
        &builder,
        "surface_create",
        json!({
            "kind": "cockpit",
            "title": "Lumières",
            "html": "<button>Salon</button>",
            "agent": "Domotique",
            "brief": "Le bouton envoie « salon ».",
        }),
    )
    .await;
    assert!(!error, "{reply}");
    assert!(reply.contains("déployée pour"), "{reply}");
    let surface = w.supervisor.registry().surfaces(&owner).unwrap().remove(0);
    assert_eq!(surface.builder.as_ref(), Some(&builder));
    assert!(
        w.supervisor
            .registry()
            .surfaces(&builder)
            .unwrap()
            .is_empty()
    );

    // Le destinataire est prévenu, avec la consigne.
    let expected = format!(
        "[Surface « Lumières » ({}) déployée pour toi par",
        surface.id
    );
    until(&mut w.events, |event| {
        matches!(event, DaemonEvent::Input { agent, text, .. }
            if *agent == owner && text.starts_with(&expected) && text.contains("Le bouton envoie « salon »."))
    })
    .await;

    // Le constructeur la retrouve et la modifie encore ; le destinataire aussi.
    let (listed, _) = call_tool(&w, &builder, "surface_list", json!({})).await;
    assert!(listed.contains(&surface.id), "{listed}");
    for agent in [&builder, &owner] {
        let (_, error) = call_tool(
            &w,
            agent,
            "surface_update",
            json!({"surface": surface.id, "html": "<p>v</p>"}),
        )
        .await;
        assert!(!error);
    }
    assert_eq!(
        w.supervisor
            .registry()
            .surface_meta(&surface.id)
            .unwrap()
            .version,
        3
    );

    // Pas pour un Worker, ni pour un agent inconnu.
    for target in [w.worker.id.to_string(), "Inconnu".into()] {
        let (_, error) = call_tool(
            &w,
            &builder,
            "surface_create",
            json!({"kind": "artifact", "title": "x", "html": "<p>x</p>", "agent": target}),
        )
        .await;
        assert!(error, "{target}");
    }
    // Pour soi-même par son nom : une surface ordinaire, sans constructeur.
    let (_, error) = call_tool(
        &w,
        &owner,
        "surface_create",
        json!({"kind": "artifact", "title": "Moi", "html": "<p>x</p>", "agent": "Domotique"}),
    )
    .await;
    assert!(!error);
    let mine = w.supervisor.registry().surfaces(&owner).unwrap().remove(1);
    assert_eq!(mine.builder, None);
}
