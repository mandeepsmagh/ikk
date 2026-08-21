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
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    *)      echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

ASSET="ikk-${OS}-${ARCH}.tar.gz"
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

BASE="https://github.com/${IKK_REPO}/releases/download/${IKK_VERSION}"
URL="${BASE}/${ASSET}"

echo "ikk ${IKK_VERSION} → ${IKK_INSTALL_DIR}/ikk"
echo "downloading ${URL}..."

# ── download + verify + install ────────────────────────────────────────────
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "$TMPDIR/ikk${EXT}"

# verify against the published SHA256SUMS
curl -fsSL "${BASE}/SHA256SUMS" -o "$TMPDIR/SHA256SUMS"
EXPECTED=$(awk -v a="$ASSET" '$2 == a { print $1; exit }' "$TMPDIR/SHA256SUMS")
if [ -z "$EXPECTED" ]; then
    echo "asset ${ASSET} not found in SHA256SUMS" >&2
    exit 1
fi
ACTUAL=$(sha256sum "$TMPDIR/ikk${EXT}" | awk '{ print $1 }')
if [ "$EXPECTED" != "$ACTUAL" ]; then
    echo "checksum mismatch!" >&2
    echo "  expected: $EXPECTED" >&2
    echo "  got:      $ACTUAL" >&2
    exit 1
fi

mkdir -p "$IKK_INSTALL_DIR"
tar xzf "$TMPDIR/ikk${EXT}" -C "$TMPDIR"
install -m 755 "$TMPDIR/ikk" "$IKK_INSTALL_DIR/ikk"

echo ""
echo "ikk installed to ${IKK_INSTALL_DIR}/ikk"
echo ""
echo "initialise (this also adds ${IKK_INSTALL_DIR} to your PATH permanently):"
echo "  ${IKK_INSTALL_DIR}/ikk init --remote github.com"
