#!/bin/sh
# nmem installer — downloads the correct binary for your platform.
# Usage: curl -fsSL https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.sh | sh

set -e

REPO="viablesys/nmem"
INSTALL_DIR="$HOME/.local/bin"

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Linux)  OS_TAG="linux" ;;
  Darwin) OS_TAG="macos" ;;
  *)
    echo "Unsupported OS: $OS" >&2
    exit 1
    ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)  ARCH_TAG="x86_64" ;;
  arm64|aarch64)  ARCH_TAG="arm64" ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

TARGET="nmem-${OS_TAG}-${ARCH_TAG}"

# Get latest release tag
if command -v curl >/dev/null 2>&1; then
  FETCH="curl -fsSL"
elif command -v wget >/dev/null 2>&1; then
  FETCH="wget -qO-"
else
  echo "Neither curl nor wget found. Install one and retry." >&2
  exit 1
fi

echo "Detecting latest release..."
TAG=$($FETCH "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$TAG" ]; then
  echo "Failed to detect latest release. Check https://github.com/${REPO}/releases" >&2
  exit 1
fi

URL="https://github.com/${REPO}/releases/download/${TAG}/${TARGET}"
echo "Downloading ${TARGET} (${TAG})..."

# Create install directory
mkdir -p "$INSTALL_DIR"

# Download
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "${INSTALL_DIR}/nmem" "$URL"
else
  wget -qO "${INSTALL_DIR}/nmem" "$URL"
fi

chmod +x "${INSTALL_DIR}/nmem"

# Create nmem data directory
mkdir -p "$HOME/.nmem"

echo ""
echo "Installed nmem to ${INSTALL_DIR}/nmem"

# Check PATH
case ":$PATH:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo ""
    echo "WARNING: ${INSTALL_DIR} is not in your PATH."
    echo "Add it with:  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac

echo ""
echo "Next: install the Claude Code plugin:"
echo "  claude plugin add /path/to/nmem"
echo ""
echo "Or if you cloned the repo:"
echo "  claude plugin add ."
