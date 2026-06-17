#!/usr/bin/env bash
#
# Bump the psychological-operations version everywhere, in sync.
#
#   bash version.sh <new-version>      e.g.  bash version.sh 0.2.0
#
# Sets one version across the whole project:
#   - every workspace crate's [package] version (Cargo.toml)
#   - their Cargo.lock entries
#   - objectiveai.json                  the plugin manifest (what
#                                        `plugins install` reads + downloads)
#   - psychological-operations-viewer/package.json   the viewer bundle
#
# The CLI's version is the release source of truth: the release workflow
# reads it, and the manifest + viewer must match it (the workflow's
# prepare job fails loudly otherwise). The x-api MCP server reports its
# version via CARGO_PKG_VERSION, so no code literal needs bumping.
#
# Pure sed, no compile. Does NOT commit. Requires GNU sed (git-bash on
# Windows, or Linux).
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

# Workspace crates (package name + Cargo.toml path).
CRATES=(
  psychological-operations-db
  psychological-operations-sdk
  psychological-operations-cli
  psychological-operations-x-api-mcp
  psychological-operations-browser
  psychological-operations-tests
)
TOMLS=(
  psychological-operations-db/Cargo.toml
  psychological-operations-sdk/Cargo.toml
  psychological-operations-cli/Cargo.toml
  psychological-operations-x-api-mcp/Cargo.toml
  psychological-operations-browser/src-tauri/Cargo.toml
  psychological-operations-tests/Cargo.toml
)

# Each crate's [package] version — the first `version = "..."` line (deps
# carry their version inside `{ ... }`, never at column 0, so this is safe).
for toml in "${TOMLS[@]}"; do
  sed -i -E '0,/^version = "[^"]*"/ s//version = "'"$new"'"/' "$toml"
done

# Cargo.lock — the `version` line directly after each crate's name line.
for name in "${CRATES[@]}"; do
  sed -i -E '/^name = "'"$name"'"$/{n;s/^version = "[^"]*"/version = "'"$new"'"/}' Cargo.lock
done

# Plugin manifest + viewer bundle (top-level "version", first occurrence).
sed -i -E '0,/"version": "[^"]*"/ s//"version": "'"$new"'"/' objectiveai.json
sed -i -E '0,/"version": "[^"]*"/ s//"version": "'"$new"'"/' psychological-operations-viewer/package.json

echo "Bumped to $new across all crates + Cargo.lock + objectiveai.json + viewer package.json"
