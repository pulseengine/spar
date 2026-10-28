#!/usr/bin/env python3
"""Assert each guard tool's --self-test CATCHES a declared source mutation.

REQ-GUARD-SELFTEST-POTENCY-001 (#405).

WHY THIS EXISTS
===============

Every artifact guardrail in this repo ships a `--self-test`: a decision table
that runs BEFORE the gate it guards is allowed to judge anything. The table
exists so that a refactor which makes the gate toothless fails loudly, instead
of letting the real gate below it pass vacuously.

But a self-test is itself an operation that can produce "nothing happened"
while reading like "it worked". A case whose assertion never actually depends
on the code under test is green forever — including after the code it was
meant to pin has rotted out from under it. REQ-GUARD-GATE-EVIDENCE-002 once
asserted, in prose, that "each was mutation-tested to show its cases are
load-bearing rather than merely green." That universal was FALSE and was
withdrawn (#405): an adversarial review applied one-line mutations to two of
the six tools and their self-tests stayed green —

  * `check_verification_filters.py`: `filt in name` ->
    `filt.split('::')[-1] in name` changed vacuity semantics for the 16-of-60
    real filters that carry `::`, and every fixture stayed green because none
    of them carried a `::`.
  * `check_lean_sorries.py`: deleting the below-floor ratchet-notice branch
    left the suite green, because no case read what the branch printed.

Both holes were then closed by hand (a `::` fixture; a `want_msg` that pins the
notice text). But nothing PREVENTS them reopening: delete either case and the
suite is green again, silently. Hand-added cases are the same species of
unfalsifiable-by-construction guard the requirement is about — one level up.

THIS IS THE MECHANISM the withdrawn prose claimed to have. It is a standing,
CI-gated oracle rather than a one-time manual shell run: for each mutant in a
declared table, apply the textual substitution to the tool's source, run that
tool's `--self-test`, and assert it now FAILS. A mutant the suite still passes
is a SURVIVOR — a real hole in that suite — and reds this check.

WHAT IT DOES AND DOES NOT BUY (stated so the claim stays honest)
================================================================

Mutation testing shows the cases PRESENT are load-bearing. It CANNOT show the
case set COVERS the hazard. A tool with a single, well-pinned case will pass
every mutation that flips that case and still miss a defect for which it has no
case at all. So a green run here means "the declared mutants are all caught",
never "this suite is complete". The declared table is an allow-list that grows;
a hazard with no mutant in the table is simply not yet asserted, and that is a
known gap, not a hidden one.

HOW IT DECIDES, and the trap it is built to avoid
=================================================

The one thing a mutation harness must not do is report a mutation that FAILED
TO APPLY as "caught" — a substitution whose `old` text has drifted out of the
source silently applies nothing, the unmodified self-test passes, and a naive
harness reads that as SURVIVED (or, worse, the inverse) when in truth it tested
nothing. This repo has already been bitten by exactly that shape. So:

  * a declared `old` that is absent, or present a different number of times
    than declared, is a hard ERROR (exit 2) naming the mutant — never a verdict;
  * `new == old` is a hard ERROR (a no-op mutation proves nothing);
  * the tool's UNMODIFIED self-test is run first and must exit 0 — a suite that
    is already red cannot have its potency judged, and a mutation applied to it
    would look "caught" for the wrong reason. That is an ERROR, not a CAUGHT.

Only after those guards does a mutant get a CAUGHT/SURVIVED verdict.

Exit codes match the sibling guardrails: 0 = every declared mutant caught,
1 = at least one survivor (a real hole), 2 = a declaration or baseline problem
that means potency could not be judged. The mutant count is printed on every
path, success included, for the same reason the other guards print one: a gate
that says nothing on success cannot distinguish "checked everything" from
"checked nothing".

stdlib-only and self-contained: it copies each tool to a temp file and runs the
mutated COPY, so the working tree is never modified even if a run is killed
mid-flight. No workflow here installs a Python package, and a required check
that depends on an unverified runner package is a check that can fail to run,
which reads as approval.
"""

from __future__ import annotations

import argparse
import io
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Mutant:
    """A declared textual substitution against one guard tool's source.

    `old` -> `new` must, when applied, make `tool`'s `--self-test` exit
    non-zero. `count` is how many times `old` is expected to occur: the default
    of 1 refuses to apply a substitution that would hit an unexpected number of
    sites, because a mutant that lands somewhere other than intended is a mutant
    that proves nothing about the case it names.
    """

    tool: str          # filename under the tools dir, e.g. "check_lean_sorries.py"
    id: str            # short stable id, unique within the table
    old: str           # exact source substring to replace
    new: str           # replacement; must differ from `old`
    note: str          # which self-test case is expected to catch it, and why it matters
    count: int = 1     # expected number of occurrences of `old`


