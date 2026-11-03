//! Fichiers de service pour installer le daemon d'un nœud (D33) : unité systemd sur un
//! serveur Linux, agent launchd sur macOS. `argos-daemon service <systemd|launchd>
//! [hub|node]` les écrit sur la sortie standard ; les poser sur la machine reste un geste
//! humain.
//!
//! Un service ne reçoit pas le `PATH` du shell (systemd utilisateur : `/usr/local/bin:/usr/bin`
//! et peu d'autres ; launchd : `/usr/bin:/bin:/usr/sbin:/sbin`) : le fichier porte celui de
//! la commande qui l'écrit, pour que le daemon trouve `claude`, `codex`, `ssh` et `git`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Libellé launchd, et nom de l'unité systemd (`argos-node.service`).
pub fn label(mode: &str) -> String {
    format!("argos-{mode}")
}

/// `PATH` à donner au service : `path`, complété par `~/.local/bin`, où s'installe
/// `claude`, et que les shells non interactifs (`ssh hôte commande`) n'ont souvent pas.
pub fn service_path(path: Option<OsString>, user_home: &Path) -> String {
    let mut dirs: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let local_bin = user_home.join(".local/bin");
    if !user_home.as_os_str().is_empty() && !dirs.contains(&local_bin) {
        dirs.push(local_bin);
    }
    std::env::join_paths(dirs)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Unité systemd « utilisateur » (`~/.config/systemd/user/`), relancée si elle tombe.
pub fn systemd_unit(binary: &Path, home: &Path, path: &str, mode: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Argos ({mode})\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={binary} {mode}\n\
         Environment=ARGOS_HOME={home}\n\
         Environment=PATH={path}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         # Sans journal systemd (Raspberry Pi OS allégé), la sortie se perdrait.\n\
         StandardOutput=append:{home}/daemon.log\n\
         StandardError=append:{home}/daemon-error.log\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        binary = binary.display(),
        home = home.display(),
    )
}

/// Agent launchd (`~/Library/LaunchAgents/<libellé>.plist`), lancé à l'ouverture de
/// session et relancé s'il tombe.
pub fn launchd_plist(binary: &Path, home: &Path, path: &str, mode: &str) -> String {
    let escape = |text: String| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let binary = escape(binary.display().to_string());
    let home = escape(home.display().to_string());
    let path = escape(path.to_string());
    let label = label(mode);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>{mode}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>ARGOS_HOME</key>
    <string>{home}</string>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardErrorPath</key>
  <string>{home}/{label}.log</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_systemd_unit_runs_the_node_with_its_home_and_restarts() {
        let unit = systemd_unit(
            Path::new("/opt/argos/argos-daemon"),
            Path::new("/srv/argos"),
            "/usr/bin:/home/argos/.local/bin",
            "node",
        );
        assert!(unit.contains("ExecStart=/opt/argos/argos-daemon node\n"));
        assert!(unit.contains("Environment=ARGOS_HOME=/srv/argos\n"));
        assert!(unit.contains("Environment=PATH=/usr/bin:/home/argos/.local/bin\n"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn the_service_path_adds_the_users_local_bin_once() {
        let home = Path::new("/home/argos");
        assert_eq!(
            service_path(Some("/usr/local/bin:/usr/bin".into()), home),
            "/usr/local/bin:/usr/bin:/home/argos/.local/bin"
        );
        assert_eq!(
            service_path(Some("/home/argos/.local/bin:/usr/bin".into()), home),
            "/home/argos/.local/bin:/usr/bin"
        );
        assert_eq!(service_path(None, home), "/home/argos/.local/bin");
    }

    #[test]
    fn the_launchd_plist_is_valid_and_escaped() {
        let plist = launchd_plist(
            Path::new("/Applications/A&B/argos-daemon"),
            Path::new("/Users/x/.argos"),
            "/opt/homebrew/bin:/usr/bin",
            "node",
        );
        assert!(plist.contains("<string>argos-node</string>"));
        assert!(plist.contains("<key>PATH</key>\n    <string>/opt/homebrew/bin:/usr/bin</string>"));
        assert!(plist.contains("<string>/Applications/A&amp;B/argos-daemon</string>"));
        assert!(plist.contains("<string>node</string>"));
        // Vérifié par l'outil d'Apple quand il est là (macOS).
        let lint = std::process::Command::new("plutil")
            .args(["-lint", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn();
        if let Ok(mut lint) = lint {
            use std::io::Write;
            lint.stdin
                .take()
                .unwrap()
                .write_all(plist.as_bytes())
                .unwrap();
            let output = lint.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}
