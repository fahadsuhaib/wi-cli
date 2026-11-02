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
Measures wi's own RSS and CPU (ps on the wi pid; child shells excluded) with 10 tabs idle for 20 s, and while
a tab prints 100 MB. Redraw rate is approximate (write bursts at the pty master). Prints measured vs budget,
FAIL when over budget by more than 20%, and exits non-zero on FAIL.
