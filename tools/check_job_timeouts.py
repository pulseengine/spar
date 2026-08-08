#!/usr/bin/env python3
"""Every job backing a required context must declare a real timeout-minutes (issue #398).

THE PROBLEM. A required status check that hangs is not a wrong-green — it is an
indefinite block with no signal. A wedged job leaves its context at "Expected —
waiting for status to be reported", which is byte-identical to "still running",
and a job with no `timeout-minutes` inherits GitHub's 360-minute default before
anything cancels it. For six hours the PR is unmergeable for a reason nobody can
see, the job holds a self-hosted runner the whole time, and with branch
protection's `strict = true` every other open PR queues behind it. `Detect
changed paths` is the sharpest case: it gates ten of the other jobs, so wedging
it stalls the entire suite.

Measured against the live required set at the time this landed, 15 of the 18
required contexts declared no `timeout-minutes`. That is not a hypothetical — it
is the blast radius of the *first* hang, whenever it comes.

THE CHECK. This is a sibling of `check_required_contexts.py`, and reuses its
restricted workflow reader rather than copying it — one parser, one set of
fail-closed refusals, `--cross-check`ed against PyYAML in the same place. That
gate answers "is the committed required set exactly the set of jobs that must be
required?"; this one answers "does every one of those jobs carry a bound that
actually reduces the blast radius of a hang?"

A job's `timeout-minutes` is *valid* here iff it is a static integer in the open
interval (0, 360):

  * missing            — the defect this gate exists for; inherits the 360 default.
  * a `${{ … }}` template — not a static bound this reader (or a human auditing
    the YAML) can evaluate; a bound you cannot read is not a bound you can trust.
  * <= 0               — not a number of minutes.
  * >= 360             — no tighter than the default it replaces, so it buys
    nothing. A declared 360 is a 360-minute blind wait with extra characters.

It does NOT check that the value is well-sized ("a few multiples of p95"). That
is a judgement call the issue leaves to a human with the run history, and a gate
that guessed it would be guessing. This gate draws the one line that is not a
judgement call: an unbounded (or uselessly-bounded) required job must not ship.

SCOPE — GATEABLE ONLY, and why that is exactly "backing a required context".
`check_required_contexts.py` proves the committed required set equals the
GATEABLE set (PR-triggered against main, no path filter, no continue-on-error,
static name). So checking every GATEABLE job here *is* checking every job that
backs a required context — and it does so straight from the workflows, with no
dependency on the committed list, which means a newly-added gateable job is
covered the moment it exists rather than the moment someone remembers to list
it. Advisory (`continue-on-error`) and undeliverable (path-scoped / templated)
jobs are deliberately out of scope: they cannot block a merge, so a hang in one
is not the six-hour merge-gate wedge this gate is about. (An advisory hang still
wastes a runner; that is real but separate, and out of scope here so this gate
stays about the merge gate.)

WHY NOT FOLD THIS INTO check_required_contexts.py. That gate's whole subject is
set membership; a timeout is a per-job property with different remedies and a
different failure. Kept apart, each self-test stays legible and neither gate's
fixtures have to carry the other's concern.

Usage:
  tools/check_job_timeouts.py                 # check the real workflows
  tools/check_job_timeouts.py --self-test     # fixture cases; run this FIRST in CI
  tools/check_job_timeouts.py --list          # print each gateable job and its timeout

Exit: 0 clean, 1 violation, 2 usage/environment error (fail-closed).
"""

from __future__ import annotations

import argparse
import glob
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from check_required_contexts import (  # noqa: E402  (path set above)
    GATEABLE,
    WORKFLOW_GLOB,
    WorkflowSyntaxError,
    classify_jobs,
    parse_workflow,
)

#: GitHub's default job timeout. A job with no `timeout-minutes` runs up to this
#: long before it is cancelled; a declared value >= this is no improvement. This
#: is the number the issue names as the blind-wait ceiling.
DEFAULT_TIMEOUT_MINUTES = 360


def validate_timeout(raw) -> str | None:
    """None if `raw` is a valid bound; otherwise the reason it is not.

    `raw` is the parsed scalar of a job's `timeout-minutes`, or None if the key
    is absent. A valid bound is a static integer strictly inside (0, 360): large
    enough to be a real number of minutes, small enough to beat the default it
    replaces.
    """
    if raw is None:
        return "declares no `timeout-minutes`"
    s = str(raw).strip()
    if "${{" in s:
        return (
            f"sets `timeout-minutes: {s}`, a template expression rather than a "
            f"static bound — a timeout you cannot read at rest is not one you can rely on"
        )
    try:
        minutes = int(s)
    except ValueError:
        return f"sets `timeout-minutes: {s}`, which is not an integer"
    if minutes <= 0:
        return f"sets `timeout-minutes: {minutes}`, which is not a positive number of minutes"
    if minutes >= DEFAULT_TIMEOUT_MINUTES:
        return (
            f"sets `timeout-minutes: {minutes}`, which is >= GitHub's "
            f"{DEFAULT_TIMEOUT_MINUTES}-minute default and so does not reduce the "
            f"blast radius of a wedged job"
        )
    return None


