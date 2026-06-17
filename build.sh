#!/usr/bin/env bash
# build.sh — isolated build script; run it directly (`bash build.sh`). It
# builds the three psychological-operations artifacts (the CLI, the
# browser, and the viewer) IN PARALLEL, to their default output locations
# (cargo `target/<profile>/`, viewer `dist/`). No staging, no packaging.
# Debug by default; pass --release for a release build.
#
# It first provisions its one build prerequisite into ./bin: ninja, which
# the browser's CEF crate (cef-dll-sys) needs for its CMake build. This is
# self-contained on purpose — build.sh does not depend on, or share code
# with, any other script.
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
BIN_DIR="$REPO_ROOT/bin"
cd "$REPO_ROOT"

# ── ninja (required by cef-dll-sys' CMake/Ninja build) ───────────────
# Downloaded from the ninja-build GitHub release (it's not a cargo crate),
# version pinned in [workspace.metadata.tools] in Cargo.toml. Idempotent:
# skips when ./bin already holds the pinned version.
NINJA_VERSION="$(sed -n 's/^ninja *= *"\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml")"
[ -n "$NINJA_VERSION" ] || { echo "ERROR: could not read ninja version from Cargo.toml" >&2; exit 1; }

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

NINJA_BIN="$BIN_DIR/ninja$EXE"
if [ -x "$NINJA_BIN" ] && [ "$("$NINJA_BIN" --version 2>/dev/null | head -1)" = "$NINJA_VERSION" ]; then
  echo "ninja $NINJA_VERSION already installed, skipping."
else
  case "$PLATFORM-$ARCH" in
    windows-x86_64)  NINJA_ASSET="ninja-win.zip"           ;;
    windows-aarch64) NINJA_ASSET="ninja-winarm64.zip"      ;;
    linux-x86_64)    NINJA_ASSET="ninja-linux.zip"         ;;
    linux-aarch64)   NINJA_ASSET="ninja-linux-aarch64.zip" ;;
    macos-*)         NINJA_ASSET="ninja-mac.zip"           ;;
    *) echo "no ninja release asset for $PLATFORM-$ARCH" >&2; exit 1 ;;
  esac
  NINJA_URL="https://github.com/ninja-build/ninja/releases/download/v${NINJA_VERSION}/${NINJA_ASSET}"
  NINJA_TMP="$(mktemp -d -t psyops-ninja.XXXXXX)"
  trap 'rm -rf "$NINJA_TMP"' EXIT
  echo "Downloading ninja $NINJA_VERSION ($NINJA_ASSET)..."
  if command -v curl >/dev/null 2>&1; then
    curl -fSL --progress-bar "$NINJA_URL" -o "$NINJA_TMP/ninja.zip"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$NINJA_TMP/ninja.zip" "$NINJA_URL"
  else
    echo "need curl or wget to download ninja" >&2; exit 1
  fi
  if command -v unzip >/dev/null 2>&1; then
    unzip -oq "$NINJA_TMP/ninja.zip" -d "$NINJA_TMP"
  elif [ "$PLATFORM" = "windows" ]; then
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$NINJA_TMP/ninja.zip")' -DestinationPath '$(cygpath -w "$NINJA_TMP")'"
  else
    echo "need unzip to extract ninja" >&2; exit 1
  fi
  [ -f "$NINJA_TMP/ninja$EXE" ] || { echo "ninja$EXE missing from archive" >&2; exit 1; }
  mkdir -p "$BIN_DIR"
  cp "$NINJA_TMP/ninja$EXE" "$NINJA_BIN"
  chmod +x "$NINJA_BIN"
  echo "Installed $NINJA_BIN ($("$NINJA_BIN" --version))"
fi
export PATH="$BIN_DIR:$PATH"

# ── build: CLI + browser (one cargo invocation) || viewer (pnpm) ─────
echo "==> build.sh ($PROFILE): CLI + browser (cargo) || viewer (pnpm)"

# CLI + browser: ONE cargo invocation, both packages — two separate
# `cargo build`s would just serialize on the target-dir lock; one
# invocation with both `-p` lets cargo parallelize the shared crate graph.
# Runs concurrently with the viewer (a separate node/vite toolchain).
cargo build $REL -p psychological-operations-cli -p psychological-operations-browser &
cargo_pid=$!
( cd psychological-operations-viewer && pnpm install --frozen-lockfile && pnpm build ) &
viewer_pid=$!

cargo_rc=0;  wait "$cargo_pid"  || cargo_rc=$?
viewer_rc=0; wait "$viewer_pid" || viewer_rc=$?
if [ "$cargo_rc" -ne 0 ] || [ "$viewer_rc" -ne 0 ]; then
  echo "build.sh FAILED (cargo=$cargo_rc viewer=$viewer_rc)" >&2
  exit 1
fi
echo "==> done ($PROFILE)"
