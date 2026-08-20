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

This script now REJECTS a truncated run (REQ-GUARD-GATE-EVIDENCE-002 (d)).

Arming it was deliberately blocked until the cause was fixed, because a
detector for an infrastructure fault that still happens turns a required
context into a coin-flip. The cause was found and fixed on 2026-08-07
(smithy PR #7), and it is worth recording because the mechanism is not
obvious: `/etc/tmpfiles.d/smithy-runner-tmp.conf` swept
`cargo-mutants-*.tmp` files older than **2h**, hourly, resting on the stated
assumption that "a job that's still running keeps mtime fresh". That is false
for cargo-mutants specifically — it copies the source tree into each worker's
temp dir **once**, at start, and never touches those files again, so their
mtime is frozen while the job is very much alive. A 3h02m run aged past the
threshold and the next sweep deleted its sources; the four workers dying
"within one second of each other" was one sweep, not four faults. The age is
now 8h, above GitHub's 6h job ceiling, so a sweep cannot reach a live job.

Note the coupling this leaves behind: 8h protects any job under the 6h
ceiling, and this repo's `mutants` job sets `timeout-minutes: 240`. Raising
that past ~7h would re-open the hazard.

So an `end_time: null` now means a genuine truncation, and is a hard failure.

Run `--self-test` to exercise the whole decision table against constructed
fixtures, including a regression case that reproduces the original bug: a tree
where the `.txt` files exist ONLY at the shallow path must be rejected, not
scored as zero survivors.

`--assert-workflow-threshold WORKFLOW` is a separate, checkout-runnable mode: it
reads the workflow file and asserts it declares exactly one bounded, un-neutered
mutation gate, printing the threshold it finds. It exists so the artifact that
VERIFIES this gate can assert the gate's *declaration* without re-running the
~3h `mutants` job, and can read the threshold from the workflow rather than
carrying its own copy — `--max-missed 210` in an artifact and `--max-missed 292`
in the workflow was a number stated twice that drifted for four months (#414).

stdlib-only, deliberately: no workflow in this repo installs a Python package,
and a required check that depends on an unverified runner package is a check
that can fail to run — which reads as approval. See REQ-GUARD-HUMAN-SCOPED-001
for the same constraint stated as a requirement.
"""

from __future__ import annotations

import argparse
import json
import os
import re
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


def _assert_completed(rdir: str) -> str:
    """Refuse to score a run that did not finish. REQ-GUARD-GATE-EVIDENCE-002 (d).

    A truncated cargo-mutants run is INTERNALLY CONSISTENT. Run 30563879100
    records missed 210 + caught 603 + unviable 793 + timeout 2 == 1608 ==
    `total_mutants`, and every one of those agrees with `outcomes.json` — while
    the log shows cargo-mutants had found 1737 and lost its worker trees 3h02m
    in. `_cross_check` cannot see this, because `total_mutants` is written
    incrementally: the report is a truthful account of the 92% that ran.

    The failure is flattering, which is what makes it dangerous: a truncated run
    UNDER-reports survivors, so it scores BETTER than the truth. 210 survivors
    sits comfortably under a 292 threshold and the gate goes green. Any
    threshold ratcheted down using such a run is a ceiling set below the floor.

    `end_time` is the discriminator — null while running, populated on a clean
    finish. It is the one field that does not degrade gracefully under
    truncation.

    Absence of `outcomes.json` is fatal here, which is stricter than
    `_cross_check` (it tolerates absence and says so). The distinction is
    deliberate: losing the cross-check costs a detector, but losing `end_time`
    means completion cannot be established at all — and "cannot prove it
    finished" must not read as "finished". That is the exact shape this whole
    requirement exists to close.
    """
    path = os.path.join(rdir, "outcomes.json")
    if not os.path.isfile(path):
        raise GateFailure(
            f"{path} is absent, so there is no way to tell a completed run from "
            f"one killed part-way. A truncated run under-reports survivors and "
            f"scores better than the truth, so this cannot be waved through (#389)."
        )
    try:
        with open(path, "r", encoding="utf-8") as fh:
            doc = json.load(fh)
    except (OSError, ValueError) as exc:
        raise GateFailure(f"{path} is present but unreadable ({exc}) — completion cannot be established")

    if "end_time" not in doc:
        raise GateFailure(
            f"{path} has no `end_time` field at all — this is not the schema "
            f"this gate was written against, and completion cannot be "
            f"established. Do not assume it finished."
        )

    end_time = doc.get("end_time")
    if end_time is None:
        total = doc.get("total_mutants")
        raise GateFailure(
            "`end_time` is null — cargo-mutants did not finish, so this report "
            "covers only the mutants that ran"
            + (f" ({total} of an unknown total)" if total is not None else "")
            + ". A truncated run UNDER-reports survivors, so it fails "
            "flatteringly: the score is better than the truth and the ratchet "
            "would be tightened onto a number that is not the floor (#389). "
            "Re-run it; do not lower the threshold to match."
        )

    return f"run completed ({end_time})"


def check(output_dir: str, max_missed: int, out=None) -> int:
    """Return 0 if the run happened and stayed within the ratchet, else 1."""
    out = out or sys.stdout
    rdir = report_dir(output_dir)

    try:
        counts = _load_counts(rdir)
        note = _cross_check(rdir, counts)
        # AFTER the cross-check on purpose: a report that disagrees with itself
        # should be reported in those terms rather than as a truncation.
        note = f"{note}; {_assert_completed(rdir)}"
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


# The line the mutation gate is enforced by: `check_mutants_report.py` invoked
# with --max-missed, which is the ratcheting flag. --self-test and --summary-file
# carry no threshold, so they cannot be mistaken for the gate; --max-missed exists
# only for the gate, which is why it — and not the defaultable --output-dir — is
# the discriminator. The threshold is captured so that a caller can single-source
# it: the artifact that VERIFIES this gate reads the number from here instead of
# restating it, which is the whole point of #414 — `--max-missed 210` in the
# artifact and `--max-missed 292` in the workflow was a number stated twice, and
# the two drifted apart for four months.
_GATE_INVOCATION = re.compile(
    r"check_mutants_report\.py\b.*--max-missed(?:=|\s+)(?P<threshold>\S+)"
)
# A gate whose exit code is swallowed cannot fail, so it is not a gate — the same
# `|| true` that let the crashed run of #389 score green. `|| :` is the same
# no-op with the shell builtin `:`. `\b` cannot follow the `:` alternative — `:`
# is not a word char, so there is no boundary there — hence the word boundary is
# tied to `true` alone. `-` on a `run:` line is YAML, not shell, and not neutering.
_NEUTERED = re.compile(r"\|\|\s*(true\b|:)")


def assert_workflow_threshold(workflow_path: str, out=None) -> int:
    """Assert the workflow declares exactly one bounded, un-neutered mutation gate.

    Runnable from a clean checkout — it reads only the committed workflow file,
    never the ~3h `mutants` job's output. This is the second step of
    TEST-GUARD-GATE-EVIDENCE: the artifact used to re-run the gate itself
    (`--output-dir mutants-out --max-missed 210`), which cannot succeed outside
    the CI job and which restated a threshold that had gone stale (#414). Here the
    artifact asserts the gate's DECLARATION instead — that the workflow still
    invokes it with a bounded integer ratchet that can actually fail — and reads
    the number from the workflow rather than carrying its own copy.

    NON-VACUITY: the discovered threshold is printed, so distinct workflows
    (--max-missed 292 vs 142) produce distinct output. A gate that is absent,
    neutered with `|| true`, given a non-integer bound, or declared twice with
    disagreeing bounds fails — the error path does not decay into the success
    reading.
    """
    out = out or sys.stdout
    try:
        with open(workflow_path, "r", encoding="utf-8") as fh:
            lines = fh.readlines()
    except OSError as exc:
        print(f"::error::cannot read workflow {workflow_path}: {exc}", file=out)
        return 1

    found: list[tuple[int, int, str]] = []  # (lineno, threshold, raw)
    for i, raw in enumerate(lines, start=1):
        # A commented-out invocation is not an enforced gate. Strip a leading
        # comment marker (workflow YAML indents its `#` comments) before matching.
        if raw.lstrip().startswith("#"):
            continue
        m = _GATE_INVOCATION.search(raw)
        if not m:
            continue
        token = m.group("threshold")
        if _NEUTERED.search(raw):
            print(
                f"::error::{workflow_path}:{i}: the mutation gate is neutered — its "
                f"exit code is swallowed by `|| true`/`|| :`, so it can never fail. "
                f"A gate that cannot fail is not a gate (#389).",
                file=out,
            )
            print(f"  {raw.strip()}", file=out)
            return 1
        if not re.fullmatch(r"\d+", token):
            print(
                f"::error::{workflow_path}:{i}: --max-missed threshold is "
                f"{token!r}, not a non-negative integer. The ratchet bound must be "
                f"a number this gate can compare survivors against.",
                file=out,
            )
            print(f"  {raw.strip()}", file=out)
            return 1
        found.append((i, int(token), raw.strip()))

    if not found:
        print(
            f"::error::{workflow_path} declares no mutation gate — no `run:` invokes "
            f"check_mutants_report.py with --max-missed. The required `Mutation "
            f"Testing` ratchet has been removed or renamed; this is the gate this "
            f"artifact exists to keep enforced (#414).",
            file=out,
        )
        return 1

    thresholds = {t for _, t, _ in found}
    if len(thresholds) > 1:
        print(
            f"::error::{workflow_path} declares the mutation threshold "
            f"{len(found)} times with disagreeing values {sorted(thresholds)} — a "
            f"number stated twice always drifts (#381, #414). Single-source it.",
            file=out,
        )
        for i, t, raw in found:
            print(f"  line {i}: --max-missed {t}   {raw}", file=out)
        return 1

    lineno, threshold, _raw = found[0]
    print(
        f"Mutation gate declared at {workflow_path}:{lineno} with a bounded "
        f"ratchet: --max-missed {threshold}. Threshold single-sourced from the "
        f"workflow (#414). Target: 0 (#382).",
        file=out,
    )
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


# Sentinel for the test fixture: remove a key rather than set it to null.
_DROP = object()


def _fixture(root: str, *, nested: bool = True, missed: int = 10, caught: int = 100,
             unviable: int = 5, timeout: int = 0, outcomes: dict | None = None,
             write_outcomes: bool = True) -> str:
    """Build a mutants-out tree. `nested=False` reproduces the #381 misreading:
    the outcome files placed where the old gate LOOKED rather than where
    cargo-mutants writes them.

    By default this writes a COMPLETE `outcomes.json` — counts consistent with
    the .txt files and a populated `end_time` — because that is what a real
    finished run looks like, and a fixture that omitted it would quietly make
    every case a truncation case. `outcomes=` overrides individual fields (pass
    `{"end_time": None}` for a truncated run); `write_outcomes=False` omits the
    file entirely.
    """
    out = os.path.join(root, "mutants-out")
    target = os.path.join(out, _REPORT_SUBDIR) if nested else out
    os.makedirs(target, exist_ok=True)
    for name, n in (("missed.txt", missed), ("caught.txt", caught),
                    ("unviable.txt", unviable), ("timeout.txt", timeout)):
        with open(os.path.join(target, name), "w", encoding="utf-8") as fh:
            for i in range(n):
                fh.write(f"crates/spar-analysis/src/x.rs:{i}:1: replace f -> usize with 0\n")
    if write_outcomes:
        doc = {
            "total_mutants": missed + caught + unviable + timeout,
            "missed": missed,
            "caught": caught,
            "start_time": "2026-08-07T00:00:00.000Z",
            "end_time": "2026-08-07T01:00:00.000Z",
        }
        if outcomes is not None:
            doc.update(outcomes)
            # `_DROP` removes a key outright, which is distinct from setting it
            # to null — "the field is missing" and "the field says not-finished"
            # are different reports and the gate must reject both.
            doc = {k: v for k, v in doc.items() if v is not _DROP}
        with open(os.path.join(target, "outcomes.json"), "w", encoding="utf-8") as fh:
            json.dump(doc, fh)
    return out


def self_test() -> int:
    import io

    passed = failed = 0

    def case(desc: str, want: int, build, max_missed: int = 292,
             want_msg: str | None = None):
        """`want_msg` pins the DIAGNOSIS, not just the verdict.

        Three different broken reports — outcomes.json absent, `end_time` key
        missing, `end_time` null — all exit 1, and each has a fallback path that
        would also exit 1 if its own check were removed. Asserting only the exit
        code therefore cannot tell whether the right check fired; mutating either
        guard away leaves every exit code unchanged. Pinning the message is what
        makes each branch load-bearing, and it is the same principle the rest of
        this gate rests on: distinct inputs must produce distinct outputs.
        """
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = build(tmp)
            buf = io.StringIO()
            got = check(out_dir, max_missed, out=buf)
            text = buf.getvalue()
            if got != want:
                failed += 1
                print(f"  FAIL {desc}: got exit {got}, want {want}")
                for line in text.splitlines()[:4]:
                    print(f"         {line}")
            elif want_msg is not None and want_msg not in text:
                failed += 1
                print(f"  FAIL {desc}: exit {got} correct, but the diagnosis is "
                      f"wrong — expected {want_msg!r}")
                for line in text.splitlines()[:4]:
                    print(f"         {line}")
            else:
                passed += 1
                print(f"  ok   {desc}")

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
    # ── truncation: REQ-GUARD-GATE-EVIDENCE-002 (d), #389 ──
    # The discriminating PAIR. Identical counts, identical cross-check, and the
    # survivor count is comfortably UNDER the threshold in both — only end_time
    # differs. A gate that ignored end_time passes both and cannot fail the
    # first, which is precisely the four months this job spent scoring crashed
    # runs as good ones.
    case("REGRESSION #389: end_time null (truncated) FAILS despite consistent counts", 1,
         lambda t: _fixture(t, missed=10, caught=100, outcomes={"end_time": None}),
         want_msg="`end_time` is null")
    case("...and the SAME report PASSES with end_time populated", 0,
         lambda t: _fixture(t, missed=10, caught=100))

    # Run 30563879100 exactly: 210+603+793+2 == 1608 == total_mutants, every
    # count agreeing with outcomes.json, 210 well under the 292 threshold — and
    # cargo-mutants had actually found 1737. Internally perfect, and wrong.
    case("run 30563879100 reproduced: self-consistent, under threshold, truncated -> FAIL", 1,
         lambda t: _fixture(t, missed=210, caught=603, unviable=793, timeout=2,
                            outcomes={"total_mutants": 1608, "end_time": None}),
         want_msg="1608 of an unknown total")
    case("...the same run WITH end_time is scored normally", 0,
         lambda t: _fixture(t, missed=210, caught=603, unviable=793, timeout=2,
                            outcomes={"total_mutants": 1608}))

    case("end_time key absent entirely (unexpected schema) FAILS", 1,
         lambda t: _fixture(t, missed=10, caught=100,
                            outcomes={"missed": 10, "caught": 100,
                                      "total_mutants": 115, "end_time": _DROP}),
         want_msg="no `end_time` field at all")

    # Stricter than _cross_check's tolerance on purpose: without outcomes.json
    # there is no way to distinguish a finished run from a killed one, and
    # "cannot prove it finished" must not read as "finished".
    case("outcomes.json absent — completion unprovable, so it FAILS", 1,
         lambda t: _fixture(t, missed=10, caught=100, write_outcomes=False),
         want_msg="is absent, so there is no way to tell")

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

    # ── the workflow-declaration path: REQ-GUARD-GATE-EVIDENCE-004, #414 ──
    # This is what the artifact's SECOND step runs. It reads a workflow file, not
    # a mutants run, so it is runnable from a clean checkout — the property #414
    # is about. It asserts on exit code AND, for the pass, on the printed
    # threshold, because a check whose output does not track its input is vacuous:
    # 292 and 142 must render differently or the gate proves nothing.
    def wcase(desc: str, want: int, body: str, want_msg: str | None = None):
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as tmp:
            wf = os.path.join(tmp, "ci.yml")
            with open(wf, "w", encoding="utf-8") as fh:
                fh.write(body)
            buf = io.StringIO()
            got = assert_workflow_threshold(wf, out=buf)
            text = buf.getvalue()
            if got != want:
                failed += 1
                print(f"  FAIL {desc}: got exit {got}, want {want}")
                for line in text.splitlines()[:4]:
                    print(f"         {line}")
            elif want_msg is not None and want_msg not in text:
                failed += 1
                print(f"  FAIL {desc}: exit {got} correct, diagnosis wrong — "
                      f"expected {want_msg!r}")
                for line in text.splitlines()[:4]:
                    print(f"         {line}")
            else:
                passed += 1
                print(f"  ok   {desc}")

    print()
    # A realistic gate: the --self-test line (no threshold) must be ignored, and
    # only the --output-dir + --max-missed line counts.
    GATE_292 = (
        "      - name: Mutants-report guardrail self-test\n"
        "        run: tools/check_mutants_report.py --self-test\n"
        "      - name: Check surviving mutants\n"
        "        run: tools/check_mutants_report.py --output-dir mutants-out --max-missed 292\n"
    )
    wcase("declared gate with a bounded threshold passes and prints it", 0,
          GATE_292, want_msg="--max-missed 292")
    # The non-vacuity pair: same shape, different number, different output.
    wcase("a DIFFERENT threshold prints DIFFERENTLY (output tracks input)", 0,
          GATE_292.replace("--max-missed 292", "--max-missed 142"),
          want_msg="--max-missed 142")
    # Gate removed entirely — the four-months-of-green failure mode, one level up.
    wcase("no gate invocation at all FAILS", 1,
          "      - name: Mutants-report guardrail self-test\n"
          "        run: tools/check_mutants_report.py --self-test\n",
          want_msg="declares no mutation gate")
    # The --self-test line must never be mistaken for the gate: it has no
    # --output-dir/--max-missed pairing, so a workflow with only it is gateless.
    wcase("the --self-test line alone is NOT a gate", 1,
          "        run: tools/check_mutants_report.py --self-test\n",
          want_msg="declares no mutation gate")
    # A neutered gate cannot fail, so it is not a gate. Both no-op forms — the
    # `|| true` the crashed run of #389 hid behind, and the `|| :` shell builtin —
    # must be caught; the `:` form was silently accepted until the `\b` bug in
    # _NEUTERED was fixed.
    wcase("a gate whose exit is swallowed by `|| true` FAILS", 1,
          GATE_292.replace("--max-missed 292",
                           "--max-missed 292 || true"),
          want_msg="neutered")
    wcase("a gate swallowed by `|| :` (shell no-op) also FAILS", 1,
          GATE_292.replace("--max-missed 292",
                           "--max-missed 292 || :"),
          want_msg="neutered")
    # A non-integer bound cannot ratchet.
    wcase("a non-integer --max-missed FAILS", 1,
          GATE_292.replace("--max-missed 292", "--max-missed none"),
          want_msg="not a non-negative integer")
    # The duplicated-fact hazard itself: two gates, disagreeing. This is exactly
    # #414's `210`-vs-`292`, and it must fail rather than silently pick one.
    wcase("two gates with disagreeing thresholds FAIL", 1,
          GATE_292 + "        run: tools/check_mutants_report.py --output-dir mutants-out --max-missed 210\n",
          want_msg="disagreeing values")
    # A commented-out invocation is not an enforced gate.
    wcase("a commented-out gate does not count as declared", 1,
          "      - name: Check surviving mutants\n"
          "        # run: tools/check_mutants_report.py --output-dir mutants-out --max-missed 292\n",
          want_msg="declares no mutation gate")
    # 0 is a LEGITIMATE bound (the ratchet's target, #382) — it must pass, or the
    # gate would punish the very state it exists to drive toward.
    wcase("a threshold of 0 (the target) is a valid bound", 0,
          GATE_292.replace("--max-missed 292", "--max-missed 0"),
          want_msg="--max-missed 0")

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
        "--assert-workflow-threshold",
        default=None,
        metavar="WORKFLOW",
        help="assert WORKFLOW (e.g. .github/workflows/ci.yml) declares exactly one "
             "bounded, un-neutered mutation gate, and print its threshold. Runnable "
             "from a clean checkout — reads the workflow, not the mutants run (#414).",
    )
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

    if args.assert_workflow_threshold:
        return assert_workflow_threshold(args.assert_workflow_threshold)

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
