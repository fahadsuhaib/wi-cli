#!/bin/bash
# usage: dev/linux/run.sh [--lang-c] [--bench]
# Builds wi in a Linux container and runs build, clippy, cargo test and the local-mode pty scripts.
# Source is copied (not built in place) so the macOS target/ is never touched; build output lives in the
# named volume wicli-linux-cache.
cd "$(dirname "$0")/../.." || exit 1
ROOT="$PWD"
LANGV=C.UTF-8; EXTRA=""
for a in "$@"; do case "$a" in --lang-c) LANGV=C;; --bench) EXTRA=bench;; esac; done
docker build -q -t wicli-linux -f dev/linux/Dockerfile dev/linux >/dev/null || exit 1
docker run --rm -e LANG="$LANGV" -e EXTRA="$EXTRA" -v "$ROOT":/src:ro -v wicli-linux-cache:/cache wicli-linux bash -c '
rsync -a --delete --exclude target --exclude .git --exclude .worktrees /src/ /work/
cd /work && ln -sfn /cache/target target   # pty scripts look for ../target/debug/wi
S=/tmp/scratch; mkdir -p $S/ws
cat > $S/test.toml <<T
[[box]]
name = "test"
host = "127.0.0.1"
port = 2222
user = "dev"
identity = "/work/dev/id_test"
known_hosts = "$S/known_hosts"
T
results=(); failed=0
step() { local name="$1"; shift; echo "=== $name"
  if "$@"; then results+=("PASS $name"); else results+=("FAIL $name"); failed=1; fi; }
echo "uname: $(uname -srm)  LANG=$LANG  SHELL=${SHELL:-<unset>}"
step "cargo build" cargo build
step "cargo clippy" cargo clippy --all-targets -- -D warnings
step "cargo test" cargo test
for t in dev/pty_*.py; do
  case "$(basename $t)" in pty_drive.py) continue;; esac
  step "$t local" python -I "$t" $S local
done
if [ -n "$EXTRA" ]; then
  step "release build" cargo build --release
  ls -l /cache/target/release/wi
  step "bench" python dev/bench.py $S
fi
echo; echo "=== summary (Linux container)"; printf "%s\n" "${results[@]}"
exit $failed'
