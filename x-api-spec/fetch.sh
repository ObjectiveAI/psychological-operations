#!/usr/bin/env bash
# Fetches the X API v2 OpenAPI spec and pins it with a sha256.
#
# Usage:
#   bash x-api-spec/fetch.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SPEC_URL="https://api.x.com/2/openapi.json"
OUT_FILE="$SCRIPT_DIR/openapi.json"
META_FILE="$SCRIPT_DIR/openapi.meta.json"

echo "Fetching $SPEC_URL ..."
curl -fLsS "$SPEC_URL" -o "$OUT_FILE"

if command -v sha256sum >/dev/null 2>&1; then
  SHA=$(sha256sum "$OUT_FILE" | awk '{print $1}')
else
  SHA=$(shasum -a 256 "$OUT_FILE" | awk '{print $1}')
fi

BYTES=$(wc -c < "$OUT_FILE" | tr -d ' ')
FETCHED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

cat > "$META_FILE" <<EOF
{
  "source_url": "$SPEC_URL",
  "fetched_at": "$FETCHED_AT",
  "sha256": "$SHA",
  "bytes": $BYTES
}
EOF

echo "Wrote $OUT_FILE ($BYTES bytes)"
echo "sha256: $SHA"
echo "Wrote $META_FILE"
