#!/usr/bin/env sh
# install.sh — download and install kube_config_updater
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/derpy4me/kube-config-updater/main/install.sh | sh
#   or to pin a version:
#   curl -fsSL ... | sh -s -- v0.2.0

set -e

REPO="derpy4me/kube-config-updater"
BINARY_NAME="kube_config_updater"
VERSION="${1:-latest}"

# ── Detect OS and arch ────────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux*)
    case "${ARCH}" in
      x86_64) ARTIFACT="${BINARY_NAME}-linux-x86_64" ;;
      *)
        echo "Unsupported Linux architecture: ${ARCH}" >&2
        echo "Only x86_64 is supported. Open an issue if you need arm64." >&2
        exit 1
        ;;
    esac
    ;;
  Darwin*)
    case "${ARCH}" in
      arm64) ARTIFACT="${BINARY_NAME}-macos-arm64" ;;
      *)
        echo "Unsupported macOS architecture: ${ARCH}" >&2
        echo "Only Apple Silicon (arm64) is supported. Intel Mac builds not yet available." >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported operating system: ${OS}" >&2
    exit 1
    ;;
esac

# ── Resolve download URL ──────────────────────────────────────────────────────

if [ "${VERSION}" = "latest" ]; then
  URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT}"
else
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
fi

# ── Choose install directory ──────────────────────────────────────────────────

if [ -w "/usr/local/bin" ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="${HOME}/.local/bin"
  mkdir -p "${INSTALL_DIR}"
fi

INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"

# ── Download ──────────────────────────────────────────────────────────────────

echo "Downloading ${ARTIFACT}..."
curl -fSL --progress-bar "${URL}" -o "${INSTALL_PATH}"
chmod +x "${INSTALL_PATH}"

# ── macOS: remove quarantine attribute ───────────────────────────────────────
# Binaries downloaded from the internet are quarantined by macOS Gatekeeper.
# The binary is ad-hoc signed but not notarized. Removing the quarantine
# attribute allows it to run without a security warning.

if [ "${OS}" = "Darwin" ]; then
  xattr -d com.apple.quarantine "${INSTALL_PATH}" 2>/dev/null || true
fi

# ── Done ──────────────────────────────────────────────────────────────────────

echo ""
echo "Installed ${BINARY_NAME} to ${INSTALL_PATH}"
echo "  $(${INSTALL_PATH} --version)"

if ! echo ":${PATH}:" | grep -q ":${INSTALL_DIR}:"; then
  echo ""
  echo "  NOTE: ${INSTALL_DIR} is not in your PATH."
  echo "  Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
  echo ""
  echo "    export PATH=\"${INSTALL_DIR}:\${PATH}\""
  echo ""
fi
