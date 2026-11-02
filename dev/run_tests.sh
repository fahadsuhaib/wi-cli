#!/bin/bash
# usage: dev/run_tests.sh [scratch-dir]   (default: the agent scratchpad; needs <scratch>/venv with pyte and test.toml)
# Environment knobs (used by CI to run the slow terminal scripts in parallel on several machines):
#   SKIP_BUILD=1   do not run cargo build/clippy/test (the binary at target/debug/wi is already there)
#   SHARD=k/n      run only shard k of n (1-based), balanced by measured run time (dev/shard.py)
#   LOCAL_ONLY=1   with SHARD: leave out the remote runs (no test box available)
#   LIST=1         only print which terminal runs would execute, then exit
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.." || exit 1
S="${1:-/private/tmp/claude-501/-Users-fahad-work-devterm/86bba6c8-edb7-4e2c-8ea5-0b344d693470/scratchpad}"
PY="$S/venv/bin/python"
results=(); failed=0
step() { # name cmd...
  local name="$1"; shift
  echo "=== $name"
  if "$@"; then results+=("PASS $name"); else results+=("FAIL $name"); failed=1; fi
}
box_up() { (exec 3<>/dev/tcp/127.0.0.1/2222) 2>/dev/null; }
run_task() { # script mode
  local t="$1" mode="$2"
  if [ "$mode" = remote ] && ! box_up; then
    results+=("SKIP $t remote (test box not reachable on 127.0.0.1:2222)"); return
  fi
  step "$t $mode" "$PY" -I "$t" "$S" "$mode"
}
if [ -z "${SKIP_BUILD:-}" ] && [ -z "${LIST:-}" ]; then
  step "cargo build" cargo build
  step "cargo clippy" cargo clippy --all-targets -- -D warnings
  step "cargo test" cargo test
fi
if [ -n "${SHARD:-}" ]; then
  flag=""; [ -n "${LOCAL_ONLY:-}" ] && flag="--local-only"
  # shellcheck disable=SC2086
  tasks="$(python3 dev/shard.py "${SHARD%/*}" "${SHARD#*/}" $flag)" || exit 1
  while read -r t mode; do
    [ -n "$t" ] || continue
    if [ -n "${LIST:-}" ]; then echo "$t $mode"; continue; fi
    run_task "$t" "$mode"
  done <<< "$tasks"
else
  for t in dev/pty_*.py; do
    [ -e "$t" ] || continue
    case "$(basename "$t")" in pty_drive.py) continue;; esac  # interactive driver, not a test
    if [ -n "${LIST:-}" ]; then echo "$t local"; grep -q '^# remote: yes' "$t" && echo "$t remote"; continue; fi
    run_task "$t" local
    if grep -q '^# remote: yes' "$t"; then run_task "$t" remote; fi
  done
fi
[ -n "${LIST:-}" ] && exit 0
echo; echo "=== summary"; printf '%s\n' "${results[@]}"
exit $failed
