#!/usr/bin/env bash
#
# macOS bundling pipeline for psychological-operations-browser.
#
# CEF on macOS requires:
#   - A helper subprocess executable wrapped in its own .app
#     bundle, placed inside the main app's Contents/Frameworks/.
#   - The Chromium Embedded Framework.framework bundle in the same
#     Contents/Frameworks/ dir.
#
# Tauri's bundler doesn't know about CEF, so this script handles
# the helper bundle + framework placement after `tauri build` has
# produced the main .app.
#
# Usage:
#
#   cd psychological-operations-browser
#   ./scripts/build-macos.sh [debug|release]
#
# Defaults to `release`. CI invokes with `release`.

set -euo pipefail

PROFILE="${1:-release}"
APP_NAME="psychological-operations-browser"
HELPER_NAME="psychological_operations_browser_helper"

# Repo root = parent of scripts/ + parent again (we live in
# psychological-operations-browser/scripts/, root is two up).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TARGET_DIR="$REPO_ROOT/target/$PROFILE"

echo "==> Building bundle-cef-app helper (one-time per checkout)"
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" --bin bundle-cef-app --$PROFILE
BUNDLE_TOOL="$TARGET_DIR/bundle-cef-app"

echo "==> Building helper binary"
cargo build --manifest-path "$REPO_ROOT/psychological-operations-browser/src-tauri/Cargo.toml" \
    --bin "$HELPER_NAME" --$PROFILE

echo "==> Building main app via Tauri"
cd "$REPO_ROOT/psychological-operations-browser"
yarn build
cargo tauri build --$PROFILE

# Tauri produces the .app at one of these paths depending on
# version + target. Find it.
MAIN_APP=""
for candidate in \
    "$REPO_ROOT/target/$PROFILE/bundle/macos/$APP_NAME.app" \
    "$REPO_ROOT/target/$PROFILE/bundle/macos/${APP_NAME}.app" \
    "$REPO_ROOT/psychological-operations-browser/src-tauri/target/$PROFILE/bundle/macos/$APP_NAME.app"; do
    if [ -d "$candidate" ]; then
        MAIN_APP="$candidate"
        break
    fi
done
if [ -z "$MAIN_APP" ]; then
    echo "ERROR: couldn't locate Tauri-produced .app bundle. Searched standard paths." >&2
    exit 1
fi
echo "==> Found main .app: $MAIN_APP"

FRAMEWORKS="$MAIN_APP/Contents/Frameworks"
mkdir -p "$FRAMEWORKS"

echo "==> Bundling helper into $HELPER_NAME.app via bundle-cef-app"
# bundle-cef-app emits target/$PROFILE/bundle/$HELPER_NAME.app.
"$BUNDLE_TOOL" "$HELPER_NAME" \
    -o "$REPO_ROOT/target/$PROFILE/bundle" \
    $([ "$PROFILE" = "release" ] && echo "--release")
HELPER_APP="$REPO_ROOT/target/$PROFILE/bundle/$HELPER_NAME.app"
if [ ! -d "$HELPER_APP" ]; then
    echo "ERROR: bundle-cef-app didn't produce $HELPER_APP" >&2
    exit 1
fi

echo "==> Copying helper .app into main app's Frameworks/"
rm -rf "$FRAMEWORKS/$HELPER_NAME.app"
cp -R "$HELPER_APP" "$FRAMEWORKS/"

echo "==> Locating CEF framework"
# cef-dll-sys extracts the framework somewhere under
# target/$PROFILE/build/cef-dll-sys-*/out/cef_macos_*/.
CEF_FRAMEWORK_SRC=$(find "$REPO_ROOT/target/$PROFILE/build" \
    -maxdepth 6 \
    -type d \
    -name "Chromium Embedded Framework.framework" \
    | head -1)
if [ -z "$CEF_FRAMEWORK_SRC" ]; then
    echo "ERROR: couldn't find Chromium Embedded Framework.framework under target/" >&2
    exit 1
fi
echo "==> Found framework at: $CEF_FRAMEWORK_SRC"

echo "==> Copying CEF framework into main app's Frameworks/"
rm -rf "$FRAMEWORKS/Chromium Embedded Framework.framework"
cp -R "$CEF_FRAMEWORK_SRC" "$FRAMEWORKS/"

echo "==> Done. Bundled app at: $MAIN_APP"
echo "==> Verify with: open '$MAIN_APP' --args --config-base-dir <dir> --x-app"
