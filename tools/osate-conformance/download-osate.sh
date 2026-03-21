#!/usr/bin/env bash
# Download and install OSATE 2.18.0 for the current platform.
set -euo pipefail

OSATE_VERSION="2.18.0"
OSATE_DIR="${OSATE_DIR:-$HOME/osate2}"
BASE_URL="https://osate-build.sei.cmu.edu/download/osate/stable/latest/products"

# Detect platform
case "$(uname -s)-$(uname -m)" in
    Darwin-arm64)  ARCHIVE="osate2-${OSATE_VERSION}-vfinal-macosx.cocoa.aarch64.tar.gz" ;;
    Darwin-x86_64) ARCHIVE="osate2-${OSATE_VERSION}-vfinal-macosx.cocoa.x86_64.tar.gz" ;;
    Linux-aarch64) ARCHIVE="osate2-${OSATE_VERSION}-vfinal-linux.gtk.aarch64.tar.gz" ;;
    Linux-x86_64)  ARCHIVE="osate2-${OSATE_VERSION}-vfinal-linux.gtk.x86_64.tar.gz" ;;
    *)
        echo "Unsupported platform: $(uname -s)-$(uname -m)"
        exit 1
        ;;
esac

echo "==> Downloading OSATE ${OSATE_VERSION} (${ARCHIVE})..."
echo "    This may take a while — CMU server can be slow."
echo "    Target: ${OSATE_DIR}"

mkdir -p "$(dirname "$OSATE_DIR")"
TMPFILE="/tmp/osate-${ARCHIVE}"

if [ -f "$TMPFILE" ]; then
    echo "    Resuming previous download..."
fi

echo "    Downloading with retry (will resume on disconnect)..."
until curl -C - -L --retry 999 --retry-delay 5 --retry-max-time 0 \
    --connect-timeout 30 -o "$TMPFILE" "${BASE_URL}/${ARCHIVE}"; do
    echo "    Connection lost. Retrying in 10s..."
    sleep 10
done

echo "==> Extracting..."
mkdir -p "$OSATE_DIR"
tar -xzf "$TMPFILE" -C "$OSATE_DIR" --strip-components=0

# macOS: remove quarantine
if [ "$(uname -s)" = "Darwin" ]; then
    echo "==> Removing macOS quarantine..."
    sudo xattr -rd com.apple.quarantine "${OSATE_DIR}/osate2.app" 2>/dev/null || true
fi

echo "==> OSATE ${OSATE_VERSION} installed to ${OSATE_DIR}"
echo ""
echo "To install EASE (Python scripting):"
echo "  1. Open OSATE: ${OSATE_DIR}/osate2.app"
echo "  2. Help → Install New Software"
echo "  3. Add: https://download.eclipse.org/ease/release/0.10.0"
echo "  4. Install 'EASE Core' + 'EASE Python Support (Py4J)'"
echo "  5. Restart OSATE"
