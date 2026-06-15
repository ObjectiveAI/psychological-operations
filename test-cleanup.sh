#!/usr/bin/env bash
# test-cleanup.sh — tear down integration-test runtime state.
#
# Runs the objectiveai host binary straight out of .objectiveai/bin and,
# IN PARALLEL, kills every server tied to THIS repo's .objectiveai dir:
#   objectiveai api kill --global     (machine-wide api lock)
#   objectiveai db kill --global      (per-state db servers, all states)
#   objectiveai viewer kill --global  (per-state viewer servers, all states)
# Output and exit codes are ignored — these are best-effort. Then it wipes
# .objectiveai/state entirely so the next run starts from a clean slate.
#
# With KILL_ONLY set (non-empty), ONLY the kills run — .objectiveai/state is
# left in place so a failed test run can be inspected afterwards.
#
# Deliberately NOT `set -e`: the kills must not abort the script.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OBJECTIVEAI_DIR="$SCRIPT_DIR/.objectiveai"

ext=""
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) ext=".exe" ;; esac
BIN="$OBJECTIVEAI_DIR/bin/objectiveai${ext}"

# --- best-effort server kills (parallel; output + exit codes ignored) -----
if [ -x "$BIN" ]; then
  echo "test-cleanup: killing api / db / viewer servers"
  "$BIN" api kill --global    >/dev/null 2>&1 &
  "$BIN" db kill --global     >/dev/null 2>&1 &
  "$BIN" viewer kill --global >/dev/null 2>&1 &
  wait
else
  echo "test-cleanup: no objectiveai binary at $BIN — skipping kills"
fi

# Reap any lingering plugin process — a `mcp x-api begin` MCP server the
# host's conduit didn't tear down outlives the test and holds the plugin
# binary + its port, which breaks the next prepare's re-stage. The objectiveai
# kill commands above don't cover it (it's the plugin, not api/db/viewer), so
# kill it by exact name. Best-effort; the test runner's own binary is
# `psychological_operations_tests-*` (underscores), so it isn't matched.
echo "test-cleanup: reaping lingering plugin MCP servers"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) taskkill //IM psychological-operations.exe //F >/dev/null 2>&1 || true ;;
  *)                    pkill -x psychological-operations            >/dev/null 2>&1 || true ;;
esac

# --- state teardown (skipped under KILL_ONLY) -----------------------------
if [ -n "${KILL_ONLY:-}" ]; then
  echo "test-cleanup: KILL_ONLY set — leaving .objectiveai/state in place"
  exit 0
fi

echo "test-cleanup: removing .objectiveai/state"
rm -rf "$OBJECTIVEAI_DIR/state"

echo "test-cleanup: done"