# The declared mutants. Each targets a guard tool's self-test at a case that
# MUST be load-bearing. Seeded with the two holes #405 demonstrated (now closed,
# and kept closed by this table), plus further mutations across the same tools.
# A mutant is added here only after it has been confirmed CAUGHT on the tree it
# lands with; a mutation that survives is not "declared and tolerated", it is a
# hole to close (or a case to add) before it may appear here.
MUTANTS: list[Mutant] = [
    # ── check_verification_filters.py ──────────────────────────────────────
    Mutant(
        tool="check_verification_filters.py",
        id="vf-colon-tail",
        old="and not any(e[3] in name for name in pool)",
        new="and not any(e[3].split('::')[-1] in name for name in pool)",
        note="#405 mutant A / #404 cross-package scope: a filter must be matched "
             "WHOLE, not by its last `::` segment. Caught by the "
             "`a ::-filter is matched WHOLE` case; survives if that case is dropped.",
    ),
    Mutant(
        tool="check_verification_filters.py",
        id="vf-none-lenient",
        old='vacuous = [e for e in empty if e[1] in _CLAIMING or e[1] == "<none>"]',
        new='vacuous = [e for e in empty if e[1] in _CLAIMING]',
        note="fail-closed on an unclassifiable status: an artifact with no "
             "readable status and a vacuous filter must FAIL, not be treated as a "
             "plan. Caught by `NO status is unclassifiable, so it fails closed`.",
    ),
    # ── check_lean_sorries.py ──────────────────────────────────────────────
    Mutant(
        tool="check_lean_sorries.py",
        id="ls-drop-below-floor",
        old='    if total < max_sorries:\n'
            '        print("", file=out)\n'
            '        print(f"::notice::{total} sorries is BELOW the floor of {max_sorries} — "\n'
            '              f"lower --max-sorries to {total} in proofs.yml so the ratchet holds "\n'
            '              f"the ground that was gained.", file=out)\n\n',
        new="",
        note="#405 mutant B: deleting the below-floor ratchet-notice branch. The "
             "notice is output-only, so an exit-code assertion cannot see it; caught "
             "only by the `below floor PASSES` case's want_msg pin on 'lower "
             "--max-sorries to 0'.",
    ),
    Mutant(
        tool="check_lean_sorries.py",
        id="ls-drop-axiom",
        old='    ("axiom", re.compile(r"^[ \\t]*axiom[ \\t]+")),\n',
        new="",
        note="an `axiom` asserts a statement with no proof — strictly worse than a "
             "sorry. Dropping it from the obligation set makes `axiom cheat : 7 = 8` "
             "count as zero. Caught by `REGRESSION #385(b): axiom COUNTS` (want_msg "
             "'axiom=1').",
    ),
    # ── run_verification.py ────────────────────────────────────────────────
    Mutant(
        tool="run_verification.py",
        id="rv-zero-executed-lenient",
        old="    if passed == 0:",
        new="    if passed < 0:",
        note="#403: a run that matched artifacts but executed zero commands (every "
             "match SKIPPED) must exit 1, not report success having verified nothing. "
             "`passed < 0` is never true, so the zero-executed guard never fires and "
             "an all-skipped run scores 0 again. Caught by the `all skipped (nothing "
             "ran)` case and the `verified nothing` message pin; survives if either "
             "is dropped.",
    ),
    # ── check_stale_scope.py ───────────────────────────────────────────────
    Mutant(
        tool="check_stale_scope.py",
        id="ss-swallow-bad-yaml",
        old='print(f"::error::cannot parse {path}: {exc}", file=sys.stderr)\n'
            '                sys.exit(2)',
        new='print(f"::error::cannot parse {path}: {exc}", file=sys.stderr)\n'
            '                continue',
        note="an artifact file the guard cannot parse must be INCONCLUSIVE, never "
             "scanned past as zero stale artifacts. The guard's error path yields "
             "its IDEAL reading — a truncated artifacts file counts nothing and the "
             "run prints `ok` — which is the red flag the whole tool is about. The "
             "property was asserted in a source comment and pinned by nothing: this "
             "mutant was confirmed SURVIVED before the `an unparseable artifact file "
             "is exit 2` case was added, and CAUGHT after.",
    ),
    Mutant(
        tool="check_stale_scope.py",
        id="ss-skip-statusless",
        old='f"closed one and must not guess.", file=sys.stderr)\n'
            '                        sys.exit(2)',
        new='f"closed one and must not guess.", file=sys.stderr)\n'
            '                        continue',
        note="an artifact declaring a release with NO status is unclassifiable, and "
             "every remaining branch reads it as not-stale if it is waved past. This "
             "reverts the guard to the silent skip it shipped with. Inert today (0 of "
             "937 artifacts) but 322 carry no top-level status, so the tree can grow "
             "into it. Caught by `a release with no status is exit 2`.",
    ),
    Mutant(
        tool="check_stale_scope.py",
        id="ss-skip-unparseable-release",
        old='f"made.", file=sys.stderr)\n'
            '                        sys.exit(2)',
        new='f"made.", file=sys.stderr)\n'
            '                        continue',
        note="a release string the guard cannot ORDER (`release: next-sprint`) is a "
             "measurement that was not made, not a release above the line. The "
             "original `if rel is not None` skipped it silently. Caught by `an "
             "unparseable release string is exit 2`; its deliberate bound — a SETTLED "
             "artifact's release need not parse — is pinned by its own case so a "
             "tightening cannot swallow it.",
    ),
    # REQ-GUARD-STALE-SCOPE-002's discriminator. Each of these four was
    # confirmed to SURVIVE with its case removed before being declared here,
    # and each is killed by EXACTLY ONE case — which is what makes the cases
    # load-bearing rather than four restatements of one assertion.
    Mutant(
        tool="check_stale_scope.py",
        id="ss-excuse-any-status",
        old="""return status == STATUS_SETTLING_A_VERIFICATION_ARTIFACT and verification_method(a) is not None""",
        new="""return verification_method(a) is not None""",
        note="dropping the status half: a test that was never BUILT (`method` + "
             "`proposed`) is the realest debt there is, and excusing it on the "
             "marker alone converts unbuilt tests into settled ones. Caught only by "
             "`a verification artifact at proposed is STILL debt`.",
    ),
    Mutant(
        tool="check_stale_scope.py",
        id="ss-never-excuse",
        old="""return status == STATUS_SETTLING_A_VERIFICATION_ARTIFACT and verification_method(a) is not None""",
        new="""return False""",
        note="collapsing the excusal to nothing restores the 90-count bug: ten "
             "TEST-GUARD-* artifacts that exist and run, counted as debt in a state "
             "nothing in this repo ever leaves (ZERO of 129 TEST-* are `verified`). "
             "Caught only by `a verification artifact at implemented is settled`.",
    ),
    Mutant(
        tool="check_stale_scope.py",
        id="ss-fields-presence-is-marker",
        old='method = fields.get("method")\n'
            '    return str(method) if method else None',
        new='return "present" if "method" in fields or True else None',
        note="reading the mere EXISTENCE of a `fields` block as the marker. "
             "`fields: null` and `fields: {}` are both real in this tree, so this "
             "would settle every artifact that happens to carry fields at all. "
             "Caught only by `an empty or null fields block does not excuse`.",
    ),
    Mutant(
        tool="check_stale_scope.py",
        id="ss-link-also-excuses",
        old="""return status == STATUS_SETTLING_A_VERIFICATION_ARTIFACT and verification_method(a) is not None""",
        new="""return status == STATUS_SETTLING_A_VERIFICATION_ARTIFACT and (verification_method(a) is not None or has_verifies_link(a))""",
        note="promoting the verifies link from a printed CROSS-CHECK to a second way "
             "of being excused. Both markers select the same ten artifacts today, so "
             "this mutant changes NO current count — it is caught only because the "
             "case constructs the divergence, which is the whole reason that case "
             "exists. Caught only by `the verifies link does not excuse on its own`.",
    ),
    # ── check_self_test_potency.py — THE SELF-REFERENCE ────────────────────
    #
    # REQ-GUARD-SELFTEST-COVERAGE-001 names this hole explicitly: the potency
    # checker ships a `--self-test` and was the one tool nothing proved potent.
    # The table now mutates itself, which is not a curiosity — a vacuous case in
    # THIS suite would make every other verdict here unreliable.
    #
    # Earning it found a real defect. `pt-empty-denominator-ok` SURVIVED on the
    # first attempt: the empty-denominator case was passing off the ORPHAN
    # branch, since against an empty directory every declared mutant names a
    # missing tool, and both branches return 2. The case was repaired to pass an
    # EMPTY mutant list — the one input only the branch under test can answer —
    # and the mutant is now caught. That is the table catching a vacuous case in
    # the gate added to catch vacuous cases.
    Mutant(
        tool="check_self_test_potency.py",
        id="pt-empty-denominator-ok",
        # SPLIT LITERAL, and it is load-bearing: this tool is its own target,
        # so a contiguous `old` would occur twice in the source — here and at
        # the real site — and `apply_mutation` replaces ALL occurrences, which
        # would mutate this declaration too. Adjacent literals keep the value
        # identical while leaving exactly one contiguous match in the file.
        old="    if not " "shipping:",
        new="    if False:",
        note="a coverage ratio over an EMPTY denominator satisfies any floor by "
             "arithmetic — the fail-open of #381's shape, where a gate that measured "
             "nothing printed a clean result. Caught only by `an EMPTY denominator is "
             "exit 2`, and only since that case stopped relying on the orphan branch.",
    ),
    Mutant(
        tool="check_self_test_potency.py",
        id="pt-no-upper-bound",
        old="    if n > " "min_covered:",
        new="    if False:",
        note="removing the UPPER arm turns the exact ratchet into a floor, so newly "
             "covered tools become unrecorded slack the next pass can re-spend "
             "instead of ground that is locked in. Caught only by `ABOVE the floor "
             "also fails`.",
    ),
    Mutant(
        tool="check_self_test_potency.py",
        id="pt-ignore-orphans",
        old="    if " "orphans:",
        new="    if False:",
        note="a table entry naming a renamed or deleted tool would shrink real "
             "coverage while the count still read correct — the hand-maintained-list "
             "drift this whole table exists to stop. Caught only by `a mutant naming "
             "a tool with no --self-test is exit 2`.",
    ),
    # ── check_closing_keywords.py ──────────────────────────────────────────
    Mutant(
        tool="check_closing_keywords.py",
        id="ck-require-eol",
        old='CLOSING = re.compile(rf"\\b({_KEYWORDS})\\b\\s*:?\\s+({_REF})", re.IGNORECASE)',
        new='CLOSING = re.compile(rf"\\b({_KEYWORDS})\\b\\s*:?\\s+({_REF})\\s*$", re.IGNORECASE)',
        note="requiring the reference to END the line is the exact hole that let "
             "`(closes #455 item 1)` through — a keyword mid-sentence inside "
             "parentheses, written by someone scoping it to one item, which GitHub "
             "de-scoped and used to close a four-item umbrella with three items "
             "open. Caught by the `(closes #455 item 1)` case and by `a correct "
             "Refs trailer does NOT excuse a keyword earlier in the prose`.",
    ),
    Mutant(
        tool="check_closing_keywords.py",
        id="ck-strip-whole-line",
        old='blanked.append(_CODE_SPAN.sub(lambda m: " " * len(m.group(0)), line))',
        new='blanked.append(" " * len(line) if "`" in line else line)',
        note="the BOUND on the code-span exemption. Blanking any line containing a "
             "backtick would hide a live keyword that merely shares a line with "
             "quoted text — `\x60Refs #1\x60 is right, Fixes #2 is wrong`. Confirmed "
             "load-bearing: with that one case removed the mutant SURVIVES, so "
             "nothing else catches it.",
    ),
    Mutant(
        tool="check_closing_keywords.py",
        id="ck-per-line-scan",
        old='scannable = "\\n".join(blanked)',
        new='scannable = "\\x00".join(blanked)',
        note="the #493 hole: a NUL is not `\\s`, so this restores the per-line scan "
             "that passed the v0.46.0 squash cf0c082 — headline ending `fix`, blank "
             "line, paragraph opening `#460` — which GitHub read as `fix #460` and "
             "used to close #460 with its fix deferred. Caught by the BLANK LINE and "
             "single-line-break cases.",
    ),
    Mutant(
        tool="check_closing_keywords.py",
        id="ck-nothing-scanned-ok",
        old='        print("::error::no PR body and no commit range were given, so nothing "\n'
            '              "was scanned. That is not a pass.", file=sys.stderr)\n'
            '        return 2',
        new='        print("::error::no PR body and no commit range were given, so nothing "\n'
            '              "was scanned. That is not a pass.", file=sys.stderr)\n'
            '        return 0',
        note="a workflow that failed to pass the body and the range must not report "
             "a clean bill of health having read no text — the shape of #381, #403 "
             "and #464. Caught by `scanning nothing is exit 2, not 0`.",
    ),
    # ── #400: the per-PR in-diff mutation gate ──────────────────────────
    Mutant(
        tool="check_mutants_report.py",
        id="mr-in-diff-zero-selected-ok",
        old='''            file=out,
        )
        return 1

    try:
        counts = _load_counts(rdir)
        note = _cross_check(rdir, counts)
        note = f"{note}; {_assert_completed(rdir)}"''',
        new='''            file=out,
        )
        return 0

    try:
        counts = _load_counts(rdir)
        note = _cross_check(rdir, counts)
        note = f"{note}; {_assert_completed(rdir)}"''',
        note="#400's named trap: `--in-diff` selecting ZERO mutants over changed "
             "source must FAIL, not read as 0 survivors (cargo-mutants 27.1.0 "
             "then writes no report at all). Caught by `changed source, ZERO "
             "selected (no report) fails`.",
    ),
    Mutant(
        tool="check_mutants_report.py",
        id="mr-in-diff-one-survivor-ok",
        old="    if missed > 0:\n",
        new="    if missed > 1:\n",
        note="DEC-MUTANTS-IN-DIFF-001: zero survivors in the lines a change "
             "touches. Caught by `ONE survivor in the diff fails`.",
    ),
    Mutant(
        tool="check_mutants_report.py",
        id="mr-workflow-in-diff-unguarded",
        old="    if pr_only not in diff_cond:\n",
        new="    if False and pr_only not in diff_cond:\n",
        note="a per-PR gate that also runs off the PR path, or not at all on "
             "PRs, is defined-but-not-enforced. Caught by `an in-diff gate that "
             "is not PR-only FAILS`.",
    ),
]


