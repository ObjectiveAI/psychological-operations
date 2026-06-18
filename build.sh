#!/usr/bin/env bash
# build.sh — isolated build orchestrator; run it directly (`bash build.sh`).
# It runs the two build legs in parallel — build-cli.sh (CLI binary + browser
# CEF bundle) and build-viewer.sh (viewer web bundle), each of which
# provisions its own deps — then zips their outputs into the plugin tree under
#   .objectiveai/bin/plugins/ObjectiveAI/psychological-operations/<version>/
# as the two RELEASE-NAMED zips plus their extracted contents:
#
#   psychological-operations-<os>-<arch>.zip  + cli/      cli_zip (CLI binary
#                                                         + browser CEF bundle)
#   psychological-operations-viewer.zip       + viewer/   the viewer web bundle
#
# The zip filenames match the GitHub release assets. Debug by default; pass
# --release for a release build.
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

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

VERSION="0.1.0"  # kept in sync by version.sh

# ── platform / arch (drives the cli_zip name + output paths) ─────────
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
if [ "$PLATFORM" = "windows" ]; then EXE=".exe"; else EXE=""; fi
TARGET="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$TARGET" ] || { echo "ERROR: could not determine host target triple" >&2; exit 1; }

# ── build legs (parallel; each provisions its own deps) ──────────────
echo "==> build.sh ($PROFILE): build-cli.sh || build-viewer.sh"
bash "$REPO_ROOT/build-cli.sh" $REL &
cli_pid=$!
bash "$REPO_ROOT/build-viewer.sh" $REL &
viewer_pid=$!
cli_rc=0;    wait "$cli_pid"    || cli_rc=$?
viewer_rc=0; wait "$viewer_pid" || viewer_rc=$?
if [ "$cli_rc" -ne 0 ] || [ "$viewer_rc" -ne 0 ]; then
  echo "build.sh FAILED (cli=$cli_rc viewer=$viewer_rc)" >&2
  exit 1
fi

# ── zip into the plugin tree (each zip lives in its own folder) ──────
PLUGIN_DIR="$REPO_ROOT/.objectiveai/bin/plugins/ObjectiveAI/psychological-operations/$VERSION"
CLI_DIR="$PLUGIN_DIR/cli"
VIEWER_DIR="$PLUGIN_DIR/viewer"
CLI_ZIP="$CLI_DIR/psychological-operations-$PLATFORM-$ARCH.zip"
VIEWER_ZIP="$VIEWER_DIR/psychological-operations-viewer.zip"

# Clean slate: wipe cli/ + viewer/ recursively, then recreate.
rm -rf "$CLI_DIR" "$VIEWER_DIR"
mkdir -p "$CLI_DIR" "$VIEWER_DIR"

# cli/ zip = the staged browser runtime (build-cli.sh left it in embed/, the
# CEF files or the .app) + the CLI binary, flat at the zip root.
BUNDLE_DIR="$REPO_ROOT/psychological-operations-browser/embed"
[ -d "$BUNDLE_DIR" ] || { echo "browser runtime not staged: $BUNDLE_DIR" >&2; exit 1; }
CLI_BIN="$REPO_ROOT/target/$PROFILE/psychological-operations$EXE"
[ -f "$CLI_BIN" ] || { echo "CLI binary not found: $CLI_BIN" >&2; exit 1; }
case "$PLATFORM" in
  windows)
    powershell.exe -NoProfile -Command \
      "Compress-Archive -Path '$(cygpath -w "$BUNDLE_DIR")\*' -DestinationPath '$(cygpath -w "$CLI_ZIP")' -Force"
    powershell.exe -NoProfile -Command \
      "Compress-Archive -Update -Path '$(cygpath -w "$CLI_BIN")' -DestinationPath '$(cygpath -w "$CLI_ZIP")'"
    ;;
  *)
    ( cd "$BUNDLE_DIR" && zip -qr "$CLI_ZIP" . )
    zip -j "$CLI_ZIP" "$CLI_BIN"
    ;;
esac

# viewer/ zip via the viewer's canonical zipper (dist/ -> flat zip at the repo
# root), then move it in.
( cd psychological-operations-viewer && node scripts/zip.mjs )
mv -f "$REPO_ROOT/psychological-operations-viewer.zip" "$VIEWER_ZIP"

# Embed each zip into its own folder (extract alongside the zip).
case "$PLATFORM" in
  windows)
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$CLI_ZIP")' -DestinationPath '$(cygpath -w "$CLI_DIR")'"
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$VIEWER_ZIP")' -DestinationPath '$(cygpath -w "$VIEWER_DIR")'"
    ;;
  *)
    unzip -o -q "$CLI_ZIP" -d "$CLI_DIR"
    unzip -o -q "$VIEWER_ZIP" -d "$VIEWER_DIR"
    ;;
esac

echo "==> done ($PROFILE) -> $PLUGIN_DIR"
echo "      cli/$(basename "$CLI_ZIP") + viewer/$(basename "$VIEWER_ZIP") (+ extracted)"
