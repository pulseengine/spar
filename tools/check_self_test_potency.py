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
    return check(MUTANTS, Path(a.tools_dir))


if __name__ == "__main__":
    sys.exit(main())
