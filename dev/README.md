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

## Hidden dev command
`wi __ls [--box NAME] <dir> [--git] [--sort name|size|mtime]` (not in `wi --help`) prints the explorer's data layer as text: lines
`<d|f|l> <size|-> <mtime|-> <hidden 0|1> <name>`, `truncated <n>` past 5,000 entries, and with `--git` `repo <root>`, `git <mark> <path>`,
`dirty <folder>`, `branch <name>` (or `nogit`). `dev/pty_listing.py` drives it locally and against the Docker box (fixtures under
`/home/dev/explorer-test`, removed afterwards).

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

## CI
`.github/workflows/ci.yml` runs on every push to `main` and every pull request. The `linux` job builds the Docker
test box, generates its own SSH key (`dev/id_test` is gitignored; never commit it) and runs `dev/run_tests.sh` in full,
remote mode included; the full log is uploaded as an artifact. The `macos` job runs build, clippy and `cargo test`
(required) and the local-mode pty scripts (not required yet). `.github/workflows/release.yml` builds
`wi-linux-x86_64.tar.gz`, `wi-linux-aarch64.tar.gz` and `wi-macos-universal.tar.gz` with `scripts/package.sh` when a
`v*` tag is pushed (the tag must equal the Cargo.toml version) and publishes them with `SHA256SUMS`.
Try a package locally with `scripts/package.sh macos-universal dist`, then `WICLI_DOWNLOAD_URL=file://$PWD/dist` with
a `SHA256SUMS` in it to test `install.sh`.
