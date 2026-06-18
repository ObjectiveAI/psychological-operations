#!/usr/bin/env bash
# install.sh — install the psychological-operations plugin into an objectiveai
# dir (default $HOME/.objectiveai), the way the host's `plugins install` works.
#
# For each of cli/ and viewer/ under the plugin's version dir:
#   1. Use the release zip already sitting in that folder if present (e.g. one
#      build.sh produced); otherwise download it from the GitHub release into
#      the folder.
#   2. Delete everything ELSE in the folder (any prior unpack) — the zip stays.
#   3. Unpack the zip in place.
#
# Our integration flow builds first, so the zips are already present and it
# never downloads.
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

# platform / arch — the cli_zip is per host.
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

# install_part <folder> <zip-name>
#   Ensure <folder>/<zip-name> is present (use the local zip, else download it
#   from the GitHub release into the folder); delete everything ELSE in the
#   folder; then unpack the zip in place.
install_part() {
  local dir="$1" zipname="$2"
  mkdir -p "$dir"
  local zip="$dir/$zipname"
  if [ ! -f "$zip" ]; then
    local url="https://github.com/$REPO/releases/download/v$VERSION/$zipname"
    echo "==> fetching $zipname"
    if command -v curl >/dev/null 2>&1; then
      curl -fSL --progress-bar "$url" -o "$zip"
    elif command -v wget >/dev/null 2>&1; then
      wget -O "$zip" "$url"
    else
      echo "need curl or wget to download $zipname" >&2; return 1
    fi
  fi
  # Delete everything in the folder except the .zip(s) — i.e. a prior unpack.
  find "$dir" -mindepth 1 -maxdepth 1 -not -name '*.zip' -exec rm -rf {} +
  echo "==> unpacking $zipname into $dir"
  case "$PLATFORM" in
    windows)
      powershell.exe -NoProfile -Command \
        "Expand-Archive -Force -LiteralPath '$(cygpath -w "$zip")' -DestinationPath '$(cygpath -w "$dir")'"
      ;;
    *)
      unzip -o -q "$zip" -d "$dir"
      ;;
  esac
}

echo "==> installing v$VERSION ($PLATFORM-$ARCH) into $PLUGIN_DIR"
install_part "$PLUGIN_DIR/cli"    "psychological-operations-$PLATFORM-$ARCH.zip"
install_part "$PLUGIN_DIR/viewer" "psychological-operations-viewer.zip"

# The plugin manifest sits at the head, above cli/ + viewer/.
cp "$REPO_ROOT/objectiveai.json" "$PLUGIN_DIR/objectiveai.json"

echo "==> installed -> $PLUGIN_DIR"
