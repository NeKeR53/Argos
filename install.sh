#!/usr/bin/env bash
# Installe Argos sur macOS ou Linux depuis la dernière release GitHub (D46).
#
#   curl -fsSL https://raw.githubusercontent.com/NeKeR53/Argos/main/install.sh | bash
#
# Détecte le système et l'architecture, télécharge l'archive et vérifie sa somme SHA-256,
# installe l'app et le daemon, déclare le daemon en service (launchd ou systemd), puis
# lance la configuration : confiée à `claude` ou `codex` s'ils sont là, sinon par
# quelques questions. Relancé sur une installation existante, il la met à jour.
#
# Options :
#   --version vX.Y.Z   une version précise plutôt que la dernière
#   --archive FICHIER  installer une archive locale (avec --sums FICHIER pour la vérifier)
#   --no-ai            configuration sans IA
#   --no-setup         pas de configuration (installation seule)
#   --no-service       ne pas installer le daemon en service
#   --no-launch        ne pas ouvrir l'app à la fin
#   --yes              aucune question : réponses par défaut
#   --node NOM         installer un nœud pair sans écran (D48) : le daemon seul, en service,
#                      sous ce nom ; l'Assistant vient d'un autre nœud
#
# Variables : ARGOS_HOME (données, défaut ~/.argos) ; pour les essais,
# ARGOS_RELEASES_URL (API des releases), ARGOS_INSTALL_APP_DIR et ARGOS_INSTALL_BIN_DIR
# (où poser l'app et le daemon), ARGOS_SERVICE_LABEL (libellé du service).
set -euo pipefail

REPO="NeKeR53/Argos"
RELEASES_URL="${ARGOS_RELEASES_URL:-https://api.github.com/repos/$REPO/releases?per_page=20}"
LABEL="${ARGOS_SERVICE_LABEL:-}"
ARGOS_HOME="${ARGOS_HOME:-$HOME/.argos}"
export ARGOS_HOME

version="" archive="" sums="" use_ai=1 setup=1 service=1 launch=1 assume_yes=0 node_name=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:?--version attend une version}"; shift 2 ;;
    --archive) archive="${2:?--archive attend un fichier}"; shift 2 ;;
    --sums) sums="${2:?--sums attend un fichier}"; shift 2 ;;
    --no-ai) use_ai=0; shift ;;
    --no-setup) setup=0; shift ;;
    --no-service) service=0; shift ;;
    --no-launch) launch=0; shift ;;
    --yes|-y) assume_yes=1; shift ;;
    --node) node_name="${2:?--node attend un nom}"; shift 2 ;;
    -h|--help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Option inconnue : $1" >&2; exit 2 ;;
  esac
done

# Un nœud sans écran n'a ni app, ni configuration de l'Assistant (D48).
if [[ -n $node_name ]]; then setup=0 launch=0; fi
LABEL="${LABEL:-$([[ -n $node_name ]] && echo argos-node || echo argos-hub)}"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf 'Erreur : %s\n' "$*" >&2; exit 1; }

# Lancé par `curl … | bash`, l'entrée standard est le script : les questions passent par
# le terminal.
tty_ok=0
if [[ $assume_yes -eq 0 ]] && { true </dev/tty; } 2>/dev/null; then tty_ok=1; fi
ask() { # ask <question> <défaut> → réponse
  local answer=""
  if [[ $tty_ok -eq 1 ]]; then
    read -r -p "$1 [$2] " answer </dev/tty || true
  fi
  printf '%s' "${answer:-$2}"
}
yes_no() { # yes_no <question> <o|n>
  case "$(ask "$1 (o/n)" "$2")" in [oOyY]*) return 0 ;; *) return 1 ;; esac
}

# --- Système et archive ------------------------------------------------------------------
os="$(uname -s)" arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64) asset="Argos-macOS-arm64.zip" sums_name="SHA256SUMS-macos-arm64.txt" ;;
  Darwin/x86_64) asset="Argos-macOS-x86_64.zip" sums_name="SHA256SUMS-macos-x86_64.txt" ;;
  Linux/x86_64|Linux/amd64) asset="Argos-Linux-x86_64.tar.gz" sums_name="SHA256SUMS-linux-x86_64.txt" ;;
  # Raspberry Pi et serveurs arm64 : le daemon seul, en nœud.
  Linux/aarch64|Linux/arm64)
    asset="Argos-Node-Linux-arm64.tar.gz" sums_name="SHA256SUMS-linux-arm64.txt"
    [[ -n $node_name ]] || fail "sur Linux arm64, Argos s'installe en nœud : relancer avec --node <nom>."
    ;;
  *) fail "pas d'archive Argos pour $os $arch (macOS arm64/x86_64, Linux x86_64 et arm64)." ;;
