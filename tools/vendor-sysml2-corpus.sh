#!/usr/bin/env bash
#
# Re-vendor the official SysML v2 corpus at a pinned commit.
#
# Replaces test-data/sysml2/download-official-suite.sh, which fetched from
# `master` through the GitHub contents API with `|| true` on every download and
# a hardcoded fallback list when the API call failed. Three problems, all of
# which this script exists to not have:
#
#   1. UNPINNED. Upstream still ships monthly incremental grammar tags, so the
#      corpus — and therefore every parse rate measured against it — moved
#      under us between runs.
#   2. FAILURE READ AS SUCCESS. `curl ... || echo SKIP` means a network blip
#      silently produces a SMALLER corpus, and a smaller corpus of
#      still-failing files reads as a BETTER parse rate. The error path yielded
#      the flattering answer.
#   3. NEVER RUN. The directories it targeted were empty, so the vacuous
#      conformance suite was measuring 43 fixtures we wrote ourselves.
#
# Usage:
#   tools/vendor-sysml2-corpus.sh                 # re-vendor at the recorded pin
#   tools/vendor-sysml2-corpus.sh <commit-sha>    # move the pin, deliberately
#
# Moving the pin is a reviewable change: it will move the count that
# crates/spar-sysml2/tests/official_corpus.rs asserts, and that test names both
# constants when it fails.

set -euo pipefail

REPO="https://github.com/Systems-Modeling/SysML-v2-Release"
PIN="29a3d2acdd49600cff872e7a55962a40400f3335"
[ $# -ge 1 ] && PIN="$1"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="${ROOT}/test-data/sysml2/official"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

echo "== vendoring the official SysML v2 corpus =="
echo "   upstream: ${REPO}"
echo "   pin:      ${PIN}"

git clone --quiet "${REPO}" "${WORK}/up"
git -C "${WORK}/up" checkout --quiet "${PIN}"

GOT="$(git -C "${WORK}/up" rev-parse HEAD)"
if [ "${GOT}" != "${PIN}" ]; then
  echo "::error::asked for ${PIN}, checked out ${GOT}" >&2
  exit 1
fi

rm -rf "${DST}"
mkdir -p "${DST}"
cp "${WORK}/up/LICENSE" "${DST}/LICENSE"

# Source models only. The rest of the 366 MB upstream tree is the Xtext pilot
# implementation, its Eclipse plugins, jars and IDE state — none of it is model
# source and none of it grades a parser.
#
# NOTE: paths in this corpus contain spaces ("Address Book Example/"). Every
# loop below is null-delimited for that reason; a `for f in $(find ...)` here
# word-splits them into fragments that do not exist. That mistake inflated the
# measured failure count twice before it was caught.
total=0
for pair in \
  "sysml/src/validation:sysml/validation" \
  "sysml/src/training:sysml/training" \
  "sysml/src/examples:sysml/examples" \
  "kerml/src/validation:kerml/validation" \
  "kerml/src/examples:kerml/examples"
do
  src="${WORK}/up/${pair%%:*}"
  rel="${pair##*:}"
  [ -d "${src}" ] || continue
  n=0
  while IFS= read -r -d '' f; do
    out="${DST}/${rel}/${f#./}"
    mkdir -p "$(dirname "${out}")"
    cp "${f}" "${out}"
    n=$((n + 1))
  done < <(cd "${src}" && find . \( -name '*.sysml' -o -name '*.kerml' \) -print0)
  printf '   %-20s %4d files\n' "${rel}" "${n}"
  total=$((total + n))
done

echo "   TOTAL                ${total} files"

# A vendoring run that copied nothing must not exit 0 — that is the same
# error-path-yields-the-ideal-reading shape the old script had.
if [ "${total}" -eq 0 ]; then
  echo "::error::vendored 0 model files — refusing to leave an empty corpus" >&2
  exit 1
fi

echo
echo "Now update, together:"
echo "  * test-data/sysml2/PROVENANCE.md  — the pin and the file counts"
echo "  * crates/spar-sysml2/tests/official_corpus.rs — OFFICIAL_TOTAL, and"
echo "    OFFICIAL_PARSING if the parse count moved"
echo
echo "  cargo test -p spar-sysml2 --test official_corpus"