def apply_mutation(src: str, mut: Mutant) -> str:
    """Return `src` with `mut.old` -> `mut.new`, or raise on a declaration fault.

    Fail-closed by design: a mutation that cannot be applied exactly as declared
    is a defect in the TABLE, not a property of the tool, and must never be
    silently smoothed into a verdict (a substitution that matches nothing would
    otherwise report the unmodified suite as SURVIVED/CAUGHT — testing nothing).
    """
    if mut.new == mut.old:
        raise ValueError(f"mutant {mut.id!r}: new == old, a no-op mutation proves nothing")
    occurrences = src.count(mut.old)
    if occurrences != mut.count:
        raise ValueError(
            f"mutant {mut.id!r}: expected `old` to occur {mut.count} time(s) in "
            f"{mut.tool}, found {occurrences}. The source has drifted from the "
            f"declaration; re-derive the mutant rather than let it apply nothing."
        )
    mutated = src.replace(mut.old, mut.new)
    if mutated == src:
        raise ValueError(f"mutant {mut.id!r}: substitution left the source unchanged")
    return mutated


def run_self_test(source: str, ref_name: str) -> tuple[int, str]:
    """Write `source` to a temp file and run it with `--self-test`.

    Runs a COPY so the real tool is never mutated on disk. `ref_name` keeps the
    temp file's basename close to the original so any path the tool prints is
    recognisable.
    """
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / ref_name
        p.write_text(source, encoding="utf-8")
        proc = subprocess.run(
            [sys.executable, str(p), "--self-test"],
            capture_output=True,
            text=True,
        )
    return proc.returncode, proc.stdout + proc.stderr


