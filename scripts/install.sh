#!/bin/sh
# nmem installer — downloads the correct binary for your platform.
# Usage: curl -fsSL https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.sh | sh
# Options:
#   --cuda   Install the CUDA-accelerated build (Linux x86_64 or Windows x86_64)
#   --rocm   Install the ROCm-accelerated build (Linux x86_64 only)
#   --metal  Install the Metal-accelerated build (macOS arm64 only)
# Windows (PowerShell): use scripts/install.ps1 instead.
# Windows (Git Bash / MSYS2 / Cygwin): this script works as-is.

set -e

REPO="viablesys/nmem"
INSTALL_DIR="$HOME/.local/bin"
GPU_VARIANT=""
EXT=""

# Parse flags
for arg in "$@"; do
  case "$arg" in
    --cuda) GPU_VARIANT="cuda" ;;
    --rocm) GPU_VARIANT="rocm" ;;
    --metal) GPU_VARIANT="metal" ;;
  esac
done

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Linux)              OS_TAG="linux" ;;
  Darwin)             OS_TAG="macos" ;;
  MINGW*|MSYS*|CYGWIN*) OS_TAG="windows"; EXT=".exe" ;;
  *)
    echo "Unsupported OS: $OS" >&2
    echo "Windows PowerShell users: irm https://raw.githubusercontent.com/${REPO}/main/scripts/install.ps1 | iex" >&2
    exit 1
    ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$OS_TAG" in
  macos)
    # Only arm64 (Apple Silicon) binary is distributed.
    # Runs natively on Apple Silicon; requires Rosetta 2 on Intel Macs.
    ARCH_TAG="arm64"
    if [ "$ARCH" = "x86_64" ]; then
      echo "Note: Intel Mac detected. Using the arm64 binary via Rosetta 2."
      echo "If Rosetta 2 is not installed: softwareupdate --install-rosetta"
    elif [ "$ARCH" = "arm64" ] && [ -z "$GPU_VARIANT" ]; then
      echo "Tip: Apple Silicon detected. For Metal GPU acceleration, re-run with --metal"
    fi
    ;;
  linux)
    case "$ARCH" in
      x86_64|amd64) ARCH_TAG="x86_64" ;;
      arm64|aarch64)
        echo "Linux arm64 does not have a prebuilt binary yet." >&2
        echo "Build from source: https://github.com/${REPO}#building" >&2
        exit 1
        ;;
      *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  windows)
    case "$ARCH" in
      x86_64|amd64) ARCH_TAG="x86_64" ;;
      *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
esac

# ROCm is Linux x86_64 only
if [ "$GPU_VARIANT" = "rocm" ] && [ "$OS_TAG" != "linux" ]; then
  echo "--rocm is only available for Linux x86_64." >&2
  exit 1
fi

# Metal is macOS arm64 only
if [ "$GPU_VARIANT" = "metal" ] && [ "$OS_TAG" != "macos" ]; then
  echo "--metal is only available for macOS arm64 (Apple Silicon)." >&2
  exit 1
fi

# CUDA/ROCm require x86_64; Metal requires arm64
if [ "$GPU_VARIANT" = "cuda" ] || [ "$GPU_VARIANT" = "rocm" ]; then
  if [ "$ARCH_TAG" != "x86_64" ]; then
    echo "--cuda/--rocm are only available for x86_64." >&2
    exit 1
  fi
fi

# Build target name
if [ -n "$GPU_VARIANT" ]; then
  TARGET="nmem-${OS_TAG}-${ARCH_TAG}-${GPU_VARIANT}"
else
  TARGET="nmem-${OS_TAG}-${ARCH_TAG}"
fi

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

URL="https://github.com/${REPO}/releases/download/${TAG}/${TARGET}${EXT}"
echo "Downloading ${TARGET}${EXT} (${TAG})..."

# Create install directory
mkdir -p "$INSTALL_DIR"

# Download
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "${INSTALL_DIR}/nmem${EXT}" "$URL"
else
  wget -qO "${INSTALL_DIR}/nmem${EXT}" "$URL"
fi

chmod +x "${INSTALL_DIR}/nmem${EXT}"

# Ad-hoc codesign on macOS — required to avoid SIGKILL from Gatekeeper
if [ "$OS_TAG" = "macos" ] && command -v codesign >/dev/null 2>&1; then
  codesign --force --sign - "${INSTALL_DIR}/nmem" 2>/dev/null || true
fi

# Create nmem data directory
mkdir -p "$HOME/.nmem"

echo ""
echo "Installed nmem to ${INSTALL_DIR}/nmem${EXT}"

# Add INSTALL_DIR to PATH in shell profile if not already present
case ":$PATH:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo ""
    # Determine which shell profile to update
    SHELL_NAME="$(basename "${SHELL:-sh}")"
    case "$SHELL_NAME" in
      zsh)
        PROFILES="$HOME/.zshrc"
        ;;
      bash)
        # macOS login shells use .bash_profile; Linux interactive shells use .bashrc
        if [ "$OS_TAG" = "macos" ]; then
          PROFILES="$HOME/.bash_profile"
        else
          PROFILES="$HOME/.bashrc"
        fi
        ;;
      fish)
        if command -v fish >/dev/null 2>&1; then
          fish -c "fish_add_path --universal \"$INSTALL_DIR\"" 2>/dev/null && \
            echo "Added $INSTALL_DIR to fish universal path. Open a new terminal to use nmem."
        else
          echo "WARNING: $INSTALL_DIR is not in your PATH. Add it manually."
        fi
        PROFILES=""
        ;;
      *)
        PROFILES="$HOME/.profile"
        ;;
    esac

    for RC in $PROFILES; do
      if ! grep -qF "$INSTALL_DIR" "$RC" 2>/dev/null; then
        printf '\n# Added by nmem installer\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$RC"
        echo "Added $INSTALL_DIR to PATH in $RC"
        echo "Open a new terminal (or run: source $RC) for the change to take effect."
      fi
    done
    ;;
esac

echo ""
echo "Next: install the Claude Code plugin:"
echo "  claude plugin marketplace add viablesys/claude-plugins"
echo "  claude plugin install nmem@viablesys"
echo ""
echo "Then restart Claude Code."
