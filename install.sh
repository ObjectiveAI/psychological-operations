#!/usr/bin/env bash
# psychological-operations plugin installer — downloads a pre-built release binary.
#
#   curl -fsSL https://raw.githubusercontent.com/WiggidyW/psychological-operations/main/install.sh | bash
#
# - Detects platform + architecture.
# - Fetches the latest published release asset from GitHub and drops it
#   at $HOME/.objectiveai/plugins/psychological-operations[.exe] so the
#   objectiveai-cli host dispatches `objectiveai psychological-operations
#   <subcmd>` to us.
# - Our own state lives at $HOME/.objectiveai/plugins/.psychological-operations/.
#
# No toolchain required. For a from-source install, clone the repo and
# run `psychological-operations-cli/install.sh` instead.

set -euo pipefail

REPO="WiggidyW/psychological-operations"
INSTALL_DIR="$HOME/.objectiveai/plugins"

for arg in "$@"; do
  case "$arg" in
    -h|--help)
      sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
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
# (linux-aarch64 is unsupported because upstream Chromium snapshots —
# required by the embedded chromium bundle — don't ship a linux-arm64 build.)
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

# No PATH wiring needed — users invoke via
#   objectiveai psychological-operations <subcmd>
# which objectiveai-cli dispatches to our binary by name lookup
# under $HOME/.objectiveai/plugins/.

echo ""
echo "Done!"
echo ""
echo "Invoke via:"
echo "  objectiveai psychological-operations --help"
