#!/usr/bin/env python3
"""An artifact that CLAIMS evidence shall carry a runnable step.

REQ-GUARD-GATE-EVIDENCE-003. Addresses #403.

WHY THIS EXISTS
===============

`tools/run_verification.py` skips an artifact with no `fields.steps[].run` and
counts the skip toward exit 0:

    if not steps:
        print(f"[SKIP] {artifact_id} (no fields.steps[].run)")
        result.skipped.append(artifact_id)
        continue
    ...
    return 0 if result.failed_count == 0 else 1

Executed against the live artifact plane:

    $ tools/run_verification.py --filter '(and (= type "feature") (has-field "test-name"))'
    passed: 0   failed: 0   skipped: 64
    EXIT=0

64 artifacts matched. Zero commands ran. Exit 0 — and in
`.github/workflows/verification-gate.yml` that makes the REQUIRED context
`Verification Gate (rivet-driven)` green having executed nothing.

Note what was already guarded and what was not. `run_verification.py:110` has
`if not ids: return 1` — "the filter matched nothing" was closed. "The filter
matched, and nothing ran" was left open. The author saw the hazard and stopped
one step short of it, which is the more instructive half of this bug.

This is REQ-GUARD-GATE-EVIDENCE-002 (c) one level up. (c) made a test filter
that selects nothing fail; an artifact with NO filter at all still yields the
ideal reading, because `check_verification_filters.py` classifies only `- run:`
steps and a step-less artifact contributes zero to every one of its counters.
Fixture proof, run against that tool: a step-less `implemented` artifact and an
empty file produce byte-identical output.

WHY A RATCHET AND NOT A HARD FAIL
=================================

MEASURED on df4a64f: **85 of 239 `type: feature` artifacts claim a completed
status and carry no runnable step** — 60 in `safety/stpa/validation.yaml`, 25 in
`artifacts/verification.yaml`. Failing all 85 at once is not a shippable change,
and a gate that must be disabled to merge anything is a gate that gets disabled.

Backfilling the seven `TEST-GUARD-*` artifacts this change also fixes takes it
to 78; the remaining 60 in `safety/stpa/validation.yaml` (whose `test-name`
values include `TBD` and `tests::*`) need triage, not a step, and that is a
separate decision.

So this is the shape `check_lean_sorries.py` and `check_mutants_report.py`
already use: a declared floor that may not rise, printed in full on every path.
New claims must carry evidence; the existing backlog is recorded rather than
hidden, and shrinking it is the ratchet walking down.

NOT CLAIMED: that a present step is the RIGHT step, or that it passes. That is
`run_verification.py`'s job. And a step existing does not make the artifact
verified — `check_verification_filters.py` answers whether the filter selects
anything, this answers whether there is a filter at all. Three different
questions, and each was independently able to report success without asking its
own.
"""

from __future__ import annotations

import argparse
import glob
import io
import os
import sys

try:
    import yaml
except ModuleNotFoundError:  # pragma: no cover - reported, never silently skipped
    print("::error::PyYAML is unavailable, so the artifact plane cannot be read. "
          "This check reports nothing rather than reporting zero violations.",
          file=sys.stderr)
    sys.exit(2)

# Statuses that ASSERT the evidence exists. Same set as
# check_verification_filters.py; a `proposed` artifact naming evidence it has not
# built yet is a plan, not a false claim.
_CLAIMING = {"implemented", "verified", "accepted"}

_DEFAULT_GLOBS = ("artifacts/*.yaml", "safety/**/*.yaml")


def _runnable_steps(artifact: dict) -> int:
    fields = artifact.get("fields")
    if not isinstance(fields, dict):
        return 0
    steps = fields.get("steps")
    if not isinstance(steps, list):
        return 0
    return sum(1 for s in steps if isinstance(s, dict) and "run" in s)


def scan(root: str, patterns=_DEFAULT_GLOBS) -> tuple[list[tuple[str, str, str]], int]:
    """([(file, id, status)] claiming-but-stepless, total feature artifacts)."""
    offenders: list[tuple[str, str, str]] = []
    total = 0
    for pattern in patterns:
        for path in sorted(glob.glob(os.path.join(root, pattern), recursive=True)):
            try:
                doc = yaml.safe_load(open(path, encoding="utf-8"))
            except (OSError, yaml.YAMLError):
                # A file this reader cannot parse is not evidence of compliance.
                # rivet validate already gates syntax; surface it and move on
                # rather than counting it as clean.
                print(f"::warning::could not parse {path}", file=sys.stderr)
                continue
            if not isinstance(doc, dict):
                continue
            for value in doc.values():
                if not isinstance(value, list):
                    continue
                for art in value:
                    if not isinstance(art, dict) or art.get("type") != "feature":
                        continue
                    total += 1
                    status = art.get("status")
                    if status in _CLAIMING and _runnable_steps(art) == 0:
                        rel = os.path.relpath(path, root)
                        offenders.append((rel, str(art.get("id")), str(status)))
    return offenders, total


