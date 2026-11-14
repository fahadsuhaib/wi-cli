#!/bin/bash
# usage: dev/run_tests.sh [scratch-dir]   (default: the agent scratchpad; needs <scratch>/venv with pyte)
# Environment knobs (used by CI to run the slow terminal scripts in parallel on several machines):
#   SKIP_BUILD=1   do not run cargo build/clippy/test (the binary at target/debug/wi is already there)
#   SHARD=k/n      run only shard k of n (1-based), balanced by measured run time (dev/shard.py)
#   LIST=1         only print which terminal runs would execute, then exit
export PATH="$HOME/.cargo/bin:$PATH"
export WICLI_RESTORE_TABS=0  # other scripts must not reopen each other's tabs; pty_restore.py unsets it
cd "$(dirname "$0")/.." || exit 1
S="${1:-/private/tmp/claude-501/-Users-fahad-work-devterm/86bba6c8-edb7-4e2c-8ea5-0b344d693470/scratchpad}"
PY="$S/venv/bin/python"
results=(); failed=0
step() { # name cmd...
  local name="$1"; shift
  echo "=== $name"
  if "$@"; then results+=("PASS $name"); else results+=("FAIL $name"); failed=1; fi
}
run_task() { # script mode
  local t="$1" mode="$2"
  step "$t $mode" "$PY" -I "$t" "$S" "$mode"
}
if [ -z "${SKIP_BUILD:-}" ] && [ -z "${LIST:-}" ]; then
  step "cargo build" cargo build
  step "cargo clippy" cargo clippy --all-targets -- -D warnings
  step "cargo test" cargo test
fi
if [ -n "${SHARD:-}" ]; then
  tasks="$(python3 dev/shard.py "${SHARD%/*}" "${SHARD#*/}")" || exit 1
  while read -r t mode; do
    [ -n "$t" ] || continue
    if [ -n "${LIST:-}" ]; then echo "$t $mode"; continue; fi
    run_task "$t" "$mode"
  done <<< "$tasks"
else
  for t in dev/pty_*.py; do
    [ -e "$t" ] || continue
    case "$(basename "$t")" in pty_drive.py) continue;; esac  # interactive driver, not a test
    if [ -n "${LIST:-}" ]; then echo "$t local"; continue; fi
    run_task "$t" local
  done
fi
[ -n "${LIST:-}" ] && exit 0
echo; echo "=== summary"; printf '%s\n' "${results[@]}"
exit $failed
