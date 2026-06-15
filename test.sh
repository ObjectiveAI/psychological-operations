#!/usr/bin/env bash
# test.sh — full integration-test cycle for psychological-operations-tests:
#
#   1. test-prepare.sh             — fetch objectiveai + build the plugin
#   2. test-cleanup.sh             — clean slate (kill servers + wipe state)
#   3. cargo nextest run           — the whole psychological-operations-tests crate
#   4. test-cleanup.sh (KILL_ONLY) — stop servers but KEEP state, so a failed
#                                    run's state can be inspected
#
# Extra args are forwarded to nextest (e.g. `bash test.sh full_loop`).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bash "$SCRIPT_DIR/test-prepare.sh"
bash "$SCRIPT_DIR/test-cleanup.sh"

# Run the tests, but capture the status so the closing cleanup always runs.
rc=0
cargo nextest run -p psychological-operations-tests "$@" || rc=$?

# Closing cleanup: kill servers but keep the state for post-mortem inspection.
KILL_ONLY=1 bash "$SCRIPT_DIR/test-cleanup.sh"

exit "$rc"
