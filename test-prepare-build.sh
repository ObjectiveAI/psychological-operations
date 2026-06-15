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
# embed/<host-triple>/debug/). The viewer build (pnpm) runs IN PARALLEL
# with the browser+CLI build, since it's independent.
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
build_viewer() {
  echo "build: viewer (pnpm install + build)"
  (
    cd "$SCRIPT_DIR/psychological-operations-viewer"
    pnpm install --frozen-lockfile
    pnpm build
  )
  clean_keep_gitignore "$VIEWER_DIR"
  # Copy dist/ CONTENTS (index.html + assets) into viewer/ — no dist/ prefix.
  cp -R "$SCRIPT_DIR/psychological-operations-viewer/dist/." "$VIEWER_DIR/"
  echo "build: viewer -> $VIEWER_DIR"
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
# the viewer UI. A viewer build failure (e.g. an @objectiveai/sdk API drift)
# must NOT block the tests — warn loudly and carry on with an unpopulated
# viewer/ (the plugin's manifest declares no viewer for the test build).
if ! wait "$viewer_pid"; then
  echo "build: WARNING — viewer build FAILED; continuing without a viewer bundle" >&2
fi

echo "build: done"
