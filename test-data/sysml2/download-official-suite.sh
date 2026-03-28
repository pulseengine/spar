#!/usr/bin/env bash
# Download official SysML v2 validation and training files from the
# Systems-Modeling/SysML-v2-Release repository.
#
# Usage: ./download-official-suite.sh
#
# This downloads files into:
#   validation/official/  -- official validation .sysml files
#   training/official/    -- one file per training category
#   examples/official/    -- official example files

set -euo pipefail

REPO="Systems-Modeling/SysML-v2-Release"
BASE_URL="https://raw.githubusercontent.com/${REPO}/master"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== Downloading SysML v2 official test files ==="
echo "Repository: ${REPO}"
echo "Target: ${SCRIPT_DIR}"

# --- Validation files ---
VALIDATION_DIR="${SCRIPT_DIR}/validation/official"
mkdir -p "${VALIDATION_DIR}"
echo ""
echo "--- Validation files ---"

# List files from the validation directory using the GitHub API
VALIDATION_FILES=$(curl -sL "https://api.github.com/repos/${REPO}/contents/sysml/src/validation" | \
    python3 -c "import json,sys; [print(x['name']) for x in json.load(sys.stdin) if x['name'].endswith('.sysml')]" 2>/dev/null || true)

if [ -z "${VALIDATION_FILES}" ]; then
    echo "Could not list validation files via API. Trying known files..."
    VALIDATION_FILES="
01-Packages-1.sysml
02-Parts-1.sysml
03-Ports-1.sysml
04-Connections-1.sysml
05-Actions-1.sysml
06-States-1.sysml
07-Requirements-1.sysml
08-Constraints-1.sysml
"
fi

for FILE in ${VALIDATION_FILES}; do
    echo "  Downloading ${FILE}..."
    curl -sL "${BASE_URL}/sysml/src/validation/${FILE}" -o "${VALIDATION_DIR}/${FILE}" 2>/dev/null || \
        echo "    SKIP: ${FILE} not found"
done

# --- Training files (one per category) ---
TRAINING_DIR="${SCRIPT_DIR}/training/official"
mkdir -p "${TRAINING_DIR}"
echo ""
echo "--- Training files ---"

TRAINING_DIRS=$(curl -sL "https://api.github.com/repos/${REPO}/contents/sysml/src/training" | \
    python3 -c "import json,sys; [print(x['name']) for x in json.load(sys.stdin) if x['type'] == 'dir']" 2>/dev/null || true)

for DIR in ${TRAINING_DIRS}; do
    # Get first .sysml file from each training directory
    FIRST_FILE=$(curl -sL "https://api.github.com/repos/${REPO}/contents/sysml/src/training/${DIR}" | \
        python3 -c "import json,sys; files=[x['name'] for x in json.load(sys.stdin) if x['name'].endswith('.sysml')]; print(files[0] if files else '')" 2>/dev/null || true)
    if [ -n "${FIRST_FILE}" ]; then
        echo "  Downloading ${DIR}/${FIRST_FILE}..."
        curl -sL "${BASE_URL}/sysml/src/training/${DIR}/${FIRST_FILE}" \
            -o "${TRAINING_DIR}/${DIR}-${FIRST_FILE}" 2>/dev/null || \
            echo "    SKIP: ${FIRST_FILE} not found"
    fi
done

# --- Example files ---
EXAMPLES_DIR="${SCRIPT_DIR}/examples/official"
mkdir -p "${EXAMPLES_DIR}"
echo ""
echo "--- Example files ---"

EXAMPLE_FILES=$(curl -sL "https://api.github.com/repos/${REPO}/contents/sysml/src/examples" | \
    python3 -c "
import json, sys
data = json.load(sys.stdin)
for item in data:
    if item['name'].endswith('.sysml'):
        print(item['name'])
    elif item['type'] == 'dir':
        print('DIR:' + item['name'])
" 2>/dev/null || true)

for ITEM in ${EXAMPLE_FILES}; do
    if [[ "${ITEM}" == DIR:* ]]; then
        SUBDIR="${ITEM#DIR:}"
        SUBFILES=$(curl -sL "https://api.github.com/repos/${REPO}/contents/sysml/src/examples/${SUBDIR}" | \
            python3 -c "import json,sys; [print(x['name']) for x in json.load(sys.stdin) if x['name'].endswith('.sysml')]" 2>/dev/null || true)
        for SF in ${SUBFILES}; do
            echo "  Downloading ${SUBDIR}/${SF}..."
            curl -sL "${BASE_URL}/sysml/src/examples/${SUBDIR}/${SF}" \
                -o "${EXAMPLES_DIR}/${SUBDIR}-${SF}" 2>/dev/null || \
                echo "    SKIP: ${SF} not found"
        done
    else
        echo "  Downloading ${ITEM}..."
        curl -sL "${BASE_URL}/sysml/src/examples/${ITEM}" \
            -o "${EXAMPLES_DIR}/${ITEM}" 2>/dev/null || \
            echo "    SKIP: ${ITEM} not found"
    fi
done

# --- GfSE models ---
GFSE_DIR="${SCRIPT_DIR}/examples/gfse"
mkdir -p "${GFSE_DIR}"
echo ""
echo "--- GfSE SysML v2 Models ---"

GFSE_REPO="GfSE/SysML-v2-Models"
GFSE_FILES=$(curl -sL "https://api.github.com/repos/${GFSE_REPO}/contents" | \
    python3 -c "import json,sys; [print(x['name']) for x in json.load(sys.stdin) if x['name'].endswith('.sysml')]" 2>/dev/null || true)

for FILE in ${GFSE_FILES}; do
    echo "  Downloading ${FILE}..."
    curl -sL "https://raw.githubusercontent.com/${GFSE_REPO}/main/${FILE}" \
        -o "${GFSE_DIR}/${FILE}" 2>/dev/null || \
        echo "    SKIP: ${FILE} not found"
done

echo ""
echo "=== Download complete ==="
TOTAL=$(find "${SCRIPT_DIR}" -name "*.sysml" | wc -l | tr -d ' ')
echo "Total .sysml files: ${TOTAL}"
