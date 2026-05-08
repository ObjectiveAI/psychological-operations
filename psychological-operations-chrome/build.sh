#!/usr/bin/env bash
# Stages the embedded Chromium bundle + packed extension into
# embed/<target>/<profile>/. Re-run is a no-op via fingerprint
# short-circuit.
#
# Usage:
#   bash psychological-operations-chrome/build.sh [--release] [--target <triple>]
#
# Per-platform Chromium revisions are read from VERSION.

set -euo pipefail

MODULE="psychological-operations-chrome"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOG_DIR="$REPO_ROOT/.logs/build"
LOG_FILE="$LOG_DIR/$MODULE.txt"
EXT_DIR="$REPO_ROOT/psychological-operations-chrome-extension"

mkdir -p "$LOG_DIR"

run() {
  # ── fingerprint short-circuit ───────────────────────────────────────────
  if ! source "$SCRIPT_DIR/fingerprint.sh" "$@"; then
    return 0
  fi

  EMBED_DIR="$SCRIPT_DIR/embed/$TARGET/$PROFILE"
  mkdir -p "$EMBED_DIR"

  # ── download upstream Chromium snapshot ─────────────────────────────────
  CHROMIUM_BUNDLE_ZIP="$EMBED_DIR/chrome-bundle.zip"
  CHROMIUM_URL="https://commondatastorage.googleapis.com/chromium-browser-snapshots/${SNAPSHOT_PLATFORM}/${CHROMIUM_REV}/${CHROMIUM_ZIP}"
  echo "Downloading $CHROMIUM_URL ..."
  curl -fLsS "$CHROMIUM_URL" -o "$CHROMIUM_BUNDLE_ZIP"
  CHROMIUM_BYTES=$(wc -c < "$CHROMIUM_BUNDLE_ZIP" | tr -d ' ')
  echo "  -> $CHROMIUM_BYTES bytes"

  # ── pack extension into a signed CRX3 ──────────────────────────────────
  # crx-pack generates extension-key.pem on first run if it's missing
  # (commit the result so every build derives the same extension ID).
  echo "Packing extension into signed CRX3 ..."
  if [ ! -x "$REPO_ROOT/target/release/crx-pack" ] && [ ! -x "$REPO_ROOT/target/release/crx-pack.exe" ]; then
    (cd "$REPO_ROOT" && cargo build -p crx-pack --release --quiet)
  fi
  CRX_PACK="$REPO_ROOT/target/release/crx-pack"
  [ -x "$CRX_PACK.exe" ] && CRX_PACK="$CRX_PACK.exe"
  "$CRX_PACK" \
    --extension-dir "$EXT_DIR" \
    --key "$SCRIPT_DIR/extension-key.pem" \
    --out "$EMBED_DIR/extension.crx" \
    --id-out "$EMBED_DIR/extension-id.txt"

  # ── also stage the unpacked extension as a tar ─────────────────────────
  # `--load-extension` (v1) requires an unpacked dir at runtime; the
  # tar gets include_bytes!'d into the Rust binary and extracted on
  # first launch alongside the chromium zip.
  EXT_TAR="$EMBED_DIR/extension.tar"
  rm -f "$EXT_TAR"
  (cd "$EXT_DIR" && tar -cf "$EXT_TAR" .)

  # ── write the launch entry path so the Rust side knows what to exec ────
  printf '%s\n' "$CHROMIUM_LAUNCH_REL" > "$EMBED_DIR/launch-entry.txt"

  # ── write a metadata file for diagnostics + provenance ─────────────────
  cat > "$EMBED_DIR/bundle.meta.json" <<EOF
{
  "chromium_rev":       "$CHROMIUM_REV",
  "snapshot_platform":  "$SNAPSHOT_PLATFORM",
  "rust_target":        "$TARGET",
  "profile":            "$PROFILE",
  "chromium_url":       "$CHROMIUM_URL",
  "chromium_bytes":     $CHROMIUM_BYTES,
  "launch_entry_rel":   "$CHROMIUM_LAUNCH_REL"
}
EOF

  # ── stamp fingerprint AFTER successful build ───────────────────────────
  echo "$CURRENT_FP" > "$FINGERPRINT_FILE"
  echo "Build complete (fingerprint: ${CURRENT_FP:0:12}...)"
}

if run "$@" > "$LOG_FILE" 2>&1; then
  echo "$MODULE: SUCCESS"
else
  echo "$MODULE: ERROR (see $LOG_FILE)"
  exit 1
fi
