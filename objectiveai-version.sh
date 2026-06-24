#!/usr/bin/env bash
# objectiveai-version.sh — set the objectiveai-sdk dependency version
# everywhere, in sync.
#
#   bash objectiveai-version.sh <new-version>     e.g. bash objectiveai-version.sh 2.2.4
#
# Updates the objectiveai-sdk pin in every workspace crate that depends on it —
# both the bare `objectiveai-sdk = "X"` (cli) and the table
# `objectiveai-sdk = { version = "X", … }` (sdk, x-mcp, tests) forms — then
# `cargo update`s Cargo.lock to match. (The lock can't just be sed'd like the
# psychological-operations crates: objectiveai-sdk is a crates.io dep, so its
# lock entry carries a checksum that must be refetched.)
#
# Does NOT commit. Requires GNU sed (git-bash on Windows, or Linux).
set -euo pipefail

new="${1:-}"
if [[ -z "$new" ]]; then
  echo "usage: $0 <new-version>" >&2
  exit 1
fi
if [[ ! "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.+][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$new' is not a valid version (expected X.Y.Z)" >&2
  exit 1
fi

cd "$(dirname "$0")"

# Workspace crates that depend on objectiveai-sdk.
TOMLS=(
  psychological-operations-cli/Cargo.toml
  psychological-operations-sdk/Cargo.toml
  psychological-operations-x-mcp/Cargo.toml
  psychological-operations-discord-mcp/Cargo.toml
  psychological-operations-browser/src-tauri/Cargo.toml
  psychological-operations-tests/Cargo.toml
)

for toml in "${TOMLS[@]}"; do
  sed -i -E \
    -e 's/^objectiveai-sdk = "[^"]*"/objectiveai-sdk = "'"$new"'"/' \
    -e 's/^(objectiveai-sdk = \{ version = )"[^"]*"/\1"'"$new"'"/' \
    "$toml"
done
echo "Set objectiveai-sdk = $new in: ${TOMLS[*]}"

# Sync Cargo.lock. Non-fatal: if $new isn't published on crates.io yet, the
# lock will catch up on the next `cargo build` once it is.
echo "==> cargo update -p objectiveai-sdk --precise $new"
if cargo update -p objectiveai-sdk --precise "$new"; then
  echo "Bumped objectiveai-sdk to $new across crate Cargo.tomls + Cargo.lock"
else
  echo "WARNING: couldn't update Cargo.lock to $new (published on crates.io yet?). The Cargo.toml pins are set; re-run \`cargo update -p objectiveai-sdk --precise $new\` once it's available." >&2
fi