def scan_gateable(workflow_glob: str = WORKFLOW_GLOB) -> list[tuple[str, str, str, object]]:
    """(context name, workflow path, job key, raw timeout) for every GATEABLE job.

    Reuses check_required_contexts' reader and classifier verbatim, so the set
    scanned here is exactly the set that gate proves must be required. The reader
    raises WorkflowSyntaxError on anything it cannot represent at the depths it
    reads; that propagates and main() renders it as exit 2 (refuse, do not guess).
    """
    out: list[tuple[str, str, str, object]] = []
    for path in sorted(glob.glob(workflow_glob)):
        with open(path) as fh:
            doc = parse_workflow(fh.read(), path)
        for jid, (bucket, name, _reasons) in classify_jobs(doc).items():
            if bucket != GATEABLE:
                continue
            raw = (doc["jobs"].get(jid) or {}).get("timeout-minutes")
            out.append((name, path, jid, raw))
    return out


def run_check(workflow_glob: str = WORKFLOW_GLOB, out=None) -> int:
    out = out or sys.stdout
    gateable = scan_gateable(workflow_glob)

    errors: list[str] = []
    for name, path, jid, raw in sorted(gateable):
        why = validate_timeout(raw)
        if why:
            errors.append(
                f"{path}: job `{jid}` produces required context {name!r} but {why}. "
                f"A wedged required job with no real bound sits at \"Expected — waiting "
                f"for status\" for up to {DEFAULT_TIMEOUT_MINUTES} minutes, holds a "
                f"runner, and (branch protection is strict) blocks every other PR."
            )

    print(f"scanned {len(gateable)} gateable job(s) backing a required context", file=out)

    if errors:
        for e in errors:
            print(f"::error::{e}", file=out)
        print(f"\nFAIL: {len(errors)} required-context job(s) without a valid timeout-minutes.", file=out)
        return 1

    print(
        f"PASS: every gateable job declares a static timeout-minutes in "
        f"(0, {DEFAULT_TIMEOUT_MINUTES}).",
        file=out,
    )
    return 0


# ── self-test ─────────────────────────────────────────────────────────────
#
# Run on every CI invocation via --self-test. A guardrail exercised only against
# a repo that already satisfies it cannot be told apart from one that returns 0
# unconditionally: the live repo is a single input. Each fixture below turns one
# verdict the other way, so deleting any single guard reds this list.

_OK = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    timeout-minutes: 30
    runs-on: ubuntu-latest
"""

_MISSING = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
"""

_TEMPLATED = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    timeout-minutes: ${{ fromJSON(env.T) }}
    runs-on: ubuntu-latest
"""

_ZERO = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    timeout-minutes: 0
    runs-on: ubuntu-latest
"""

_NONINT = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    timeout-minutes: soon
    runs-on: ubuntu-latest
"""

_OVER_DEFAULT = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    timeout-minutes: 400
    runs-on: ubuntu-latest
"""

_JUST_UNDER = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    timeout-minutes: 359
    runs-on: ubuntu-latest
"""

#: A continue-on-error job with no timeout. It cannot block a merge, so it is out
#: of scope and must NOT be flagged — treating it as gateable would report a
#: violation this gate does not own.
_ADVISORY_NO_TIMEOUT = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha (advisory)
    continue-on-error: true
    runs-on: ubuntu-latest
"""

#: A path-scoped workflow: its job is UNDELIVERABLE, never required, so a missing
#: timeout is not this gate's concern.
_UNDELIVERABLE_NO_TIMEOUT = """
name: W
on:
  pull_request:
    paths:
      - "src/**"
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
"""

