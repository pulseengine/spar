#!/usr/bin/env python3
"""Execute every rivet `type: feature` artifact's `fields.steps[].run` commands.

Reads artifacts via `rivet list --filter <sexp>` + `rivet get <id> --format json`,
runs each step under a configurable shell, collects per-artifact pass/fail, and
writes a structured JSON summary alongside human-readable stdout progress.

Usage:
    tools/run_verification.py [--filter '<sexp>'] [--results-json PATH] [--shell SHELL]

Defaults:
    --filter '(= type "feature")'
    --results-json verification-results.json
    --shell        bash

Exit code:
    0  if at least one command executed and none failed
    1  if any artifact failed, OR if artifacts matched the filter but zero
       commands ran (every match was skipped) — a gate that verified nothing
       must not report success (#403)
    2  if `rivet list` itself failed

The `if not ids: return 1` below closed "the filter matched nothing". This
closes the sibling one line down — "the filter matched, and nothing ran":
`tools/run_verification.py --filter '(and (= type "feature") (has-field
"test-name"))'` reported passed:0 failed:0 skipped:64 EXIT=0, so the REQUIRED
`Verification Gate (rivet-driven)` context went green having executed nothing.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Optional


@dataclass
class Result:
    filter: str
    total: int = 0
    passed_count: int = 0
    failed_count: int = 0
    skipped_count: int = 0
    passed: list[str] = field(default_factory=list)
    failed: list[str] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)


def rivet_list_ids(filter_sexp: str) -> list[str]:
    proc = subprocess.run(
        ["rivet", "list", "--filter", filter_sexp, "--format", "json"],
        capture_output=True,
        text=True,
        check=True,
    )
    data = json.loads(proc.stdout)
    return [a["id"] for a in data.get("artifacts", [])]


def rivet_get_steps(artifact_id: str) -> list[str]:
    proc = subprocess.run(
        ["rivet", "get", artifact_id, "--format", "json"],
        capture_output=True,
        text=True,
        check=True,
    )
    data = json.loads(proc.stdout)
    return [s["run"] for s in data.get("fields", {}).get("steps", []) if "run" in s]


def run_one_step(cmd: str, shell: str) -> bool:
    """Return True iff exit code is 0."""
    proc = subprocess.run(
        [shell, "-c", cmd],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return proc.returncode == 0


def decide_exit(total: int, passed: int, failed: int, skipped: int) -> tuple[int, str]:
    """Return (exit_code, reason) for a completed run over a non-empty match set.

    The caller has already handled the empty match (`if not ids: return 1`), so
    ``total`` here is > 0.

    Three cases, in order:

    1. Any artifact failed  -> 1.
    2. Nothing executed — every matched artifact was skipped (``passed == 0`` and
       ``failed == 0``, so ``skipped == total``) -> 1. This is the #403 defect:
       the gate matched artifacts, ran zero commands, and previously returned 0.
       A gate that verified nothing must not report success. Left ratchet-compatible
       on purpose: a *mixed* run where some command ran and some claiming artifact
       was skipped still passes — bounding the skip backlog is
       ``check_evidence_steps.py``'s job, not this runner's. This closes only the
       hole where *nothing at all* was verified.
    3. Otherwise -> 0.
    """
    if failed > 0:
        return 1, f"{failed} artifact(s) failed"
    if passed == 0:
        # failed == 0 here, so every one of the `total` matches was skipped.
        return 1, (
            f"matched {total} artifact(s) but executed 0 verification commands "
            f"({skipped} skipped); refusing to report success having verified nothing"
        )
    return 0, f"{passed} passed, {skipped} skipped, 0 failed"


def decide_scope(matched: set[str], required: set[str], min_matched: int) -> tuple[int, str]:
    """Return (exit_code, reason) for whether the matched set may be verified.

    REQ-VERIFY-GATE-SCOPE-001 (#358, #513). The workflow lets a `Verify-Filter:`
    line in the PR body REPLACE the default filter. Before this check, that line
    could also NARROW it: `Verify-Filter: (= id "X")` turned the required
    `Verification Gate (rivet-driven)` context into a check of one artifact,
    chosen by the author of the PR it gates. The exemption was declared where
    the exempted party edits it, the shape #385 warned about.

    So an override may only WIDEN. ``required`` is what the default filter
    matches; every one of those artifacts must still be in ``matched``, and the
    reason names the ones that were dropped. With no override, ``matched`` IS
    ``required`` and this holds trivially.

    ``min_matched`` is a floor on the size of the matched set, for the case the
    widen check cannot see: the default filter itself shrinking (a tag renamed,
    a field dropped). It does not re-check what the default matches; it stops it
    quietly matching less.
    """
    missing = sorted(required - matched)
    if missing:
        shown = ", ".join(missing[:10]) + (" ..." if len(missing) > 10 else "")
        return 1, (
            f"the filter drops {len(missing)} artifact(s) the default filter "
            f"checks ({shown}); a Verify-Filter override may only widen the gate"
        )
    if len(matched) < min_matched:
        return 1, (
            f"matched {len(matched)} artifact(s), below the declared floor of "
            f"{min_matched}; the filter has stopped matching what it used to"
        )
    return 0, f"matched {len(matched)} artifact(s), covering all {len(required)} default ones"


# ── self-test (the decision that turns a run's tallies into an exit code) ────
#
# Runs BEFORE the gate it guards, and without rivet or a shell, so a refactor
# that lets `decide_exit` report success over a run that executed nothing fails
# here rather than by letting the real gate pass vacuously. The load-bearing
# case is "matched, every match SKIPPED -> 1": it is the exact #403 defect, and
# it is what makes this a detector rather than an assertion. Its assertion pins
# BOTH the exit code and the reason text ("verified nothing"), because a fix that
# returned the right code with a silent reason would still leave the required
# context reading green with no explanation on the PR.
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

    print("run_verification self-test")

    # (total, passed, failed, skipped) -> expected exit code
    cases = [
        ("all passed",                 (3, 3, 0, 0), 0),
        ("passed with some skipped",   (5, 2, 0, 3), 0),
        ("one artifact failed",        (4, 3, 1, 0), 1),
        ("all skipped (nothing ran)",  (64, 0, 0, 64), 1),   # THE #403 case
        ("single skip, nothing ran",   (1, 0, 0, 1), 1),
        ("a failure outranks a skip",  (2, 0, 1, 1), 1),
    ]
    for desc, (t, p, f, s), want_code in cases:
        got_code, _reason = decide_exit(t, p, f, s)
        report(f"decide_exit {desc}", got_code == want_code, got_code, want_code)

    # NON-VACUITY / message pin: the zero-executed verdict must say WHY, or the
    # required PR context reads green-adjacent with no reason. A code-only fix
    # would pass every case above and still fail here.
    code, reason = decide_exit(64, 0, 0, 64)
    report("zero-executed verdict is red", code == 1, code, 1)
    report("zero-executed verdict names the cause",
           "verified nothing" in reason, reason, "...verified nothing")

    # A passing run must NOT carry the zero-executed message.
    _, ok_reason = decide_exit(3, 3, 0, 0)
    report("a real pass does not claim 'verified nothing'",
           "verified nothing" not in ok_reason, ok_reason, "no 'verified nothing'")

    # ── decide_scope: a PR-body override may widen the gate, never narrow it ──
    D = {"A", "B", "C"}
    scope_cases = [
        ("no override: matched IS the default", D, D, 3, 0),
        ("a widening override passes", D | {"X", "Y"}, D, 3, 0),
        # THE #513 HOLE: `Verify-Filter: (= id "A")` narrowed the required gate.
        ("REGRESSION #513: a narrowing override FAILS", {"A"}, D, 1, 1),
        ("an override that swaps the set (same size) FAILS", {"A", "B", "X"}, D, 3, 1),
        ("dropping ONE default artifact fails", D - {"C"}, D, 2, 1),
        ("below the matched floor fails", D, D, 4, 1),
        ("exactly at the floor passes", D, D, 3, 0),
    ]
    for desc, matched, required, floor, want_code in scope_cases:
        got_code, _r = decide_scope(set(matched), set(required), floor)
        report(f"decide_scope {desc}", got_code == want_code, got_code, want_code)
    _, why = decide_scope({"A"}, D, 1)
    report("a narrowing verdict names the dropped artifacts",
           "B, C" in why and "only widen" in why, why, "...B, C ... only widen")

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--filter",
        default='(= type "feature")',
        help='rivet s-expression filter (default: %(default)r)',
    )
    parser.add_argument(
        "--results-json",
        default="verification-results.json",
        type=Path,
        help="path for the JSON summary (default: %(default)s)",
    )
    parser.add_argument(
        "--shell",
        default=os.environ.get("VERIFY_SHELL", "bash"),
        help="shell used to execute each step (default: %(default)s)",
    )
    parser.add_argument(
        "--must-cover",
        default=None,
        help="a filter whose every match --filter must also match (the gate's "
             "default); an override may widen the gate, never narrow it",
    )
    parser.add_argument(
        "--min-matched",
        type=int,
        default=0,
        help="fail when --filter matches fewer artifacts than this floor",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the decision-table self-test and exit (no rivet, no shell)",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    result = Result(filter=args.filter)

    print("== rivet verification gate ==")
    print(f"filter: {args.filter}")
    print(f"shell:  {args.shell}")
    print()

    try:
        ids = rivet_list_ids(args.filter)
    except subprocess.CalledProcessError as e:
        print(f"rivet list failed: {e.stderr}", file=sys.stderr)
        return 2

    if not ids:
        print("No artifacts matched filter.", file=sys.stderr)
        args.results_json.write_text(json.dumps(asdict(result), indent=2))
        return 1

    print(f"matched {len(ids)} artifacts")

    required: set[str] = set(ids)
    if args.must_cover is not None and args.must_cover != args.filter:
        try:
            required = set(rivet_list_ids(args.must_cover))
        except subprocess.CalledProcessError as e:
            print(f"rivet list failed for --must-cover: {e.stderr}", file=sys.stderr)
            return 2
        if not required:
            # The default matching nothing would make every override "cover" it.
            print(f"::error::--must-cover filter matched nothing: {args.must_cover}",
                  file=sys.stderr)
            return 2
    code, why = decide_scope(set(ids), required, args.min_matched)
    print(f"scope:  {why}")
    if code:
        print(f"::error::{why}", file=sys.stderr)
        result.total = len(ids)
        args.results_json.write_text(json.dumps(asdict(result), indent=2))
        return code
    print()
    result.total = len(ids)

    for artifact_id in ids:
        try:
            steps = rivet_get_steps(artifact_id)
        except subprocess.CalledProcessError as e:
            print(f"[FAIL] {artifact_id}: rivet get failed: {e.stderr}")
            result.failed.append(artifact_id)
            continue

        if not steps:
            print(f"[SKIP] {artifact_id} (no fields.steps[].run)")
            result.skipped.append(artifact_id)
            continue

        print(f"[RUN ] {artifact_id}")
        ok = True
        for cmd in steps:
            print(f"       + {cmd}")
            if not run_one_step(cmd, args.shell):
                ok = False
                print(f"       ✗ failed: {cmd}")
                break

        if ok:
            print(f"[ OK ] {artifact_id}")
            result.passed.append(artifact_id)
        else:
            print(f"[FAIL] {artifact_id}")
            result.failed.append(artifact_id)

    result.passed_count = len(result.passed)
    result.failed_count = len(result.failed)
    result.skipped_count = len(result.skipped)

    args.results_json.write_text(json.dumps(asdict(result), indent=2))

    print()
    print("== summary ==")
    print(f"passed:  {result.passed_count}")
    print(f"failed:  {result.failed_count}")
    print(f"skipped: {result.skipped_count}")
    if result.failed:
        print("failed IDs:")
        for fid in result.failed:
            print(f"  - {fid}")

    code, reason = decide_exit(
        result.total,
        result.passed_count,
        result.failed_count,
        result.skipped_count,
    )
    print(reason)
    return code


if __name__ == "__main__":
    sys.exit(main())
