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
#   0. tag is lightweight, not annotated           -> fail (see below)
#   1. C not an ancestor of origin/main            -> skip (release-branch tag)
#   2. enumerate C..origin/main on the FIRST-PARENT
#      spine, dated before T                       -> the excluded set
#   3. empty                                       -> pass
#   4. non-empty, tag message carries a
#      `Tag-Point:` trailer                        -> pass, listing them
#   5. non-empty, it does not                      -> fail, listing them
#
# WHY --first-parent, AND WHY THE OBVIOUS VERSION IS WRONG. A clean-room audit
# refuted the first design, which enumerated every commit in `C..origin/main`
# dated before T. `--before` filters on COMMITTER DATE — when a commit was
# written — but the question is when it LANDED ON MAIN. Those differ, and the
# gap is unbounded: a branch whose commits are dated the 3rd, merged on the 10th,
# injects commits "dated before T" into a range they were not reachable from at
# tag time. Consequences, both demonstrated against a scratch repo:
#
#   * NOT IDEMPOTENT. Same tag object, same tagged commit, byte-identical
#     ->  PASS at tag time, FAIL when re-run after that branch merged. A gate
#     whose verdict drifts as unrelated commits land is exactly what #366
#     rejected in its first design; this reintroduced it one level down.
#   * OVER-REPORTS, contradicting the soundness direction this header used to
#     claim ("weakens in the PERMISSIVE direction — it under-reports, never
#     over-reports"). It is a false accusation against a tagger who did tag the
#     tip of main. That claim was wrong and is deleted.
#
# The first-parent spine fixes both. It contains exactly the squash and merge
# commits, whose committer dates ARE their landing times, so `--before` becomes
# exact rather than approximate. A late-merged branch contributes its merge
# commit (dated at merge time, correctly after T) and not its old constituents.
# Detection is unaffected: the v0.31.0 miss (9557dd0) is a squash commit on the
# spine. This repo squash-merges today, which made the defect latent rather than
# active — main carries 91 merge commits, so "latent" is not "impossible".
#
# WHAT THIS DOES NOT DO — stated so it is not mistaken for coverage:
#
#   * It cannot verify the acknowledgement is TRUE. It requires a `Tag-Point:`
#     trailer; a trailer saying "Tag-Point: whatever" passes. This is a speed
#     bump against autopilot, not a proof. Its value is forcing the tagger to
#     look at the list — precisely what did not happen for v0.31.0 and v0.33.0.
#   * It cannot catch tagging a commit that is too NEW. Different failure.
#   * It cannot catch work merged AFTER the tag. Nothing at tag time can, and
#     step 2 deliberately stops trying.
#   * On the first-parent spine it does not see commits that reached main other
#     than through a merge or squash — a direct push to main. Branch protection
#     forbids those.
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

# A LIGHTWEIGHT TAG MAKES THIS GATE VACUOUS — it must be rejected, not tolerated.
#
# This header used to claim a lightweight tag "degrades to a slightly weaker
# check". It degrades to NO check, and an audit demonstrated it on this repo:
# the same commit tagged both ways gave FAIL (annotated) and PASS (lightweight).
#
# The mechanism: `creatordate` on a lightweight tag is the TAGGED COMMIT'S OWN
# committer date. So `--before=T` asks for commits descended from C that predate
# C — on a linear history, always empty. Unconditional PASS. Worse, the rule-4
# grep reads `%(contents)`, which for a lightweight tag returns the COMMIT
# message: the acknowledgement can be satisfied by text the tagger never wrote.
#
# The trigger is not exotic, it is the sloppier path: `git tag v0.36.0` instead
# of `git tag -s v0.36.0`. The gate would disarm on exactly the mistake that
# most needs gating. Repo policy is signed annotated tags regardless.
OBJTYPE="$(git for-each-ref "refs/tags/${TAG}" --format='%(objecttype)')"
if [ "${OBJTYPE}" != "tag" ]; then
  echo "::error::${TAG} is a lightweight tag (objecttype=${OBJTYPE:-missing})."
  echo "::error::This gate cannot evaluate one: its date is the tagged commit's"
  echo "::error::own committer date, so the excluded set is always empty and the"
  echo "::error::check silently passes whatever the tag point is."
  echo "::error::Re-tag with a signed annotated tag:  git tag -s ${TAG}"
  exit 1
fi

T="$(git for-each-ref "refs/tags/${TAG}" --format='%(taggerdate:iso-strict)')"
if [ -z "${T}" ]; then
  echo "::error::Could not read a tagger date for ${TAG}."
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

EXCLUDED="$(git log --first-parent --format='%h %ci %s' --before="${T}" "${C}..origin/main")"

if [ -z "${EXCLUDED}" ]; then
  echo "PASS: ${TAG} at ${C} (${T}) — nothing predating the tag was left out."
  exit 0
fi

COUNT="$(printf '%s\n' "${EXCLUDED}" | wc -l | tr -d ' ')"

# ANCHORED, NOT ANYWHERE. This was `grep -qiE 'tag[ -]point'` over the whole
# message, which the same audit showed disarms itself: the commit that ADDS this
# gate has the subject "ci(release): gate the tag point …", and release-notes tag
# messages here list commit subjects verbatim (v0.35.0's does). So the first
# release whose notes happen to quote that subject would auto-acknowledge — the
# gate switching itself off with text nobody wrote as a statement.
#
# Anchoring to the start of a line separates the two cases exactly. A quoted
# subject appears mid-line, after a short hash or a bullet; an acknowledgement is
# something the tagger began a line with. Both spellings are accepted, because
# the point is deliberateness, not syntax:
#
#     Tag-Point: bump commit abc1234 — held under the batching policy
#     Tag point is the bump commit ff470a2, all 17 checks green.   <- v0.34.0
#
# Requiring the trailer form alone was tried and rejected: it fails v0.34.0,
# whose acknowledgement is specific, deliberate, and correct. A gate that rejects
# a real acknowledgement teaches people to route around it.
if git tag -l "${TAG}" --format='%(contents)' | grep -qiE '^[[:space:]]*Tag[ -]point'; then
  echo "PASS (acknowledged): ${TAG} excludes ${COUNT} commit(s) that predate it,"
  echo "and its message states the tag point on a line of its own:"
  printf '%s\n' "${EXCLUDED}" | sed 's/^/  excluded: /'
  exit 0
fi

echo "::error::${TAG} was cut at ${C} (${T}), but ${COUNT} commit(s) were"
echo "::error::already on main before that and are not in the release:"
printf '%s\n' "${EXCLUDED}" | sed 's/^/::error::  /'
echo "::error::"
echo "::error::Either tag the tip of main, or state the tag point at the START"
echo "::error::of a line in the tag message, e.g.:"
echo "::error::"
echo "::error::  Tag point is the bump commit abc1234; the tag was deliberately"
echo "::error::  held after the bump merged, under the batching policy."
echo "::error::"
echo "::error::Anchored, not anywhere: a mid-line mention (release notes quoting"
echo "::error::a commit subject that contains the words) is not an acknowledgement"
echo "::error::and no longer counts as one."
exit 1
