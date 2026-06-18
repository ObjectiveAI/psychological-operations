#!/usr/bin/env bash
# test-unit.sh — run the unit tests of every workspace crate EXCEPT the
# integration-test crate (psychological-operations-tests), via nextest. Each
# crate's full output goes to .logs/test/<crate>-<timestamp>.txt. Exits 0 only
# if every crate passed (each is run + judged independently).
set -euo pipefail

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

mkdir -p "$REPO_ROOT/.logs/test"
ts="$(date +%Y%m%d-%H%M%S)"

overall=0
for crate in "${CRATES[@]}"; do
  log="$REPO_ROOT/.logs/test/$crate-$ts.txt"
  echo "==> nextest run -p $crate  (log: $log)"
  rc=0
  cargo nextest run -p "$crate" --no-tests=pass > "$log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "    PASS $crate"
  else
    echo "    FAIL $crate (rc=$rc)"
    overall=1
  fi
done

echo "==> test-unit.sh: $([ "$overall" -eq 0 ] && echo 'all crates passed' || echo 'FAILURES — see logs')"
exit "$overall"
