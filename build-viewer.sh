#!/usr/bin/env bash
# build-viewer.sh — build the viewer release output into
# psychological-operations-viewer/dist/ (the vite build). Accepts --release
# for symmetry with the other legs (vite build is a production build either
# way).
#
# Self-contained: it installs the viewer's node deps first. The root build.sh
# is what zips dist/ afterwards. Build output is captured to
# .logs/build/psychological-operations-viewer-<ts>.txt (same shape as the test
# logs); build.sh exports a shared BUILD_TS so a run's logs sort together.
set -euo pipefail

for arg in "$@"; do
  case "$arg" in
    --release) ;;  # vite build is production regardless
    *) echo "build-viewer.sh: unknown arg: $arg (usage: build-viewer.sh [--release])" >&2; exit 1 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_LOG_DIR="$REPO_ROOT/.logs/build"
mkdir -p "$BUILD_LOG_DIR"
BUILD_TS="${BUILD_TS:-$(date +%Y%m%d-%H%M%S)}"
LOG="$BUILD_LOG_DIR/psychological-operations-viewer-$BUILD_TS.txt"

echo "==> build-viewer.sh: psychological-operations-viewer  (log: $LOG)"
rc=0
( cd "$REPO_ROOT/psychological-operations-viewer" && pnpm install --frozen-lockfile && pnpm build ) > "$LOG" 2>&1 || rc=$?
if [ "$rc" -eq 0 ]; then
  echo "    SUCCESS psychological-operations-viewer"
else
  echo "    ERROR psychological-operations-viewer (see $LOG)" >&2
  exit "$rc"
fi
echo "==> build-viewer.sh done"
