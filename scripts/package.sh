#!/usr/bin/env bash
# usage: scripts/package.sh <target> <outdir>
#   target: x86_64-unknown-linux-musl | aarch64-unknown-linux-musl | macos-universal
# Builds a release binary and packs it as <outdir>/wi-<name>.tar.gz (+ .sha256), where <name> is
# linux-x86_64, linux-aarch64 or macos-universal. Works on macOS and Linux.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <x86_64-unknown-linux-musl|aarch64-unknown-linux-musl|macos-universal> <outdir>" >&2; exit 2; }
TARGET=$1
OUT=$2
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

case "$TARGET" in
  x86_64-unknown-linux-musl)  NAME=linux-x86_64 ;;
  aarch64-unknown-linux-musl) NAME=linux-aarch64 ;;
  macos-universal)            NAME=macos-universal ;;
  *) echo "unknown target '$TARGET'" >&2; exit 2 ;;
esac

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
host_os=$(uname -s); host_arch=$(uname -m)

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PKG="$STAGE/wi-$NAME"
mkdir -p "$PKG/docs"

if [ "$TARGET" = macos-universal ]; then
  [ "$host_os" = Darwin ] || { echo "macos-universal needs a macOS host (lipo)" >&2; exit 1; }
  export MACOSX_DEPLOYMENT_TARGET=11.0   # for cargo and the C compiler (aws-lc-sys)
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --target "$t"
  done
  lipo -create -output "$PKG/wi" target/aarch64-apple-darwin/release/wi target/x86_64-apple-darwin/release/wi
  strip "$PKG/wi"
  info=$(lipo -info "$PKG/wi")
  echo "$info"
  for arch in arm64 x86_64; do
    lipo -thin "$arch" "$PKG/wi" -output "$STAGE/slice-$arch"
    minos=$(otool -l "$STAGE/slice-$arch" | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}')
    echo "$arch minos $minos"
    [ "$minos" = 11.0 ] || { echo "$arch slice has minos '$minos', expected 11.0" >&2; exit 1; }
  done
  case "$info" in *x86_64*arm64*|*arm64*x86_64*) ;; *) echo "universal binary is missing an architecture" >&2; exit 1 ;; esac
else
  cargo build --release --locked --target "$TARGET"
  cp "target/$TARGET/release/wi" "$PKG/wi"
  strip "$PKG/wi"
fi
chmod 755 "$PKG/wi"
cp README.md "$PKG/README.md"
cp docs/KEYBOARD.md "$PKG/docs/KEYBOARD.md"

ASSET="wi-$NAME.tar.gz"
tar -C "$STAGE" -czf "$OUT/$ASSET" "wi-$NAME"
(cd "$OUT" && sha256 "$ASSET" > "$ASSET.sha256")

# Verify the output.
listing=$(tar -tzf "$OUT/$ASSET")
for f in "wi-$NAME/wi" "wi-$NAME/README.md" "wi-$NAME/docs/KEYBOARD.md"; do
  echo "$listing" | grep -qx "$f" || { echo "archive is missing $f" >&2; exit 1; }
done
(cd "$OUT" && if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$ASSET.sha256"; else shasum -a 256 -c "$ASSET.sha256"; fi)

# Run it when the target matches this host.
runnable=no
case "$TARGET:$host_os:$host_arch" in
  macos-universal:Darwin:*) runnable=yes ;;
  x86_64-unknown-linux-musl:Linux:x86_64) runnable=yes ;;
  aarch64-unknown-linux-musl:Linux:aarch64|aarch64-unknown-linux-musl:Linux:arm64) runnable=yes ;;
esac
if [ "$runnable" = yes ]; then
  "$PKG/wi" --version
else
  echo "(not run: $TARGET does not match this host $host_os/$host_arch)"
fi

echo "produced:"
ls -l "$OUT/$ASSET" "$OUT/$ASSET.sha256"
cat "$OUT/$ASSET.sha256"
