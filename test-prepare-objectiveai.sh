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
set -euo pipefail

# --- the pinned version ----------------------------------------------------
OBJECTIVEAI_VERSION="2.2.0"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$SCRIPT_DIR/.objectiveai/bin"
VERSION_FILE="$BIN_DIR/version.txt"

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
# Add more entries here (e.g. the viewer host binary) as the tests need them.
ASSETS=(
  "objectiveai|objectiveai-${platarch}${ext}"
  "objectiveai-api|objectiveai-${platarch}-api${ext}"
)

# --- up-to-date check ------------------------------------------------------
up_to_date=1
if [ -f "$VERSION_FILE" ] && [ "$(cat "$VERSION_FILE")" = "$OBJECTIVEAI_VERSION" ]; then
  for entry in "${ASSETS[@]}"; do
    dest="${entry%%|*}"
    [ -f "$BIN_DIR/${dest}${ext}" ] || up_to_date=0
  done
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

# Stamp the marker only after every binary downloaded cleanly.
echo "$OBJECTIVEAI_VERSION" > "$VERSION_FILE"
echo "objectiveai: installed v$OBJECTIVEAI_VERSION ($platarch) in $BIN_DIR"
