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
#   5. Unpack this host's freshly-built cli_zip (correct os/arch/version) in
#      place — into its own cli/ folder — overwriting whatever's there.
#   6. Apply the global API config the integration run needs (mcp timeout,
#      backoff) via the freshly-installed host.
#   7. Run the integration suite (cargo nextest -p psychological-operations-
#      tests) with all output captured to a timestamped log under .logs/test/,
#      then `objectiveai kill-all`, then exit 0/1 on the nextest result.
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
case "$(uname -m)" in
  x86_64|amd64)  ARCH="x86_64"  ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
if [ "$PLATFORM" = "windows" ]; then EXE=".exe"; else EXE=""; fi

# Plugin version from the manifest (build.sh stages the tree under it; version.sh
# keeps the two in sync).
VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO_ROOT/objectiveai.json" | head -1)"
[ -n "$VERSION" ] || { echo "ERROR: could not read version from objectiveai.json" >&2; exit 1; }

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

# 5. Unpack the freshly-built cli_zip for this host into its own folder,
#    overwriting whatever's there.
CLI_ZIP="$BIN_DIR/plugins/ObjectiveAI/psychological-operations/$VERSION/cli/psychological-operations-$PLATFORM-$ARCH.zip"
[ -f "$CLI_ZIP" ] || { echo "cli_zip not found: $CLI_ZIP (run build.sh first)" >&2; exit 1; }
CLI_DEST="$(dirname "$CLI_ZIP")"
echo "==> unpacking $(basename "$CLI_ZIP") into $CLI_DEST"
case "$PLATFORM" in
  windows)
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$CLI_ZIP")' -DestinationPath '$(cygpath -w "$CLI_DEST")'"
    ;;
  *)
    unzip -o -q "$CLI_ZIP" -d "$CLI_DEST"
    ;;
esac

# 6. Global API config for the integration run.
echo "==> objectiveai api config (global)"
"$HOST" api config mcp-timeout-ms set 300000 --global
"$HOST" api config backoff-max-elapsed-time-ms set 0 --global

# 7. Run the integration suite, capturing ALL output to a timestamped log.
#    Then stop the host's servers and exit 0/1 on the nextest result.
mkdir -p "$REPO_ROOT/.logs/test"
LOG="$REPO_ROOT/.logs/test/psychological-operations-tests-$(date +%Y%m%d-%H%M%S).txt"
echo "==> nextest run -p psychological-operations-tests  (log: $LOG)"
rc=0
cargo nextest run -p psychological-operations-tests > "$LOG" 2>&1 || rc=$?

echo "==> objectiveai kill-all"
"$HOST" kill-all || true

echo "==> test-integration.sh: done (nextest rc=$rc)"
if [ "$rc" -eq 0 ]; then exit 0; else exit 1; fi