def check(root: str, max_stepless: int, out=sys.stdout, patterns=_DEFAULT_GLOBS) -> int:
    offenders, total = scan(root, patterns)

    if total == 0:
        print("::error::no `type: feature` artifacts found — the scan is broken. "
              "Zero violations because nothing was read is the exact failure this "
              "gate exists to prevent.", file=out)
        return 2

    print("== evidence-step guardrail ==", file=out)
    print(f"feature artifacts scanned:     {total}", file=out)
    print(f"claiming evidence, no step:    {len(offenders)}   "
          f"(declared floor: {max_stepless})", file=out)

    by_file: dict[str, int] = {}
    for path, _aid, _st in offenders:
        by_file[path] = by_file.get(path, 0) + 1
    for path, n in sorted(by_file.items(), key=lambda kv: -kv[1]):
        print(f"  {n:4}  {path}", file=out)
    for path, aid, status in offenders:
        print(f"       {aid}  (status={status})  {path}", file=out)

    if len(offenders) > max_stepless:
        print("", file=out)
        print(f"::error::{len(offenders)} artifacts claim evidence with no runnable "
              f"step, above the declared floor of {max_stepless}.", file=out)
        print("::error::run_verification.py SKIPS a step-less artifact and counts "
              "the skip toward exit 0, so each of these closes its V without "
              "executing anything (#403). Give it a `fields.steps[].run`, or set "
              "its status to `proposed` until the evidence exists.", file=out)
        return 1

    if len(offenders) < max_stepless:
        print("", file=out)
        print(f"::notice::{len(offenders)} is BELOW the floor of {max_stepless} — "
              f"lower --max-stepless to {len(offenders)} so the ratchet holds the "
              f"ground that was gained.", file=out)

    print(f"\n{len(offenders)} claiming-but-stepless artifacts, at or under the "
          f"declared floor of {max_stepless}.", file=out)
    return 0


def self_test() -> int:
    import tempfile
    from pathlib import Path

    passed = failed = 0

    def case(desc, want, files, floor, want_msg=None):
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            for name, body in files.items():
                p = Path(td, name)
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(body, encoding="utf-8")
            buf = io.StringIO()
            # Deliberately NOT passing `patterns`: the self-test must exercise
            # the DEFAULT globs. Supplying its own hid a mutation that narrowed
            # the default to artifacts/ only — which would drop 60 of the 78
            # live offenders and still look clean.
            got = check(td, floor, out=buf)
        text = buf.getvalue()
        if got != want:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want {want}")
            for line in text.splitlines()[:5]:
                print(f"         {line}")
        elif want_msg is not None and want_msg not in text:
            failed += 1
            print(f"  FAIL {desc}: exit {got} correct, diagnosis wrong — "
                  f"expected {want_msg!r}")
        else:
            passed += 1
            print(f"  ok   {desc}")

    WITH_STEP = """artifacts:
  - id: TEST-OK
    type: feature
    status: implemented
    fields:
      method: automated-test
      steps:
        - run: cargo test -p spar-wasm -- render_basic
"""
    NO_STEP = """artifacts:
  - id: TEST-BARE
    type: feature
    status: implemented
"""
    NO_STEP_PROPOSED = NO_STEP.replace("implemented", "proposed")
    EMPTY_STEPS = """artifacts:
  - id: TEST-EMPTY
    type: feature
    status: verified
    fields:
      method: automated-test
      steps: []
"""
    STEP_NO_RUN = """artifacts:
  - id: TEST-NORUN
    type: feature
    status: implemented
    fields:
      steps:
        - description: someone will do this by hand
"""

    print("check_evidence_steps self-test")
    # THE BUG. A claiming artifact with no step is skipped by run_verification
    # and the skip counts toward exit 0.
    case("REGRESSION #403: claiming + no step breaches a floor of 0", 1,
         {"artifacts/v.yaml": NO_STEP}, 0,
         want_msg="claim evidence with no runnable step")
    # The discriminating pair: same artifact, same absence, only status differs.
    # A checker ignoring status gives both the same verdict and cannot fail the
    # first — and failing `proposed` would push authors to write the artifact
    # only after the test, inverting the order the methodology depends on.
    case("...and the SAME artifact as `proposed` passes", 0,
         {"artifacts/v.yaml": NO_STEP_PROPOSED}, 0)
    case("a claiming artifact WITH a runnable step passes", 0,
         {"artifacts/v.yaml": WITH_STEP}, 0)
    # Absence wearing a costume: the key exists but carries nothing runnable.
    case("`steps: []` is no step", 1, {"artifacts/v.yaml": EMPTY_STEPS}, 0)
    case("a step with no `run:` key is no step", 1,
         {"artifacts/v.yaml": STEP_NO_RUN}, 0)
    # The ratchet.
    case("at the floor passes", 0, {"artifacts/v.yaml": NO_STEP}, 1)
    case("below the floor passes AND says to lower it", 0,
         {"artifacts/v.yaml": WITH_STEP}, 3,
         want_msg="lower --max-stepless to 0")
    # safety/ is scanned too — 60 of the 85 live offenders are there, so a
    # checker reading only artifacts/ would report a third of the truth.
    case("safety/**/*.yaml is scanned, not just artifacts/", 1,
         {"safety/stpa/validation.yaml": NO_STEP}, 0)
    # Broken scans must not read as clean.
    case("no feature artifacts at all is a broken scan", 2,
         {"artifacts/v.yaml": "artifacts: []\n"}, 0,
         want_msg="the scan is broken")

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", default=".")
    ap.add_argument("--max-stepless", type=int, default=78,
                    help="declared floor; 85 measured on df4a64f, 78 after the "
                         "seven TEST-GUARD-* artifacts were backfilled")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return check(a.root, a.max_stepless)


if __name__ == "__main__":
    sys.exit(main())