esac
# Une app Intel sur un Mac Apple Silicon tournerait sous Rosetta : on prend l'archive native.
if [[ $os == Darwin && $arch == x86_64 ]] && [[ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" == 1 ]]; then
  asset="Argos-macOS-arm64.zip" sums_name="SHA256SUMS-macos-arm64.txt"
fi

sha256() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/argos-install.XXXXXX")"
trap 'rm -rf "$work"' EXIT

if [[ -z $archive ]]; then
  say "Recherche de la release d'Argos ($asset)…"
  releases="$(curl -fsSL -H 'Accept: application/vnd.github+json' "$RELEASES_URL")" \
    || fail "impossible de lire les releases ($RELEASES_URL)."
  # Les URL de téléchargement, les plus récentes d'abord ; sans jq ni python.
  urls="$(printf '%s' "$releases" | grep -o '"browser_download_url": *"[^"]*"' | sed 's/.*"\(http[^"]*\)"$/\1/')"
  pick() { # pick <nom> → URL de la version demandée, ou de la plus récente
    local pattern="/$1\$"
    [[ -n $version ]] && pattern="/download/v${version#v}/$1\$"
    printf '%s\n' "$urls" | grep -E "$pattern" | head -n 1 || true
  }
  archive_url="$(pick "$asset")" sums_url="$(pick "$sums_name")"
  [[ -n $archive_url && -n $sums_url ]] || fail "aucune release ${version:+$version }ne contient $asset et $sums_name."
  say "Téléchargement de $archive_url"
  curl -fL --progress-bar -o "$work/$asset" "$archive_url" || fail "téléchargement échoué."
  curl -fsSL -o "$work/$sums_name" "$sums_url" || fail "téléchargement des sommes échoué."
  archive="$work/$asset" sums="$work/$sums_name"
fi
[[ -f $archive ]] || fail "archive introuvable : $archive"
if [[ -n $sums ]]; then
  expected="$(grep -E "[[:space:]]\*?$(basename "$archive")\$" "$sums" | cut -d' ' -f1 | head -n 1)"
  [[ -n $expected ]] || expected="$(grep -E "[[:space:]]\*?$asset\$" "$sums" | cut -d' ' -f1 | head -n 1)"
  [[ -n $expected ]] || fail "$asset absent du fichier de sommes."
  [[ "$(sha256 "$archive")" == "$expected" ]] || fail "somme SHA-256 incorrecte : archive corrompue ou modifiée."
  say "Somme SHA-256 vérifiée."
else
  say "Archive locale sans fichier de sommes : non vérifiée."
fi

extract="$work/extrait"
mkdir -p "$extract"
case "$archive" in
  *.zip) if [[ $os == Darwin ]]; then ditto -x -k "$archive" "$extract"; else unzip -q "$archive" -d "$extract"; fi ;;
  *) tar -xzf "$archive" -C "$extract" ;;
esac
root="$extract"
[[ -f $root/argos-daemon ]] || root="$extract/Argos"
[[ -f $root/argos-daemon ]] || fail "argos-daemon absent de l'archive."
new_version="$("$root/argos-daemon" version 2>/dev/null || true)"
say "Installation d'Argos ${new_version:-(version inconnue)}…"

# --- Installation -------------------------------------------------------------------------
service_kind="none"
if [[ $os == Darwin ]]; then
  if [[ -n ${ARGOS_INSTALL_APP_DIR:-} ]]; then app_dir="$ARGOS_INSTALL_APP_DIR"
  elif [[ -w /Applications ]]; then app_dir="/Applications"
  else app_dir="$HOME/Applications"; fi
  bin_dir="${ARGOS_INSTALL_BIN_DIR:-$HOME/Library/Application Support/Argos/bin}"
  app="$app_dir/Argos.app"
  mkdir -p "$bin_dir"
  # Archive non notariée : sans ça, Gatekeeper refuse d'ouvrir l'app.
  xattr -dr com.apple.quarantine "$root" 2>/dev/null || true
  if [[ -z $node_name ]]; then
    mkdir -p "$app_dir"
    rm -rf "$app_dir/.Argos.app.new"
    ditto "$root/Argos.app" "$app_dir/.Argos.app.new"
    rm -rf "$app"
    mv "$app_dir/.Argos.app.new" "$app"
  else
    app=""
  fi
  [[ $service -eq 1 ]] && service_kind="launchd"
