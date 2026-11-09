#!/usr/bin/env bash
# usage: scripts/package-audio.sh <macos-arm64|linux-x86_64|linux-aarch64> <outdir>
# Builds audio/ (the separate `wi-audio` program) natively on this machine and packs it as
# <outdir>/wi-audio-<name>.tar.gz (+ .sha256). Needs cmake and a C++ compiler; on Linux also the
# ALSA development files (libasound2-dev). It is built natively, not with musl: the audio library
# and the C++ runtime are linked dynamically against the system, so the binary is not static.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <macos-arm64|linux-x86_64|linux-aarch64> <outdir>" >&2; exit 2; }
NAME=$1
OUT=$2
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

host_os=$(uname -s); host_arch=$(uname -m)
case "$NAME:$host_os:$host_arch" in
  macos-arm64:Darwin:arm64) ;;
  linux-x86_64:Linux:x86_64) ;;
  linux-aarch64:Linux:aarch64|linux-aarch64:Linux:arm64) ;;
  macos-arm64:*|linux-x86_64:*|linux-aarch64:*) echo "$NAME must be built on a matching host (this is $host_os/$host_arch)" >&2; exit 1 ;;
  *) echo "unknown name '$NAME'" >&2; exit 2 ;;
esac

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PKG="$STAGE/wi-audio-$NAME"
mkdir -p "$PKG"

if [ "$host_os" = Darwin ]; then export MACOSX_DEPLOYMENT_TARGET=11.0; fi
(cd audio && cargo build --release --locked)
cp audio/target/release/wi-audio "$PKG/wi-audio"
strip "$PKG/wi-audio"
chmod 755 "$PKG/wi-audio"
cp docs/AUDIO.md "$PKG/AUDIO.md"
cp LICENSE "$PKG/LICENSE"

ASSET="wi-audio-$NAME.tar.gz"
tar -C "$STAGE" -czf "$OUT/$ASSET" "wi-audio-$NAME"
(cd "$OUT" && sha256 "$ASSET" > "$ASSET.sha256")

listing=$(tar -tzf "$OUT/$ASSET")
for f in "wi-audio-$NAME/wi-audio" "wi-audio-$NAME/AUDIO.md"; do
  echo "$listing" | grep -qx "$f" || { echo "archive is missing $f" >&2; exit 1; }
done
(cd "$OUT" && if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$ASSET.sha256"; else shasum -a 256 -c "$ASSET.sha256"; fi)

"$PKG/wi-audio" --version
if [ "$host_os" = Linux ]; then ldd "$PKG/wi-audio" || true; else otool -L "$PKG/wi-audio" | head -20; fi
echo "produced:"
ls -l "$OUT/$ASSET" "$OUT/$ASSET.sha256"
cat "$OUT/$ASSET.sha256"
