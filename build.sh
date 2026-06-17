#!/usr/bin/env bash
# build.sh — isolated build script; run it directly (`bash build.sh`). It
# builds the three psychological-operations artifacts and packages them into
# the two RELEASE-NAMED zips at the repo root:
#
#   psychological-operations-<os>-<arch>.zip   the cli_zip — the CLI binary
#                                              + the browser CEF bundle, flat
#                                              (what the host extracts into
#                                              <plugin>/cli and points
#                                              OBJECTIVEAI_BIN_DIR at)
#   psychological-operations-viewer.zip        the viewer web bundle
#
# These are the same filenames the GitHub release uploads. Debug by default;
# pass --release for a release build (applies to the cargo + browser-bundle
# builds; the viewer's vite build is unconditional).
#
# It also provisions its one build prerequisite into ./bin: ninja, which the
# browser's CEF crate (cef-dll-sys) needs for its CMake build. Self-contained
# on purpose — build.sh does not share code with any other script.
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

# ── platform / arch (drives the ninja asset + the cli_zip name) ──────
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

TARGET="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$TARGET" ] || { echo "ERROR: could not determine host target triple" >&2; exit 1; }

# ── build: CLI + browser (one cargo invocation) || viewer ────────────
# CLI + browser as ONE cargo invocation, both packages — two separate
# `cargo build`s would just serialize on the target-dir lock; one invocation
# lets cargo parallelize the shared crate graph. The viewer runs its own
# release pipeline (install + vite build + zip) concurrently — that already
# emits psychological-operations-viewer.zip at the repo root.
echo "==> build.sh ($PROFILE): CLI + browser (cargo) || viewer (pnpm)"

cargo build $REL -p psychological-operations-cli -p psychological-operations-browser &
cargo_pid=$!
bash "$REPO_ROOT/psychological-operations-viewer/build.sh" &
viewer_pid=$!

cargo_rc=0; wait "$cargo_pid" || cargo_rc=$?
if [ "$cargo_rc" -ne 0 ]; then
  echo "build.sh FAILED (cargo=$cargo_rc)" >&2
  wait "$viewer_pid" 2>/dev/null || true
  exit 1
fi

# ── browser CEF bundle (CEF runtime + browser exe) via build-bundle ──
# build-bundle stages the exact CEF runtime file set and zips it (reusing
# the browser build we just did). Overlaps the still-running viewer.
echo "==> browser bundle ($TARGET, $PROFILE)"
case "$PLATFORM" in
  windows)
    ( cd "$REPO_ROOT/psychological-operations-browser/scripts" \
        && powershell.exe -NoProfile -ExecutionPolicy Bypass -File build-bundle.ps1 ${REL:+-Release} )
    ;;
  *)
    bash "$REPO_ROOT/psychological-operations-browser/scripts/build-bundle.sh" $REL
    ;;
esac
BUNDLE_ZIP="$REPO_ROOT/psychological-operations-browser/embed/$TARGET/$PROFILE/browser-bundle.zip"
[ -f "$BUNDLE_ZIP" ] || { echo "browser bundle not found: $BUNDLE_ZIP" >&2; wait "$viewer_pid" 2>/dev/null || true; exit 1; }

# ── cli_zip = the browser bundle + the CLI binary, flat at the root ──
# The browser bundle zip is already flat (CEF runtime + browser exe); copy
# it under the release cli_zip name and append the CLI binary — no
# unzip/rezip of the ~190 MB runtime.
CLI_ZIP="$REPO_ROOT/psychological-operations-$PLATFORM-$ARCH.zip"
CLI_BIN="$REPO_ROOT/target/$PROFILE/psychological-operations$EXE"
[ -f "$CLI_BIN" ] || { echo "CLI binary not found: $CLI_BIN" >&2; wait "$viewer_pid" 2>/dev/null || true; exit 1; }
rm -f "$CLI_ZIP"
cp "$BUNDLE_ZIP" "$CLI_ZIP"
case "$PLATFORM" in
  windows)
    powershell.exe -NoProfile -Command \
      "Compress-Archive -Update -Path '$(cygpath -w "$CLI_BIN")' -DestinationPath '$(cygpath -w "$CLI_ZIP")'"
    ;;
  *)
    zip -j "$CLI_ZIP" "$CLI_BIN"
    ;;
esac
echo "==> wrote $(basename "$CLI_ZIP")"

# ── wait for the viewer zip ──────────────────────────────────────────
viewer_rc=0; wait "$viewer_pid" || viewer_rc=$?
if [ "$viewer_rc" -ne 0 ]; then
  echo "build.sh FAILED (viewer=$viewer_rc)" >&2
  exit 1
fi
echo "==> wrote psychological-operations-viewer.zip"

echo "==> done ($PROFILE) -> psychological-operations-$PLATFORM-$ARCH.zip + psychological-operations-viewer.zip"
