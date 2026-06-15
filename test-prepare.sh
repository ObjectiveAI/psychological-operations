#!/usr/bin/env bash
# test-prepare.sh — prepare the integration-test environment.
#
# Runs the two prep steps IN PARALLEL and waits for both:
#   1. test-prepare-objectiveai.sh — fetch the pinned objectiveai release
#      binaries for this platform into .objectiveai/bin/.
#   2. test-prepare-build.sh — build the local psychological-operations
#      plugin (debug CLI + viewer) into the committed plugin tree under
#      .objectiveai/bin/plugins/.
#
# Exit non-zero if either step fails.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "test-prepare: starting (objectiveai fetch + plugin build, in parallel)"

bash "$SCRIPT_DIR/test-prepare-objectiveai.sh" &
oai_pid=$!
bash "$SCRIPT_DIR/test-prepare-build.sh" &
build_pid=$!

oai_rc=0
build_rc=0
wait "$oai_pid"   || oai_rc=$?
wait "$build_pid" || build_rc=$?

if [ "$oai_rc" -ne 0 ]; then
  echo "test-prepare: objectiveai step FAILED (exit $oai_rc)" >&2
fi
if [ "$build_rc" -ne 0 ]; then
  echo "test-prepare: build step FAILED (exit $build_rc)" >&2
fi
if [ "$oai_rc" -ne 0 ] || [ "$build_rc" -ne 0 ]; then
  exit 1
fi

echo "test-prepare: done"
