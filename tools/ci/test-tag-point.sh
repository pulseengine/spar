#!/usr/bin/env bash
# Regression test for tools/ci/check-tag-point.sh.
#
# WHY THIS EXISTS
#
# check-tag-point.sh was written for #366, merged, marked implemented, and then
# never ran — it only fires on a tag push, and there was no tag between its
# landing and v0.36.0. Its FIRST real execution reported "v0.36.0 is a
# lightweight tag" about a correctly annotated, signed tag, and blocked the
# release.
#
# The cause: for a tag push `github.sha` is the PEELED COMMIT, so
# actions/checkout creates refs/tags/<TAG> pointing at the commit — a
# lightweight ref — no matter what the remote holds. The gate read that and
# judged the tag, not the tag object.
#
# Both cases below are the ones that actually matter and neither had a test:
#   1. annotated upstream + lightweight local ref  -> must PASS  (the false accusation)
#   2. genuinely lightweight, absent upstream      -> must FAIL  (the real defect)
#
# Case 2 is what stops the fix for case 1 from turning the gate into a no-op.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
GATE="${HERE}/check-tag-point.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT
G=(git -c user.name=t -c user.email=t@t -c commit.gpgsign=false -c tag.gpgsign=false)

pass=0; fail=0
say() { if [ "$1" = ok ]; then pass=$((pass+1)); echo "  ok   $2"; else fail=$((fail+1)); echo "  FAIL $2"; fi; }

# --- upstream with an ANNOTATED tag, and main ahead of it -------------------
"${G[@]}" init --quiet --bare "${TMP}/origin.git"
"${G[@]}" clone --quiet "${TMP}/origin.git" "${TMP}/up"
cd "${TMP}/up"
echo a > a; "${G[@]}" add a; "${G[@]}" commit --quiet -m "base"
echo b > b; "${G[@]}" add b; "${G[@]}" commit --quiet -m "release bump"
TAGGED="$("${G[@]}" rev-parse HEAD)"
"${G[@]}" tag -a v1.0.0 -m "v1.0.0

Tag-Point: later work is deliberately excluded."
echo c > c; "${G[@]}" add c; "${G[@]}" commit --quiet -m "landed after the tag"
"${G[@]}" push --quiet origin main --tags

# --- CASE 1: reproduce actions/checkout — lightweight local ref ------------
"${G[@]}" init --quiet "${TMP}/ci"
cd "${TMP}/ci"
"${G[@]}" remote add origin "${TMP}/origin.git"
"${G[@]}" fetch --quiet origin "${TAGGED}"
"${G[@]}" update-ref refs/tags/v1.0.0 "${TAGGED}"
[ "$("${G[@]}" for-each-ref refs/tags/v1.0.0 --format='%(objecttype)')" = commit ] \
  || { echo "  FAIL fixture did not reproduce the lightweight ref"; exit 1; }
if GITHUB_REF=refs/tags/v1.0.0 "${GATE}" >/dev/null 2>&1; then
  say ok "annotated upstream + lightweight local ref (the CI shape) PASSES"
else
  say no "annotated upstream + lightweight local ref (the CI shape) PASSES"
fi

# --- CASE 2: genuinely lightweight, not on the remote ----------------------
"${G[@]}" update-ref refs/tags/v9.9.9 "${TAGGED}"
if GITHUB_REF=refs/tags/v9.9.9 "${GATE}" >/dev/null 2>&1; then
  say no "a genuinely lightweight tag still FAILS"
else
  say ok "a genuinely lightweight tag still FAILS"
fi

echo ""
echo "${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