# Verdicts.
CAUGHT = "CAUGHT"
SURVIVED = "SURVIVED"
ERROR = "ERROR"


def evaluate(mut: Mutant, tool_path: Path) -> tuple[str, str]:
    """(verdict, detail) for one mutant against the tool at `tool_path`."""
    if not tool_path.is_file():
        return ERROR, f"tool not found: {tool_path}"
    src = tool_path.read_text(encoding="utf-8")

    # Baseline: the unmodified suite must be green, or potency is undefined and a
    # mutation would look "caught" for a reason that has nothing to do with it.
    base_rc, base_out = run_self_test(src, tool_path.name)
    if base_rc != 0:
        tail = "\n".join(base_out.splitlines()[-4:])
        return ERROR, (f"baseline --self-test is not green (exit {base_rc}); a red "
                       f"suite cannot be potency-judged.\n{tail}")

    try:
        mutated = apply_mutation(src, mut)
    except ValueError as e:
        return ERROR, str(e)

    rc, _out = run_self_test(mutated, tool_path.name)
    if rc != 0:
        return CAUGHT, f"self-test exits {rc} under the mutation"
    return SURVIVED, ("self-test still exits 0 under the mutation — no case reads "
                      "the behaviour this mutant changes")


# ── Coverage: a guard tool shall not ship a self-test no mutant exercises ────
#
# REQ-GUARD-SELFTEST-COVERAGE-001 (#461). The table above is hand-maintained and
# had NO completeness check, so the gate read "the self-tests are potent" while
# exercising a handful of them. ci.yml describes it as applying the table to
# "those tools" — the guardrails as a class.
#
# This is the shape #381 recorded: `Mutation Testing` read `mutants-out/` while
# cargo-mutants wrote `mutants-out/mutants.out/` — required and vacuous for four
# months, 210 survivors printed as 0. A gate measuring a narrower thing than its
# name claims. check_stale_scope.py came within one commit of landing outside
# this table, and the potency checker itself is still outside it.
#
# EXACT, TWO-SIDED, like DEFAULT_MAX and official_corpus_parsing.txt: below fails as a
# regression, above fails until the constant is raised. Coverage can then only
# rise, and closing the remaining tools can ratchet across releases instead of
# needing one heroic pass.
#
# Re-derived mechanically 2026-10-13: 5 of 18. The requirement says 4 of 17,
# measured 2026-10-09 — #474 then added check_closing_keywords.py together with
# its mutants, moving both figures. That drift is the reason this is a derived
# count with a locked constant rather than a number in a sentence.
MIN_COVERED = 7

