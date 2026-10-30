#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
if ! /usr/bin/nc -z 127.0.0.1 7878 2>/dev/null; then
  nohup "$root/argos-daemon" hub >"${TMPDIR:-/tmp}/argos-daemon.log" 2>&1 </dev/null &
  for ((i=0; i<120; i++)); do
    /usr/bin/nc -z 127.0.0.1 7878 2>/dev/null && break
    sleep 0.25
  done
fi
if ! /usr/bin/nc -z 127.0.0.1 7878 2>/dev/null; then
  echo "Daemon indisponible : consulter ${TMPDIR:-/tmp}/argos-daemon.log"
  exit 1
fi
/usr/bin/open "$root/Argos.app"