SELF_TESTS = [
    (
        "gateable job with a valid timeout",
        _OK, 0,
        "the checker can return 0 at all — without this the other cases are "
        "consistent with a script that always fails",
    ),
    (
        "gateable job with no timeout-minutes",
        _MISSING, 1,
        "THE DEFECT: an unbounded required job inherits the 360-minute default "
        "and wedges the merge gate for six hours with no signal (the 15-job shape)",
    ),
    (
        "gateable job with a templated timeout",
        _TEMPLATED, 1,
        "a `${{ }}` timeout is not a static bound a reader can evaluate; it could "
        "resolve to nothing or to something huge, and the gate must not bless it",
    ),
    (
        "gateable job with timeout-minutes: 0",
        _ZERO, 1,
        "zero (and any non-positive) is not a real number of minutes",
    ),
    (
        "gateable job with a non-integer timeout",
        _NONINT, 1,
        "a non-integer would be a YAML error at runtime; catch it at rest, not in "
        "the wedged job it was supposed to bound",
    ),
    (
        "gateable job with timeout >= the 360 default",
        _OVER_DEFAULT, 1,
        "a value at or above GitHub's 360-minute default reduces no blast radius — "
        "a declared 360 is the same six-hour blind wait with extra characters",
    ),
    (
        "gateable job with timeout just under the default",
        _JUST_UNDER, 0,
        "the upper bound must be satisfiable, not merely restrictive: 359 beats "
        "the default and is accepted",
    ),
    (
        "advisory job with no timeout is not flagged",
        _ADVISORY_NO_TIMEOUT, 0,
        "a continue-on-error job cannot block a merge, so it is out of scope; "
        "flagging it would claim a violation this gate does not own",
    ),
    (
        "undeliverable (path-scoped) job with no timeout is not flagged",
        _UNDELIVERABLE_NO_TIMEOUT, 0,
        "a path-scoped job can never be required, so its missing timeout is not "
        "the merge-gate wedge this gate guards",
    ),
]


def self_test() -> int:
    import io
    import os
    import tempfile

    failures = 0
    for name, wf_text, want, proves in SELF_TESTS:
        with tempfile.TemporaryDirectory() as tmp:
            # `.yaml`, the extension the original `*.yml` glob could not see —
            # every fixture therefore also exercises discovery through the shared
            # WORKFLOW_GLOB, which is `.y*ml`.
            with open(os.path.join(tmp, "w.yaml"), "w") as fh:
                fh.write(wf_text)
            buf = io.StringIO()
            try:
                got = run_check(os.path.join(tmp, "*.y*ml"), out=buf)
            except WorkflowSyntaxError as exc:
                got = 2
                print(f"refused: {exc}", file=buf)

        mark = "ok  " if got == want else "FAIL"
        if got != want:
            failures += 1
        print(f"  {mark} {name}: exit {got} (want {want})")
        print(f"       proves: {proves}")
        if got != want:
            print("       ---- output ----")
            for line in buf.getvalue().splitlines():
                print(f"       {line}")

    # The fixtures run against a glob this function builds, so they never touch
    # the real corpus. Two production-only ways to go silently green are checked
    # directly, against the same WORKFLOW_GLOB the real run uses:
    #
    #   1. The scan finds zero gateable jobs. On this repo that is impossible
    #      (there are ~18) — so zero means the reader went blind, which must fail,
    #      not pass on a corpus it never read. This is the empty-scan failure mode
    #      the rest of tools/ guards against, asserted here rather than assumed.
    try:
        live = scan_gateable(WORKFLOW_GLOB)
    except WorkflowSyntaxError as exc:
        print(f"  FAIL live scan refused to parse a workflow: {exc}")
        return 1
    if not live:
        print("  FAIL live scan found zero gateable jobs")
        print("       proves: a scan that sees nothing must fail, not report PASS "
              "on a corpus it never read")
        failures += 1
    else:
        print(f"  ok   live scan found {len(live)} gateable job(s)")
        print("       proves: the production scan is not silently empty")

    #   2. The glob misses `.yaml`. Actions loads .yml AND .yaml, so a glob that
    #      sees only one reports PASS on a directory it never fully read.
    import fnmatch

    if not fnmatch.fnmatch(".github/workflows/x.yaml", WORKFLOW_GLOB):
        print(f"  FAIL WORKFLOW_GLOB does not match `.yaml`: {WORKFLOW_GLOB!r}")
        failures += 1
    else:
        print("  ok   WORKFLOW_GLOB matches both .yml and .yaml")

    total = len(SELF_TESTS) + 2
    if failures:
        print(f"\n{failures} of {total} self-test(s) FAILED.")
        return 1
    print(f"\n{total} self-test(s) passed.")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--workflows", default=WORKFLOW_GLOB)
    p.add_argument("--self-test", action="store_true")
    p.add_argument("--list", dest="do_list", action="store_true")
    args = p.parse_args()

    if args.self_test:
        return self_test()

    if args.do_list:
        for name, path, jid, raw in sorted(scan_gateable(args.workflows)):
            shown = "(none)" if raw is None else raw
            print(f"{shown!s:>8}  {name}  [{path} :: {jid}]")
        return 0

    return run_check(args.workflows)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except WorkflowSyntaxError as exc:
        # Fail closed, exactly as the shared reader's own entry point does: a
        # workflow shape the reader cannot classify is one whose timeout it cannot
        # read either.
        print(f"::error::{exc}", file=sys.stderr)
        print(
            "::error::The workflow reader is deliberately restricted (see "
            "check_required_contexts.py) and stops rather than guessing.",
            file=sys.stderr,
        )
        raise SystemExit(2)
