mod daemon;
mod orb;
mod shell;
mod voice;

use serde::Serialize;
use tauri::{AppHandle, Manager, RunEvent, Url, WindowEvent};

pub use shell::{
    EVENT_PANIC_SHORTCUT, EVENT_PUSH_TO_TALK, EVENT_VOICE_STATE, VoiceMode, VoiceState,
    set_voice_state,
};
pub use voice::{EVENT_VOICE, EVENT_VOICE_STATUS, EVENT_VOICE_TRANSCRIPT, VoiceStatus};

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

/// Quitte l'app (menu de l'orbe) : sans icône dans le Dock, c'est la seule sortie. La
/// voix s'arrête d'abord, pour ne pas laisser `whisper-server` orphelin.
#[tauri::command]
fn quit_app(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        voice::shutdown(&app).await;
        app.exit(0);
    });
}

/// Raccourcis globaux en vigueur, et les erreurs d'enregistrement du démarrage.
pub struct Shortcuts {
    config: std::sync::Mutex<daemon::GuiConfig>,
    errors: std::sync::Mutex<Vec<String>>,
}

/// Ce que la console affiche et modifie : `[gui]` sans ce qui n'est pas un raccourci.
#[derive(Debug, Clone, Serialize)]
pub struct ShortcutsView {
    push_to_talk: String,
    console: String,
    panic: String,
    /// Raccourcis refusés au démarrage (illisibles, ou pris par une autre app).
    errors: Vec<String>,
}

impl Shortcuts {
    fn view(&self) -> ShortcutsView {
        let config = self.config.lock().expect("verrou empoisonné");
        ShortcutsView {
            push_to_talk: config.push_to_talk.clone(),
            console: config.console.clone(),
            panic: config.panic.clone(),
            errors: self.errors.lock().expect("verrou empoisonné").clone(),
        }
    }
}

mod commands {
    use super::{AppHandle, Shortcuts, ShortcutsView, VoiceMode, daemon, shell, voice};
    use tauri::State;

    /// Raccourcis globaux en vigueur.
    #[tauri::command]
    pub fn shortcuts(shortcuts: State<'_, Shortcuts>) -> ShortcutsView {
        shortcuts.view()
    }

    /// Change un raccourci (`key` : `push_to_talk`, `console` ou `panic`) : actif tout de
    /// suite, puis écrit dans `[gui]` de `config.toml`. Refusé, l'ancien reste.
    #[tauri::command]
    pub fn set_shortcut(
        app: AppHandle,
        shortcuts: State<'_, Shortcuts>,
        key: String,
        keys: String,
    ) -> Result<ShortcutsView, String> {
        let keys = keys.trim().to_string();
        {
            let mut config = shortcuts.config.lock().expect("verrou empoisonné");
            shell::replace_shortcut(&app, &config, &key, &keys)?;
            match key.as_str() {
                "push_to_talk" => config.push_to_talk = keys.clone(),
                "console" => config.console = keys.clone(),
                _ => config.panic = keys.clone(),
            }
        }
        shortcuts
            .errors
            .lock()
            .expect("verrou empoisonné")
            .retain(|error| !error.contains(shell::shortcut_label(&key)));
        // Actif même si l'écriture échoue ; l'erreur le dit.
        let home = daemon::argos_home()?;
        daemon::save_gui_value(&home, &key, &keys)
            .map_err(|error| format!("raccourci actif, mais non enregistré : {error}"))?;
        Ok(shortcuts.view())
    }

    /// État vocal de l'orbe (`state` : `"listening"`, `"speaking"` ou `null` ; `level` :
    /// 0 à 1), pour une voix côté front ou pour essayer l'orbe.
    #[tauri::command]
    pub fn set_voice_state(app: AppHandle, state: Option<VoiceMode>, level: f32) {
        shell::set_voice_state(&app, state, level);
    }

    /// État de la voix (prête, en démarrage, désactivée ou en panne, avec la raison) et
    /// réglage de lecture des réponses tapées.
    #[tauri::command]
    pub fn voice_status(voice: State<'_, voice::Voice>) -> voice::VoiceInfo {
        voice.info()
    }

    /// Fragment de la réponse de l'Assistant à prononcer.
    #[tauri::command]
    pub fn voice_speak_delta(app: AppHandle, text: String) {
        voice::speak_delta(&app, &text);
    }

    /// Fin de la réponse : le reste est prononcé.
    #[tauri::command]
    pub fn voice_finish_speech(app: AppHandle) {
        voice::finish_speech(&app);
    }

    /// Coupe la lecture en cours.
    #[tauri::command]
    pub fn voice_stop_speech(app: AppHandle) {
        voice::stop_speech(&app);
    }

    /// La pastille de l'orbe change de taille (points) : la fenêtre suit, centre fixe.
    #[tauri::command]
    pub fn orb_resize(app: AppHandle, width: f64, height: f64) {
        crate::orb::resize(&app, width, height);
    }

    /// Double-clic ou menu de l'orbe : retour à sa place par défaut.
    #[tauri::command]
    pub fn orb_reset_position(app: AppHandle) {
        crate::orb::reset(&app);
    }

