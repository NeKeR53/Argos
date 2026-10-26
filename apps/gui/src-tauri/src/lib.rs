mod daemon;
mod shell;

use serde::Serialize;
use tauri::{AppHandle, Manager, Url, WindowEvent};

pub use shell::{
    EVENT_PANIC_SHORTCUT, EVENT_PUSH_TO_TALK, EVENT_VOICE_STATE, VoiceMode, VoiceState,
    set_voice_state,
};

#[derive(Serialize)]
struct CoreInfo {
    version: &'static str,
    protocol: u32,
}

#[tauri::command]
fn core_info() -> CoreInfo {
    CoreInfo {
        version: env!("CARGO_PKG_VERSION"),
        protocol: argos_core::PROTOCOL_VERSION,
    }
}

/// URL de l'API du daemon local, relue à chaque appel : le front la redemande à chaque
/// tentative de connexion, un jeton recréé est donc pris en compte sans relancer la GUI.
#[tauri::command]
fn daemon_url() -> Result<String, String> {
    daemon::url_in(&daemon::argos_home()?)
}

/// Base des URL de webhooks des déclencheurs (`http://<[mcp] listen>`).
#[tauri::command]
fn hooks_base() -> Result<String, String> {
    daemon::hooks_base_in(&daemon::argos_home()?)
}

/// Pourquoi une connexion a échoué : daemon absent ou jeton refusé. Hors du thread
/// principal, car la sonde attend le réseau.
#[tauri::command(async)]
fn probe_daemon(url: String) -> daemon::Probe {
    daemon::probe(&url)
}

/// Montre ou cache la console (clic sur l'orbe).
#[tauri::command]
fn toggle_console(app: AppHandle) {
    shell::toggle_console(&app);
}

/// Quitte l'app (menu de l'orbe) : sans icône dans le Dock, c'est la seule sortie.
#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

mod commands {
    use super::{AppHandle, VoiceMode, shell};

    /// État vocal de l'orbe (`state` : `"listening"`, `"speaking"` ou `null` ; `level` :
    /// 0 à 1), pour une voix côté front ou pour essayer l'orbe.
    #[tauri::command]
    pub fn set_voice_state(app: AppHandle, state: Option<VoiceMode>, level: f32) {
        shell::set_voice_state(&app, state, level);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    // Garde des navigations, posée par un plugin pour couvrir toutes les webviews. Le
    // serveur de dev n'est permis qu'en debug, quand l'app le charge.
    let dev_url = if cfg!(debug_assertions) {
        context.config().build.dev_url.clone()
    } else {
        None
    };
    let navigation_guard = tauri::plugin::Builder::<tauri::Wry>::new("navigation-guard")
        .on_navigation(move |_, url: &Url| {
            let allowed = shell::navigation_allowed(url, dev_url.as_ref());
            if !allowed {
                eprintln!("navigation refusée : {url}");
            }
            allowed
        })
        .build();

    tauri::Builder::default()
        .plugin(navigation_guard)
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            core_info,
            daemon_url,
            hooks_base,
            probe_daemon,
            toggle_console,
            quit_app,
            commands::set_voice_state,
        ])
        .setup(|app| {
            // L'orbe tient lieu d'icône : pas d'icône dans le Dock (D38).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            place_orb(app.handle());
            let config = daemon::argos_home()
                .and_then(|home| daemon::gui_config_in(&home))
                .unwrap_or_else(|error| {
                    eprintln!("section [gui] ignorée : {error}");
                    daemon::GuiConfig::default()
                });
            for error in shell::register_shortcuts(app.handle(), &config) {
                eprintln!("{error}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Fermer la console la cache : l'orbe reste, et la rouvre.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == shell::MAIN_WINDOW
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(context)
        .expect("échec du lancement de la GUI Argos");
}

/// L'orbe se pose en haut à droite de l'écran principal.
fn place_orb(app: &AppHandle) {
    let Some(orb) = app.get_webview_window(shell::ORB_WINDOW) else {
        return;
    };
    let (Ok(Some(monitor)), Ok(size)) = (orb.primary_monitor(), orb.outer_size()) else {
        return;
    };
    let margin = (32.0 * monitor.scale_factor()) as i32;
    let x = monitor.position().x + monitor.size().width as i32 - size.width as i32 - margin;
    let y = monitor.position().y + margin * 2;
    let _ = orb.set_position(tauri::PhysicalPosition::new(x, y));
}
