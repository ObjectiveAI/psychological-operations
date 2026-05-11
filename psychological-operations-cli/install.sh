#!/usr/bin/env bash
# Builds and installs psychological-operations as an objectiveai-cli plugin.
#
# - Builds psychological-operations-cli in release mode (skips if fingerprint unchanged)
# - Drops the binary at $HOME/.objectiveai/plugins/psychological-operations/plugin[.exe]
#   so the objectiveai-cli host can dispatch
#   `objectiveai psychological-operations <subcmd>` to it.
# - State files (data.db, psyops/, config.json, x_app.json, tokens/, …)
#   live alongside the binary in the same per-plugin subdir.
#
# Usage:
#   bash psychological-operations-cli/install.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# Per-plugin subdir under objectiveai-cli's plugins root. Matches the
# layout objectiveai-cli's own `plugins install` command produces:
# `<plugins>/<repository>/plugin[.exe]` + state files alongside.
INSTALL_DIR="$HOME/.objectiveai/plugins/psychological-operations"

# Detect platform
case "$(uname -s)" in
  CYGWIN*|MINGW*|MSYS*) PLATFORM="windows" ;;
  Darwin*)              PLATFORM="macos"   ;;
  *)                    PLATFORM="linux"   ;;
esac

# Match upstream's install-binary naming convention.
if [ "$PLATFORM" = "windows" ]; then
  DST_BIN_NAME="plugin.exe"
  SRC_BIN_NAME="psychological-operations.exe"
else
  DST_BIN_NAME="plugin"
  SRC_BIN_NAME="psychological-operations"
fi

# ── Fingerprint ────────────────────────────────────────────────────────
# Hash all source files that affect the CLI build. Skip the build if the
# installed binary's fingerprint matches.

# Use a plugin-specific fingerprint filename so we don't collide with
# objectiveai-cli's own .fingerprint or with any other plugin's.
FINGERPRINT_FILE="$INSTALL_DIR/.fingerprint-psychological-operations"

# Cross-platform SHA-256 wrapper. macOS runners ship `shasum` (Perl) but
# not GNU `sha256sum`; Linux/Windows-Git-bash typically ship both — we
# prefer `sha256sum` when it exists so output format stays identical
# across the common case, and fall back to `shasum -a 256` otherwise.
if command -v sha256sum >/dev/null 2>&1; then
  _sha256() { sha256sum "$@"; }
else
  _sha256() { shasum -a 256 "$@"; }
fi

compute_fingerprint() {
  {
    # psychological-operations-cli sources
    find "$SCRIPT_DIR/src" -type f -name '*.rs' | sort
    echo "$SCRIPT_DIR/Cargo.toml"

    # Shared lockfile
    echo "$REPO_ROOT/Cargo.lock"
  } | while IFS= read -r file; do
    if [ -f "$file" ]; then
      relpath="${file#"$REPO_ROOT/"}"
      printf '%s\n' "$relpath"
      # Strip the path from the hash line — sha256sum's default output
      # `<hash>  <path>` would otherwise embed the runner's absolute path
      # (different on Linux, macOS, Windows) and break cross-runner
      # fingerprint matching.
      _sha256 "$file" | awk '{print $1}'
    else
      printf '%s\n' "$file"
    fi
  done | _sha256 | awk '{print $1}'
}

CURRENT_FP=$(compute_fingerprint)

if [ -f "$FINGERPRINT_FILE" ]; then
  STORED_FP=$(cat "$FINGERPRINT_FILE")
  if [ "$CURRENT_FP" = "$STORED_FP" ] && [ -f "$INSTALL_DIR/$DST_BIN_NAME" ]; then
    echo "psychological-operations is up to date (fingerprint: ${CURRENT_FP:0:12}...)"
    exit 0
  fi
fi

# ── Build embedded dependencies ────────────────────────────────────────
# The CLI embeds the upstream Chromium snapshot bundle + packed extension via
# build.rs (guarded on its own fingerprint, so re-runs are no-ops). The
# objectiveai-api dep also embeds the claude-agent-sdk-runner and the
# mcp-filesystem; their build.shes need to run from inside the objectiveai
# workspace so cargo can resolve their package names.

echo "Building embedded dependencies..."
bash "$REPO_ROOT/psychological-operations-chromium/build.sh" --release
bash "$REPO_ROOT/objectiveai/objectiveai-claude-agent-sdk-runner/build.sh" --release
(cd "$REPO_ROOT/objectiveai" && bash objectiveai-mcp-filesystem/build.sh --target x86_64-unknown-linux-musl --release)

# ── Build CLI ──────────────────────────────────────────────────────────

echo "Building psychological-operations-cli (release)..."
cargo build --release -p psychological-operations-cli \
  --manifest-path "$REPO_ROOT/Cargo.toml"

SRC="$REPO_ROOT/target/release/$SRC_BIN_NAME"
if [ ! -f "$SRC" ]; then
  echo "ERROR: expected binary at $SRC" >&2
  exit 1
fi

# ── Install ────────────────────────────────────────────────────────────

mkdir -p "$INSTALL_DIR"
# `mv` onto a running Windows exe fails ("in use"); prefer `cp` so a
# later install over an in-use binary degrades to a clearer error.
cp "$SRC" "$INSTALL_DIR/$DST_BIN_NAME"
chmod +x "$INSTALL_DIR/$DST_BIN_NAME"
echo "$CURRENT_FP" > "$FINGERPRINT_FILE"
echo "Installed $INSTALL_DIR/$DST_BIN_NAME"

# No PATH wiring needed — users invoke via
#   objectiveai psychological-operations <subcmd>
# which objectiveai-cli dispatches to our binary by name lookup
# under $HOME/.objectiveai/plugins/.

echo ""
echo "Done!"
echo ""
echo "Invoke via:"
echo "  objectiveai psychological-operations --help"
