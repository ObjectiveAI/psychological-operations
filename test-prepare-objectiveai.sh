#!/usr/bin/env bash
# test-prepare-objectiveai.sh — install the precompiled objectiveai release
# binaries for the current platform into .objectiveai/bin/.
#
# The pinned version is the constant below. A version.txt marker inside
# .objectiveai/bin/ records what's installed; the download is skipped when
# the marker already matches AND every expected binary is present. The
# committed plugins/ tree under bin/ is never touched.
#
# Release assets are bare binaries (no archive) named
#   objectiveai-<os>-<arch>[.exe]        (the host)
#   objectiveai-<os>-<arch>-api[.exe]    (the deterministic mock API server)
# from github.com/ObjectiveAI/objectiveai/releases/download/v<version>/.
#
# `objectiveai-db` (the per-state postgres supervisor the host spawns) is
# ALSO required, but the v2.2.0 release does NOT ship it (upstream packaging
# gap — the release has cli/api/mcp/viewer, no db). So we build/copy it from
# a local objectiveai checkout instead — see `ensure_db` below.
set -euo pipefail

# --- the pinned version ----------------------------------------------------
OBJECTIVEAI_VERSION="2.2.0"
# objectiveai source checkout that provides objectiveai-db (the release
# omits it). Defaults to a sibling clone next to this repo.
OBJECTIVEAI_SRC="${OBJECTIVEAI_SRC:-}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$SCRIPT_DIR/.objectiveai/bin"
VERSION_FILE="$BIN_DIR/version.txt"
[ -n "$OBJECTIVEAI_SRC" ] || OBJECTIVEAI_SRC="$SCRIPT_DIR/../objectiveai"

# --- platform → asset suffix ----------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)                plat_os="linux" ;;
  Darwin)               plat_os="macos" ;;
  MINGW*|MSYS*|CYGWIN*) plat_os="windows" ;;
  *) echo "objectiveai: unsupported OS '$os'" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64)  plat_arch="x86_64" ;;
  arm64|aarch64) plat_arch="aarch64" ;;
  *) echo "objectiveai: unsupported arch '$arch'" >&2; exit 1 ;;
esac
ext=""
[ "$plat_os" = "windows" ] && ext=".exe"
platarch="${plat_os}-${plat_arch}"

# --- binaries to install: "<dest-name-in-bin>|<release-asset>" ------------
# The host (`objectiveai`) and the deterministic mock API server
# (`objectiveai-api`) come from the release. `objectiveai-db` is sourced
# separately (ensure_db) since the release omits it. `objectiveai-viewer`
# is skipped — the suite never opens the viewer.
ASSETS=(
  "objectiveai|objectiveai-${platarch}${ext}"
  "objectiveai-api|objectiveai-${platarch}-api${ext}"
)

# Build/copy objectiveai-db from a local objectiveai checkout (the release
# doesn't ship it). Idempotent: a prebuilt binary in the source tree's
# target/{debug,release} is copied as-is; otherwise it's `cargo build`-ed.
ensure_db() {
  local dest="$BIN_DIR/objectiveai-db${ext}"
  [ -f "$dest" ] && return 0
  if [ ! -d "$OBJECTIVEAI_SRC" ]; then
    echo "objectiveai: objectiveai-db is missing from the v$OBJECTIVEAI_VERSION release;" >&2
    echo "  build it from an objectiveai checkout. Set OBJECTIVEAI_SRC=<path> (looked in" >&2
    echo "  '$OBJECTIVEAI_SRC')." >&2
    exit 1
  fi
  local prebuilt=""
  for p in "$OBJECTIVEAI_SRC/target/release/objectiveai-db${ext}" \
           "$OBJECTIVEAI_SRC/target/debug/objectiveai-db${ext}"; do
    [ -f "$p" ] && { prebuilt="$p"; break; }
  done
  if [ -z "$prebuilt" ]; then
    echo "objectiveai: building objectiveai-db from $OBJECTIVEAI_SRC"
    ( cd "$OBJECTIVEAI_SRC" && cargo build -p objectiveai-db )
    prebuilt="$OBJECTIVEAI_SRC/target/debug/objectiveai-db${ext}"
  fi
  [ -f "$prebuilt" ] || { echo "objectiveai: objectiveai-db build produced no binary" >&2; exit 1; }
  cp "$prebuilt" "$dest"
  [ "$plat_os" = "windows" ] || chmod +x "$dest"
  echo "objectiveai: installed objectiveai-db from $prebuilt"
}

# --- up-to-date check ------------------------------------------------------
up_to_date=1
if [ -f "$VERSION_FILE" ] && [ "$(cat "$VERSION_FILE")" = "$OBJECTIVEAI_VERSION" ]; then
  for entry in "${ASSETS[@]}"; do
    dest="${entry%%|*}"
    [ -f "$BIN_DIR/${dest}${ext}" ] || up_to_date=0
  done
  [ -f "$BIN_DIR/objectiveai-db${ext}" ] || up_to_date=0
else
  up_to_date=0
fi
if [ "$up_to_date" = "1" ]; then
  echo "objectiveai: v$OBJECTIVEAI_VERSION already installed in $BIN_DIR"
  exit 0
fi

# --- download --------------------------------------------------------------
mkdir -p "$BIN_DIR"
base_url="https://github.com/ObjectiveAI/objectiveai/releases/download/v$OBJECTIVEAI_VERSION"
for entry in "${ASSETS[@]}"; do
  dest="${entry%%|*}"
  asset="${entry#*|}"
  out="$BIN_DIR/${dest}${ext}"
  url="$base_url/$asset"
  echo "objectiveai: downloading $asset"
  if ! curl -fSL --retry 3 -o "$out" "$url"; then
    echo "objectiveai: FAILED to download $url" >&2
    exit 1
  fi
  [ "$plat_os" = "windows" ] || chmod +x "$out"
done

# objectiveai-db (not in the release) — build/copy from source.
ensure_db

# Stamp the marker only after every binary is in place.
echo "$OBJECTIVEAI_VERSION" > "$VERSION_FILE"
echo "objectiveai: installed v$OBJECTIVEAI_VERSION ($platarch) in $BIN_DIR"