else
  bin_dir="${ARGOS_INSTALL_BIN_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/argos}"
  app="$bin_dir/Argos.AppImage"
  mkdir -p "$bin_dir"
  if [[ -n $node_name ]]; then
    app=""
  else
    install -m 755 "$root/Argos.AppImage" "$app.new" && mv "$app.new" "$app"
  fi
  for file in lancer-argos.sh LISEZ-MOI.txt; do
    [[ -f $root/$file ]] && install -m "$([[ $file == *.sh ]] && echo 755 || echo 644)" "$root/$file" "$bin_dir/$file"
  done
  if [[ $service -eq 1 ]] && command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
    service_kind="systemd"
  fi
fi
daemon="$bin_dir/argos-daemon"
install -m 755 "$root/argos-daemon" "$daemon.new"
mv "$daemon.new" "$daemon"

# Où sont l'app et le service : lu par `argos-daemon update` pour les mises à jour.
{
  echo "# Écrit par install.sh : lu par « argos-daemon update » pour remplacer l'app et relancer le service."
  [[ -n $app ]] && echo "app = \"$app\""
  echo "service = \"$service_kind\""
  echo "label = \"$LABEL\""
} >"$bin_dir/install.toml"

if [[ $os == Linux && -z $node_name ]]; then
  mkdir -p "$HOME/.local/bin" "${XDG_DATA_HOME:-$HOME/.local/share}/applications"
  ln -sf "$daemon" "$HOME/.local/bin/argos-daemon"
  cat >"${XDG_DATA_HOME:-$HOME/.local/share}/applications/argos.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Argos
Comment=Assistant personnel
Exec="$bin_dir/lancer-argos.sh"
Terminal=false
Categories=Utility;
EOF
  if ! ldconfig -p 2>/dev/null | grep -q libfuse.so.2; then
    say "FUSE 2 semble absent : l'AppImage demande « sudo apt install libfuse2 » (libfuse2t64 sur Ubuntu 24.04)."
  fi
fi

# Configuration créée, ou migrée vers le format de cette version.
"$daemon" config init || fail "configuration illisible : $ARGOS_HOME/config.toml"
if [[ -n $node_name ]]; then
  [[ $os == Linux ]] && { mkdir -p "$HOME/.local/bin"; ln -sf "$daemon" "$HOME/.local/bin/argos-daemon"; }
  "$daemon" config set node.name "\"$node_name\"" >/dev/null
  "$daemon" config set node.assistant false >/dev/null
fi

# --- Service ------------------------------------------------------------------------------
if [[ $service_kind == launchd ]]; then
  plist="$HOME/Library/LaunchAgents/$LABEL.plist"
  mkdir -p "$HOME/Library/LaunchAgents"
  "$daemon" service launchd hub | sed "s|<string>argos-hub</string>|<string>$LABEL</string>|" >"$plist"
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$plist"
  say "Daemon installé en service launchd ($LABEL)."
elif [[ $service_kind == systemd ]]; then
  unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  mkdir -p "$unit_dir"
  "$daemon" service systemd hub >"$unit_dir/$LABEL.service"
  systemctl --user daemon-reload
  systemctl --user enable "$LABEL" >/dev/null 2>&1 || true
  systemctl --user restart "$LABEL"
  say "Daemon installé en service systemd ($LABEL)."
else
  say "Daemon sans service : l'app le démarre (lanceur) ; « argos-daemon hub » le lance à la main."
fi

if [[ -n $node_name ]]; then
  say "Nœud « $node_name » installé : daemon $daemon, données $ARGOS_HOME."
  for _ in $(seq 1 40); do [[ -s $ARGOS_HOME/node-token ]] && break; sleep 0.25; done
  host="$(hostname -s 2>/dev/null || hostname)"
  echo
  echo "Pour le relier, ajouter sur un autre nœud (dans son config.toml), puis le relancer :"
  echo "  [peers.$node_name]"
  echo "  ssh = \"$host\"                 # hôte de ~/.ssh/config qui joint cette machine"
  echo "  token_file = \"~/.argos/nodes/$node_name.token\""
  echo "et y copier le jeton de ce nœud : $ARGOS_HOME/node-token"
  [[ $os == Linux && $service_kind == systemd ]] && \
    echo "Pour qu'il tourne sans session ouverte : sudo loginctl enable-linger $USER"
