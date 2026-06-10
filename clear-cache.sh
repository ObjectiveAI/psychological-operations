#!/usr/bin/env bash
# clear-cache.sh — wipe regenerable build caches (contents-only).
#
# Directory targets are EMPTIED in place — the folder node itself (and,
# for a symlinked target, the symlink + its real dir) survives, so mount
# points, held inodes, and symlinks stay intact. File targets are removed.
#
# Every path that gets wiped is enumerated below. The list is exhaustive
# at the time of writing; if you add a new cache, also add it here.
#
# Preserves (intentional — these are consumed by builds / history, NOT caches):
#
#   - .logs/             (build/test log capture — keep history)
#   - .git/              (obviously)
#   - psychological-operations-chromium/embed/
#                        (staged upstream chromium snapshot — multi-hundred-MB
#                        download from commondatastorage; reproducible but slow.
#                        build.sh's fingerprint short-circuits when sources are
#                        unchanged, so re-runs are cheap anyway.)
#   - Source, lockfiles (Cargo.lock), config (objectiveai.json,
#                        .gitignore, *.pem)
#
# Usage:
#   bash clear-cache.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# wipe_path — clear a cache location WITHOUT removing the node itself.
#
#   - directory (or symlink to one) → delete everything inside it,
#     preserving the (now-empty) directory. For a symlink we resolve to
#     the physical dir first (`cd` + `pwd -P`), so the symlink AND its
#     real target both survive — just emptied. Keeps mount points, held
#     inodes, and symlinks intact.
#   - regular file (e.g. the viewer .zip) or dangling symlink → removed
#     outright; a file has no "contents" to preserve.
#   - missing path → skipped silently.
wipe_path() {
  local p="$1"
  if [ -d "$p" ]; then
    local real
    real=$(cd "$p" && pwd -P)
    echo "Emptying $p/"
    find "$real" -mindepth 1 -delete
  elif [ -e "$p" ] || [ -L "$p" ]; then
    echo "Removing $p"
    rm -f -- "$p"
  fi
}

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
  wipe_path "$t"
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
  wipe_path "$d"
done

# ---------------------------------------------------------------------------
# Viewer build outputs — node_modules + dist + the zipped release asset.
# `pnpm install` re-fetches; `build.sh` re-produces the zip.
# ---------------------------------------------------------------------------
VIEWER_CACHES=(
  psychological-operations-viewer/node_modules
  psychological-operations-viewer/dist
  psychological-operations-viewer.zip
)

for p in "${VIEWER_CACHES[@]}"; do
  wipe_path "$p"
done

freed_after=$(df -k "$REPO_ROOT" | awk 'NR==2 {print $4}')
delta_gb=$(awk -v b="$freed_before" -v a="$freed_after" \
  'BEGIN { printf "%.1f", (a - b) / 1024 / 1024 }')

echo
echo "Done. Freed ${delta_gb} GiB on the repo's filesystem."