# Fixtures for THIS tool's own self-test, written into a temp dir. They are
# named in the table but are not guard tools, and they ship no `--self-test`
# flag, so they fall out of the denominator on their own. Named anyway, because
# a future stub that did ship one would silently inflate the coverage ratio.
STUB_FIXTURES = frozenset({"nope.py", "stub_potent.py", "stub_blind.py", "stub_red.py"})

# How a tool declares it has a self-test. Matching the argparse FLAG, not the
# mere string "--self-test", is deliberate: a tool that only mentions the flag
# in a docstring or passes it to a subprocess does not ship one, and counting it
# would put a tool in the denominator that can never leave it.
_SELF_TEST_FLAG = re.compile(r"""add_argument\(\s*["']--self-test""")


def ships_a_self_test(path: Path) -> bool:
    """Whether this file exposes a `--self-test` argparse flag."""
    try:
        return _SELF_TEST_FLAG.search(path.read_text(encoding="utf-8")) is not None
    except OSError:
        # Unreadable is not "ships none". Same rule the other guards hold to:
        # a measurement that could not be made must not read as a clean one.
        print(f"::error::cannot read {path} to determine self-test coverage",
              file=sys.stderr)
        sys.exit(2)


def coverage(mutants: list[Mutant], tools_dir: Path, min_covered: int,
             out=sys.stdout) -> int:
    """Gate the number of guard tools the mutant table actually exercises."""
    shipping = sorted(
        p.name for p in tools_dir.glob("*.py")
        if p.name not in STUB_FIXTURES and ships_a_self_test(p)
    )
    if not shipping:
        # Zero tools found is the fail-open this guard class keeps producing: a
        # wrong --tools-dir, or a glob that matched nothing, would otherwise
        # satisfy any floor and print a clean result.
        print(f"::error::no tool under {tools_dir} ships a --self-test flag. A "
              f"coverage ratio over an empty denominator is not a measurement.",
              file=out)
        return 2

    registered = {m.tool for m in mutants}
    covered = [t for t in shipping if t in registered]
    uncovered = [t for t in shipping if t not in registered]

    print("", file=out)
    print("== self-test coverage ratchet ==", file=out)
    print(f"tools shipping --self-test: {len(shipping)}", file=out)
    print(f"exercised by a declared mutant: {len(covered)} (declared {min_covered})",
          file=out)
    for t in uncovered:
        print(f"  UNCOVERED  {t}", file=out)

    # A mutant naming a tool that is not in the denominator is a stale table
    # entry pointing at a renamed or deleted tool, and it would quietly shrink
    # coverage while the count still looked right.
    orphans = sorted(registered - set(shipping) - STUB_FIXTURES)
    if orphans:
        print(f"::error::mutant table names tool(s) that ship no --self-test under "
              f"{tools_dir}: {', '.join(orphans)}. A mutant against a tool with no "
              f"self-test cannot be caught by one.", file=out)
        return 2

    n = len(covered)
    if n < min_covered:
        print(f"::error::{n} of {len(shipping)} guard tools are exercised by a declared "
              f"mutant, BELOW the declared {min_covered}. A tool that ships a "
              f"--self-test with no mutant has nothing distinguishing a potent suite "
              f"from a vacuous one. Add a mutant — confirmed SURVIVED before the case "
              f"is written, CAUGHT after — or this is a regression.", file=out)
        return 1
    if n > min_covered:
        print(f"::error::{n} of {len(shipping)} guard tools are exercised, ABOVE the "
              f"declared {min_covered}. That is a win: raise MIN_COVERED to {n} so it "
              f"is locked in rather than left as slack the next pass can re-spend.",
              file=out)
        return 1

    print(f"coverage is exactly {n}; {len(uncovered)} tool(s) remain to ratchet in.",
          file=out)
    return 0


