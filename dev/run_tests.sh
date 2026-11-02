#!/bin/bash
# usage: dev/run_tests.sh [scratch-dir]   (default: the agent scratchpad; needs <scratch>/venv with pyte and test.toml)
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
step "cargo build" cargo build
step "cargo clippy" cargo clippy --all-targets -- -D warnings
step "cargo test" cargo test
for t in dev/pty_*.py; do
  [ -e "$t" ] || continue
  case "$(basename "$t")" in pty_drive.py) continue;; esac  # interactive driver, not a test
  step "$t local" "$PY" -I "$t" "$S" local
  if grep -q remote "$t"; then
    if (exec 3<>/dev/tcp/127.0.0.1/2222) 2>/dev/null; then step "$t remote" "$PY" -I "$t" "$S" remote
    else results+=("SKIP $t remote (test box not reachable on 127.0.0.1:2222)"); fi
  fi
done
echo; echo "=== summary"; printf '%s\n' "${results[@]}"
exit $failed
