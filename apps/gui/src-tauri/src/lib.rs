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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_info])
        .run(tauri::generate_context!())
        .expect("échec du lancement de la GUI Argos");
}