def check(mutants: list[Mutant], tools_dir: Path, out=sys.stdout) -> int:
    ids = [m.id for m in mutants]
    if len(ids) != len(set(ids)):
        dup = sorted({i for i in ids if ids.count(i) > 1})
        print(f"::error::duplicate mutant id(s): {', '.join(dup)}", file=out)
        return 2

    print("== self-test potency guardrail ==", file=out)
    print(f"declared mutants: {len(mutants)}", file=out)

    caught: list[str] = []
    survived: list[tuple[Mutant, str]] = []
    errored: list[tuple[Mutant, str]] = []
    for mut in mutants:
        verdict, detail = evaluate(mut, tools_dir / mut.tool)
        print(f"  {verdict:8}  {mut.id:24} [{mut.tool}]", file=out)
        if verdict == CAUGHT:
            caught.append(mut.id)
        elif verdict == SURVIVED:
            survived.append((mut, detail))
        else:
            errored.append((mut, detail))

    print(f"\ncaught: {len(caught)}   survived: {len(survived)}   "
          f"errored: {len(errored)}", file=out)

    # A declaration/baseline fault means potency could not be judged: cannot-judge
    # must not read as clean, and it takes precedence over a survivor verdict
    # because a stale table can manufacture either.
    if errored:
        print("", file=out)
        for mut, detail in errored:
            print(f"::error::mutant {mut.id!r} ({mut.tool}) could not be judged: {detail}",
                  file=out)
        print("\nRefusing to report success over a mutant that was not actually "
              "exercised.", file=out)
        return 2

    if survived:
        print("", file=out)
        for mut, detail in survived:
            print(f"::error::mutant {mut.id!r} SURVIVED {mut.tool}'s self-test: {detail}",
                  file=out)
            print(f"    {mut.note}", file=out)
        print("\nA surviving mutant is a self-test case that is not load-bearing: "
              "the tool would keep passing after the behaviour it guards is broken. "
              "Add or repair the case until the mutant is caught.", file=out)
        return 1

    print(f"\nAll {len(caught)} declared mutants are caught by the self-test they "
          f"target.", file=out)
    return 0


# ── self-test (the harness proving itself non-vacuous) ──────────────────────
#
# A potency harness whose own verdict does not depend on whether the target
# suite is potent is exactly the failure it exists to catch. So the proof is
# executed, not asserted: the SAME mutation is run against two stub tools that
# differ ONLY in whether the stub's suite carries the boundary case that reads
# the mutated behaviour. Potent stub -> CAUGHT, blind stub -> SURVIVED. Distinct
# inputs, distinct outputs.

