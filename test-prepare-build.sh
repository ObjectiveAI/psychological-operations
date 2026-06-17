#!/usr/bin/env bash
# test-prepare-build.sh — build the local psychological-operations plugin
# into the committed objectiveai plugin tree for the integration tests:
#
#   .objectiveai/bin/plugins/objectiveai/psychological-operations/1.0.0/
#     cli/      <- the psychological-operations CLI (DEBUG) binary
#     viewer/   <- the viewer web bundle (objectiveai-viewer-compatible:
#                  index.html + assets, served via plugin://)
#
# DEBUG (not release) so it compiles fast. The CLI embeds the browser, so
# the browser bundle is staged first (the CLI's build.rs requires it at
# embed/<host-triple>/debug/). The viewer (build.sh -> zip -> unpack into
# viewer/) runs IN PARALLEL with the browser+CLI build, since it's
# independent.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_DIR="$SCRIPT_DIR/.objectiveai/bin/plugins/objectiveai/psychological-operations/1.0.0"
CLI_DIR="$PLUGIN_DIR/cli"
VIEWER_DIR="$PLUGIN_DIR/viewer"

# Host target triple (matches the embed dir the CLI's build.rs looks in,
# and the default cargo target-dir layout used by the browser bundle).
TARGET="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$TARGET" ] || { echo "build: could not determine host target triple" >&2; exit 1; }
ext=""
case "$TARGET" in *windows*) ext=".exe" ;; esac

# Wipe a plugin subdir's build outputs but keep its committed .gitignore.
clean_keep_gitignore() {
  [ -d "$1" ] || return 0
  find "$1" -mindepth 1 -not -name '.gitignore' -exec rm -rf {} + 2>/dev/null || true
}

# --- viewer (parallel) -----------------------------------------------------
# Build the release viewer zip via the SAME build.sh CI/release uses
# (pnpm install + pnpm build + scripts/zip.mjs -> repo-root
# psychological-operations-viewer.zip), then UNPACK it into viewer/ —
# mirroring how the objectiveai host installs the manifest's `viewer_zip`
# (download + extract into <plugin>/viewer/). Exercises the real packaging
# path rather than a dist/ shortcut, so a broken zip surfaces here.
build_viewer() {
  echo "build: viewer (build.sh -> zip, then unpack)"
  bash "$SCRIPT_DIR/psychological-operations-viewer/build.sh"
  local zip="$SCRIPT_DIR/psychological-operations-viewer.zip"
  [ -f "$zip" ] || { echo "build: viewer zip missing at $zip" >&2; return 1; }
  clean_keep_gitignore "$VIEWER_DIR"
  # Unpack the zip (root = index.html + assets, no dist/ prefix) into
  # viewer/. git-bash ships no `unzip`, so the Windows leg uses
  # PowerShell's Expand-Archive (cygpath converts to Windows paths).
  case "$TARGET" in
    *windows*)
      powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
        "Expand-Archive -Force -LiteralPath '$(cygpath -w "$zip")' -DestinationPath '$(cygpath -w "$VIEWER_DIR")'"
      ;;
    *)
      unzip -o -q "$zip" -d "$VIEWER_DIR"
      ;;
  esac
  rm -f "$zip"
  echo "build: viewer -> $VIEWER_DIR (unpacked from zip)"
}
build_viewer &
viewer_pid=$!

# --- browser bundle (debug) + CLI (debug) ---------------------------------
echo "build: browser bundle ($TARGET, debug)"
case "$TARGET" in
  *windows*)
    # The POSIX build-bundle.sh refuses Windows targets; use the .ps1.
    ( cd "$SCRIPT_DIR/psychological-operations-browser/scripts" \
        && powershell.exe -NoProfile -ExecutionPolicy Bypass -File build-bundle.ps1 )
    ;;
  *)
    bash "$SCRIPT_DIR/psychological-operations-browser/scripts/build-bundle.sh"
    ;;
esac

echo "build: psychological-operations CLI (debug)"
cargo build -p psychological-operations-cli --manifest-path "$SCRIPT_DIR/Cargo.toml"

CLI_BIN="$SCRIPT_DIR/target/debug/psychological-operations${ext}"
[ -f "$CLI_BIN" ] || { echo "build: CLI binary missing at $CLI_BIN" >&2; exit 1; }
clean_keep_gitignore "$CLI_DIR"
cp "$CLI_BIN" "$CLI_DIR/psychological-operations${ext}"
echo "build: CLI -> $CLI_DIR/psychological-operations${ext}"

# --- wait for the parallel viewer build -----------------------------------
# Non-fatal: the integration suite is the Rust crate, which never exercises
# the viewer UI. A viewer build failure must NOT block the tests — warn
# loudly and carry on with an empty viewer/. The staged manifest still
# declares the viewer (viewer_zip + viewer_routes); that only matters if
# someone opens the viewer tab, which the Rust tests never do.
if ! wait "$viewer_pid"; then
  echo "build: WARNING — viewer build FAILED; continuing with an empty viewer/" >&2
fi

echo "build: done"
