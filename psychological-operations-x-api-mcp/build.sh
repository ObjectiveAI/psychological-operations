#!/usr/bin/env bash
# Builds psychological-operations-x-api-mcp and places the binary in
# embed/<target>/<profile>/. Skips the build if the source fingerprint
# hasn't changed. All arguments are forwarded to cargo build.
# Output is captured to .logs/build/psychological-operations-x-api-mcp.txt.
#
# Default target is the rustc host triple — the embedded binary runs on
# the operator's machine (extracted into a temp dir at runtime by the
# CLI), so it must match the CLI's own target.
#
# Usage:
#   bash psychological-operations-x-api-mcp/build.sh [--release] [--target <triple>] [...]

set -euo pipefail

MODULE="psychological-operations-x-api-mcp"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOG_DIR="$REPO_ROOT/.logs/build"
LOG_FILE="$LOG_DIR/$MODULE.txt"

mkdir -p "$LOG_DIR"

run() {
  # Check fingerprint — returns 1 if embed/ is up to date (not an error).
  if ! source "$SCRIPT_DIR/fingerprint.sh" "$@"; then
    return 0
  fi

  # Build with a separate target dir to avoid cargo lock contention with
  # any embedder that's also building. Always pass --target so the output
  # lands in <target-dir>/<triple>/<profile>/.
  TARGET_DIR="$REPO_ROOT/target-$MODULE"
  echo "Building $MODULE ($PROFILE, $TARGET)..."
  if ! cargo build -p "$MODULE" --target-dir "$TARGET_DIR" --target "$TARGET" "$@"; then
    return 1
  fi

  # Copy binary to embed/<target>/<profile>/
  EMBED_DIR="$SCRIPT_DIR/embed/$TARGET/$PROFILE"
  mkdir -p "$EMBED_DIR"

  if [[ "$TARGET" == *"windows"* ]]; then
    BINARY_NAME="$MODULE.exe"
  else
    BINARY_NAME="$MODULE"
  fi

  BUILT="$TARGET_DIR/$TARGET/$PROFILE/$BINARY_NAME"
  if [ ! -f "$BUILT" ]; then
    echo "ERROR: expected binary at $BUILT" >&2
    return 1
  fi

  cp "$BUILT" "$EMBED_DIR/$BINARY_NAME"

  # Stamp the fingerprint only after successful build
  echo "$CURRENT_FP" > "$FINGERPRINT_FILE"
  echo "Build complete (fingerprint: ${CURRENT_FP:0:12}...)"
}

if run "$@" > "$LOG_FILE" 2>&1; then
  echo "$MODULE: SUCCESS"
else
  echo "$MODULE: ERROR (see $LOG_FILE)"
  exit 1
fi
