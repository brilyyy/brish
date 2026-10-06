#!/bin/sh
# briSH installer: prebuilt binary from GitHub Releases into
# ${PREFIX:-/usr/local/bin}. Verifies sha256; fail-closed on mismatch.
#
#   curl -fsSL https://raw.githubusercontent.com/brilyyy/brish/main/install.sh | sh
#   VERSION=v0.1.0 ./install.sh        # pin a tag
#   PREFIX=$HOME/.local/bin ./install.sh
set -eu

REPO="${BRISH_REPO:-brilyyy/brish}"
BIN=brish
PREFIX="${PREFIX:-/usr/local/bin}"
VERSION="${1:-${VERSION:-latest}}"

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$arch" in
    arm64) arch=aarch64 ;;
    amd64) arch=x86_64 ;;
esac
case "$os" in
    linux)  plat=unknown-linux-gnu ;;
    darwin) plat=apple-darwin ;;
    *)
        echo "install.sh: unsupported OS '$os' (linux/darwin only)" >&2
        echo "hint: cargo install --git https://github.com/$REPO $BIN" >&2
        exit 1
        ;;
esac

if [ "$VERSION" = latest ]; then
    tag=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
        "https://github.com/$REPO/releases/latest" |
        sed 's|.*/tag/||')
else
    tag=$VERSION
fi
target="$arch-$plat"
asset="brish-$tag-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "install.sh: $asset ($tag)" >&2
if ! curl -fsSL "$url" -o "$tmp/$asset"; then
    echo "install.sh: no prebuilt for $target at $tag" >&2
    echo "hint: cargo install --git https://github.com/$REPO $BIN" >&2
    exit 1
fi
curl -fsSL "$url.sha256" -o "$tmp/$asset.sha256"

cd "$tmp"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$asset.sha256"
else
    shasum -a 256 -c "$asset.sha256"
fi
tar -xzf "$asset"
# expected path inside sha file may be bare name
chmod 755 "$BIN"

if [ ! -d "$PREFIX" ]; then
    echo "install.sh: $PREFIX missing — creating (may need sudo)" >&2
    mkdir -p "$PREFIX" 2>/dev/null || sudo mkdir -p "$PREFIX"
fi
if [ -w "$PREFIX" ]; then
    mv -f "$BIN" "$PREFIX/$BIN"
else
    sudo mv -f "$BIN" "$PREFIX/$BIN"
fi
echo "installed: $PREFIX/$BIN" >&2
case ":$PATH:" in
    *":$PREFIX:"*) ;;
    *) echo "note: add $PREFIX to PATH" >&2 ;;
esac
"$PREFIX/$BIN" --version 2>/dev/null || true
