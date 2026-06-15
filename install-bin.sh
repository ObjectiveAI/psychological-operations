#!/usr/bin/env bash
# Installs build tools into ./bin/ using versions pinned in
# [workspace.metadata.tools] in Cargo.toml — same pattern as
# objectiveai's build-bin.sh, except ninja isn't a cargo crate, so we
# download the prebuilt single-binary release from GitHub (the
# install.sh download pattern) instead of `cargo install`.
#
# Currently installs:
#   ninja — required by cef-dll-sys: the browser crate's CEF C++
#   wrapper (libcef_dll_wrapper) builds via CMake's Ninja generator.
#   CMake itself is found via Visual Studio's bundled copy on Windows.
#   cargo-nextest — runs the integration suite (test.sh). Installed via
#   `cargo install --root` into ./bin (NOT the host ~/.cargo/bin), same
#   source objectiveai's build-bin.sh uses.
#
# Usage:
#   bash install-bin.sh
#
# Then put ./bin on PATH for builds that need it, e.g.:
#   PATH="$PWD/bin:$PATH" cargo check --workspace

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN_DIR="$REPO_ROOT/bin"

# ── Pinned versions from [workspace.metadata.tools] ──────────────────
NINJA_VERSION=$(sed -n 's/^ninja *= *"\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml")
[ -n "$NINJA_VERSION" ] || { echo "ERROR: Could not read ninja version from Cargo.toml" >&2; exit 1; }
CARGO_NEXTEST_VERSION=$(sed -n 's/^cargo-nextest *= *"\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml")
[ -n "$CARGO_NEXTEST_VERSION" ] || { echo "ERROR: Could not read cargo-nextest version from Cargo.toml" >&2; exit 1; }

# ── Detect platform → release asset name ─────────────────────────────
case "$(uname -s)" in
  Linux*)               PLATFORM="linux"   ;;
  Darwin*)              PLATFORM="macos"   ;;
  CYGWIN*|MINGW*|MSYS*) PLATFORM="windows" ;;
  *)
    echo "unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64)  ARCH="x86_64"  ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *)
    echo "unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

# ninja-build/ninja release asset naming (the mac zip is universal).
case "$PLATFORM-$ARCH" in
  windows-x86_64)  NINJA_ASSET="ninja-win.zip"           ;;
  windows-aarch64) NINJA_ASSET="ninja-winarm64.zip"      ;;
  linux-x86_64)    NINJA_ASSET="ninja-linux.zip"         ;;
  linux-aarch64)   NINJA_ASSET="ninja-linux-aarch64.zip" ;;
  macos-*)         NINJA_ASSET="ninja-mac.zip"           ;;
  *)
    echo "no ninja release asset for $PLATFORM-$ARCH" >&2
    exit 1
    ;;
esac

if [ "$PLATFORM" = "windows" ]; then
  EXE_SUFFIX=".exe"
else
  EXE_SUFFIX=""
fi

# ── ninja ─────────────────────────────────────────────────────────────
install_ninja() {
  local bin="$BIN_DIR/ninja$EXE_SUFFIX"

  # Idempotent: skip when the installed version already matches the pin.
  if [ -x "$bin" ]; then
    local installed
    installed=$("$bin" --version 2>/dev/null | head -1 || true)
    if [ "$installed" = "$NINJA_VERSION" ]; then
      echo "ninja $NINJA_VERSION already installed, skipping."
      return
    fi
  fi

  local url="https://github.com/ninja-build/ninja/releases/download/v${NINJA_VERSION}/${NINJA_ASSET}"
  local tmp
  tmp=$(mktemp -d -t psyops-ninja.XXXXXX)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  echo "Downloading ninja $NINJA_VERSION ($NINJA_ASSET)..."
  if command -v curl >/dev/null 2>&1; then
    # -L follows the release-asset redirect; -f fails hard on 4xx/5xx
    # instead of writing an HTML error page into the zip.
    curl -fSL --progress-bar "$url" -o "$tmp/ninja.zip"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$tmp/ninja.zip" "$url"
  else
    echo "need curl or wget to download" >&2
    return 1
  fi

  if command -v unzip >/dev/null 2>&1; then
    unzip -oq "$tmp/ninja.zip" -d "$tmp"
  elif [ "$PLATFORM" = "windows" ]; then
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$tmp/ninja.zip")' -DestinationPath '$(cygpath -w "$tmp")'"
  else
    echo "need unzip to extract" >&2
    return 1
  fi

  [ -f "$tmp/ninja$EXE_SUFFIX" ] || { echo "ninja$EXE_SUFFIX missing from archive" >&2; return 1; }

  mkdir -p "$BIN_DIR"
  cp "$tmp/ninja$EXE_SUFFIX" "$bin"
  chmod +x "$bin"
  echo "Installed $bin ($("$bin" --version))"
}

install_ninja

# ── cargo-nextest ─────────────────────────────────────────────────────
# `cargo install --root "$REPO_ROOT"` lands the binary at
# "$BIN_DIR/cargo-nextest" (cargo appends `bin/` to --root). Invoke it as
# `"$BIN_DIR/cargo-nextest" nextest run …` (or via PATH). NOT installed to
# the host ~/.cargo/bin.
install_cargo_nextest() {
  local bin="$BIN_DIR/cargo-nextest$EXE_SUFFIX"

  # Idempotent: skip when the installed version already matches the pin.
  if [ -x "$bin" ]; then
    local installed
    installed=$("$bin" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
    if [ "$installed" = "$CARGO_NEXTEST_VERSION" ]; then
      echo "cargo-nextest $CARGO_NEXTEST_VERSION already installed, skipping."
      return
    fi
  fi

  echo "Installing cargo-nextest $CARGO_NEXTEST_VERSION (cargo install --root)..."
  cargo install cargo-nextest --version "$CARGO_NEXTEST_VERSION" --locked --root "$REPO_ROOT"
  echo "Installed $bin ($("$bin" --version 2>/dev/null | head -1))"
}

install_cargo_nextest

echo ""
echo "Done. Tools at $BIN_DIR/"
echo "Add it to PATH (e.g. \`export PATH=\"$BIN_DIR:\$PATH\"\`) so the"
echo "browser crate's CEF build can find ninja and \`cargo nextest\` resolves."
