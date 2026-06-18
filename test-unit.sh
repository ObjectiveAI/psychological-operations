#!/usr/bin/env bash
# test-unit.sh — run the unit tests of every workspace crate EXCEPT the
# integration-test crate (psychological-operations-tests), via nextest.
#
# Two phases (mirroring objectiveai's):
#   1. PREBUILD each crate's test binaries ONE AT A TIME via `nextest --no-run`,
#      capturing per-crate output to .logs/build/<crate>-nextest-<ts>.txt — so
#      the run phase doesn't rebuild concurrently against the shared target dir
#      (which would oversubscribe it). Every crate is attempted; any build
#      failure aborts before the run phase.
#   2. RUN the now build-free suites in PARALLEL, each crate's output to
#      .logs/test/<crate>-<ts>.txt.
# Exits 0 only if every crate passed.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# ninja lives in ./bin (provisioned by build-cli.sh); the browser crate's CEF
# build needs it on PATH.
export PATH="$REPO_ROOT/bin:$PATH"

CRATES=(
  psychological-operations-db
  psychological-operations-sdk
  psychological-operations-cli
  psychological-operations-x-api-mcp
  psychological-operations-browser
)

# One timestamp for the whole run, so a run's logs sort together.
ts="$(date +%Y%m%d-%H%M%S)"
BUILD_LOG_DIR="$REPO_ROOT/.logs/build"
TEST_LOG_DIR="$REPO_ROOT/.logs/test"
mkdir -p "$BUILD_LOG_DIR" "$TEST_LOG_DIR"

# ── Phase 1: prebuild the test binaries, one crate at a time ─────────
prebuild_failed=0
for crate in "${CRATES[@]}"; do
  log="$BUILD_LOG_DIR/$crate-nextest-$ts.txt"
  echo "==> prebuild $crate  (log: $log)"
  if ! cargo nextest run --no-run -p "$crate" > "$log" 2>&1; then
    echo "    BUILD FAILED $crate (see .logs/build/$crate-nextest-$ts.txt)" >&2
    prebuild_failed=1
  fi
done
if [ "$prebuild_failed" -ne 0 ]; then
  echo "==> test-unit.sh: one or more test builds failed; aborting" >&2
  exit 1
fi

# ── Phase 2: run each crate's suite, all in parallel (build-free now) ─
pids=()
pid_crates=()
for crate in "${CRATES[@]}"; do
  log="$TEST_LOG_DIR/$crate-$ts.txt"
  echo "==> nextest run -p $crate  (log: $log)"
  cargo nextest run --no-tests=pass -p "$crate" > "$log" 2>&1 &
  pids+=("$!")
  pid_crates+=("$crate")
done

overall=0
for i in "${!pids[@]}"; do
  if wait "${pids[$i]}"; then
    echo "    PASS ${pid_crates[$i]}"
  else
    echo "    FAIL ${pid_crates[$i]}"
    overall=1
  fi
done

echo "==> test-unit.sh: $([ "$overall" -eq 0 ] && echo 'all crates passed' || echo 'FAILURES — see logs')"
exit "$overall"
