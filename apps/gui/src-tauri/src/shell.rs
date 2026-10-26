//! Coquille de l'app (D37–D38) : orbe et console, raccourcis globaux, points d'accroche
//! de la voix, et garde des navigations des webviews (surfaces comprises, D28).
//!
//! Points d'accroche pour la voix (la crate `argos-voice` s'y branchera) :
//! - événement [`EVENT_PUSH_TO_TALK`] `{ "pressed": bool }`, émis à l'appui et au
//!   relâchement du raccourci push-to-talk ; écoutable en Rust (`app.listen`) comme en JS ;
//! - [`set_voice_state`] (et la commande `set_voice_state { state, level }`) : pose l'état
//!   vocal de l'orbe, émis en [`EVENT_VOICE_STATE`] `{ "state": "listening" | "speaking" |
//!   null, "level": 0..1 }`. `null` rend la main à l'état de l'Assistant (repos ou réflexion).

use crate::daemon::GuiConfig;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, Url};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub const MAIN_WINDOW: &str = "main";
pub const ORB_WINDOW: &str = "orb";

/// `{ "pressed": bool }` : appui ou relâchement du push-to-talk.
pub const EVENT_PUSH_TO_TALK: &str = "push-to-talk";
/// [`VoiceState`] : ce que fait la voix, pour l'orbe.
pub const EVENT_VOICE_STATE: &str = "voice-state";
/// Sans charge : un appui sur le raccourci du panic stop. L'orbe compte les appuis
/// (deux en moins de 2 s) et envoie l'arrêt au daemon.
pub const EVENT_PANIC_SHORTCUT: &str = "panic-shortcut";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    /// L'utilisateur parle (push-to-talk maintenu).
    Listening,
    /// L'Assistant parle (TTS).
    Speaking,
}

/// État vocal de l'orbe. `level` (0 à 1) module l'animation d'écoute et de parole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoiceState {
    pub state: Option<VoiceMode>,
    pub level: f32,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct PushToTalk {
    pressed: bool,
}

/// Pose l'état vocal de l'orbe ; le niveau est borné à 0..1.
pub fn set_voice_state<R: Runtime>(app: &AppHandle<R>, state: Option<VoiceMode>, level: f32) {
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let _ = app.emit(EVENT_VOICE_STATE, VoiceState { state, level });
}

/// Montre la console (au premier plan) ou la cache.
pub fn toggle_console<R: Runtime>(app: &AppHandle<R>) {
    let Some(console) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let visible = console.is_visible().unwrap_or(false);
    let focused = console.is_focused().unwrap_or(false);
    if visible && focused {
        let _ = console.hide();
    } else {
        let _ = console.show();
        let _ = console.unminimize();
        let _ = console.set_focus();
    }
}

/// Enregistre les raccourcis globaux de `[gui]`. Rend les erreurs (raccourci illisible
/// ou déjà pris par une autre app) sans interrompre les autres.
pub fn register_shortcuts<R: Runtime>(app: &AppHandle<R>, config: &GuiConfig) -> Vec<String> {
    let shortcuts = app.global_shortcut();
    let mut errors = vec![];
    let mut add = |name: &str, keys: &str, handler: fn(&AppHandle<R>, ShortcutState)| {
        let parsed = match keys.parse::<Shortcut>() {
            Ok(shortcut) => shortcut,
            Err(error) => {
                errors.push(format!("raccourci {name} « {keys} » illisible : {error}"));
                return;
            }
        };
        if let Err(error) =
            shortcuts.on_shortcut(parsed, move |app, _, event| handler(app, event.state))
        {
            errors.push(format!(
                "raccourci {name} « {keys} » indisponible : {error}"
            ));
        }
    };
    add("push-to-talk", &config.push_to_talk, |app, state| {
        let pressed = state == ShortcutState::Pressed;
        let _ = app.emit(EVENT_PUSH_TO_TALK, PushToTalk { pressed });
    });
    add("console", &config.console, |app, state| {
        if state == ShortcutState::Pressed {
            toggle_console(app);
        }
    });
    add("panic stop", &config.panic, |app, state| {
        if state == ShortcutState::Pressed {
            // L'orbe montre la confirmation et, au second appui, envoie l'arrêt.
            if let Some(orb) = app.get_webview_window(ORB_WINDOW) {
                let _ = orb.show();
            }
            let _ = app.emit(EVENT_PANIC_SHORTCUT, ());
        }
    });
    errors
}

/// Navigations permises dans les webviews de l'app, cadres des surfaces compris : l'app
/// elle-même (serveur de dev ou protocole de l'app), les pages des surfaces
/// (`about:srcdoc`) et `about:blank`, `data:`, `blob:`. Tout le reste est refusé : une
/// surface ne peut pas naviguer vers une URL externe pour en faire sortir des données.
pub fn navigation_allowed(url: &Url, dev_url: Option<&Url>) -> bool {
    match url.scheme() {
        "about" => matches!(url.path(), "srcdoc" | "blank"),
        "data" | "blob" => true,
        // Protocole de l'app sous macOS et Linux (`tauri://localhost`).
        "tauri" => url.host_str() == Some("localhost"),
        // Protocole de l'app sous Windows et Android.
        "http" | "https" if url.host_str() == Some("tauri.localhost") => true,
        _ => dev_url.is_some_and(|dev| dev.origin() == url.origin()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(url: &str) -> bool {
        let dev = Url::parse("http://localhost:1420").unwrap();
        navigation_allowed(&Url::parse(url).unwrap(), Some(&dev))
    }

    #[test]
    fn the_app_and_surface_pages_may_load() {
        for url in [
            "http://localhost:1420/",
            "http://localhost:1420/orb.html?x=1",
            "tauri://localhost/index.html",
            "https://tauri.localhost/",
            "about:srcdoc",
            "about:blank",
            "data:text/html,<p>salut</p>",
            "blob:http://localhost:1420/1b2c",
        ] {
            assert!(allowed(url), "{url}");
        }
    }

    #[test]
    fn external_navigations_are_refused() {
        for url in [
            "https://example.org/?vol=secret",
            "http://localhost:1421/",
            "http://127.0.0.1:1420/",
            "https://localhost:1420/",
            "http://localhost.evil.test:1420/",
            "tauri://evil/",
            "about:config",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:x@example.org",
        ] {
            assert!(!allowed(url), "{url}");
        }
        // Sans serveur de dev (app compilée), seul le protocole de l'app passe.
        let url = Url::parse("http://localhost:1420/").unwrap();
        assert!(!navigation_allowed(&url, None));
    }
}
