#!/usr/bin/env bash
# build-and-test.sh — dev helper. Build everything, then run the test suites.
# Runs build.sh (exits 1 if it fails), then test.sh (exits with test.sh's
# code). Takes no arguments.
set -euo pipefail

if [ "$#" -gt 0 ]; then
  echo "build-and-test.sh: takes no arguments" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

echo "==> build-and-test.sh: build.sh"
if ! bash "$REPO_ROOT/build.sh"; then
  echo "build-and-test.sh: build failed" >&2
  exit 1
fi

echo "==> build-and-test.sh: test.sh"
rc=0
bash "$REPO_ROOT/test.sh" || rc=$?
exit "$rc"
