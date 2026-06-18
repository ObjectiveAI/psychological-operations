#!/usr/bin/env bash
# build.sh — isolated build orchestrator; run it directly (`bash build.sh`).
# It runs the two build legs in parallel — build-cli.sh (CLI binary + browser
# CEF bundle) and build-viewer.sh (viewer web bundle), each of which
# provisions its own deps — then zips their outputs into the plugin tree under
#   .objectiveai/bin/plugins/ObjectiveAI/psychological-operations/<version>/
# as the two RELEASE-NAMED zips, each in its own folder (NOT extracted):
#
#   cli/psychological-operations-<os>-<arch>.zip   CLI binary + browser CEF runtime
#   viewer/psychological-operations-viewer.zip     the viewer web bundle
#
# The zip filenames match the GitHub release assets. Debug by default; pass
# --release for a release build. --no-cli / --no-viewer skip that leg (its
# build AND its zip); passing both is a no-op.
#
# Usage:
#   bash build.sh                # debug, both legs
#   bash build.sh --release      # release, both legs
#   bash build.sh --no-viewer    # CLI only
#   bash build.sh --no-cli       # viewer only
set -euo pipefail

REL=""
PROFILE="debug"
NO_CLI=0
NO_VIEWER=0
for arg in "$@"; do
  case "$arg" in
    --release)   REL="--release"; PROFILE="release" ;;
    --no-cli)    NO_CLI=1 ;;
    --no-viewer) NO_VIEWER=1 ;;
    *) echo "build.sh: unknown arg: $arg (usage: build.sh [--release] [--no-cli] [--no-viewer])" >&2; exit 1 ;;
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

if [ "$NO_CLI" = "1" ] && [ "$NO_VIEWER" = "1" ]; then
  echo "==> build.sh: --no-cli + --no-viewer — nothing to build."
  exit 0
fi

# ── build legs (parallel; each provisions its own deps) ──────────────
echo "==> build.sh ($PROFILE)"
cli_pid=""
viewer_pid=""
if [ "$NO_CLI" = "0" ];    then bash "$REPO_ROOT/build-cli.sh" $REL & cli_pid=$!; fi
if [ "$NO_VIEWER" = "0" ]; then bash "$REPO_ROOT/build-viewer.sh" $REL & viewer_pid=$!; fi
cli_rc=0
if [ -n "$cli_pid" ]; then wait "$cli_pid" || cli_rc=$?; fi
viewer_rc=0
if [ -n "$viewer_pid" ]; then wait "$viewer_pid" || viewer_rc=$?; fi
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

mkdir -p "$PLUGIN_DIR"
# The plugin manifest sits at the head, above cli/ + viewer/.
cp "$REPO_ROOT/objectiveai.json" "$PLUGIN_DIR/objectiveai.json"

if [ "$NO_CLI" = "0" ]; then
  # cli/ zip = the staged browser runtime (build-cli.sh left it in embed/, the
  # CEF files or the .app) + the CLI binary, flat at the zip root.
  rm -rf "$CLI_DIR"; mkdir -p "$CLI_DIR"
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
  echo "      cli/$(basename "$CLI_ZIP")"
fi

if [ "$NO_VIEWER" = "0" ]; then
  # viewer/ zip via the viewer's canonical zipper (dist/ -> flat zip at the repo
  # root), then move it in.
  rm -rf "$VIEWER_DIR"; mkdir -p "$VIEWER_DIR"
  ( cd psychological-operations-viewer && node scripts/zip.mjs )
  mv -f "$REPO_ROOT/psychological-operations-viewer.zip" "$VIEWER_ZIP"
  echo "      viewer/$(basename "$VIEWER_ZIP")"
fi

echo "==> done ($PROFILE) -> $PLUGIN_DIR"
