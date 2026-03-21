#!/usr/bin/env bash
# Generate OSATE reference data for conformance testing.
#
# Prerequisites:
# 1. OSATE installed (run download-osate.sh)
# 2. EASE Python plugin installed in OSATE
#
# This script starts OSATE with a temporary workspace, imports test
# models, and runs the EASE script to generate reference data.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OSATE_DIR="${OSATE_DIR:-$HOME/osate2}"
WORKSPACE="/tmp/osate-conformance-workspace"
EASE_SCRIPT="$SCRIPT_DIR/ease-scripts/generate_references.py"

# Find OSATE executable
if [ "$(uname -s)" = "Darwin" ]; then
    OSATE_BIN="$OSATE_DIR/osate2.app/Contents/MacOS/osate"
else
    OSATE_BIN="$OSATE_DIR/osate"
fi

if [ ! -f "$OSATE_BIN" ]; then
    echo "ERROR: OSATE not found at $OSATE_BIN"
    echo "Run: ./tools/osate-conformance/download-osate.sh"
    exit 1
fi

echo "==> Creating temporary workspace at $WORKSPACE"
rm -rf "$WORKSPACE"
mkdir -p "$WORKSPACE"

echo "==> Starting OSATE..."
echo "    Once OSATE opens:"
echo "    1. Window → Show View → Script Shell"
echo "    2. Change shell to Python (Py4J)"
echo "    3. Run: loadScript('$EASE_SCRIPT')"
echo ""
echo "    Or manually: Run → Run Script... → select $EASE_SCRIPT"
echo ""
echo "    Reference data will be written to:"
echo "    $SCRIPT_DIR/reference-data/"
echo ""

"$OSATE_BIN" -data "$WORKSPACE" &

echo "OSATE started (PID $!). Follow the instructions above."
echo ""
echo "When done, close OSATE and run:"
echo "  python3 $SCRIPT_DIR/compare.py"
