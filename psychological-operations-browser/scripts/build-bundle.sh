#!/usr/bin/env bash
# Build the psychological-operations-browser binary together with its
# CEF runtime, zip them, and stage them under
# embed/<target>/<profile>/ for the CLI's build.rs to include_bytes!.
#
# Usage:
#   bash scripts/build-bundle.sh                # debug, host target
#   bash scripts/build-bundle.sh --release      # release, host target
#   bash scripts/build-bundle.sh --target x86_64-unknown-linux-gnu

set -euo pipefail

RELEASE=0
TARGET=""
SKIP_BUILD=0
NOZIP=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --release) RELEASE=1; shift ;;
        --target)  TARGET="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --no-zip) NOZIP=1; shift ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
BROWSER_ROOT="$( dirname "$SCRIPT_DIR" )"
WORKSPACE_ROOT="$( dirname "$BROWSER_ROOT" )"

PROFILE="debug"
[[ $RELEASE -eq 1 ]] && PROFILE="release"
if [[ -z "$TARGET" ]]; then
    TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi
[[ -z "$TARGET" ]] && { echo "could not determine host target" >&2; exit 1; }

echo "==> build-bundle: target=$TARGET profile=$PROFILE"

if [[ $SKIP_BUILD -eq 0 ]]; then
    pushd "$WORKSPACE_ROOT" >/dev/null
    cargo_args=(build -p psychological-operations-browser --target "$TARGET")
    [[ $RELEASE -eq 1 ]] && cargo_args+=(--release)
    echo "==> cargo ${cargo_args[*]}"
    cargo "${cargo_args[@]}"
    popd >/dev/null
fi

TARGET_DIR="$WORKSPACE_ROOT/target/$TARGET/$PROFILE"
# --no-zip stages the runtime straight into embed/ (the caller zips it); zip
# mode keeps the per-target embed/<triple>/<profile>/staging/ layout (its zip
# lands in embed/<triple>/<profile>/).
if [[ $NOZIP -eq 1 ]]; then
  EMBED_DIR="$BROWSER_ROOT/embed"
  STAGING="$EMBED_DIR"
else
  EMBED_DIR="$BROWSER_ROOT/embed/$TARGET/$PROFILE"
  STAGING="$EMBED_DIR/staging"
fi

[[ -d "$TARGET_DIR" ]] || { echo "target dir not found: $TARGET_DIR" >&2; exit 1; }
mkdir -p "$EMBED_DIR"
rm -rf "$STAGING"
mkdir -p "$STAGING"

# Files to copy: browser exe + lib + CEF runtime. Per-OS extensions
# differ; on Windows the script .ps1 sibling has the authoritative
# Windows list. Linux/macOS lists below.
case "$TARGET" in
    *windows*)
        echo "this is the POSIX sibling — use scripts/build-bundle.ps1 for Windows targets" >&2
        exit 1
        ;;
    *linux*)
        FILES=(
            "psychological-operations-browser"
            "libpsychological_operations_browser_lib.so"
            "libcef.so"
            "chrome-sandbox"
            "icudtl.dat"
            "v8_context_snapshot.bin"
            "chrome_100_percent.pak"
            "chrome_200_percent.pak"
            "resources.pak"
        )
        ENTRY="psychological-operations-browser"
        ;;
    *apple*|*darwin*)
        # macOS uses .app bundles — the entry is inside
        # psychological-operations-browser.app/Contents/MacOS/.
        # Caller copies the whole .app dir from
        # target/<target>/<profile>/bundle/macos/.
        APP_BUNDLE="$WORKSPACE_ROOT/target/$TARGET/$PROFILE/bundle/macos/psychological-operations-browser.app"
        [[ -d "$APP_BUNDLE" ]] || { echo "no .app bundle at $APP_BUNDLE — run cargo tauri build first" >&2; exit 1; }
        cp -R "$APP_BUNDLE" "$STAGING/"
        ENTRY="psychological-operations-browser.app/Contents/MacOS/psychological-operations-browser"
        FILES=()
        ;;
    *)
        echo "unsupported target: $TARGET" >&2
        exit 1
        ;;
esac

for f in "${FILES[@]:-}"; do
    [[ -z "$f" ]] && continue
    src="$TARGET_DIR/$f"
    [[ -e "$src" ]] || { echo "missing runtime file: $src" >&2; exit 1; }
    cp -R "$src" "$STAGING/$f"
done

# CEF locales/ on Linux.
if [[ "$TARGET" == *linux* ]]; then
    [[ -d "$TARGET_DIR/locales" ]] || { echo "missing locales dir" >&2; exit 1; }
    cp -R "$TARGET_DIR/locales" "$STAGING/locales"
fi

echo "==> staged $STAGING"

# Zip the staging dir flat — unless --no-zip, in which case the staging dir
# (embed/<triple>/<profile>/) IS the output and the caller zips it directly.
if [[ $NOZIP -eq 0 ]]; then
  printf "%s" "$ENTRY" > "$EMBED_DIR/browser-entry.txt"
  BUNDLE_ZIP="$EMBED_DIR/browser-bundle.zip"
  rm -f "$BUNDLE_ZIP"
  echo "==> compressing $BUNDLE_ZIP"
  ( cd "$STAGING" && zip -qr "$BUNDLE_ZIP" . )
  BUNDLE_BYTES="$(stat -c%s "$BUNDLE_ZIP" 2>/dev/null || stat -f%z "$BUNDLE_ZIP")"
  echo "==> wrote $BUNDLE_ZIP ($BUNDLE_BYTES bytes)"
  echo "==> wrote $EMBED_DIR/browser-entry.txt"
fi