else
  say "Argos est installé : app $app, daemon $daemon, données $ARGOS_HOME."
fi

# --- Configuration ------------------------------------------------------------------------
configure_by_hand() {
  local engines=() engine model
  command -v claude >/dev/null && engines+=(claude)
  command -v codex >/dev/null && engines+=(codex)
  if [[ ${#engines[@]} -eq 0 ]]; then
    say "Ni claude ni codex n'est installé. Installez-en un puis authentifiez-le :"
    echo "  npm install -g @anthropic-ai/claude-code && claude"
    echo "  npm install -g @openai/codex && codex login"
    engine="$(ask "Moteur de l'Assistant (claude ou codex)" claude)"
  elif [[ ${#engines[@]} -eq 1 ]]; then
    engine="${engines[0]}"
  else
    engine="$(ask "Moteur de l'Assistant (claude ou codex)" claude)"
  fi
  case "$engine" in
    codex) model="gpt-6.1-sol"
      codex login status >/dev/null 2>&1 || say "Codex n'est pas authentifié : lancez « codex login »." ;;
    *) engine="claude" model="claude-opus-5-5"
      command -v claude >/dev/null && say "Vérifiez que « claude » démarre et est connecté (commande /login au besoin)." ;;
  esac
  "$daemon" config set assistant.engine "$engine" >/dev/null
  "$daemon" config set assistant.model "$model" >/dev/null
  say "Assistant : $engine ($model)."
  local wake
  wake="$(ask "Mot de déclenchement en écoute mains libres (vide : aucun)" Argos)"
  "$daemon" config set voice.wake_word "\"$wake\"" >/dev/null 2>&1 \
    || echo "  (mot de déclenchement à régler dans [voice] une fois la voix installée)"
  echo
  echo "Reste à faire, si vous le souhaitez (README, section Installation) :"
  echo "  - la voix locale : whisper-server et un modèle ggml (brew install whisper-cpp sur macOS), puis [voice] ;"
  echo "  - Telegram : docs/TELEGRAM.md, section [channels.telegram]."
}

if [[ $setup -eq 1 ]]; then
  ai=""
  if [[ $use_ai -eq 1 && $tty_ok -eq 1 ]]; then
    for candidate in claude codex; do
      command -v "$candidate" >/dev/null && { ai="$candidate"; break; }
    done
  fi
  if [[ -n $ai ]] && yes_no "Confier la configuration à $ai (moteur, voix, mot de déclenchement, Telegram) ?" o; then
    prompt="$("$daemon" setup prompt)"
    say "$ai prend la main ; quittez-le quand la configuration est terminée."
    "$ai" "$prompt" </dev/tty || say "$ai s'est arrêté ; relancez ce script avec --no-ai pour la configuration guidée."
  else
    configure_by_hand
  fi
  case "$service_kind" in
    launchd) launchctl kickstart -k "gui/$(id -u)/$LABEL" ;;
    systemd) systemctl --user restart "$LABEL" ;;
  esac
fi

if [[ $os == Darwin && -z $node_name ]]; then
  echo
  echo "Micro : au premier appui sur Alt+Espace, macOS demande l'accès au micro pour Argos."
  echo "Les versions ne sont pas signées par Apple : après une mise à jour, macOS peut oublier"
  echo "cette permission. Réglages Système → Confidentialité et sécurité → Microphone : retirer"
  echo "Argos puis le réautoriser ; l'app réessaie le micro toutes les 15 s."
fi
echo
echo "Mises à jour : « $daemon update » cherche une nouvelle version, « $daemon update apply »"
echo "l'installe ; la console les propose aussi (Réglages → Mises à jour)."

if [[ $launch -eq 1 ]]; then
  if [[ $os == Darwin ]]; then
    [[ $service_kind == launchd ]] || nohup "$daemon" hub >"$ARGOS_HOME/daemon.log" 2>&1 </dev/null &
    open "$app"
  elif [[ -n ${DISPLAY:-}${WAYLAND_DISPLAY:-} ]]; then
    nohup "$bin_dir/lancer-argos.sh" >/dev/null 2>&1 </dev/null &
  fi
fi
