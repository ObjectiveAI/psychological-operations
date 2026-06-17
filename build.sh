#!/usr/bin/env bash
# build.sh — build the three psychological-operations artifacts (the CLI,
# the browser, and the viewer) IN PARALLEL, to their default output
# locations (cargo `target/<profile>/`, viewer `dist/`). No staging, no
# packaging. Debug by default; pass --release for a release build.
#
# It also provisions the build prerequisite into ./bin: ninja, which the
# browser's CEF crate (cef-dll-sys) needs for its CMake build. (This and
# the cargo-nextest provisioning were absorbed from the old install-bin.sh,
# which is gone.) The provisioning lives in functions so `source build.sh`
# loads them WITHOUT building — test.sh sources it to reuse them (it also
# wants cargo-nextest, which the build itself does not).
#
# Usage:
#   bash build.sh            # debug
#   bash build.sh --release  # release
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$REPO_ROOT/bin"

# ── platform / arch → release-asset naming ───────────────────────────
case "$(uname -s)" in
  Linux*)               _PLATFORM="linux"   ;;
  Darwin*)              _PLATFORM="macos"   ;;
  CYGWIN*|MINGW*|MSYS*) _PLATFORM="windows" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  _ARCH="x86_64"  ;;
  arm64|aarch64) _ARCH="aarch64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
if [ "$_PLATFORM" = "windows" ]; then _EXE=".exe"; else _EXE=""; fi

# ── ninja (required by cef-dll-sys' CMake/Ninja build) ───────────────
# Downloaded from the ninja-build GitHub release (not a cargo crate),
# pinned to [workspace.metadata.tools] in Cargo.toml. Idempotent.
provision_ninja() {
  local ver bin asset url tmp
  ver=$(sed -n 's/^ninja *= *"\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml")
  [ -n "$ver" ] || { echo "ERROR: could not read ninja version from Cargo.toml" >&2; return 1; }
  bin="$BIN_DIR/ninja$_EXE"
  if [ -x "$bin" ] && [ "$("$bin" --version 2>/dev/null | head -1)" = "$ver" ]; then
    echo "ninja $ver already installed, skipping."
    return
  fi
  case "$_PLATFORM-$_ARCH" in
    windows-x86_64)  asset="ninja-win.zip"           ;;
    windows-aarch64) asset="ninja-winarm64.zip"      ;;
    linux-x86_64)    asset="ninja-linux.zip"         ;;
    linux-aarch64)   asset="ninja-linux-aarch64.zip" ;;
    macos-*)         asset="ninja-mac.zip"           ;;
    *) echo "no ninja release asset for $_PLATFORM-$_ARCH" >&2; return 1 ;;
  esac
  url="https://github.com/ninja-build/ninja/releases/download/v${ver}/${asset}"
  tmp=$(mktemp -d -t psyops-ninja.XXXXXX)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  echo "Downloading ninja $ver ($asset)..."
  if command -v curl >/dev/null 2>&1; then
    curl -fSL --progress-bar "$url" -o "$tmp/ninja.zip"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$tmp/ninja.zip" "$url"
  else
    echo "need curl or wget to download ninja" >&2; return 1
  fi
  if command -v unzip >/dev/null 2>&1; then
    unzip -oq "$tmp/ninja.zip" -d "$tmp"
  elif [ "$_PLATFORM" = "windows" ]; then
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$tmp/ninja.zip")' -DestinationPath '$(cygpath -w "$tmp")'"
  else
    echo "need unzip to extract ninja" >&2; return 1
  fi
  [ -f "$tmp/ninja$_EXE" ] || { echo "ninja$_EXE missing from archive" >&2; return 1; }
  mkdir -p "$BIN_DIR"
  cp "$tmp/ninja$_EXE" "$bin"
  chmod +x "$bin"
  echo "Installed $bin ($("$bin" --version))"
}

# ── cargo-nextest (used by test.sh, NOT the build) ───────────────────
# `cargo install --root "$REPO_ROOT"` lands it at ./bin/cargo-nextest
# (cargo appends bin/), NOT the host ~/.cargo/bin. Idempotent.
provision_cargo_nextest() {
  local ver bin installed
  ver=$(sed -n 's/^cargo-nextest *= *"\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml")
  [ -n "$ver" ] || { echo "ERROR: could not read cargo-nextest version from Cargo.toml" >&2; return 1; }
  bin="$BIN_DIR/cargo-nextest$_EXE"
  if [ -x "$bin" ]; then
    installed=$("$bin" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
    [ "$installed" = "$ver" ] && { echo "cargo-nextest $ver already installed, skipping."; return; }
  fi
  echo "Installing cargo-nextest $ver (cargo install --root)..."
  cargo install cargo-nextest --version "$ver" --locked --root "$REPO_ROOT"
}

# ── the build ────────────────────────────────────────────────────────
build_main() {
  local REL="" PROFILE="debug" arg
  for arg in "$@"; do
    case "$arg" in
      --release) REL="--release"; PROFILE="release" ;;
      *) echo "build.sh: unknown arg: $arg (usage: build.sh [--release])" >&2; return 1 ;;
    esac
  done

  provision_ninja
  export PATH="$BIN_DIR:$PATH"
  cd "$REPO_ROOT"

  echo "==> build.sh ($PROFILE): CLI + browser (cargo) || viewer (pnpm)"

  # CLI + browser: ONE cargo invocation, both packages — two separate
  # `cargo build`s would just serialize on the target-dir lock; one
  # invocation with both `-p` lets cargo parallelize the shared crate
  # graph. Runs concurrently with the viewer (a separate node/vite
  # toolchain).
  cargo build $REL -p psychological-operations-cli -p psychological-operations-browser &
  local cargo_pid=$!
  ( cd psychological-operations-viewer && pnpm install --frozen-lockfile && pnpm build ) &
  local viewer_pid=$!

  local cargo_rc=0 viewer_rc=0
  wait "$cargo_pid"  || cargo_rc=$?
  wait "$viewer_pid" || viewer_rc=$?
  if [ "$cargo_rc" -ne 0 ] || [ "$viewer_rc" -ne 0 ]; then
    echo "build.sh FAILED (cargo=$cargo_rc viewer=$viewer_rc)" >&2
    return 1
  fi
  echo "==> done ($PROFILE)"
}

# Build only when EXECUTED; `source build.sh` just loads the helpers.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  build_main "$@"
fi
