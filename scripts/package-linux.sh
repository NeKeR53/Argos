#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
cargo build --locked --release -p argos-daemon
(cd apps/gui && npm ci && npm run tauri build -- --bundles appimage)

appimage="$(ls "$root"/target/release/bundle/appimage/*.AppImage 2>/dev/null | head -n 1)"
daemon="$root/target/release/argos-daemon"
if [[ ! -f "$appimage" || ! -f "$daemon" ]]; then
  echo "Build incomplet : application ou daemon manquant." >&2
  exit 1
fi
arch="$(uname -m)"
out="$root/target/distribution/linux-$arch"
stage="$root/target/distribution/linux-stage/Argos"
rm -rf "$root/target/distribution/linux-stage"
mkdir -p "$out" "$stage"
install -m 755 "$appimage" "$stage/Argos.AppImage"
install -m 755 "$daemon" "$stage/argos-daemon"
install -m 755 "$root/scripts/linux/lancer-argos.sh" "$stage/lancer-argos.sh"
install -m 644 "$root/scripts/linux/LISEZ-MOI.txt" "$stage/LISEZ-MOI.txt"
archive="Argos-Linux-$arch.tar.gz"
tar -C "$root/target/distribution/linux-stage" -czf "$out/$archive" Argos
(cd "$out" && sha256sum "$archive" > "SHA256SUMS-linux-$arch.txt")
echo "Artefact : $out/$archive"
