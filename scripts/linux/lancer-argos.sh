#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
log="${TMPDIR:-/tmp}/argos-daemon.log"
up() { (exec 3<>/dev/tcp/127.0.0.1/7878) 2>/dev/null; }
if ! up; then
  nohup "$root/argos-daemon" hub >"$log" 2>&1 </dev/null &
  for ((i=0; i<120; i++)); do
    up && break
    sleep 0.25
  done
fi
if ! up; then
  echo "Daemon indisponible : consulter $log"
  exit 1
fi
exec "$root/Argos.AppImage" "$@"
