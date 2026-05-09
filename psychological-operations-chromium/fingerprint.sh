#!/usr/bin/env bash
# Computes a SHA256 fingerprint of all source files that affect the
# embedded Chromium bundle + packed extension. Mirrors the SDK-runner
# fingerprint scheme.
#
# Usage:
#   source fingerprint.sh [--target <triple>] [--release]
#
# Exports: CURRENT_FP, FINGERPRINT_FILE, TARGET, PROFILE, CHROMIUM_REV,
#          SNAPSHOT_PLATFORM, CHROMIUM_ZIP, CHROMIUM_LAUNCH_REL
# Returns 0 if fingerprint changed (build needed), 1 if up to date.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
EXT_DIR="$REPO_ROOT/psychological-operations-chromium-extension"

# Parse --target and --release from args
TARGET=""
PROFILE="debug"
prev_was_target=0
for arg in "$@"; do
  if [ "$prev_was_target" = "1" ]; then
    TARGET="$arg"
    prev_was_target=0
    continue
  fi
  prev_was_target=0
  [ "$arg" = "--target" ] && prev_was_target=1
  [ "$arg" = "--release" ] && PROFILE="release"
done
if [ -z "$TARGET" ]; then
  TARGET=$(rustc -vV | grep '^host:' | awk '{print $2}')
fi

# Map Rust target triple -> Chromium snapshot platform dir + zip name
# + launch entry path (relative to the extracted zip's top dir).
case "$TARGET" in
  x86_64-pc-windows-msvc|x86_64-pc-windows-gnu)
    SNAPSHOT_PLATFORM="Win_x64"
    CHROMIUM_ZIP="chrome-win.zip"
    CHROMIUM_LAUNCH_REL="chrome-win/chrome.exe"
    ;;
  i686-pc-windows-msvc|i686-pc-windows-gnu)
    SNAPSHOT_PLATFORM="Win"
    CHROMIUM_ZIP="chrome-win.zip"
    CHROMIUM_LAUNCH_REL="chrome-win/chrome.exe"
    ;;
  x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl)
    SNAPSHOT_PLATFORM="Linux_x64"
    CHROMIUM_ZIP="chrome-linux.zip"
    CHROMIUM_LAUNCH_REL="chrome-linux/chrome"
    ;;
  aarch64-apple-darwin)
    SNAPSHOT_PLATFORM="Mac_Arm"
    CHROMIUM_ZIP="chrome-mac.zip"
    CHROMIUM_LAUNCH_REL="chrome-mac/Chromium.app/Contents/MacOS/Chromium"
    ;;
  x86_64-apple-darwin)
    SNAPSHOT_PLATFORM="Mac"
    CHROMIUM_ZIP="chrome-mac.zip"
    CHROMIUM_LAUNCH_REL="chrome-mac/Chromium.app/Contents/MacOS/Chromium"
    ;;
  *)
    echo "ERROR: unsupported target '$TARGET' (no Chromium snapshot platform mapping)" >&2
    return 2 2>/dev/null || exit 2
    ;;
esac

# Resolve CHROMIUM_REV from VERSION via the SNAPSHOT_PLATFORM key.
# Each platform pipeline runs independently and may publish at
# different revs, so VERSION holds one entry per platform.
VERSION_KEY=$(echo "$SNAPSHOT_PLATFORM" | tr '[:lower:]' '[:upper:]')
CHROMIUM_REV=$(grep -E "^${VERSION_KEY}=" "$SCRIPT_DIR/VERSION" | head -n1 | cut -d= -f2 | tr -d '[:space:]')
if [ -z "$CHROMIUM_REV" ]; then
  echo "ERROR: VERSION has no entry for $VERSION_KEY (target '$TARGET')" >&2
  return 2 2>/dev/null || exit 2
fi

EMBED_DIR="$SCRIPT_DIR/embed/$TARGET/$PROFILE"
FINGERPRINT_FILE="$EMBED_DIR/.fingerprint"

if command -v sha256sum >/dev/null 2>&1; then
  _sha256() { sha256sum "$@"; }
else
  _sha256() { shasum -a 256 "$@"; }
fi

compute_fingerprint() {
  {
    echo "PROFILE=$PROFILE"
    echo "TARGET=$TARGET"
    echo "CHROMIUM_REV=$CHROMIUM_REV"
    echo "SNAPSHOT_PLATFORM=$SNAPSHOT_PLATFORM"
    echo "FILE=$SCRIPT_DIR/VERSION"
    echo "FILE=$SCRIPT_DIR/build.sh"
    # Hash every file inside the extension dir so a change to any
    # popup/content/background script triggers a rebuild.
    if [ -d "$EXT_DIR" ]; then
      while IFS= read -r f; do
        echo "FILE=$f"
      done < <(find "$EXT_DIR" -type f -not -name '.DS_Store' | sort)
    fi
  } | while IFS= read -r line; do
    case "$line" in
      FILE=*)
        f="${line#FILE=}"
        if [ -f "$f" ]; then
          relpath="${f#"$REPO_ROOT/"}"
          printf '%s\n' "$relpath"
          _sha256 "$f" | awk '{print $1}'
        else
          printf '%s\n' "$f"
        fi
        ;;
      *)
        printf '%s\n' "$line"
        ;;
    esac
  done | _sha256 | awk '{print $1}'
}

CURRENT_FP=$(compute_fingerprint)
export CURRENT_FP FINGERPRINT_FILE TARGET PROFILE
export CHROMIUM_REV SNAPSHOT_PLATFORM CHROMIUM_ZIP CHROMIUM_LAUNCH_REL

if [ -f "$FINGERPRINT_FILE" ]; then
  STORED_FP=$(cat "$FINGERPRINT_FILE")
  if [ "$CURRENT_FP" = "$STORED_FP" ]; then
    echo "embed/$TARGET/$PROFILE is up to date (fingerprint: ${CURRENT_FP:0:12}...)"
    return 1 2>/dev/null || exit 1
  fi
  echo "Fingerprint changed: ${STORED_FP:0:12}... -> ${CURRENT_FP:0:12}..."
fi
