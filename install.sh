#!/bin/sh
# Installs Kerdr's latest release into ~/.local/bin (or $KERDR_INSTALL_DIR).
#   curl -fsSL https://raw.githubusercontent.com/drewdunne/kerdr/main/install.sh | sh
set -eu
REPO="drewdunne/kerdr"
DIR="${KERDR_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${KERDR_VERSION:-latest}"

err() { echo "kerdr install: $*" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || err "needs curl"
command -v tar >/dev/null 2>&1 || err "needs tar"

case "$(uname -s)" in
    Linux) os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) err "no release for $(uname -s); build it with: cargo install --git https://github.com/$REPO" ;;
esac
case "$(uname -s)-$(uname -m)" in
    Darwin-x86_64) err "no release for Intel Macs yet; build it with: cargo install --git https://github.com/$REPO" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) err "no release for $(uname -m); build it with: cargo install --git https://github.com/$REPO" ;;
esac
asset="kerdr-${arch}-${os}.tar.gz"
if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "kerdr install: downloading $asset"
curl -fsSL "$base/$asset" -o "$tmp/$asset" || err "couldn't download $base/$asset"
if curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
    want=$(cut -d' ' -f1 "$tmp/$asset.sha256")
    if command -v sha256sum >/dev/null 2>&1; then
        got=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
    else
        got=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
    fi
    [ "$want" = "$got" ] || err "checksum mismatch for $asset"
fi
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$DIR"
install -m 755 "$tmp/kerdr" "$DIR/kerdr.new"
mv "$DIR/kerdr.new" "$DIR/kerdr"
echo "kerdr install: $("$DIR/kerdr" --version) is in $DIR"
case ":$PATH:" in
    *":$DIR:"*) ;;
    *) echo "kerdr install: add $DIR to your PATH" ;;
esac
command -v herdr >/dev/null 2>&1 || echo "kerdr install: Kerdr runs herdr; get it at https://herdr.dev/docs/install/"
echo "kerdr install: next, kerdr herdr-keys --install, then try kerdr demo"
