//! Fichiers de service pour installer le daemon d'un nœud (D33) : unité systemd sur un
//! serveur Linux, agent launchd sur macOS. `argos-daemon service <systemd|launchd>
//! [hub|node]` les écrit sur la sortie standard ; les poser sur la machine reste un geste
//! humain.

use std::path::Path;

/// Libellé launchd, et nom de l'unité systemd (`argos-node.service`).
pub fn label(mode: &str) -> String {
    format!("argos-{mode}")
}

/// Unité systemd « utilisateur » (`~/.config/systemd/user/`), relancée si elle tombe.
pub fn systemd_unit(binary: &Path, home: &Path, mode: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Argos ({mode})\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={binary} {mode}\n\
         Environment=ARGOS_HOME={home}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        binary = binary.display(),
        home = home.display(),
    )
}

/// Agent launchd (`~/Library/LaunchAgents/<libellé>.plist`), lancé à l'ouverture de
/// session et relancé s'il tombe.
pub fn launchd_plist(binary: &Path, home: &Path, mode: &str) -> String {
    let escape = |text: String| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let binary = escape(binary.display().to_string());
    let home = escape(home.display().to_string());
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
            "node",
        );
        assert!(unit.contains("ExecStart=/opt/argos/argos-daemon node\n"));
        assert!(unit.contains("Environment=ARGOS_HOME=/srv/argos\n"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn the_launchd_plist_is_valid_and_escaped() {
        let plist = launchd_plist(
            Path::new("/Applications/A&B/argos-daemon"),
            Path::new("/Users/x/.argos"),
            "node",
        );
        assert!(plist.contains("<string>argos-node</string>"));
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
