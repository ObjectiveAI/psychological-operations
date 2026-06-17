#!/usr/bin/env bash
# build.sh — build the three psychological-operations artifacts: the CLI,
# the browser, and the viewer. Debug by default; pass --release for a
# release build. Each lands in its default output location (cargo
# `target/<profile>/`, viewer `dist/`) — no staging, no packaging.
#
# Usage:
#   bash build.sh            # debug
#   bash build.sh --release  # release
set -euo pipefail

REL=""
PROFILE="debug"
for arg in "$@"; do
  case "$arg" in
    --release) REL="--release"; PROFILE="release" ;;
    *) echo "build.sh: unknown arg: $arg (usage: build.sh [--release])" >&2; exit 1 ;;
  esac
done

cd "$(dirname "${BASH_SOURCE[0]}")"

echo "==> build.sh ($PROFILE)"

echo "==> CLI"
cargo build $REL -p psychological-operations-cli

echo "==> browser"
cargo build $REL -p psychological-operations-browser

echo "==> viewer"
( cd psychological-operations-viewer && pnpm install --frozen-lockfile && pnpm build )

echo "==> done ($PROFILE)"
