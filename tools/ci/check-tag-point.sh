#!/usr/bin/env bash
#
# Release-tag-point gate — issue #366.
#
# The failure this exists to catch: a release tag placed on a commit that is
# *behind* main, silently excluding work that was already finished and merged.
# It happened twice — v0.31.0 missed a parser fix by 5m06s, v0.33.0 excluded two
# commits — and nothing in CI noticed either time.
#
# `check-versions` cannot catch it. That job measures the version *window*: it
# asserts the tag matches Cargo.toml, and Cargo.toml reads X.Y.Z from the bump
# commit until the next bump — a window that spans both the included and the
# excluded commits. The invariant is circular with respect to the question.
#
# So this gate stops asking "which release does this commit belong to" (an
# unanswerable question, whose answer would drift as unrelated commits land) and
# asks the one that was actually at stake:
#
#   Is anything that was already finished being left out, and did you say so?
#
# Given tag vX.Y.Z at commit C with tag date T:
#
#   1. C not an ancestor of origin/main            -> skip (release-branch tag)
#   2. enumerate C..origin/main, dated before T    -> the excluded set
#   3. empty                                       -> pass
#   4. non-empty, tag message states a tag point   -> pass, listing them
#   5. non-empty, it does not                      -> fail, listing them
#
# Step 2's `--before=T` is what makes the gate both race-proof and idempotent.
# Race-proof: a merge landing seconds after the tag push cannot fail the job,
# because it postdates T. Idempotent: both inputs (the tag object, and the set
# of commits dated before T that are reachable from main) are immutable, so a
# re-run months later returns the same verdict. A gate whose answer changes as
# unrelated commits land is not a gate.
#
# WHAT THIS DOES NOT DO — stated so it is not mistaken for coverage:
#
#   * It cannot verify the acknowledgement is TRUE. It greps the tag message for
#     a stated tag point; a message saying "tag point: whatever" passes. This is
#     a speed bump against autopilot, not a proof. Its value is forcing the
#     tagger to look at the list — precisely what did not happen for v0.31.0 and
#     v0.33.0.
#   * It cannot catch tagging a commit that is too NEW. Different failure.
#   * It cannot catch work merged AFTER the tag. Nothing at tag time can, and
#     step 2 deliberately stops trying.
#   * It assumes tag dates and committer dates come from comparably-set clocks.
#     They do here (one maintainer, one machine). A badly skewed clock would
#     weaken step 2 in the PERMISSIVE direction — it under-reports, never
#     over-reports.
#
# Usage:
#   tools/ci/check-tag-point.sh v0.36.0     # explicit (backtesting, local runs)
#   tools/ci/check-tag-point.sh             # reads GITHUB_REF (CI)
#
# Exit: 0 pass or skip, 1 fail, 2 usage/environment error.

set -euo pipefail

TAG="${1:-}"
if [ -z "${TAG}" ]; then
  REF="${GITHUB_REF:-}"
  case "${REF}" in
    refs/tags/*) TAG="${REF#refs/tags/}" ;;
    *)
      echo "usage: $0 <tag>   (or set GITHUB_REF=refs/tags/vX.Y.Z)" >&2
      exit 2
      ;;
  esac
fi

if ! C="$(git rev-parse --verify --quiet "${TAG}^{commit}")"; then
  echo "::error::${TAG} does not resolve to a commit. If this is CI, the tag"
  echo "::error::object was not fetched — the job needs fetch-depth: 0."
  exit 2
fi

# `creatordate` is taggerdate for an annotated tag and committerdate for a
# lightweight one. Releases here are signed annotated tags, but reading the
# field that is defined for both means a lightweight tag degrades to a slightly
# weaker check rather than to an empty `--before=`, which git would not reject.
T="$(git for-each-ref "refs/tags/${TAG}" --format='%(creatordate:iso-strict)')"
if [ -z "${T}" ]; then
  echo "::error::Could not read a creation date for ${TAG}."
  exit 2
fi

# Explicit refspec: do not rely on whatever remote.origin.fetch the checkout
# action happened to configure. A missing refs/remotes/origin/main would make
# the rev-list below fail loudly, but fetching it outright is one line.
git fetch --no-tags --quiet origin "+refs/heads/main:refs/remotes/origin/main"

if ! git merge-base --is-ancestor "${C}" origin/main; then
  echo "${TAG} (${C}) is not an ancestor of origin/main — release-branch tag, skipping."
  exit 0
fi

EXCLUDED="$(git log --format='%h %ci %s' --before="${T}" "${C}..origin/main")"

if [ -z "${EXCLUDED}" ]; then
  echo "PASS: ${TAG} at ${C} (${T}) — nothing predating the tag was left out."
  exit 0
fi

COUNT="$(printf '%s\n' "${EXCLUDED}" | wc -l | tr -d ' ')"

if git tag -l "${TAG}" --format='%(contents)' | grep -qiE 'tag[ -]point'; then
  echo "PASS (acknowledged): ${TAG} excludes ${COUNT} commit(s) that predate it,"
  echo "and its message states the tag point:"
  printf '%s\n' "${EXCLUDED}" | sed 's/^/  excluded: /'
  exit 0
fi

echo "::error::${TAG} was cut at ${C} (${T}), but ${COUNT} commit(s) were"
echo "::error::already on main before that and are not in the release:"
printf '%s\n' "${EXCLUDED}" | sed 's/^/::error::  /'
echo "::error::"
echo "::error::Either tag the tip of main, or state the tag point in the tag"
echo "::error::message (e.g. \"Tag point is the bump commit abc1234; the tag was"
echo "::error::deliberately held after the bump merged, under the batching policy.\")"
exit 1
