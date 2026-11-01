//! Consigne de configuration confiée à un CLI d'IA par l'installeur (D46) :
//! `argos-daemon setup prompt` l'écrit, complétée des chemins de cette installation.

use crate::update::{Layout, Service};
use std::path::Path;

const PROMPT: &str = include_str!("setup.md");

pub fn prompt(home: &Path, layout: &Layout) -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    let (service, restart) = match layout.service {
        Service::Launchd => (
            format!("agent launchd `{}`", layout.label),
            format!("`launchctl kickstart -k gui/$(id -u)/{}`", layout.label),
        ),
        Service::Systemd => (
            format!("unité systemd utilisateur `{}`", layout.label),
            format!("`systemctl --user restart {}`", layout.label),
        ),
        Service::None => (
            "aucun : le daemon est lancé avec l'application".to_string(),
            "quitter puis relancer Argos".to_string(),
        ),
    };
    let app = layout
        .app
        .as_ref()
        .map_or("non trouvée".to_string(), |app| {
            format!("`{}`", app.display())
        });
    PROMPT
        .replace("{os}", os)
        .replace("{arch}", std::env::consts::ARCH)
        .replace("{daemon}", &layout.daemon.display().to_string())
        .replace("{app}", &app)
        .replace("{home}", &home.display().to_string())
        .replace("{config}", &home.join("config.toml").display().to_string())
        .replace("{service}", &service)
        .replace("{restart}", &restart)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn every_placeholder_is_filled() {
        let layout = Layout {
            daemon: PathBuf::from("/opt/argos/argos-daemon"),
            app: None,
            service: Service::Launchd,
            label: "argos-hub".into(),
        };
        let text = prompt(Path::new("/tmp/argos-essai"), &layout);
        for placeholder in [
            "{os}",
            "{arch}",
            "{daemon}",
            "{app}",
            "{home}",
            "{config}",
            "{service}",
            "{restart}",
        ] {
            assert!(!text.contains(placeholder), "{placeholder} non remplacé");
        }
        assert!(
            text.contains("ARGOS_HOME=\"/tmp/argos-essai\" \"/opt/argos/argos-daemon\" config set")
        );
        assert!(text.contains("gui/$(id -u)/argos-hub"));
    }
}
