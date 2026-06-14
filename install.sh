#!/bin/sh
set -e

# ── ikk install script ──────────────────────────────────────────────────────
# Usage: curl -fsSL https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.sh | sh
# Options:
#   IKK_VERSION=0.2.0      Install a specific version (default: latest)
#   IKK_INSTALL_DIR=...     Install to a specific directory (default: ~/.local/bin)

IKK_REPO="mandeepsmagh/ikk"
IKK_VERSION="${IKK_VERSION:-latest}"
IKK_INSTALL_DIR="${IKK_INSTALL_DIR:-$HOME/.ikk/bin}"

# ── detect platform ────────────────────────────────────────────────────────
case "$(uname -s)" in
    Linux)  OS="unknown-linux-gnu" ;;
    Darwin) OS="apple-darwin" ;;
    *)      echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

TARGET="${ARCH}-${OS}"
EXT=".tar.gz"

# ── resolve version ────────────────────────────────────────────────────────
if [ "$IKK_VERSION" = "latest" ]; then
    IKK_VERSION=$(curl -fsSL "https://api.github.com/repos/${IKK_REPO}/releases/latest" \
        | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    if [ -z "$IKK_VERSION" ]; then
        echo "failed to resolve latest version" >&2
        exit 1
    fi
fi

URL="https://github.com/${IKK_REPO}/releases/download/${IKK_VERSION}/ikk-${TARGET}${EXT}"

echo "ikk ${IKK_VERSION} → ${IKK_INSTALL_DIR}/ikk"
echo "downloading ${URL}..."

# ── download + verify + install ────────────────────────────────────────────
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "$TMPDIR/ikk${EXT}"

# download checksum
curl -fsSL "${URL}.sha256" -o "$TMPDIR/ikk${EXT}.sha256"
(cd "$TMPDIR" && sha256sum -c "ikk${EXT}.sha256")

mkdir -p "$IKK_INSTALL_DIR"
tar xzf "$TMPDIR/ikk${EXT}" -C "$TMPDIR"
install -m 755 "$TMPDIR/ikk" "$IKK_INSTALL_DIR/ikk"

echo ""
echo "ikk installed to ${IKK_INSTALL_DIR}/ikk"
echo ""
echo "initialise (this also adds ${IKK_INSTALL_DIR} to your PATH permanently):"
echo "  ${IKK_INSTALL_DIR}/ikk init --remote github.com"
