#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
cargo build --locked --release -p argos-daemon
(cd apps/gui && npm ci && npm run tauri build -- --bundles app)

app="$root/target/release/bundle/macos/Argos.app"
daemon="$root/target/release/argos-daemon"
if [[ ! -d "$app" || ! -f "$daemon" ]]; then
  echo "Build incomplet : application ou daemon manquant." >&2
  exit 1
fi
out="$root/target/distribution/macos-$(uname -m)"
stage="$root/target/distribution/macos-stage"
rm -rf "$stage"
mkdir -p "$out" "$stage"
ditto "$app" "$stage/Argos.app"
install -m 755 "$daemon" "$stage/argos-daemon"
install -m 755 "$root/scripts/macos/Lancer-Argos.command" "$stage/Lancer-Argos.command"
install -m 644 "$root/scripts/macos/LISEZ-MOI.txt" "$stage/LISEZ-MOI.txt"
archive="Argos-macOS-$(uname -m).zip"
(cd "$stage" && ditto -c -k --sequesterRsrc . "$out/$archive")
(cd "$out" && shasum -a 256 "$archive" > "SHA256SUMS-macos-$(uname -m).txt")
echo "Artefact : $out/$archive"
