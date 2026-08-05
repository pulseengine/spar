#!/usr/bin/env python3
"""Gate a cargo-mutants run on its surviving-mutant count — and, first, on the
run having actually happened.

WHY THIS IS A SCRIPT AND NOT FOUR LINES OF SHELL
================================================

It was four lines of shell, and it was vacuous for four months (#381).

`.github/workflows/ci.yml` ran `cargo mutants --output mutants-out` and then
read `mutants-out/missed.txt`. But `--output DIR` names the directory in which
to CREATE `mutants.out` — the report is at `mutants-out/mutants.out/`. The path
the gate read has never existed. `grep` failed into `|| echo 0`, an `[ -f ]`
test was false, `0 -gt 142` was false, exit 0. `Mutation Testing` is a required
status check that runs for roughly three hours; run 30563879100 uploaded 210
surviving mutants against a threshold of 142 and printed
`Surviving mutants: 0`.

What made it invisible is worth stating, because it is the property to design
against: **the error path produced the ideal reading.** `|| true`, `2>/dev/null`
and `|| echo 0` are each defensible in isolation; together they turn "the file
is not there" into `0`, and `0` is the best possible score for this metric. No
one reading the log approvingly could have noticed.

Three consequences shape this file:

1. **Absence fails.** Every path this script reads is required to exist, and a
   missing one exits non-zero with a message naming the path. The cargo-mutants
   invocation ends in `|| true` (it exits non-zero whenever ANY mutant survives,
   and the ratchet tolerates a bounded number), so the run step cannot report
   failure — which means this step must prove the run happened rather than infer
   it from a quiet directory.

2. **The nesting lives in exactly one place.** Callers pass `--output-dir`, the
   same value handed to `cargo mutants --output`, and `_REPORT_SUBDIR` below
   applies the nesting. Nobody has to remember it twice, which is how it came to
   be remembered wrongly once.

3. **The counts are cross-checked against cargo-mutants' own JSON.** This is the
   detector that would have caught the original bug on day one and did not
   exist. `outcomes.json` records `missed`/`caught` independently of the `.txt`
   files; if the two disagree, our reading of the report is wrong and the run is
   rejected rather than scored. A gate that prints a number should compare that
   number against an independent measurement of the same number — the tell in
   #381 sat in the repo for four months as exactly such a pair (a commit message
   saying "Current survivors: 142" next to CI logs saying 0) and was never
   diffed.

The report layout is not guessed. It is transcribed from the `mutants-report`
artifact of run 30563879100, uploaded with `path: mutants-out/` — so the
artifact root is a listing of that directory, and its root contains exactly one
entry:

    mutants-out/
    └── mutants.out/
        ├── caught.txt      603 lines
        ├── missed.txt      210 lines
        ├── unviable.txt    793 lines
        ├── timeout.txt       2 lines
        ├── outcomes.json   total_mutants 1608, missed 210, caught 603,
        │                   end_time null          <-- SEE BELOW
        ├── mutants.json
        ├── debug.log
        ├── lock.json
        ├── diff/
        └── log/

The *shape* above is authoritative; its *numbers* are not, and the reason is
the next defect in this same gate. That run did not finish. cargo-mutants had
found 1737 mutants; 3h02m in, the per-worker temp trees it copies the source
into were deleted underneath it —

    ERROR Worker thread failed: ".../cargo-mutants-spar-2xA4rk.tmp/crates/
    spar-analysis/src/weight_power.rs" does not exist, refusing to create it

— four workers at once, on runner1, consistent with the runner's disk-pressure
cleanup hook reaping `_tmp` during a live job. `|| true` swallowed the exit.
So 1608 is how many mutants got tested, not how many exist, and 210 is the
survivor count of the 92% that ran.

That matters beyond bookkeeping: **a truncated run under-reports survivors**,
so it fails safe in appearance and unsafe in fact — fewer survivors is a
better score. The cross-check in `_cross_check` cannot see it either, because
cargo-mutants writes `total_mutants` incrementally: 210+603+793+2 == 1608
agrees with itself perfectly. The discriminator is `end_time`, which is null
on the crashed run and populated on the completed one (30586785648: 1737
tested, 292 missed).

This script does NOT yet reject a truncated run — arming that against a job
that demonstrably crashes on one runner would make a required context flap on
infrastructure. The detector plus the runner fix are #389. Until then, treat
any survivor count from a run without an `end_time` as a lower bound.

Run `--self-test` to exercise the whole decision table against constructed
fixtures, including a regression case that reproduces the original bug: a tree
where the `.txt` files exist ONLY at the shallow path must be rejected, not
scored as zero survivors.

stdlib-only, deliberately: no workflow in this repo installs a Python package,
and a required check that depends on an unverified runner package is a check
that can fail to run — which reads as approval. See REQ-GUARD-HUMAN-SCOPED-001
for the same constraint stated as a requirement.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile

# cargo-mutants creates `mutants.out` INSIDE the directory given to `--output`.
# This is the single fact whose duplication caused #381; it is stated here once.
_REPORT_SUBDIR = "mutants.out"

# The four per-outcome files cargo-mutants writes. `missed` and `caught` are
# load-bearing for the verdict; the other two are reported for context only.
_REQUIRED_FILES = ("missed.txt", "caught.txt")
_CONTEXT_FILES = ("unviable.txt", "timeout.txt")


def report_dir(output_dir: str) -> str:
    """The directory cargo-mutants actually writes, given a `--output` value."""
    return os.path.join(output_dir, _REPORT_SUBDIR)


def _count_lines(path: str) -> int:
    """Number of mutants listed in a cargo-mutants outcome file.

    Counts non-blank lines rather than newlines, so a report whose final line
    lacks a trailing newline is not silently undercounted by one. For the real
    artifact both agree exactly (210/603/793/2), and any disagreement with
    `outcomes.json` is caught by the cross-check below.
    """
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return sum(1 for line in fh if line.strip())


class GateFailure(Exception):
    """A condition under which the run must not be scored."""


def _load_counts(rdir: str) -> dict[str, int]:
    """Read the outcome counts, refusing anything that is not a complete run."""
    if not os.path.isdir(rdir):
        raise GateFailure(
            f"no cargo-mutants report directory at {rdir} — the run did not "
            f"complete. This is not zero survivors; it is no measurement. "
            f"(cargo-mutants writes {_REPORT_SUBDIR!r} inside the --output "
            f"directory; check that --output-dir here matches --output there.)"
        )

    counts = {}
    for name in _REQUIRED_FILES:
        path = os.path.join(rdir, name)
        if not os.path.isfile(path):
            raise GateFailure(
                f"{path} is missing — the report is incomplete, so the run "
                f"cannot be scored."
            )
        counts[name[: -len(".txt")]] = _count_lines(path)

    for name in _CONTEXT_FILES:
        path = os.path.join(rdir, name)
        counts[name[: -len(".txt")]] = _count_lines(path) if os.path.isfile(path) else 0

    return counts


def _cross_check(rdir: str, counts: dict[str, int]) -> str:
    """Diff our line counts against cargo-mutants' own JSON tally.

    Returns a human-readable note. Raises GateFailure on disagreement: if the
    two independent readings of the same quantity differ, we do not know which
    is right, and scoring on the wrong one is precisely #381.
    """
    path = os.path.join(rdir, "outcomes.json")
    if not os.path.isfile(path):
        # Not fatal: older cargo-mutants releases may not write it. Say so out
        # loud rather than passing silently, because losing this cross-check is
        # losing the detector that catches a misread report.
        return f"outcomes.json absent at {path} — counts NOT cross-checked"

    try:
        with open(path, "r", encoding="utf-8") as fh:
            doc = json.load(fh)
    except (OSError, ValueError) as exc:
        raise GateFailure(f"{path} is present but unreadable ({exc}) — refusing to score a report we cannot verify")

    mismatches = []
    for key in ("missed", "caught"):
        recorded = doc.get(key)
        if recorded is None:
            continue
        if recorded != counts[key]:
            mismatches.append(f"{key}: {counts[key]} line(s) in {key}.txt vs {recorded} in outcomes.json")

    if mismatches:
        raise GateFailure(
            "the report disagrees with itself, so it is being misread: "
            + "; ".join(mismatches)
            + ". Do not adjust the threshold to match — find out which reading is wrong."
        )

    return "counts agree with outcomes.json"


def check(output_dir: str, max_missed: int, out=None) -> int:
    """Return 0 if the run happened and stayed within the ratchet, else 1."""
    out = out or sys.stdout
    rdir = report_dir(output_dir)

    try:
        counts = _load_counts(rdir)
        note = _cross_check(rdir, counts)
    except GateFailure as exc:
        print(f"::error::{exc}", file=out)
        if os.path.isdir(output_dir):
            print(f"Tree under {output_dir}/:", file=out)
            for root, dirs, files in os.walk(output_dir):
                depth = root[len(output_dir) :].count(os.sep)
                if depth > 1:
                    dirs[:] = []
                    continue
                for name in sorted(files)[:12]:
                    print(f"  {os.path.join(root, name)}", file=out)
        else:
            print(f"{output_dir}/ does not exist at all.", file=out)
        return 1

    missed, caught = counts["missed"], counts["caught"]
    print(
        f"Surviving mutants: {missed}  "
        f"(caught {caught}, unviable {counts['unviable']}, timeout {counts['timeout']}; {note})",
        file=out,
    )

    # A report that exists but caught nothing means the test suite never ran —
    # every mutant unviable, or the package/target filter matched nothing. That
    # also scores zero survivors, the best possible value, so it has to be
    # rejected explicitly or it sails straight through the ratchet below.
    if caught == 0:
        print(
            "::error::0 mutants caught — the test suite did not run. A 0 here is "
            "absence, not success.",
            file=out,
        )
        return 1

    if missed > max_missed:
        print(
            f"::error::{missed} mutant(s) survived (threshold: {max_missed}) — add tests to kill them",
            file=out,
        )
        missed_path = os.path.join(rdir, "missed.txt")
        with open(missed_path, "r", encoding="utf-8", errors="replace") as fh:
            for i, line in enumerate(fh):
                if i >= 30:
                    print("  … (see the mutants-report artifact for the rest)", file=out)
                    break
                print(f"  {line.rstrip()}", file=out)
        return 1

    print(f"Mutant survivors ({missed}) within threshold ({max_missed}). Target: 0 (#382).", file=out)
    return 0


def summarize(output_dir: str, shard: str, summary, out=None) -> int:
    """Render the weekly quality report as markdown into `summary`.

    Advisory — the weekly does not gate, so this always returns 0. The one
    thing it must never do is render absence as a table of zeros. Reading the
    shallow path made every weekly summary show `0 missed / 0 caught /
    0 unviable / 0 timeout` (#381), which is not merely wrong but *impossible*:
    zero caught AND zero unviable would mean cargo-mutants found nothing to
    test at all. It still rendered as a plausible table, which is why it was
    read for four months without anyone registering it as a fault. Absence now
    prints as absence.

    Shares `report_dir()` and `_load_counts()` with the gating path on purpose:
    the fact that cargo-mutants nests its report under `mutants.out/` is the
    fact that cost four months, and it lives in exactly one place.
    """
    out = out or sys.stdout
    rdir = report_dir(output_dir)

    try:
        counts = _load_counts(rdir)
        note = _cross_check(rdir, counts)
    except GateFailure as exc:
        print(f"> ⚠️ **No usable cargo-mutants report** — {exc}", file=summary)
        print(">", file=summary)
        print("> Counts are omitted rather than rendered as 0. A zero table here "
              "would be absence wearing the costume of a clean result.", file=summary)
        print(f"::warning::no usable cargo-mutants report under {rdir}", file=out)
        return 0

    print(f"## cargo-mutants weekly — {shard}", file=summary)
    print("", file=summary)
    print(f"Report: `{rdir}` ({note})", file=summary)
    print("", file=summary)
    print("| Outcome | Count |", file=summary)
    print("|---------|------:|", file=summary)
    print(f"| 🟥 Missed  (test suite did not catch) | {counts['missed']} |", file=summary)
    print(f"| 🟩 Caught  (test suite caught)        | {counts['caught']} |", file=summary)
    print(f"| ⏱  Timeout                           | {counts['timeout']} |", file=summary)
    print(f"| ⚪ Unviable (build failed)           | {counts['unviable']} |", file=summary)
    print("", file=summary)

    if counts["caught"] == 0:
        print("> ⚠️ **0 caught** — the test suite did not run against these mutants; "
              "the missed count below is not a quality signal.", file=summary)
        print("", file=summary)
        print("::warning::0 mutants caught — the test suite did not run", file=out)

    if counts["missed"] > 0:
        print("<details><summary>First 50 missed mutants</summary>", file=summary)
        print("", file=summary)
        print("```", file=summary)
        with open(os.path.join(rdir, "missed.txt"), "r", encoding="utf-8", errors="replace") as fh:
            for i, line in enumerate(fh):
                if i >= 50:
                    break
                if line.strip():
                    print(line.rstrip(), file=summary)
        print("```", file=summary)
        print("</details>", file=summary)

    print(f"Summarised {counts['missed']} missed / {counts['caught']} caught "
          f"/ {counts['unviable']} unviable / {counts['timeout']} timeout.", file=out)
    return 0


# ── self-test ───────────────────────────────────────────────────────────────


def _fixture(root: str, *, nested: bool = True, missed: int = 10, caught: int = 100,
             unviable: int = 5, timeout: int = 0, outcomes: dict | None = None) -> str:
    """Build a mutants-out tree. `nested=False` reproduces the #381 misreading:
    the outcome files placed where the old gate LOOKED rather than where
    cargo-mutants writes them."""
    out = os.path.join(root, "mutants-out")
    target = os.path.join(out, _REPORT_SUBDIR) if nested else out
    os.makedirs(target, exist_ok=True)
    for name, n in (("missed.txt", missed), ("caught.txt", caught),
                    ("unviable.txt", unviable), ("timeout.txt", timeout)):
        with open(os.path.join(target, name), "w", encoding="utf-8") as fh:
            for i in range(n):
                fh.write(f"crates/spar-analysis/src/x.rs:{i}:1: replace f -> usize with 0\n")
    if outcomes is not None:
        with open(os.path.join(target, "outcomes.json"), "w", encoding="utf-8") as fh:
            json.dump(outcomes, fh)
    return out


def self_test() -> int:
    import io

    passed = failed = 0

    def case(desc: str, want: int, build, max_missed: int = 292):
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = build(tmp)
            buf = io.StringIO()
            got = check(out_dir, max_missed, out=buf)
            if got == want:
                passed += 1
                print(f"  ok   {desc}")
            else:
                failed += 1
                print(f"  FAIL {desc}: got exit {got}, want {want}")
                for line in buf.getvalue().splitlines()[:4]:
                    print(f"         {line}")

    print("check_mutants_report self-test")

    # The bug itself. Both of these greened for four months.
    case("no mutants-out at all (crashed or OOM'd run)", 1,
         lambda t: os.path.join(t, "mutants-out"))
    case("mutants-out exists but is empty", 1,
         lambda t: (os.makedirs(os.path.join(t, "mutants-out")), os.path.join(t, "mutants-out"))[1])
    case("REGRESSION #381: outcome files ONLY at the shallow path", 1,
         lambda t: _fixture(t, nested=False, missed=500, caught=100))

    # Normal operation.
    case("valid report, 10 survivors, under threshold", 0,
         lambda t: _fixture(t, missed=10, caught=100))
    case("valid report, 0 survivors", 0,
         lambda t: _fixture(t, missed=0, caught=100))
    case("survivors exactly at the threshold", 0,
         lambda t: _fixture(t, missed=292, caught=708))
    case("survivors one over the threshold", 1,
         lambda t: _fixture(t, missed=293, caught=708))

    # Absence that scores as perfection.
    case("report present but 0 caught (suite never ran)", 1,
         lambda t: _fixture(t, missed=0, caught=0))
    case("0 caught AND 0 missed (the impossible all-zero report)", 1,
         lambda t: _fixture(t, missed=0, caught=0, unviable=0))

    # Incomplete reports.
    case("missed.txt removed", 1,
         lambda t: (_fixture(t), os.remove(os.path.join(t, "mutants-out", _REPORT_SUBDIR, "missed.txt")),
                    os.path.join(t, "mutants-out"))[2])
    case("caught.txt removed", 1,
         lambda t: (_fixture(t), os.remove(os.path.join(t, "mutants-out", _REPORT_SUBDIR, "caught.txt")),
                    os.path.join(t, "mutants-out"))[2])

    # The cross-check — the detector that did not exist.
    case("outcomes.json agrees with the .txt counts", 0,
         lambda t: _fixture(t, missed=10, caught=100, outcomes={"missed": 10, "caught": 100}))
    case("outcomes.json disagrees (report misread)", 1,
         lambda t: _fixture(t, missed=10, caught=100, outcomes={"missed": 210, "caught": 100}))
    case("outcomes.json unparseable", 1,
         lambda t: (_fixture(t), open(os.path.join(t, "mutants-out", _REPORT_SUBDIR, "outcomes.json"),
                                      "w").write("{not json"), os.path.join(t, "mutants-out"))[2])
    case("outcomes.json absent — passes, but says so", 0,
         lambda t: _fixture(t, missed=10, caught=100))

    # ── the weekly summary path ──
    # It never gates, so its only failure mode is rendering a believable table
    # that says nothing happened. These assert on the RENDERED MARKDOWN, not on
    # an exit code — for an advisory report the text is the entire product.
    def scase(desc: str, build, want_in=(), want_not_in=()):
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = build(tmp)
            summary, log = io.StringIO(), io.StringIO()
            summarize(out_dir, "shard 0/8", summary, out=log)
            rendered = summary.getvalue()
            bad = [s for s in want_in if s not in rendered]
            bad += [f"UNWANTED {s!r}" for s in want_not_in if s in rendered]
            if not bad:
                passed += 1
                print(f"  ok   {desc}")
            else:
                failed += 1
                print(f"  FAIL {desc}: {bad}")

    scase("weekly: no report — says absent, renders NO table",
          lambda t: os.path.join(t, "mutants-out"),
          want_in=("No usable cargo-mutants report",),
          want_not_in=("| Outcome | Count |", "| 0 |"))
    scase("weekly REGRESSION #381: shallow path must not render 0/0/0/0",
          lambda t: _fixture(t, nested=False, missed=210, caught=603),
          want_in=("No usable cargo-mutants report",),
          want_not_in=("| Outcome | Count |",))
    scase("weekly: real counts reach the table",
          lambda t: _fixture(t, missed=210, caught=603, unviable=793, timeout=2),
          want_in=("| 210 |", "| 603 |", "| 793 |", "| 2 |", "First 50 missed mutants"))
    scase("weekly: 0 caught is flagged, not tabulated silently",
          lambda t: _fixture(t, missed=0, caught=0, unviable=0),
          want_in=("**0 caught**",))

    print(f"\n{passed} passed, {failed} failed")
    return 0 if failed == 0 else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument(
        "--output-dir",
        default="mutants-out",
        help="the value passed to `cargo mutants --output`; the report is read "
             f"from <output-dir>/{_REPORT_SUBDIR}/",
    )
    ap.add_argument(
        "--max-missed",
        type=int,
        default=None,
        help="ratchet threshold — fail if more mutants survive than this. Lower it as tests improve; see #382.",
    )
    ap.add_argument("--self-test", action="store_true", help="run the built-in decision table")
    ap.add_argument(
        "--summary-file",
        default=None,
        help="render the advisory weekly markdown report to this file (append) "
             "instead of gating — e.g. $GITHUB_STEP_SUMMARY. Never fails the job.",
    )
    ap.add_argument("--shard", default="", help="shard label for the summary heading")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    # Summary mode is advisory and deliberately separate from the gate: the
    # weekly runs the whole workspace with no ratchet, so it has no threshold
    # to check against. Annotations go to stdout, markdown to the file, so the
    # ::warning:: still reaches the job log when stdout is not the summary.
    if args.summary_file:
        with open(args.summary_file, "a", encoding="utf-8") as fh:
            return summarize(args.output_dir, args.shard, fh)

    if args.max_missed is None:
        ap.error("--max-missed is required (or use --self-test / --summary-file)")
    return check(args.output_dir, args.max_missed)


if __name__ == "__main__":
    sys.exit(main())
