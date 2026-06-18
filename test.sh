#!/usr/bin/env bash
# test.sh — full integration-test cycle for psychological-operations-tests:
#
#   0. install-bin.sh              — ensure ./bin has ninja
#   1. test-prepare.sh             — fetch objectiveai + build the plugin
#   2. test-cleanup.sh             — clean slate (kill servers + wipe state)
#   3. cargo nextest run           — the whole psychological-operations-tests crate
#                                    (cargo-nextest presumed present on the host)
#   4. test-cleanup.sh (KILL_ONLY) — stop servers but KEEP state, so a failed
#                                    run's state can be inspected
#
# Extra args are forwarded to nextest (e.g. `bash test.sh full_loop`).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ninja (for the CEF browser bundle test-prepare builds) goes into ./bin via
# install-bin.sh. cargo-nextest is NOT installed here — `cargo nextest` is
# presumed already present on the host.
bash "$SCRIPT_DIR/install-bin.sh"
export PATH="$SCRIPT_DIR/bin:$PATH"

# Reap any processes leaked by a PRIOR run first — a lingering plugin MCP
# server holds the staged plugin binary, which would block test-prepare's
# re-stage. (KILL_ONLY: don't touch state here.)
KILL_ONLY=1 bash "$SCRIPT_DIR/test-cleanup.sh"

bash "$SCRIPT_DIR/test-prepare.sh"
bash "$SCRIPT_DIR/test-cleanup.sh"

# Run the tests, but capture the status so the closing cleanup always runs.
rc=0
cargo nextest run -p psychological-operations-tests "$@" || rc=$?

# Closing cleanup: kill servers but keep the state for post-mortem inspection.
KILL_ONLY=1 bash "$SCRIPT_DIR/test-cleanup.sh"

exit "$rc"
