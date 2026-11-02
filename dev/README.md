# dev/

## Test box (for remote mode)
1. `colima start`
2. `docker build -t devbox-test dev && docker run -d --name devbox-test -p 2222:22 devbox-test`
3. Key is `dev/id_test`; your scratch dir needs a `test.toml` with a `[[box]] name = "test"` entry for 127.0.0.1:2222.
   Python needs `pyte`: `python3 -m venv <scratch>/venv && <scratch>/venv/bin/pip install pyte`.

## Tests
`dev/run_tests.sh [scratch-dir]` runs cargo build, clippy (-D warnings), cargo test (includes tests/cli.rs),
then every `dev/pty_*.py` (except the interactive `pty_drive.py`) in local mode, and remote mode when the
script mentions it and the box answers on port 2222. Prints a summary and exits non-zero on any failure.

## Benchmarks
`cargo build --release && python dev/bench.py <scratch-dir> [--remote]`
Measures wi's own RSS and CPU (the flood is judged in CPU milliseconds per MB of output, budget 120) (ps on the wi pid; child shells excluded) with 10 tabs idle for 20 s, and while
a tab prints 100 MB. Redraw rate is approximate (write bursts at the pty master). Prints measured vs budget,
FAIL when over budget by more than 20%, and exits non-zero on FAIL.

## Linux check
`dev/linux/run.sh [--bench] [--lang-c]` builds an arm64 Debian (rust:slim-bookworm) image with python3 + pyte, copies the
source into the container (the host `target/` is untouched; build output lives in the docker volume `wicli-linux-cache`),
then runs build, clippy, `cargo test` and every `dev/pty_*.py` in local mode, printing a PASS/FAIL summary. `--bench` adds a
release build and `dev/bench.py`. `--lang-c` runs with `LANG=C` instead of `C.UTF-8`. Remote-mode scripts are not run there.
Notes: SHELL is unset in the container so wi uses /bin/sh (dash); `pty_input.py` falls back to bash when zsh is missing.
`pty_mouse.py` is flaky on both macOS and Linux (about 1 run in 3 fails its wheel-down check).
