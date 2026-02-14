#!/bin/bash
set -euo pipefail

REPO="ianzepp/cassio"
BINARY="cassio"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

# Detect OS
OS="$(uname -s)"
case "$OS" in
    Darwin) OS="apple-darwin" ;;
    *)
        echo "Error: unsupported OS: $OS (cassio currently supports macOS only)"
        exit 1
        ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
    arm64|aarch64) ARCH="aarch64" ;;
    x86_64)        ARCH="x86_64" ;;
    *)
        echo "Error: unsupported architecture: $ARCH"
        exit 1
        ;;
esac

TARGET="${ARCH}-${OS}"
ASSET="cassio-${TARGET}.tar.gz"

# Get latest release tag
echo "Finding latest release..."
TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)
if [ -z "$TAG" ]; then
    echo "Error: could not find latest release"
    exit 1
fi
echo "Latest release: ${TAG}"

# Download
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading ${ASSET}..."
curl -fsSL "$URL" -o "${TMPDIR}/${ASSET}"

# Extract
tar xzf "${TMPDIR}/${ASSET}" -C "$TMPDIR"

# Install
if [ -w "$INSTALL_DIR" ]; then
    mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
    echo "Installing to ${INSTALL_DIR} (requires sudo)..."
    sudo mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

chmod +x "${INSTALL_DIR}/${BINARY}"

echo "Installed ${BINARY} ${TAG} to ${INSTALL_DIR}/${BINARY}"
