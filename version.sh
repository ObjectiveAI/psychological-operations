#!/usr/bin/env bash
# version.sh — set the version of every psychological-operations package
# (and inter-package dependency reference) to a single value. Touches the
# workspace Cargo.toml files and the Chrome extension manifest. Skips
# Cargo.lock — that regenerates on next build.
#
# Usage:
#   bash version.sh <new-version>
# Example:
#   bash version.sh 0.99

set -euo pipefail

if [ "$#" -ne 1 ] || [ -z "${1:-}" ]; then
  echo "Usage: $0 <new-version>" >&2
  echo "Example: $0 0.99" >&2
  exit 1
fi

NEW_VERSION="$1"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Primitives
# ---------------------------------------------------------------------------
# awk-based so the script is portable between GNU and BSD toolchains
# (macOS sed's `-i` flag and GNU sed's `0,/pat/` range syntax don't agree).

# Rewrite the first line matching $pat with the literal replacement $repl.
# If no match is found, the file is left unchanged (no error).
first_line_replace() {
  local file="$1"
  local pat="$2"
  local repl="$3"
  local tmp
  tmp=$(mktemp)
  awk -v pat="$pat" -v repl="$repl" '
    !done && $0 ~ pat { print repl; done=1; next }
    { print }
  ' "$file" > "$tmp"
  mv "$tmp" "$file"
}

# On each line matching $line_pat, replace every occurrence of $token_pat
# with $token_repl. Other lines pass through unchanged.
inline_substitute() {
  local file="$1"
  local line_pat="$2"
  local token_pat="$3"
  local token_repl="$4"
  local tmp
  tmp=$(mktemp)
  awk -v lp="$line_pat" -v tp="$token_pat" -v tr="$token_repl" '
    $0 ~ lp { gsub(tp, tr) }
    { print }
  ' "$file" > "$tmp"
  mv "$tmp" "$file"
}

# ---------------------------------------------------------------------------
# Per-file-type updaters
# ---------------------------------------------------------------------------

# Cargo.toml [package] version. The first `^version = "..."` line is the
# package version; third-party dependency version specs never appear at
# column 0 without a leading `= {`, so this targets the right line.
set_toml_package_version() {
  local file="$1"
  first_line_replace "$file" \
    '^version = "[^"]+"' \
    "version = \"$NEW_VERSION\""
}

# Inter-package Cargo.toml dependency version pins.
# Matches lines like:
#   psychological-operations-cli = { path = "...", version = "X.Y.Z", ... }
# ...and rewrites the `version = "..."` token inside. Anchored on
# `^psychological-operations` so it never touches `objectiveai = { ... }`
# or other unrelated deps.
set_cargo_psyops_deps() {
  local file="$1"
  inline_substitute "$file" \
    '^psychological-operations(-[a-zA-Z0-9_-]+)?[[:space:]]*=' \
    'version = "[0-9][^"]*"' \
    "version = \"$NEW_VERSION\""
}

# Chrome manifest.json `"version": "..."` — first occurrence in the file
# (manifest layout puts it near the top, right after `"name"`).
set_manifest_json_version() {
  local file="$1"
  first_line_replace "$file" \
    '^[[:space:]]*"version":[[:space:]]*"[^"]+"' \
    "  \"version\": \"$NEW_VERSION\","
}

# ---------------------------------------------------------------------------
# File lists
# ---------------------------------------------------------------------------

CARGO_TOMLS=(
  psychological-operations-cli/Cargo.toml
  psychological-operations-mcp/Cargo.toml
  psychological-operations-chrome/crx-pack/Cargo.toml
)

MANIFEST_JSONS=(
  psychological-operations-chrome-extension/manifest.json
)

# ---------------------------------------------------------------------------
# Apply
# ---------------------------------------------------------------------------

update() {
  local kind="$1"
  local rel="$2"
  local file="$REPO_ROOT/$rel"
  if [ ! -f "$file" ]; then
    echo "  skip      $rel (not found)"
    return
  fi
  echo "  $kind  $rel"
  case "$kind" in
    cargo)
      set_toml_package_version "$file"
      set_cargo_psyops_deps "$file"
      ;;
    manifest)
      set_manifest_json_version "$file"
      ;;
  esac
}

echo "Setting version to $NEW_VERSION"

for rel in "${CARGO_TOMLS[@]}";    do update cargo    "$rel"; done
for rel in "${MANIFEST_JSONS[@]}"; do update manifest "$rel"; done

echo
echo "Done. Cargo.lock will refresh on next build."
