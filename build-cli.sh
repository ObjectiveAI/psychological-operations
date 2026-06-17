#!/usr/bin/env bash
# build-cli.sh — build the CLI plugin's binaries: the CLI release binary +
# the browser CEF bundle (browser-bundle.zip). Debug by default; --release
# for a release build.
#
# Self-contained: it provisions its own build prerequisites — ninja (for the
# browser's CEF/CMake build) into ./bin, and on macOS the browser's yarn
# deps. The root build.sh is what zips these outputs. Outputs:
#   target/<profile>/psychological-operations[.exe]
#   psychological-operations-browser/embed/<triple>/<profile>/browser-bundle.zip
set -euo pipefail

REL=""
PROFILE="debug"
for arg in "$@"; do
  case "$arg" in
    --release) REL="--release"; PROFILE="release" ;;
    *) echo "build-cli.sh: unknown arg: $arg (usage: build-cli.sh [--release])" >&2; exit 1 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$REPO_ROOT/bin"
cd "$REPO_ROOT"

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

# ── ninja (required by cef-dll-sys' CMake/Ninja build) ───────────────
# Downloaded from the ninja-build GitHub release (it's not a cargo crate),
# version pinned in [workspace.metadata.tools] in Cargo.toml. Idempotent.
NINJA_VERSION="$(sed -n 's/^ninja *= *"\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml")"
[ -n "$NINJA_VERSION" ] || { echo "ERROR: could not read ninja version from Cargo.toml" >&2; exit 1; }
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

echo "==> build-cli.sh ($PROFILE)"
case "$PLATFORM" in
  macos)
    # CEF on macOS only runs from a .app bundle (the tauri bundler builds it;
    # plain `cargo build` makes the exe but not the .app). Install the
    # browser's node deps, build the CLI, then the browser .app via its
    # yarn/tauri toolchain (the beforeBuildCommand builds the frontend), then
    # stage it with build-bundle (--skip-build: package the bundle, no cargo
    # rebuild).
    ( cd psychological-operations-browser && yarn install --immutable )
    cargo build $REL -p psychological-operations-cli
    TAURI_DEBUG=""
    if [ "$PROFILE" = "debug" ]; then TAURI_DEBUG="--debug"; fi
    ( cd psychological-operations-browser && yarn tauri build --target "$TARGET" $TAURI_DEBUG )
    bash psychological-operations-browser/scripts/build-bundle.sh --skip-build $REL
    ;;
  windows)
    cargo build $REL -p psychological-operations-cli -p psychological-operations-browser
    ( cd psychological-operations-browser/scripts \
        && powershell.exe -NoProfile -ExecutionPolicy Bypass -File build-bundle.ps1 ${REL:+-Release} )
    ;;
  *)
    cargo build $REL -p psychological-operations-cli -p psychological-operations-browser
    bash psychological-operations-browser/scripts/build-bundle.sh $REL
    ;;
esac
echo "==> build-cli.sh done ($PROFILE)"
