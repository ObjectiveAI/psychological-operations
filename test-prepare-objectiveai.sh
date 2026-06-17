#!/usr/bin/env bash
# test-prepare-objectiveai.sh — install the precompiled objectiveai release
# binaries for the current platform into .objectiveai/bin/.
#
# The pinned version is the constant below. A version.txt marker inside
# .objectiveai/bin/ records what's installed; the download is skipped
# whenever the marker already matches the pinned version. The individual
# binaries are intentionally NOT checked for presence, so a locally-patched
# binary (e.g. a hand-swapped api) survives a prepare as long as version.txt
# is unchanged. The committed plugins/ tree under bin/ is never touched.
#
# Release assets are bare binaries (no archive) named
#   objectiveai-<os>-<arch>[.exe]         (the host)
#   objectiveai-<os>-<arch>-api[.exe]     (the objectiveai API server)
#   objectiveai-<os>-<arch>-db[.exe]      (the per-state postgres supervisor)
#   objectiveai-<os>-<arch>-viewer[.exe]  (the viewer server)
# from github.com/ObjectiveAI/objectiveai/releases/download/v<version>/.
set -euo pipefail

# --- the pinned version ----------------------------------------------------
OBJECTIVEAI_VERSION="2.2.3"

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
# The host plus the sibling binaries it spawns out of the same bin dir:
# `objectiveai-db` (per-state postgres supervisor), `objectiveai-api`
# (the objectiveai API server), and `objectiveai-viewer` (the viewer server the host
# launches for plugin viewer bundles — needed for upcoming viewer-path
# tests).
ASSETS=(
  "objectiveai|objectiveai-${platarch}${ext}"
  "objectiveai-api|objectiveai-${platarch}-api${ext}"
  "objectiveai-db|objectiveai-${platarch}-db${ext}"
  "objectiveai-viewer|objectiveai-${platarch}-viewer${ext}"
)

# --- up-to-date check ------------------------------------------------------
# Version marker only: if version.txt matches the pinned version, skip the
# download entirely. The individual binaries are intentionally NOT checked,
# so a locally-patched binary isn't clobbered while version.txt is current.
if [ -f "$VERSION_FILE" ] && [ "$(cat "$VERSION_FILE")" = "$OBJECTIVEAI_VERSION" ]; then
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
