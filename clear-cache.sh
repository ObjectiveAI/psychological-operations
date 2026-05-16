#!/usr/bin/env bash
# clear-cache.sh — wipe regenerable build caches.
#
# Every path that gets removed is enumerated below. The list is exhaustive
# at the time of writing; if you add a new cache, also add it here.
#
# Preserves (intentional — these are consumed by builds / history, NOT caches):
#
#   - .logs/             (build/test log capture — keep history)
#   - .git/              (obviously)
#   - Source, lockfiles (Cargo.lock), config (objectiveai.json,
#                        .gitignore, *.pem)
#
# Usage:
#   bash clear-cache.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# Snapshot disk before so we can report freed space.
freed_before=$(df -k "$REPO_ROOT" | awk 'NR==2 {print $4}')

# ---------------------------------------------------------------------------
# Cargo build outputs. Two roots: the workspace `target/` and the
# integration test harness's per-process `tests/.target-binaries/` (where
# `psyops_binary()` builds a release psyops binary into the `psyops/`
# subdir and `objectiveai_binary()` caches the downloaded host release
# into `objectiveai-release/`).
# ---------------------------------------------------------------------------
RUST_TARGETS=(
  target
  psychological-operations-cli/tests/.target-binaries
)

for t in "${RUST_TARGETS[@]}"; do
  if [ -L "$t" ]; then
    real=$(readlink "$t")
    echo "Removing $t/ → $real (symlink target)"
    rm -rf -- "$real" 2>/dev/null || true
    rm -f -- "$t"
  elif [ -d "$t" ]; then
    echo "Removing $t/"
    rm -rf -- "$t"
  fi
done

# ---------------------------------------------------------------------------
# Chromium snapshot bundle staged by psychological-operations-chromium/build.sh.
# Re-fetches upstream Chromium + zips the extension; idempotent on rerun
# (fingerprint.sh short-circuits when sources are unchanged).
# ---------------------------------------------------------------------------
TOP_LEVEL_CACHES=(
  psychological-operations-chromium/embed
)

for d in "${TOP_LEVEL_CACHES[@]}"; do
  if [ -d "$d" ]; then
    echo "Removing $d/"
    rm -rf -- "$d"
  fi
done

# ---------------------------------------------------------------------------
# Stale shared state from previous test runs. Each `cargo test` run wipes
# its own per-test `.t-<name>/` dirs under the OS temp dir, but the
# legacy `tests/.objectiveai/` may linger from older runs where the
# harness installed the host CLI into the tree.
# ---------------------------------------------------------------------------
STALE_TEST_STATE=(
  psychological-operations-cli/tests/.objectiveai
)

for d in "${STALE_TEST_STATE[@]}"; do
  if [ -d "$d" ]; then
    echo "Removing $d/"
    rm -rf -- "$d"
  fi
done

freed_after=$(df -k "$REPO_ROOT" | awk 'NR==2 {print $4}')
delta_gb=$(awk -v b="$freed_before" -v a="$freed_after" \
  'BEGIN { printf "%.1f", (a - b) / 1024 / 1024 }')

echo
echo "Done. Freed ${delta_gb} GiB on the repo's filesystem."
