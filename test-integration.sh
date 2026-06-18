#!/usr/bin/env bash
# test-integration.sh — reset the local .objectiveai integration sandbox to a
# clean slate, keeping the expensive-to-rebuild bits.
#
#   1. If the objectiveai host binary is present, run `kill-all` so it stops
#      any servers it left running (they'd otherwise hold files open).
#   2. Wipe .objectiveai/bin/ down to the keepers — the `plugins` and `pg-bin`
#      dirs (more may be added later), plus any .zip sitting DIRECTLY in bin/.
#      Everything else goes: host binaries, other dirs, and zips nested inside
#      those removed dirs.
#   3. Delete the state folder (.objectiveai/state) entirely.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OAI_DIR="$REPO_ROOT/.objectiveai"
BIN_DIR="$OAI_DIR/bin"

case "$(uname -s)" in
  CYGWIN*|MINGW*|MSYS*) EXE=".exe" ;;
  *)                    EXE=""     ;;
esac

# 1. Stop any running host servers.
HOST="$BIN_DIR/objectiveai$EXE"
if [ -x "$HOST" ]; then
  echo "==> objectiveai kill-all"
  "$HOST" kill-all || true
fi

# 2. Clean bin/ down to the keepers.
if [ -d "$BIN_DIR" ]; then
  shopt -s nullglob
  for entry in "$BIN_DIR"/*; do
    name="$(basename "$entry")"
    case "$name" in
      plugins|pg-bin) continue ;;   # keepers (more may be added later)
    esac
    # Keep a .zip sitting directly in bin/; zips nested in removed dirs go.
    if [ -f "$entry" ] && [ "$name" != "${name%.zip}" ]; then
      continue
    fi
    rm -rf "$entry"
  done
  shopt -u nullglob
fi

# 3. Delete the state folder entirely.
rm -rf "$OAI_DIR/state"

echo "==> test-integration.sh: sandbox reset"
