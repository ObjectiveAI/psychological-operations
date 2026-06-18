#!/usr/bin/env bash
# install.sh — install the psychological-operations plugin into an objectiveai
# dir (default $HOME/.objectiveai), the way the host's `plugins install` works.
#
# Re-uses what's already in the plugin's version dir ONLY if all three
# artifacts are present — the cli_zip (in cli/), the viewer_zip (in viewer/),
# and objectiveai.json (at the head). If ANY is missing, all three are
# downloaded from the GitHub release. Then each folder is stripped of
# everything but its zip and the zip is unpacked in place.
#
# Our integration flow builds first (build.sh writes all three), so it re-uses
# and never downloads.
#
# Usage:
#   bash install.sh [--dir <objectiveai-dir>]   # --dir defaults to ~/.objectiveai
set -euo pipefail

DIR="$HOME/.objectiveai"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dir)   DIR="$2"; shift 2 ;;
    --dir=*) DIR="${1#--dir=}"; shift ;;
    *) echo "install.sh: unknown arg: $1 (usage: install.sh [--dir <dir>])" >&2; exit 1 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="ObjectiveAI/psychological-operations"

case "$(uname -s)" in
  Linux*)               PLATFORM="linux"   ;;
  Darwin*)              PLATFORM="macos"   ;;
  CYGWIN*|MINGW*|MSYS*) PLATFORM="windows" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  ARCH="x86_64"  ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO_ROOT/objectiveai.json" | head -1)"
[ -n "$VERSION" ] || { echo "ERROR: could not read version from objectiveai.json" >&2; exit 1; }

PLUGIN_DIR="$DIR/bin/plugins/ObjectiveAI/psychological-operations/$VERSION"
CLI_DIR="$PLUGIN_DIR/cli"
VIEWER_DIR="$PLUGIN_DIR/viewer"
CLI_ZIP_NAME="psychological-operations-$PLATFORM-$ARCH.zip"
VIEWER_ZIP_NAME="psychological-operations-viewer.zip"
RELEASE_URL="https://github.com/$REPO/releases/download/v$VERSION"

download() {  # download <url> <dest>
  echo "==> fetching $(basename "$2")"
  if command -v curl >/dev/null 2>&1; then
    curl -fSL --progress-bar "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$2" "$1"
  else
    echo "need curl or wget to download $(basename "$2")" >&2; return 1
  fi
}

# Re-use the existing artifacts only if ALL THREE are present (both zips + the
# manifest); otherwise fetch all three from the release.
if [ -f "$CLI_DIR/$CLI_ZIP_NAME" ] \
   && [ -f "$VIEWER_DIR/$VIEWER_ZIP_NAME" ] \
   && [ -f "$PLUGIN_DIR/objectiveai.json" ]; then
  echo "==> reusing existing artifacts in $PLUGIN_DIR"
else
  echo "==> fetching v$VERSION artifacts into $PLUGIN_DIR"
  mkdir -p "$CLI_DIR" "$VIEWER_DIR"
  download "$RELEASE_URL/$CLI_ZIP_NAME"    "$CLI_DIR/$CLI_ZIP_NAME"
  download "$RELEASE_URL/$VIEWER_ZIP_NAME" "$VIEWER_DIR/$VIEWER_ZIP_NAME"
  download "$RELEASE_URL/objectiveai.json" "$PLUGIN_DIR/objectiveai.json"
fi

# Per folder: delete everything but the zip(s), then unpack the zip in place.
unpack() {  # unpack <folder> <zip-name>
  local dir="$1" zipname="$2"
  find "$dir" -mindepth 1 -maxdepth 1 -not -name '*.zip' -exec rm -rf {} +
  echo "==> unpacking $zipname into $dir"
  case "$PLATFORM" in
    windows)
      powershell.exe -NoProfile -Command \
        "Expand-Archive -Force -LiteralPath '$(cygpath -w "$dir/$zipname")' -DestinationPath '$(cygpath -w "$dir")'"
      ;;
    *)
      unzip -o -q "$dir/$zipname" -d "$dir"
      ;;
  esac
}

echo "==> installing v$VERSION ($PLATFORM-$ARCH) into $PLUGIN_DIR"
unpack "$CLI_DIR"    "$CLI_ZIP_NAME"
unpack "$VIEWER_DIR" "$VIEWER_ZIP_NAME"

echo "==> installed -> $PLUGIN_DIR"
