#!/usr/bin/env bash
# test.sh — run the test suites. Runs both by default; --no-unit and
# --no-integration skip the respective one. Exits 0 only if every suite that
# ran exited 0, else 1.
#
# Each sub-script (test-unit.sh, test-integration.sh) is fully self-contained
# — this just orchestrates them.
set -euo pipefail

NO_UNIT=0
NO_INTEGRATION=0
for arg in "$@"; do
  case "$arg" in
    --no-unit)        NO_UNIT=1 ;;
    --no-integration) NO_INTEGRATION=1 ;;
    *) echo "test.sh: unknown arg: $arg (usage: test.sh [--no-unit] [--no-integration])" >&2; exit 1 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

overall=0

if [ "$NO_UNIT" = "0" ]; then
  echo "==> test.sh: unit"
  bash "$REPO_ROOT/test-unit.sh" || overall=1
fi

if [ "$NO_INTEGRATION" = "0" ]; then
  echo "==> test.sh: integration"
  bash "$REPO_ROOT/test-integration.sh" || overall=1
fi

echo "==> test.sh: $([ "$overall" -eq 0 ] && echo PASS || echo FAIL)"
exit "$overall"