_STUB_POTENT = '''#!/usr/bin/env python3
import sys
THRESHOLD = 10
def classify(n):
    return "high" if n > THRESHOLD else "low"
def self_test():
    ok = True
    ok &= classify(11) == "high"
    ok &= classify(5) == "low"
    ok &= classify(10) == "low"  # BOUNDARY: the load-bearing case
    print("stub", "ok" if ok else "FAIL")
    return 0 if ok else 1
if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv else 0)
'''

# Identical, minus the boundary case: the suite no longer reads classify(10).
_STUB_BLIND = _STUB_POTENT.replace(
    '    ok &= classify(10) == "low"  # BOUNDARY: the load-bearing case\n', ""
)

# A stub whose own self-test is already red, so its potency cannot be judged.
_STUB_RED = _STUB_POTENT.replace('classify(11) == "high"', 'classify(11) == "low"')

# The mutation both potent and blind stubs are challenged with: `>` -> `>=`,
# which flips classify(10) from "low" to "high".
_STUB_MUT = ('n > THRESHOLD', 'n >= THRESHOLD')


def self_test() -> int:
    passed = failed = 0

    def report(desc: str, ok: bool, got=None, want=None) -> None:
        nonlocal passed, failed
        if ok:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: got {got!r}, want {want!r}")

    print("check_self_test_potency self-test")

    with tempfile.TemporaryDirectory() as td:
        d = Path(td)
        (d / "stub_potent.py").write_text(_STUB_POTENT, encoding="utf-8")
        (d / "stub_blind.py").write_text(_STUB_BLIND, encoding="utf-8")
        (d / "stub_red.py").write_text(_STUB_RED, encoding="utf-8")

        base = dict(id="m", old=_STUB_MUT[0], new=_STUB_MUT[1], note="stub")

        # THE NON-VACUITY PAIR. Same mutation, two suites; the only difference is
        # the boundary case. A harness that returned a verdict independent of the
        # suite would give these two the same answer and could not tell them apart.
        v_potent, _ = evaluate(Mutant(tool="stub_potent.py", **base), d / "stub_potent.py")
        report("a potent suite CATCHES the mutation", v_potent == CAUGHT, v_potent, CAUGHT)
        v_blind, _ = evaluate(Mutant(tool="stub_blind.py", **base), d / "stub_blind.py")
        report("a blind suite lets the SAME mutation SURVIVE", v_blind == SURVIVED,
               v_blind, SURVIVED)

        # A mutation whose `old` is absent must ERROR (stale declaration), never
        # be smoothed into CAUGHT/SURVIVED — the trap the harness is built around.
        v_absent, det_absent = evaluate(
            Mutant(tool="stub_potent.py", id="m", old="not_in_source_xyz",
                   new="whatever", note="stub"),
            d / "stub_potent.py")
        report("a `old`-absent mutant is ERROR, not a verdict", v_absent == ERROR,
               v_absent, ERROR)
        report("...and it names the drift, not a traceback",
               "drifted" in det_absent or "occur" in det_absent, det_absent, "occur/drifted")

        # A no-op mutation (new == old) proves nothing and must ERROR.
        v_noop, _ = evaluate(
            Mutant(tool="stub_potent.py", id="m", old="THRESHOLD", new="THRESHOLD",
                   note="stub"),
            d / "stub_potent.py")
        report("a no-op mutation (new == old) is ERROR", v_noop == ERROR, v_noop, ERROR)

        # `old` present more times than declared must ERROR (ambiguous landing),
        # not apply to an unexpected site. THRESHOLD appears 3x in the stub.
        v_ambig, _ = evaluate(
            Mutant(tool="stub_potent.py", id="m", old="THRESHOLD", new="LIMIT",
                   note="stub", count=1),
            d / "stub_potent.py")
        report("an ambiguous `old` (wrong count) is ERROR", v_ambig == ERROR,
               v_ambig, ERROR)

        # A tool whose baseline suite is already red cannot be potency-judged.
        v_red, det_red = evaluate(Mutant(tool="stub_red.py", **base), d / "stub_red.py")
        report("a tool with a red baseline suite is ERROR, not CAUGHT",
               v_red == ERROR and "baseline" in det_red, (v_red, det_red), "ERROR/baseline")

        # A missing tool file is ERROR.
        v_missing, _ = evaluate(Mutant(tool="nope.py", **base), d / "nope.py")
        report("a missing tool file is ERROR", v_missing == ERROR, v_missing, ERROR)

        # check() aggregation: a table with one CAUGHT + one SURVIVED exits 1 and
        # a survivor takes a table from green to red.
        buf = io.StringIO()
        rc_mixed = check(
            [Mutant(tool="stub_potent.py", **base),
             Mutant(tool="stub_blind.py", id="m2", old=_STUB_MUT[0], new=_STUB_MUT[1],
                    note="stub")],
            d, out=buf)
        report("check(): a surviving mutant reds the table (exit 1)", rc_mixed == 1,
               rc_mixed, 1)

        # check(): all-caught table exits 0.
        buf = io.StringIO()
        rc_ok = check([Mutant(tool="stub_potent.py", **base)], d, out=buf)
        report("check(): an all-caught table is green (exit 0)", rc_ok == 0, rc_ok, 0)

        # check(): an errored mutant exits 2 and takes precedence over any survivor.
        buf = io.StringIO()
        rc_err = check(
            [Mutant(tool="stub_blind.py", **base),  # would SURVIVE
             Mutant(tool="stub_potent.py", id="m2", old="not_here_zzz", new="x",
                    note="stub")],  # ERROR
            d, out=buf)
        report("check(): an errored mutant exits 2 (cannot-judge over survivor)",
               rc_err == 2, rc_err, 2)

        # check(): duplicate ids are rejected before any mutant runs.
        buf = io.StringIO()
        rc_dup = check(
            [Mutant(tool="stub_potent.py", **base),
             Mutant(tool="stub_potent.py", **base)],
            d, out=buf)
        report("check(): duplicate mutant ids are rejected (exit 2)", rc_dup == 2,
               rc_dup, 2)

    # ── the coverage ratchet, proved the same way: executed, not asserted ───
    #
    # A denominator is built from scratch each time, because the gate's whole
    # claim is that it reads the REAL set of tools shipping a --self-test. Two
    # files ship one, a third does not, so the denominator must come out 2 and
    # not 3 — and the `--self-test`-in-a-docstring file is what separates
    # "declares a self-test" from "mentions the string".
    with tempfile.TemporaryDirectory() as td:
        d = Path(td)
        shipping = ('import argparse\n'
                    'ap = argparse.ArgumentParser()\n'
                    'ap.add_argument("--self-test", action="store_true")\n')
        (d / "a_tool.py").write_text(shipping, encoding="utf-8")
        (d / "b_tool.py").write_text(shipping, encoding="utf-8")
        (d / "c_nohook.py").write_text(
            '"""Mentions --self-test in prose only."""\n'
            'CMD = [sys.executable, "x", "--self-test"]\n', encoding="utf-8")
        mut = dict(id="m", old="x", new="y", note="n")
        one = [Mutant(tool="a_tool.py", **mut)]
        two = [Mutant(tool="a_tool.py", **mut),
               Mutant(tool="b_tool.py", id="m2", old="x", new="y", note="n")]

        buf = io.StringIO()
        rc = coverage(two, d, 2, out=buf)
        report("coverage(): exact coverage passes (exit 0)", rc == 0, rc, 0)
        report("coverage(): the denominator counts FLAGS, not mentions of the string",
               "shipping --self-test: 2" in buf.getvalue(), buf.getvalue()[-90:], 2)

        buf = io.StringIO()
        rc_below = coverage(one, d, 2, out=buf)
        report("coverage(): BELOW the floor fails (exit 1)", rc_below == 1, rc_below, 1)
        report("coverage(): a below-floor run NAMES the uncovered tool",
               "UNCOVERED  b_tool.py" in buf.getvalue(), buf.getvalue()[-90:], True)

        rc_above = coverage(two, d, 1, out=io.StringIO())
        report("coverage(): ABOVE the floor also fails — the bound is EXACT, so a "
               "win is locked in and not pocketed (exit 1)", rc_above == 1, rc_above, 1)

        # The fail-open this guard class keeps producing. An empty denominator
        # satisfies ANY floor by arithmetic, so it must be refused outright
        # rather than divided by.
        #
        # THE MUTANT TABLE CAUGHT THIS CASE BEING VACUOUS, which is worth
        # recording where the case lives. It first passed `two` here and
        # asserted exit 2 — and got it from the ORPHAN branch, because against
        # an empty directory every declared mutant names a tool that is not
        # there. Deleting the empty-denominator branch outright left the case
        # green. An EMPTY mutant list is the input only this branch can answer:
        # no mutants means no orphans, so 2 can come from nowhere else.
        with tempfile.TemporaryDirectory() as empty:
            rc_empty = coverage([], Path(empty), 0, out=io.StringIO())
        report("coverage(): an EMPTY denominator is exit 2, not a satisfied floor",
               rc_empty == 2, rc_empty, 2)

        # A table entry pointing at a renamed or deleted tool would quietly
        # shrink real coverage while the count still read correct.
        rc_orphan = coverage(
            two + [Mutant(tool="c_nohook.py", id="m3", old="x", new="y", note="n")],
            d, 2, out=io.StringIO())
        report("coverage(): a mutant naming a tool with no --self-test is exit 2",
               rc_orphan == 2, rc_orphan, 2)

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--tools-dir", default="tools",
                    help="directory holding the guard tools named in the mutant table")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    # BOTH gates run, and the worse verdict wins — 2 (could not judge) over
    # 1 (ratchet) over 0. Returning early on the first non-zero would hide a
    # coverage regression behind a surviving mutant, and the two are independent
    # findings an operator needs to see together.
    rc_potency = check(MUTANTS, Path(a.tools_dir))
    rc_coverage = coverage(MUTANTS, Path(a.tools_dir), MIN_COVERED)
    return max(rc_potency, rc_coverage)


if __name__ == "__main__":
    sys.exit(main())
