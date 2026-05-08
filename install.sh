#!/usr/bin/env bash
# psychological-operations CLI installer — downloads a pre-built release binary.
#
#   curl -fsSL https://raw.githubusercontent.com/WiggidyW/psychological-operations/main/install.sh | bash
#
# - Detects platform + architecture.
# - Fetches the latest published release asset from GitHub and drops it
#   at ~/.psychological-operations/psychological-operations
#   (or psychological-operations.exe on Windows).
# - Adds ~/.psychological-operations to PATH.
#
# No toolchain required. For a from-source install, clone the repo and
# run `psychological-operations-cli/install.sh` instead.

set -euo pipefail

REPO="WiggidyW/psychological-operations"
INSTALL_DIR="$HOME/.psychological-operations"

for arg in "$@"; do
  case "$arg" in
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown option: $arg" >&2
      exit 2
      ;;
  esac
done

# ── Detect platform ───────────────────────────────────────────────────

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

# Only these platform/arch combos have release assets.
# (linux-aarch64 is unsupported because Chrome for Testing — required by
# the embedded chrome bundle — does not ship a linux-arm64 build.)
SUPPORTED=0
case "$PLATFORM-$ARCH" in
  linux-x86_64|macos-x86_64|macos-aarch64|windows-x86_64) SUPPORTED=1 ;;
esac
if [ "$SUPPORTED" = "0" ]; then
  echo "no release asset for $PLATFORM-$ARCH" >&2
  exit 1
fi

# ── Asset name + destination ──────────────────────────────────────────

ASSET="psychological-operations-${PLATFORM}-${ARCH}"
if [ "$PLATFORM" = "windows" ]; then
  ASSET="${ASSET}.exe"
  DST_NAME="psychological-operations.exe"
else
  DST_NAME="psychological-operations"
fi

URL="https://github.com/${REPO}/releases/latest/download/${ASSET}"
TMP=$(mktemp -t psyops.XXXXXX)
trap 'rm -f "$TMP"' EXIT

# ── Download ──────────────────────────────────────────────────────────

echo "Downloading $ASSET..."
if command -v curl >/dev/null 2>&1; then
  # -L follows the redirect from /releases/latest/download/... to the
  # actual asset URL; -f fails hard on 4xx/5xx instead of writing HTML.
  curl -fSL --progress-bar "$URL" -o "$TMP"
elif command -v wget >/dev/null 2>&1; then
  wget -O "$TMP" "$URL"
else
  echo "need curl or wget to download" >&2
  exit 1
fi

if [ ! -s "$TMP" ]; then
  echo "download produced an empty file" >&2
  exit 1
fi

# ── Install ───────────────────────────────────────────────────────────

mkdir -p "$INSTALL_DIR"
DST="$INSTALL_DIR/$DST_NAME"
# `mv` onto a running Windows exe fails ("in use"); prefer `cp` so a
# later install over an in-use binary degrades to a clearer error.
cp "$TMP" "$DST"
chmod +x "$DST"
echo "Installed $DST"

# ── PATH ──────────────────────────────────────────────────────────────
#
# A child process can't mutate its parent shell's environment, so the
# canonical pattern (rustup, etc.) is to write a sourceable env file.
# Future shells pick it up via a one-liner appended to the user's rc;
# the current shell sources it on demand.

write_env_file() {
  cat > "$INSTALL_DIR/env" <<'EOF'
#!/bin/sh
# psychological-operations shell setup. Source this file from your shell
# rc, or run
#   . "$HOME/.psychological-operations/env"
# to put `psychological-operations` on PATH for the current shell.

case ":${PATH}:" in
    *:"$HOME/.psychological-operations":*) ;;
    *) export PATH="$HOME/.psychological-operations:$PATH" ;;
esac
EOF
}

add_to_path() {
  local shell_rc="$1"
  local line='. "$HOME/.psychological-operations/env"'
  if [ -f "$shell_rc" ] && grep -qF '.psychological-operations/env' "$shell_rc"; then
    return
  fi
  {
    echo ""
    echo "# psychological-operations CLI"
    echo "$line"
  } >> "$shell_rc"
  echo "Added to PATH in $shell_rc"
}

write_env_file

case "$PLATFORM" in
  windows)
    INSTALL_DIR_WIN="$(cygpath -w "$INSTALL_DIR" 2>/dev/null || echo "$INSTALL_DIR")"
    CURRENT_PATH=$(powershell.exe -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path', 'User')" 2>/dev/null | tr -d '\r' || true)
    if echo "$CURRENT_PATH" | grep -qiF '.psychological-operations'; then
      echo "PATH already contains $INSTALL_DIR_WIN"
    else
      powershell.exe -NoProfile -Command \
        "[Environment]::SetEnvironmentVariable('Path', '$INSTALL_DIR_WIN;' + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')" 2>/dev/null
      echo "Added $INSTALL_DIR_WIN to user PATH (restart cmd/PowerShell to use it)."
    fi
    # Also wire up Git Bash / MSYS via the env file.
    [ -f "$HOME/.bashrc" ] && add_to_path "$HOME/.bashrc"
    ;;
  macos)
    add_to_path "$HOME/.zshrc"
    ;;
  linux)
    [ -f "$HOME/.bashrc" ] && add_to_path "$HOME/.bashrc"
    [ -f "$HOME/.zshrc" ]  && add_to_path "$HOME/.zshrc"
    ;;
esac

echo ""
echo "Done!"
echo ""
echo "To use psychological-operations in your current shell, run:"
echo '  . "$HOME/.psychological-operations/env"'
echo ""
echo "(New shells will pick it up automatically.)"
