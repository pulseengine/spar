#!/usr/bin/env python3
"""Assert no closing keyword reaches a merge, in a PR body or a commit message.

GitHub auto-closes an issue when a merged PR's BODY carries a closing keyword,
and again when a commit carrying one lands on the default branch. This project's
closure rule is the opposite of both: an issue closes against a PUBLISHED
RELEASE, because merged is not released. v0.43.0 was tagged, built all 17
artifacts, and published nothing (#464) — so "merged" and "available" are
separated here by more than pedantry.

## The three instances this exists for

  * #459  `Fixes #459.`           in PR #463's BODY      -> closed on merge
  * #464  `Fixes #464.`           in PR #465's BODY      -> closed on merge
  * #455  `(closes #455 item 1)`  in e293a2d's MESSAGE   -> closed the UMBRELLA

Both paths are live, which is why this reads the body AND every commit message
in the range. Checking only one would have caught two of the three.

## Why a qualifier must not exempt it

The third instance is the dangerous one, and it was written by someone being
PRECISE. The commit ends with the correct trailer `Refs #455, #468`, and its
prose says `(closes #455 item 1)` — "closes item one of #455" in English.
GitHub's parser matches `closes #455`, discards "item 1", and closes a
four-item issue with three items open. An external reporter saw their issue
closed while the work was outstanding.

So the rule is: a closing keyword immediately preceding an issue reference is
forbidden REGARDLESS of what follows it. A checker that allowed
`closes #455 item 1` because of the trailing words would reproduce exactly the
bug it is meant to prevent.

## What this deliberately does NOT do

It does not decide when an issue should close — that stays a human judgement
against a published release. It only removes the AUTOMATIC premature close,
which is the half a machine can own. `Refs #N` is the allowed form and is not
flagged.

Nor does it claim the keyword is always wrong: on a repo that closes on merge it
is correct. It is wrong HERE, which is why the gate is in this repo and not a
lint anyone should adopt.

Usage:
    tools/check_closing_keywords.py --body-file <f> --range <base>..<head>
    tools/check_closing_keywords.py --text "some message"
    tools/check_closing_keywords.py --self-test
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

# GitHub's closing keywords, all inflections it honours.
_KEYWORDS = r"clos(?:e|es|ed)|fix(?:|es|ed)|resolv(?:e|es|ed)"

# An issue reference GitHub resolves: #N, GH-N, owner/repo#N, or a full URL.
_REF = r"(?:#\d+|GH-\d+|[\w.-]+/[\w.-]+#\d+|https?://github\.com/[\w.-]+/[\w.-]+/issues/\d+)"

# Deliberately NOT anchored at a line start and with NOTHING after the ref:
# the #455 instance sat mid-sentence inside parentheses with "item 1" after it.
# `re.I` because GitHub honours `Fixes`, `fixed`, `FIX` alike.
CLOSING = re.compile(rf"\b({_KEYWORDS})\b\s*:?\s+({_REF})", re.IGNORECASE)


# A fenced block or an inline code span, which GitHub does NOT act on.
#
# MEASURED, not assumed: PR #471's body carried the literal `Closes #469.`
# inside backticks — placed there by the note explaining this very rule — and
# #469 remained OPEN after that PR merged. So a code span is inert to GitHub's
# parser, and flagging it would make it impossible to DOCUMENT the rule without
# tripping it. That is not a hypothetical: this guard's first run against
# reality failed #471 for exactly that reason.
_FENCE = re.compile(r"^\s*(```|~~~)")
# Double-backtick spans FIRST: ``like `this` `` is how a code span that
# itself contains backticks is written, and GitHub renders it as code. A
# single-backtick-only pattern mis-parses it and leaves text that looked
# quoted exposed — which over-flagged THIS guard's own pull-request body on
# its first run. Over-flagging documentation is how a gate gets routed
# around, so the rule must tolerate being written about.
_CODE_SPAN = re.compile(r"``.*?``|`[^`]*`")


def findings(text: str, where: str) -> list[tuple[str, str, str]]:
    """(where, keyword+ref, the line it sat on) for each closing keyword.

    Code spans and fenced blocks are skipped — see `_CODE_SPAN`.
    """
    out = []
    in_fence = False
    for line in text.splitlines():
        if _FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        # Blank out inline code BEFORE matching, so a keyword quoted for
        # documentation is inert here exactly as it is to GitHub.
        scannable = _CODE_SPAN.sub(lambda m: " " * len(m.group(0)), line)
        for m in CLOSING.finditer(scannable):
            out.append((where, m.group(0), line.strip()))
    return out


def commit_messages(rng: str) -> list[tuple[str, str]]:
    """(sha, message) for each commit in `rng`. Exits 2 if git cannot answer."""
    try:
        shas = subprocess.run(
            ["git", "rev-list", rng], capture_output=True, text=True, check=True
        ).stdout.split()
    except subprocess.CalledProcessError as exc:
        # A range git cannot resolve is NOT an absence of findings. The whole
        # point of this guard is that silence must not read as a clean result.
        print(f"::error::cannot resolve commit range {rng!r}: {exc.stderr.strip()}",
              file=sys.stderr)
        sys.exit(2)
    msgs = []
    for sha in shas:
        msg = subprocess.run(
            ["git", "log", "-1", "--format=%B", sha],
            capture_output=True, text=True, check=True
        ).stdout
        msgs.append((sha[:9], msg))
    return msgs


def check(body: str | None, rng: str | None, out=sys.stdout) -> int:
    found: list[tuple[str, str, str]] = []
    scanned = 0

    if body is not None:
        found += findings(body, "PR body")
        scanned += 1
    if rng is not None:
        for sha, msg in commit_messages(rng):
            found += findings(msg, f"commit {sha}")
            scanned += 1

    # Nothing scanned is not "nothing found". A workflow that failed to pass the
    # body and the range would otherwise report a clean bill of health having
    # read no text at all — the exact shape of the defects this repo keeps
    # finding (#381, #403, #464).
    if scanned == 0:
        print("::error::no PR body and no commit range were given, so nothing "
              "was scanned. That is not a pass.", file=sys.stderr)
        return 2

    print("== closing-keyword guardrail ==", file=out)
    print(f"texts scanned: {scanned}", file=out)

    if not found:
        print("ok — no closing keyword; `Refs #N` is the allowed form", file=out)
        return 0

    print(f"::error::{len(found)} closing keyword(s) found. GitHub auto-closes on "
          f"these, but an issue here closes against a PUBLISHED release — merged "
          f"is not released (v0.43.0 built everything and published nothing). Use "
          f"`Refs #N`.", file=sys.stderr)
    for where, hit, line in found:
        print(f"  {where}: {hit!r}", file=sys.stderr)
        print(f"    in: {line[:100]}", file=sys.stderr)
    print("\nA qualifier does not help: `(closes #455 item 1)` closed a four-item "
          "issue with three items open, because GitHub discards the \"item 1\".",
          file=sys.stderr)
    return 1


# ── self-test ────────────────────────────────────────────────────────────────

CASES: list[tuple[str, str, int, str]] = [
    # (name, text, want_findings, proves)
    (
        "`Fixes #1` is rejected",
        "Fixes #1",
        1,
        "the base case; #459 and #464 were closed on merge by exactly this",
    ),
    (
        "`Refs #1` is ALLOWED",
        "Refs #1",
        0,
        "the discriminating half — a checker flagging every `#N` would also "
        "reject the one form we require, and would not be testing the keyword",
    ),
    (
        "`(closes #455 item 1)` is rejected despite the qualifier",
        "Carries the fix from #468\n(closes #455 item 1): an end-to-end latency ...",
        1,
        "THE instance this guard exists for. Allowing it because of the trailing "
        "'item 1' would reproduce the bug: GitHub discards the qualifier and "
        "closed a four-item umbrella with three items open",
    ),
    (
        "a correct `Refs` trailer does NOT excuse a keyword earlier in the prose",
        "(closes #455 item 1): something\n\nRefs #455, #468",
        1,
        "the real e293a2d shape — the author used the right trailer AND a scoped "
        "parenthetical; the trailer does not save it",
    ),
    (
        "every inflection GitHub honours",
        "close #1 closes #2 closed #3 fix #4 fixes #5 fixed #6 "
        "resolve #7 resolves #8 resolved #9",
        9,
        "dropping any inflection leaves a live path; GitHub treats all nine alike",
    ),
    (
        "case is irrelevant",
        "FIXES #1\nFixed #2\ncLoSeS #3",
        3,
        "GitHub is case-insensitive here, so a case-sensitive check is a hole",
    ),
    (
        "`Fixes: #1` with a colon is rejected",
        "Fixes: #1",
        1,
        "GitHub accepts the colon form; omitting it leaves a path open",
    ),
    (
        "cross-repo and URL references are rejected",
        "Fixes pulseengine/rivet#7\nCloses https://github.com/pulseengine/spar/issues/8",
        2,
        "both forms close across repos; `#N` alone is not the whole surface",
    ),
    (
        "a bare issue mention is not a closing keyword",
        "See #1 and #2. Related to #3. Refs #4.",
        0,
        "the guard must not become noise on ordinary cross-references, or it "
        "gets routed around",
    ),
    (
        "a keyword inside a CODE SPAN is inert, as it is to GitHub",
        "This was `Closes #469.` and was changed to `Refs`.",
        0,
        "MEASURED: #471's body carried exactly this and #469 stayed OPEN after "
        "the merge, so GitHub does not act on code spans. Flagging it would make "
        "the rule impossible to document — this guard's first reality run failed "
        "#471 for precisely that reason",
    ),
    (
        "a DOUBLE-backtick span is inert too",
        "the bound: (``  `Refs #1` is right, Fixes #2 is wrong  ``) stays quoted",
        0,
        "``...`` is how a code span containing backticks is written, and GitHub "
        "renders it as code. A single-backtick-only strip mis-parses it and "
        "exposed `Fixes #2` — measured on THIS guard's own PR body, which it "
        "wrongly failed. Over-flagging documentation is how a gate gets disabled",
    ),
    (
        "a keyword inside a FENCED block is inert",
        "before\n```\nFixes #1\n```\nafter",
        0,
        "the same exemption for the block form, so a quoted commit message in a "
        "PR body does not trip the gate",
    ),
    (
        "the exemption does NOT swallow a real keyword on the same line",
        "`Refs #1` is right, Fixes #2 is wrong",
        1,
        "THE BOUND on the exemption: blanking the code span must not blank the "
        "rest of the line. Without this case, an over-broad strip (e.g. dropping "
        "the whole line that contains any backtick) would hide live keywords",
    ),
    (
        "a word merely containing a keyword is not one",
        "This prefixes #1 and closeness #2 and affixed #3",
        0,
        "`\\b` word boundaries: without them 'prefixes #1' matches 'fixes #1' "
        "and the guard cries wolf on prose",
    ),
]


def self_test() -> int:
    import tempfile
    from pathlib import Path

    failures = 0
    for name, text, want, proves in CASES:
        got = len(findings(text, "t"))
        ok = got == want
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] {name}: {got} finding(s) (want {want})")
        print(f"       proves: {proves}")

    # Exit codes, not just counts.
    rc_clean = check("Refs #1", None, out=open("/dev/null", "w"))
    rc_dirty = check("Fixes #1", None, out=open("/dev/null", "w"))
    for label, got, want in (("clean body exits 0", rc_clean, 0),
                             ("dirty body exits 1", rc_dirty, 1)):
        ok = got == want
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] {label}: exit {got} (want {want})")
    print("       proves: the verdict reaches the process result, not only stdout")

    # Nothing scanned must be INCONCLUSIVE, never a pass.
    rc = check(None, None, out=open("/dev/null", "w"))
    ok = rc == 2
    failures += not ok
    print(f"[{'PASS' if ok else 'FAIL'}] scanning nothing is exit 2, not 0: {rc}")
    print("       proves: a guard that read no text must not report clean — the "
          "shape of #381, #403 and #464")

    # An unresolvable range must be inconclusive too.
    with tempfile.TemporaryDirectory() as td:
        try:
            rc = check(None, "definitely-not-a-ref..also-not", out=open("/dev/null", "w"))
        except SystemExit as exc:
            rc = exc.code
        ok = rc == 2
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] an unresolvable commit range is exit 2: {rc}")
        print("       proves: git failing to answer is not an absence of findings")

    print(f"\n{failures} self-test(s) failed." if failures else "\nAll self-tests passed.")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--body-file", help="file containing the PR body")
    ap.add_argument("--text", help="literal text to scan")
    ap.add_argument("--range", dest="rng", help="git commit range, e.g. origin/main..HEAD")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    body = args.text
    if args.body_file:
        from pathlib import Path
        try:
            body = Path(args.body_file).read_text()
        except OSError as exc:
            print(f"::error::cannot read --body-file {args.body_file!r}: {exc}",
                  file=sys.stderr)
            return 2
    return check(body, args.rng)


if __name__ == "__main__":
    sys.exit(main())
