#!/usr/bin/env bash
# Builds and installs the psychological-operations CLI from source.
#
# - Builds psychological-operations-cli in release mode (skips if fingerprint unchanged)
# - Copies the binary to ~/.psychological-operations/ as 'psychological-operations'
#   (or 'psychological-operations.exe' on Windows)
# - Adds ~/.psychological-operations to PATH if not already present
#
# Usage:
#   bash psychological-operations-cli/install.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALL_DIR="$HOME/.psychological-operations"

# Detect platform
case "$(uname -s)" in
  CYGWIN*|MINGW*|MSYS*) PLATFORM="windows" ;;
  Darwin*)              PLATFORM="macos"   ;;
  *)                    PLATFORM="linux"   ;;
esac

if [ "$PLATFORM" = "windows" ]; then
  BIN_NAME="psychological-operations.exe"
else
  BIN_NAME="psychological-operations"
fi

# ── Fingerprint ────────────────────────────────────────────────────────
# Hash all source files that affect the CLI build. Skip the build if the
# installed binary's fingerprint matches.

FINGERPRINT_FILE="$INSTALL_DIR/.fingerprint"

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
  if [ "$CURRENT_FP" = "$STORED_FP" ] && [ -f "$INSTALL_DIR/$BIN_NAME" ]; then
    echo "psychological-operations is up to date (fingerprint: ${CURRENT_FP:0:12}...)"
    exit 0
  fi
fi

# ── Build embedded dependencies ────────────────────────────────────────
# The CLI embeds the upstream Chromium snapshot bundle + packed extension via
# build.rs (guarded on its own fingerprint, so re-runs are no-ops).

echo "Building embedded dependencies..."
bash "$REPO_ROOT/psychological-operations-chromium/build.sh" --release

# ── Build CLI ──────────────────────────────────────────────────────────

echo "Building psychological-operations-cli (release)..."
cargo build --release -p psychological-operations-cli \
  --manifest-path "$REPO_ROOT/Cargo.toml"

SRC="$REPO_ROOT/target/release/$BIN_NAME"
if [ ! -f "$SRC" ]; then
  echo "ERROR: expected binary at $SRC" >&2
  exit 1
fi

# ── Install ────────────────────────────────────────────────────────────

mkdir -p "$INSTALL_DIR"
# `mv` onto a running Windows exe fails ("in use"); prefer `cp` so a
# later install over an in-use binary degrades to a clearer error.
cp "$SRC" "$INSTALL_DIR/$BIN_NAME"
chmod +x "$INSTALL_DIR/$BIN_NAME"
echo "$CURRENT_FP" > "$FINGERPRINT_FILE"
echo "Installed $INSTALL_DIR/$BIN_NAME"

# ── PATH ───────────────────────────────────────────────────────────────
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
  echo "" >> "$shell_rc"
  echo "# psychological-operations CLI" >> "$shell_rc"
  echo "$line" >> "$shell_rc"
  echo "Added to PATH in $shell_rc"
}

write_env_file

case "$PLATFORM" in
  windows)
    INSTALL_DIR_WIN="$(cygpath -w "$INSTALL_DIR")"
    CURRENT_PATH=$(powershell.exe -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path', 'User')" 2>/dev/null | tr -d '\r')
    if echo "$CURRENT_PATH" | grep -qiF '.psychological-operations'; then
      echo "PATH already contains $INSTALL_DIR_WIN"
    else
      powershell.exe -NoProfile -Command \
        "[Environment]::SetEnvironmentVariable('Path', '$INSTALL_DIR_WIN;' + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')" 2>/dev/null
      echo "Added $INSTALL_DIR_WIN to user PATH (restart cmd/PowerShell to use it)."
    fi
    # Also wire up Git Bash / MSYS via the env file.
    if [ -f "$HOME/.bashrc" ]; then
      add_to_path "$HOME/.bashrc"
    fi
    ;;
  macos)
    add_to_path "$HOME/.zshrc"
    ;;
  linux)
    if [ -f "$HOME/.bashrc" ]; then
      add_to_path "$HOME/.bashrc"
    fi
    if [ -f "$HOME/.zshrc" ]; then
      add_to_path "$HOME/.zshrc"
    fi
    ;;
esac

echo ""
echo "Done!"
echo ""
echo "To use psychological-operations in your current shell, run:"
echo '  . "$HOME/.psychological-operations/env"'
echo ""
echo "(New shells will pick it up automatically.)"
