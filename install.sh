#!/usr/bin/env bash
# install.sh — install the built psychological-operations plugin into an
# objectiveai dir. Assumes build.sh has already run: it reads the release zips
# build.sh produced under the repo's .objectiveai/ and unpacks them into the
# target dir's plugin tree, exactly like the host's `plugins install` would.
#
#   <dir>/bin/plugins/ObjectiveAI/psychological-operations/<version>/
#     objectiveai.json
#     cli/      <- unpacked cli_zip   (CLI binary + browser CEF runtime)
#     viewer/   <- unpacked viewer_zip
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

PLUGIN_REL="bin/plugins/ObjectiveAI/psychological-operations/$VERSION"
SRC="$REPO_ROOT/.objectiveai/$PLUGIN_REL"
DEST="$DIR/$PLUGIN_REL"
CLI_ZIP="$SRC/cli/psychological-operations-$PLATFORM-$ARCH.zip"
VIEWER_ZIP="$SRC/viewer/psychological-operations-viewer.zip"
[ -f "$CLI_ZIP" ]    || { echo "cli_zip not found: $CLI_ZIP (run build.sh first)" >&2; exit 1; }
[ -f "$VIEWER_ZIP" ] || { echo "viewer_zip not found: $VIEWER_ZIP (run build.sh first)" >&2; exit 1; }

echo "==> installing v$VERSION ($PLATFORM-$ARCH) into $DEST"

# Clear the existing cli/ + viewer/ folders first, then recreate.
rm -rf "$DEST/cli" "$DEST/viewer"
mkdir -p "$DEST/cli" "$DEST/viewer"

# The plugin manifest sits at the head, above cli/ + viewer/.
cp "$REPO_ROOT/objectiveai.json" "$DEST/objectiveai.json"

# Unpack each zip into its folder.
case "$PLATFORM" in
  windows)
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$CLI_ZIP")' -DestinationPath '$(cygpath -w "$DEST/cli")'"
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$VIEWER_ZIP")' -DestinationPath '$(cygpath -w "$DEST/viewer")'"
    ;;
  *)
    unzip -o -q "$CLI_ZIP"    -d "$DEST/cli"
    unzip -o -q "$VIEWER_ZIP" -d "$DEST/viewer"
    ;;
esac

echo "==> installed -> $DEST"
