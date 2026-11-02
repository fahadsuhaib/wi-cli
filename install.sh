#!/bin/sh
# Installs the latest wi release (https://github.com/fahadsuhaib/wi-cli) into ~/.local/bin.
#   WICLI_INSTALL_DIR   where to put the binary (default: $HOME/.local/bin)
#   WICLI_VERSION       a release tag such as v0.1.0 (default: the latest release)
#   WICLI_DOWNLOAD_URL  base URL or file:// directory holding the assets (for testing)
# It downloads one archive and SHA256SUMS, checks the checksum, and copies the `wi` binary. Nothing else.
set -eu

fail() { echo "install.sh: $*" >&2; exit 1; }

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
  Linux:x86_64|Linux:amd64)   name=linux-x86_64 ;;
  Linux:aarch64|Linux:arm64)  name=linux-aarch64 ;;
  Darwin:*)                   name=macos-universal ;;
  *) fail "unsupported platform $os/$arch (wi has builds for Linux x86_64, Linux aarch64 and macOS)" ;;
esac
asset="wi-$name.tar.gz"

repo=https://github.com/fahadsuhaib/wi-cli
if [ -n "${WICLI_DOWNLOAD_URL:-}" ]; then
  base=${WICLI_DOWNLOAD_URL%/}
elif [ -n "${WICLI_VERSION:-}" ]; then
  base=$repo/releases/download/$WICLI_VERSION
else
  base=$repo/releases/latest/download
fi
dest=${WICLI_INSTALL_DIR:-$HOME/.local/bin}

if command -v curl >/dev/null 2>&1; then
  get() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  get() { wget -q -O "$2" "$1"; }
else
  fail "needs curl or wget"
fi
if command -v sha256sum >/dev/null 2>&1; then
  sum() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sum() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "needs sha256sum or shasum"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

get "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
get "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "could not download $base/SHA256SUMS"

want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1; exit }' "$tmp/SHA256SUMS")
[ -n "$want" ] || fail "$asset is not listed in SHA256SUMS"
got=$(sum "$tmp/$asset")
[ "$want" = "$got" ] || fail "checksum mismatch for $asset (expected $want, got $got); nothing was installed"

tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/wi-$name/wi" ] || fail "archive does not contain wi-$name/wi"
mkdir -p "$dest"
cp "$tmp/wi-$name/wi" "$dest/wi.new"
chmod 755 "$dest/wi.new"
mv -f "$dest/wi.new" "$dest/wi"

"$dest/wi" --version
case ":$PATH:" in
  *":$dest:"*) ;;
  *) echo "note: $dest is not on your PATH; add it, e.g.  export PATH=\"$dest:\$PATH\"" ;;
esac