    /// Mode d'essai sans micro (debug seulement) : une transcription comme si
    /// l'utilisateur l'avait dite, envoyée à l'Assistant par le front.
    #[cfg(debug_assertions)]
    #[tauri::command]
    pub fn voice_simulate(app: AppHandle, text: String) {
        voice::simulate(&app, text);
    }
}

/// Commandes du front ; `voice_simulate` n'existe qu'en debug.
macro_rules! handler {
    ($($debug_only:path),*) => {
        tauri::generate_handler![
            core_info,
            daemon_url,
            hooks_base,
            probe_daemon,
            toggle_console,
            quit_app,
            commands::set_voice_state,
            commands::shortcuts,
            commands::set_shortcut,
            commands::voice_status,
            commands::voice_speak_delta,
            commands::voice_finish_speech,
            commands::voice_stop_speech,
            commands::orb_resize,
            commands::orb_reset_position,
            $($debug_only),*
        ]
    };
}

type InvokeHandler = dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static;

#[cfg(debug_assertions)]
fn invoke_handler() -> Box<InvokeHandler> {
    Box::new(handler![commands::voice_simulate])
}

#[cfg(not(debug_assertions))]
fn invoke_handler() -> Box<InvokeHandler> {
    Box::new(handler![])
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

    let home = daemon::argos_home();
    let config_text = home.as_deref().ok().and_then(daemon::config_text_in);
    let gui = home
        .and_then(|home| daemon::gui_config_in(&home))
        .unwrap_or_else(|error| {
            eprintln!("section [gui] ignorée : {error}");
            daemon::GuiConfig::default()
        });

    let app = tauri::Builder::default()
        .plugin(navigation_guard)
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(voice::Voice::new(gui.speak_typed_replies))
        .manage(orb::OrbPlacement::default())
        .manage(Shortcuts {
            config: std::sync::Mutex::new(gui.clone()),
            errors: std::sync::Mutex::default(),
        })
        .invoke_handler(invoke_handler())
        .setup(move |app| {
            // L'orbe tient lieu d'icône : pas d'icône dans le Dock (D38).
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                if let Some(console) = app.get_webview_window(shell::MAIN_WINDOW) {
                    // SAFETY: Tauri fournit le NSWindow vivant, sur le thread principal du setup.
                    let window =
                        unsafe { &*console.ns_window()?.cast::<objc2_app_kit::NSWindow>() };
                    // Une app accessoire doit autoriser explicitement le plein écran de sa console.
                    window.setCollectionBehavior(
                        window.collectionBehavior()
                            | objc2_app_kit::NSWindowCollectionBehavior::FullScreenPrimary,
                    );
                }
            }
            orb::restore(app.handle());
            let errors = shell::register_shortcuts(app.handle(), &gui);
            for error in &errors {
                eprintln!("{error}");
            }
            *app.state::<Shortcuts>()
                .errors
                .lock()
                .expect("verrou empoisonné") = errors;
            // Sans bloquer : le modèle se charge dans une tâche.
            voice::start(app.handle(), config_text.as_deref());
            #[cfg(debug_assertions)]
            simulate_from_env(app.handle());
            exit_on_signals(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Fermer la console la cache : l'orbe reste, et la rouvre.
            WindowEvent::CloseRequested { api, .. } if window.label() == shell::MAIN_WINDOW => {
                api.prevent_close();
                let _ = window.hide();
            }
            // L'orbe déplacée : sa position est gardée pour le prochain lancement.
            WindowEvent::Moved(_) if window.label() == shell::ORB_WINDOW => {
                orb::moved(window.app_handle());
            }
            _ => {}
        })
        .build(context)
        .expect("échec du lancement de la GUI Argos");

    app.run(|app, event| {
        // Toute sortie (quitter, signal de fin de session…) arrête la voix, et avec elle
        // `whisper-server`. Sans effet si `quit_app` l'a déjà fait.
        if let RunEvent::Exit = event {
            tauri::async_runtime::block_on(voice::shutdown(app));
        }
    });
}

/// SIGTERM, SIGHUP ou SIGINT (fin de session, `kill`, terminal fermé) : sortie propre,
/// voix arrêtée d'abord, sans quoi `whisper-server` resterait orphelin. Un SIGKILL, lui,
/// ne se rattrape pas.
fn exit_on_signals(app: &AppHandle) {
    use tokio::signal::unix::{SignalKind, signal};
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let streams = [
            SignalKind::terminate(),
            SignalKind::hangup(),
            SignalKind::interrupt(),
        ]
        .map(signal);
        let [Ok(mut term), Ok(mut hup), Ok(mut int)] = streams else {
            eprintln!("signaux de fin non surveillés");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = hup.recv() => {}
            _ = int.recv() => {}
        }
        voice::shutdown(&app).await;
        app.exit(0);
    });
}

/// Debug seulement : `ARGOS_VOICE_SIMULATE="phrase"` la « dit » une fois, 10 s après le
/// lancement (le temps que l'orbe se connecte au daemon), comme `voice_simulate`. Sert à
/// essayer la boucle vocale complète sans micro ni clic, depuis un script.
#[cfg(debug_assertions)]
fn simulate_from_env(app: &AppHandle) {
    let Ok(text) = std::env::var("ARGOS_VOICE_SIMULATE") else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        eprintln!("voix : essai simulé « {text} »");
        voice::simulate(&app, text);
    });
}
