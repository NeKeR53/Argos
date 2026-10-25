mod daemon;

use serde::Serialize;

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

/// Pourquoi une connexion a échoué : daemon absent ou jeton refusé. Hors du thread
/// principal, car la sonde attend le réseau.
#[tauri::command(async)]
fn probe_daemon(url: String) -> daemon::Probe {
    daemon::probe(&url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            core_info,
            daemon_url,
            probe_daemon
        ])
        .run(tauri::generate_context!())
        .expect("échec du lancement de la GUI Argos");
}
