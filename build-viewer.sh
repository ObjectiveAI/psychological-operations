#!/usr/bin/env bash
# build-viewer.sh — build the viewer release output into
# psychological-operations-viewer/dist/ (the vite build). Accepts --release
# for symmetry with the other legs (vite build is a production build either
# way).
#
# Self-contained: it installs the viewer's node deps first. The root build.sh
# is what zips dist/ afterwards.
set -euo pipefail

for arg in "$@"; do
  case "$arg" in
    --release) ;;  # vite build is production regardless
    *) echo "build-viewer.sh: unknown arg: $arg (usage: build-viewer.sh [--release])" >&2; exit 1 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "==> build-viewer.sh"
( cd "$REPO_ROOT/psychological-operations-viewer" && pnpm install --frozen-lockfile && pnpm build )
echo "==> build-viewer.sh done"
