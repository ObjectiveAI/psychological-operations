#!/usr/bin/env bash
# test-integration.sh — reset the local .objectiveai integration sandbox
# (keeping the expensive-to-rebuild bits), reinstall the host + the freshly
# built plugin, then run the integration suite.
#
#   1. If the objectiveai host binary is present, run `kill-all` so it stops
#      any servers it left running (they'd otherwise hold files open).
#   2. Wipe .objectiveai/bin/ down to the keepers — the `plugins` and `pg-bin`
#      dirs (more may be added later), plus any .zip sitting DIRECTLY in bin/.
#      Everything else goes: host binaries, other dirs, and zips nested inside
#      those removed dirs.
#   3. Delete the state folder (.objectiveai/state) entirely.
#   4. (Re)install the objectiveai host via the upstream curl installer,
#      pointed at our .objectiveai (--objectiveai-dir) and told not to touch
#      PATH / shell rc (--no-export-path).
#   5. Install the freshly-built plugin (cli + viewer) into the sandbox via
#      install.sh (the zips are already present, so it just unpacks in place).
#   6. Apply the global API config the integration run needs (mcp timeout,
#      backoff) via the freshly-installed host.
#   7. Prebuild the integration test binaries (logged under .logs/build/, the
#      same shape as the unit-test prebuild), then run the suite (cargo nextest
#      -p psychological-operations-tests) with all output captured to a
#      timestamped log under .logs/test/, then `objectiveai kill-all`, then
#      exit 0/1 on the nextest result.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OAI_DIR="$REPO_ROOT/.objectiveai"
BIN_DIR="$OAI_DIR/bin"

case "$(uname -s)" in
  Linux*)               PLATFORM="linux"   ;;
  Darwin*)              PLATFORM="macos"   ;;
  CYGWIN*|MINGW*|MSYS*) PLATFORM="windows" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
if [ "$PLATFORM" = "windows" ]; then EXE=".exe"; else EXE=""; fi

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

# 4. (Re)install the objectiveai host into our .objectiveai dir, without
#    touching PATH / shell rc.
echo "==> installing objectiveai host into $OAI_DIR"
curl -fsSL https://raw.githubusercontent.com/ObjectiveAI/objectiveai/main/install.sh \
  | bash -s -- --no-export-path --objectiveai-dir "$OAI_DIR"

# 5. Install the freshly-built plugin (cli + viewer) into the sandbox. The zips
#    are already in the tree (build.sh produced them), so install.sh just
#    cleans + unpacks them in place — no download.
echo "==> installing the built plugin into $OAI_DIR"
bash "$REPO_ROOT/install.sh" --dir "$OAI_DIR"

# 6. Global API config for the integration run.
echo "==> objectiveai api config (global)"
"$HOST" api config mcp-timeout-ms set 300000 --global
"$HOST" api config backoff-max-elapsed-time-ms set 0 --global

# 7. Prebuild the test binaries first (logged to .logs/build, same shape as the
#    unit-test prebuild) so the run only executes; then run the suite, capturing
#    ALL output to a timestamped log. Then stop the host's servers and exit 0/1.
ts="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$REPO_ROOT/.logs/build" "$REPO_ROOT/.logs/test"
BUILD_LOG="$REPO_ROOT/.logs/build/psychological-operations-tests-nextest-$ts.txt"
echo "==> prebuild psychological-operations-tests  (log: $BUILD_LOG)"
if ! cargo nextest run --no-run -p psychological-operations-tests > "$BUILD_LOG" 2>&1; then
  echo "==> test-integration.sh: test build failed (see $BUILD_LOG)" >&2
  "$HOST" kill-all || true
  exit 1
fi

LOG="$REPO_ROOT/.logs/test/psychological-operations-tests-$ts.txt"
echo "==> nextest run -p psychological-operations-tests  (log: $LOG)"
rc=0
cargo nextest run -p psychological-operations-tests > "$LOG" 2>&1 || rc=$?

echo "==> objectiveai kill-all"
"$HOST" kill-all || true

echo "==> test-integration.sh: done (nextest rc=$rc)"
if [ "$rc" -eq 0 ]; then exit 0; else exit 1; fi
