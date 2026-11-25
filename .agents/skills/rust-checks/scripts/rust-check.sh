#!/usr/bin/env bash
# Post-implementation check suite for herdr-spreader (check-only).
# Usage: bash .agents/skills/rust-checks/scripts/rust-check.sh
# Exit 0 only when fmt, clippy (-D warnings), build, and test all pass.
set -u

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO_ROOT" || exit 1

declare -a NAMES CMDS RESULTS
NAMES=("fmt" "clippy" "build" "test")
CMDS=(
  "cargo fmt --check"
  "cargo clippy --all-targets -- -D warnings"
  "cargo build --all-targets"
  "cargo test"
)

failures=0
for i in "${!NAMES[@]}"; do
  name="${NAMES[$i]}"
  echo "==> [${name}] ${CMDS[$i]}"
  if OUT=$(${CMDS[$i]} 2>&1); then
    echo "PASS ${name}"
  else
    echo "FAIL ${name} (exit $?)"
    printf '%s\n' "$OUT" | tail -40
    failures=$((failures + 1))
  fi
  echo
done

echo "summary: ${failures} failed check(s)"
[ "$failures" -eq 0 ]
