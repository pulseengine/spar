#!/usr/bin/env python3
"""Keep the required-status-check set honest against the workflows (issue #372).

THE PROBLEM. A branch-protection "required status check" is a *claim*: it says
"this PR cannot merge until that job passed." Whether the claim is true depends
on facts that live somewhere else entirely — in the workflow files — and nothing
reconciles the two. Three ways they drift, each failing differently:

  1. A job is added to a PR-triggered workflow and nobody adds it to the
     required set. It runs, it goes red, and the PR merges anyway. The gate
     looks full; it is one job short.

     `osate-corpus` has this symptom today — and this gate deliberately does
     NOT fix it, which is worth stating rather than letting the reader assume
     otherwise. `aadl-interop.yml` carries a workflow-level
     `on.pull_request.paths:` filter, so the classifier buckets it
     UNDELIVERABLE and rules that it must *not* be required (see 3 — requiring
     it would deadlock every PR outside those paths). The gate reports PASS
     with the symptom open. The remedy is a change to that workflow — always
     run, filter with a job-level `if:` — after which this gate will demand
     the context be required. Until someone makes it, `osate-corpus` is a job
     that can go red while the PR merges, and nothing here flags it.

  2. A job carries `continue-on-error: true`. It reports SUCCESS whatever
     happened — pass, fail, or never really ran. Requiring it enforces nothing,
     and even *not* requiring it still renders a green tick beside its name in
     the PR UI, which a reader reasonably takes as "this was checked."

  3. A required job lives in a workflow with a workflow-level
     `on.pull_request.paths:` filter. This is the trap, and it is worth stating
     plainly because the obvious fix causes it: a workflow that does not trigger
     reports *nothing at all* — not "skipped", nothing — so the required context
     sits at "Expected — waiting for status" forever and the PR can never merge.
     Every PR that misses those paths deadlocks.

     The fix is NOT to add the context. It is to make the workflow always run
     and move the filter to a job-level `if:`. An `if:`-skipped job reports
     "skipped", and a skipped required context is SATISFIED. That distinction —
     "did not run" vs "ran and skipped" — is the entire difference between a
     deadlocked repo and a working path filter, and it is invisible in the
     workflow YAML.

So each job falls into exactly one of three buckets, and the bucket determines
whether it may be required:

  GATEABLE     PR-triggered, no workflow-level path filter, no
               continue-on-error, static (non-templated) name  -> MUST be required
  ADVISORY     continue-on-error: true                         -> must NOT be required,
                                                                  and must say so in its name
  UNDELIVERABLE workflow-level pull_request paths filter, or a
               templated/matrix name with no static context    -> must NOT be required

WHY A COMMITTED LIST RATHER THAN THE LIVE API. Reading branch protection
(`GET /repos/{o}/{r}/branches/{b}/protection`) needs an admin token, and the
Actions `GITHUB_TOKEN` is not one. A gate that cannot run in CI is not a gate,
so the expected set is committed to `.github/required-contexts.txt` and this
script reconciles WORKFLOWS <-> LIST hermetically on every PR. Reconciling
LIST <-> BRANCH PROTECTION needs the admin token and is available via
`--against-api` for a maintainer or a scheduled admin-credentialed run.

That split is a real limitation, stated rather than hidden: this gate proves the
committed list matches the workflows. It does not, by itself, prove GitHub is
enforcing that list. Those are different claims and only one of them is
mechanically checked here.

WHY A HAND-ROLLED PARSER INSTEAD OF PyYAML. Same rule the human-scoped
guardrail next to it in ci.yml follows: no workflow in this repo installs a
Python package, so importing PyYAML would make a *required* check depend on an
unverified runner package that a runner-image update could remove. The reader
below handles only the fixed shape a GitHub workflow has at the three depths
this gate reads, and is fail-closed — anything it does not fully recognise at
those positions exits 2 (red) rather than being scanned past. `--cross-check`
validates it against PyYAML on the real workflow corpus wherever PyYAML happens
to be available, which is how the restriction is kept honest rather than
assumed.

That leaves a gap worth naming, because it is the one an audit found: in CI —
the only place this gate is load-bearing — `--cross-check` does NOT run, for the
same PyYAML reason. So the safety of the reader in CI cannot rest on agreeing
with a reference parser; it has to rest on the reader refusing what it cannot
represent. Four constructs previously read wrong and silently, and two of them
*flipped a verdict* rather than merely losing information:

  * `pull_request: {branches: [main], paths: [...]}` (flow mapping) — read as an
    opaque string, so `paths` disappeared, the job was ruled GATEABLE, and the
    gate ordered you to require a context that would never be delivered. The
    gate handing you the exact deadlock it exists to prevent.
  * a file indented with four spaces — every job invisible, zero jobs, zero
    violations, PASS. Fail-OPEN, which the paragraph above claims cannot happen.
  * a folded/literal block scalar as a job `name:` — the context name read as
    `>-`.
  * YAML anchors and aliases in a name.

All four now raise (`_reject_unsupported` plus the empty-`jobs:` check) and each
has a fixture. `--cross-check` remains the belt for local work; the refusals are
the braces that actually ship.

Usage:
  tools/check_required_contexts.py                 # reconcile workflows <-> committed list
  tools/check_required_contexts.py --self-test     # fixture cases; run this FIRST in CI
  tools/check_required_contexts.py --print         # emit the GATEABLE set (regenerate the list)
  tools/check_required_contexts.py --against-api   # also diff against live branch protection
  tools/check_required_contexts.py --cross-check   # agree with PyYAML on the real corpus (local)

Exit: 0 clean, 1 violation, 2 usage/environment error.
"""

from __future__ import annotations

import argparse
import fnmatch
import glob
import json
import os
import re
import subprocess
import sys

#: `*.y*ml`, not `*.yml`. GitHub Actions loads BOTH extensions from
#: .github/workflows/, so a `sneaky.yaml` holding a gateable job is invisible to
#: a `*.yml` glob — the gate reports PASS on a repo it never fully read. That is
#: failure mode 1 with the gate's own blind spot supplying the silence.
WORKFLOW_GLOB = ".github/workflows/*.y*ml"
LIST_PATH = ".github/required-contexts.txt"
PROTECTED_BRANCH = "main"

GATEABLE = "GATEABLE"
ADVISORY = "ADVISORY"
UNDELIVERABLE = "UNDELIVERABLE"
NOT_PR = "NOT_PR"

#: An advisory job's name is the only thing a PR reader sees. Requiring the
#: marker is not cosmetics: `continue-on-error` severs the tick from the
#: outcome, so an unmarked name renders a claim the run does not support.
ADVISORY_MARKER = "advisory"


class WorkflowSyntaxError(Exception):
    """Something at a depth this gate reads was not recognised. Fail closed."""


_KEY = re.compile(r"^(?P<indent> *)(?P<key>[A-Za-z_][\w.-]*|'[^']*'|\"[^\"]*\"): ?(?P<val>.*)$")
_ITEM = re.compile(r"^(?P<indent> *)- +(?P<val>.*)$")


def _strip_comment(line: str) -> str:
    """Drop a trailing `#` comment, respecting quotes.

    Naively splitting on `#` corrupts `name: Bazel test (//...)` the moment
    someone writes a name containing one, and this gate's whole subject is job
    names.
    """
    out, quote = [], None
    for i, ch in enumerate(line):
        if quote:
            out.append(ch)
            if ch == quote:
                quote = None
        elif ch in "'\"":
            quote = ch
            out.append(ch)
        elif ch == "#" and (i == 0 or line[i - 1] in " \t"):
            break
        else:
            out.append(ch)
    return "".join(out).rstrip()


#: YAML constructs this reader cannot represent, at positions where misreading
#: one changes a verdict. Each is legal YAML and legal Actions, so "nobody writes
#: that here" is a property of today's files, not of the format — and the failure
#: is silent in the dangerous direction. The flow mapping is the sharp one:
#:
#:     on:
#:       pull_request: {branches: [main], paths: ["src/**"]}
#:
#: reads as the *string* "{branches: ...}", `classify_workflow` does
#: `if not isinstance(pr, dict): pr = {}`, the paths filter evaporates, and the
#: job is ruled GATEABLE. The gate then instructs you to require a context that
#: will not be delivered — it hands you the exact deadlock it exists to prevent.
#: Refusing to parse is the only safe answer; a reader that guesses here is worse
#: than no reader.
def _reject_unsupported(val, path: str, lineno: int, where: str) -> None:
    if not isinstance(val, str):
        return
    v = val.strip()
    if not v:
        return
    if v.startswith("{"):
        kind = "a flow mapping"
    elif v[0] in "&*":
        kind = "a YAML anchor or alias"
    elif v[0] in "|>":
        kind = "a block scalar"
    else:
        return
    raise WorkflowSyntaxError(
        f"{path}:{lineno}: {where} is {kind} ({v!r}), which this restricted "
        f"reader cannot represent. Rewrite it as block-style YAML. Guessing "
        f"here can flip a job's classification and deadlock every PR."
    )


def _scalar(val: str):
    val = val.strip()
    if not val:
        return None
    if val[0] in "'\"" and val[-1] == val[0] and len(val) >= 2:
        return val[1:-1]
    if val.startswith("[") and val.endswith("]"):
        inner = val[1:-1].strip()
        return [_scalar(x) for x in inner.split(",")] if inner else []
    if val in ("true", "True"):
        return True
    if val in ("false", "False"):
        return False
    return val


def parse_workflow(text: str, path: str = "<str>") -> dict:
    """Extract exactly what this gate reads, and refuse anything else.

    A GitHub workflow is not free-form YAML at these depths — the schema fixes
    the structure, so a line reader keyed on indentation is sufficient AND
    checkable. What it reads:

        on.pull_request.{branches,paths,paths-ignore}   (indent 0/2/4)
        jobs.<id>.{name,continue-on-error,timeout-minutes}  (indent 0/2/4)

    Everything deeper (steps, strategy, with:) is deliberately invisible — a
    step's `name:` sits at indent 8 and must not be mistaken for a job's.
    (`timeout-minutes` is read here purely so a sibling gate —
    check_job_timeouts.py — can reuse this reader; nothing in *this* gate looks
    at it, which is why cross_check does not compare it.)
    """
    doc: dict = {"on": None, "jobs": {}}
    section = None          # None | "on" | "jobs"
    trigger = None          # current key under `on:`
    job = None              # current key under `jobs:`
    listing = None          # (container, key) currently accumulating `- items`
    saw_jobs = False        # a `jobs:` key was seen at indent 0

    for lineno, raw in enumerate(text.splitlines(), 1):
        line = _strip_comment(raw)
        if not line.strip():
            continue

        item = _ITEM.match(line)
        if item and listing is not None and len(item.group("indent")) >= listing[2]:
            listing[0][listing[1]].append(_scalar(item.group("val")))
            continue

        m = _KEY.match(line)
        if not m:
            # Fail closed at exactly the positions a misread would change a
            # verdict, and nowhere else. Being stricter than this rejects
            # legitimate structure the gate provably never reads — `on.schedule`
            # is a list of mappings, and refusing to parse a nightly cron would
            # be a false alarm, not caution.
            ind = len(line) - len(line.lstrip(" "))
            reads_here = (ind == 2 and section in ("on", "jobs")) or (
                ind == 4 and section == "on" and trigger == "pull_request"
            )
            if reads_here:
                raise WorkflowSyntaxError(f"{path}:{lineno}: unrecognised where this gate reads: {line!r}")
            continue

        indent = len(m.group("indent"))
        key = _scalar(m.group("key")) if m.group("key")[0] in "'\"" else m.group("key")
        val = _scalar(m.group("val"))
        listing = None

        if indent == 0:
            # YAML 1.1 parses a bare `on:` as the boolean True; here it is text,
            # so the trap the reference parser springs does not arise — but the
            # key still has to be matched in both spellings.
            section = "on" if key in ("on", "'on'", '"on"', "True", "true") else ("jobs" if key == "jobs" else None)
            if section is not None:
                _reject_unsupported(val, path, lineno, f"`{key}:`")
            if section == "on":
                doc["on"] = {} if val is None else {t: {} for t in (val if isinstance(val, list) else [val])}
            if section == "jobs":
                saw_jobs = True
            trigger = job = None
            continue

        if indent == 2 and section == "on":
            _reject_unsupported(val, path, lineno, f"`on.{key}:`")
            trigger = key
            doc["on"][key] = {} if val is None else val
            continue

        if indent == 4 and section == "on" and trigger == "pull_request":
            if not isinstance(doc["on"].get("pull_request"), dict):
                raise WorkflowSyntaxError(f"{path}:{lineno}: options under a non-mapping pull_request")
            _reject_unsupported(val, path, lineno, f"`on.pull_request.{key}:`")
            if val is None:
                doc["on"]["pull_request"][key] = []
                listing = (doc["on"]["pull_request"], key, 6)
            else:
                doc["on"]["pull_request"][key] = val
            continue

        if indent == 2 and section == "jobs":
            _reject_unsupported(val, path, lineno, f"`jobs.{key}:`")
            job = key
            doc["jobs"][key] = {}
            continue

        if indent == 4 and section == "jobs" and job is not None:
            if key in ("name", "continue-on-error", "timeout-minutes"):
                _reject_unsupported(val, path, lineno, f"`jobs.{job}.{key}:`")
                doc["jobs"][job][key] = val
            continue

    # A `jobs:` block that yielded nothing is not an empty workflow — it is a
    # workflow this reader failed to see into, and the difference matters because
    # the two render identically downstream: zero jobs means zero violations
    # means PASS. That is fail-OPEN, and it is reachable without anything exotic
    # — a file indented with four spaces puts every job id at indent 4, where the
    # reader is not looking and the indent-2 fail-closed check never fires.
    # Actions rejects a jobs-less workflow anyway, so this cannot false-alarm on
    # a file that CI would actually run.
    if saw_jobs and not doc["jobs"]:
        raise WorkflowSyntaxError(
            f"{path}: a `jobs:` block is present but no job was recognised at "
            f"indent 2. This reader requires 2-space indentation; a workflow it "
            f"cannot see into would silently report zero violations."
        )

    return doc


def _on(doc: dict):
    """Return the workflow's trigger block."""
    return doc.get("on")


def classify_workflow(doc: dict) -> tuple[bool, list[str]]:
    """(does it trigger on PRs against the protected branch, disqualifying reasons)."""
    on = _on(doc)
    if not isinstance(on, dict):
        return False, []
    if "pull_request" not in on:
        return False, []
    pr = on["pull_request"] or {}
    if not isinstance(pr, dict):
        pr = {}

    branches = pr.get("branches")
    if branches and not any(fnmatch.fnmatch(PROTECTED_BRANCH, b) for b in branches):
        return False, []

    reasons = []
    if pr.get("paths"):
        reasons.append(
            "workflow-level `on.pull_request.paths` — a non-triggered workflow "
            "reports nothing, so requiring this context deadlocks every PR that "
            "misses those paths. Convert to always-run + a job-level `if:` first; "
            "an if:-skipped job reports \"skipped\", which SATISFIES a required context"
        )
    if pr.get("paths-ignore"):
        reasons.append(
            "workflow-level `on.pull_request.paths-ignore` — same deadlock as "
            "`paths`; move the filter to a job-level `if:`"
        )
    return True, reasons


def classify_jobs(doc: dict) -> dict[str, tuple[str, str, list[str]]]:
    """job key -> (bucket, rendered context name, reasons)."""
    is_pr, wf_reasons = classify_workflow(doc)
    out: dict[str, tuple[str, str, list[str]]] = {}

    for key, job in (doc.get("jobs") or {}).items():
        job = job or {}
        name = job.get("name", key)
        if not is_pr:
            out[key] = (NOT_PR, name, ["workflow does not run on pull_request against " + PROTECTED_BRANCH])
            continue

        reasons = list(wf_reasons)

        # `continue-on-error` is checked before the workflow-level reasons are
        # acted on because the remedy differs: an advisory job should be marked,
        # a path-scoped job should be restructured. Both are "must not be
        # required", for unrelated causes.
        if job.get("continue-on-error") is True:
            out[key] = (ADVISORY, name, ["`continue-on-error: true` — reports success regardless of outcome"] + reasons)
            continue

        if "${{" in str(name):
            reasons.append(
                "name is templated (matrix or expression) — it renders to N "
                "contexts at runtime, none equal to this literal, so no static "
                "string can be required"
            )

        out[key] = ((UNDELIVERABLE if reasons else GATEABLE), name, reasons)

    return out


def scan(workflow_glob: str = WORKFLOW_GLOB) -> dict[str, tuple[str, str, str, list[str]]]:
    """context name -> (bucket, workflow path, job key, reasons)."""
    found: dict[str, tuple[str, str, str, list[str]]] = {}
    for path in sorted(glob.glob(workflow_glob)):
        with open(path) as fh:
            doc = parse_workflow(fh.read(), path)
        for key, (bucket, name, reasons) in classify_jobs(doc).items():
            if bucket == NOT_PR:
                continue
            found[name] = (bucket, path, key, reasons)
    return found


def read_list(path: str) -> list[str]:
    out = []
    with open(path) as fh:
        for line in fh:
            # Same comment rule as the workflow reader: `#` only opens a comment
            # at line start or after whitespace. A naive split makes
            # `--print > required-contexts.txt` a lossy round-trip for any
            # context whose name contains `#` (`Test #1` -> `Test`), which the
            # parser would then report as a stale entry AND a missing job.
            line = _strip_comment(line).strip()
            if line:
                out.append(line)
    return out


def run_check(workflow_glob: str, list_path: str, out=None) -> int:
    out = out or sys.stdout
    found = scan(workflow_glob)
    required = read_list(list_path)

    gateable = {n for n, (b, *_) in found.items() if b == GATEABLE}
    errors: list[str] = []

    # 1. Every gateable job must be required. This is the miss that leaves the
    #    gate one job short while looking full.
    for name in sorted(gateable - set(required)):
        _, path, key, _ = found[name]
        errors.append(
            f"{path}: job `{key}` produces context {name!r}, which runs on every "
            f"PR and cannot fail open — but it is not in {list_path}. Either "
            f"require it, or give it `continue-on-error` and mark it advisory."
        )

    # 2. Every required context must be produced by a gateable job. Catches both
    #    stale entries (renamed/deleted jobs -> permanently "Expected") and
    #    entries pointing at jobs that cannot deliver.
    for name in required:
        if name in gateable:
            continue
        if name not in found:
            errors.append(
                f"{list_path}: required context {name!r} is not produced by any "
                f"job in any PR-triggered workflow. A required context nothing "
                f"reports sits at \"Expected — waiting for status\" forever."
            )
            continue
        bucket, path, key, reasons = found[name]
        why = "; ".join(reasons) or "unknown"
        errors.append(
            f"{list_path}: required context {name!r} ({path}, job `{key}`) is "
            f"{bucket} and must not be required: {why}"
        )

    # 3. An advisory job must say so in its name. `continue-on-error` makes the
    #    tick unconditional; the name is the only place a reader can learn that.
    for name, (bucket, path, key, _) in sorted(found.items()):
        if bucket == ADVISORY and ADVISORY_MARKER not in name.lower():
            errors.append(
                f"{path}: job `{key}` is `continue-on-error` but its name "
                f"{name!r} does not say so. It renders a green tick in the PR UI "
                f"whatever happened; the name is the only signal that the tick "
                f"is not a verdict. Add \"({ADVISORY_MARKER})\"."
            )

    counts = {b: sum(1 for v in found.values() if v[0] == b) for b in (GATEABLE, ADVISORY, UNDELIVERABLE)}
    print(
        f"scanned {len(found)} PR-triggered job(s): "
        f"{counts[GATEABLE]} gateable, {counts[ADVISORY]} advisory, "
        f"{counts[UNDELIVERABLE]} undeliverable; {len(required)} required context(s)",
        file=out,
    )

    if errors:
        for e in errors:
            print(f"::error::{e}", file=out)
        print(f"\nFAIL: {len(errors)} required-context violation(s).", file=out)
        return 1

    print("PASS: the committed required set is exactly the gateable set.", file=out)
    return 0


def cross_check(workflow_glob: str) -> int:
    """Agree with PyYAML on the real corpus, or say exactly where we differ.

    The restricted reader above is the price of not adding a package dependency
    to a required check. That price is only worth paying if the reader is
    *right*, and "it passed on the repo that already passes" does not show that.
    So where a reference parser is available — a developer machine, not CI — the
    two are run side by side over every real workflow file and every field this
    gate actually reads.
    """
    try:
        import yaml  # noqa: PLC0415 - deliberately optional, absent in CI
    except ImportError:
        print("::error::--cross-check needs PyYAML. It is deliberately NOT a")
        print("::error::dependency of the gate itself; install it locally to run this.")
        return 2

    diffs = 0
    files = sorted(glob.glob(workflow_glob))
    for path in files:
        text = open(path).read()
        mine = parse_workflow(text, path)
        ref = yaml.safe_load(text) or {}
        ref_on = ref.get(True, ref.get("on"))

        def norm_pr(on):
            if not isinstance(on, dict) or "pull_request" not in on:
                return None
            pr = on["pull_request"] or {}
            pr = pr if isinstance(pr, dict) else {}
            return {k: pr.get(k) for k in ("branches", "paths", "paths-ignore")}

        if norm_pr(mine["on"]) != norm_pr(ref_on):
            print(f"::error::{path}: pull_request trigger differs — mine={norm_pr(mine['on'])!r} ref={norm_pr(ref_on)!r}")
            diffs += 1

        ref_jobs = ref.get("jobs") or {}
        if set(mine["jobs"]) != set(ref_jobs):
            print(f"::error::{path}: job id set differs — {set(mine['jobs']) ^ set(ref_jobs)}")
            diffs += 1
            continue
        for jid, rj in ref_jobs.items():
            rj = rj or {}
            for field in ("name", "continue-on-error"):
                if mine["jobs"][jid].get(field) != rj.get(field):
                    print(
                        f"::error::{path}: job `{jid}` field {field!r} differs — "
                        f"mine={mine['jobs'][jid].get(field)!r} ref={rj.get(field)!r}"
                    )
                    diffs += 1

    if diffs:
        print(f"\nFAIL: {diffs} disagreement(s) with PyYAML across {len(files)} workflow(s).")
        return 1
    print(f"PASS: the restricted reader agrees with PyYAML on every field it reads, across {len(files)} workflow(s).")
    return 0


def against_api(list_path: str, repo: str) -> int:
    """Diff the committed list against live branch protection. Needs an admin token."""
    cmd = ["gh", "api", f"repos/{repo}/branches/{PROTECTED_BRANCH}/protection/required_status_checks"]
    try:
        raw = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    except FileNotFoundError:
        print("::error::`gh` not found; --against-api needs the GitHub CLI.")
        return 2
    except subprocess.CalledProcessError as exc:
        print(
            "::error::could not read branch protection. This needs an ADMIN "
            "token — the Actions GITHUB_TOKEN is not one, which is why the "
            f"committed list exists at all. gh said: {exc.stderr.strip()}"
        )
        return 2

    live = set(json.loads(raw).get("contexts") or [])
    want = set(read_list(list_path))
    if live == want:
        print(f"PASS: branch protection on {PROTECTED_BRANCH} matches {list_path} ({len(want)} contexts).")
        return 0
    for name in sorted(want - live):
        print(f"::error::{name!r} is in {list_path} but NOT enforced on {PROTECTED_BRANCH}.")
    for name in sorted(live - want):
        print(f"::error::{name!r} is enforced on {PROTECTED_BRANCH} but NOT in {list_path}.")
    return 1


# ── self-test ─────────────────────────────────────────────────────────────
#
# These run on every CI invocation via --self-test. A guardrail exercised only
# against a repo that already satisfies it is indistinguishable from one that
# returns 0 unconditionally: the live repo is a single input, and a single
# input cannot demonstrate that distinct inputs give distinct verdicts.
#
# Each fixture kills exactly one guard. Deleting any guard reds this list.

_CLEAN = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
"""

_ADVISORY_UNMARKED = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
  b:
    name: Beta
    continue-on-error: true
    runs-on: ubuntu-latest
"""

_ADVISORY_MARKED = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
  b:
    name: Beta (advisory)
    continue-on-error: true
    runs-on: ubuntu-latest
"""

_PATH_SCOPED = """
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

_PATHS_IGNORE_SCOPED = """
name: W
on:
  pull_request:
    paths-ignore:
      - "docs/**"
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
"""

#: `pull_request` as a flow mapping. Valid YAML, valid Actions, and the reader
#: sees the value as an opaque string — so the `paths` filter vanishes and the
#: job is ruled GATEABLE. Must be refused, not guessed at.
_FLOW_MAPPING = """
name: W
on:
  pull_request: {branches: [main], paths: ["src/**"]}
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
"""

#: Four-space indentation throughout. Every job id lands at indent 4, where the
#: reader is not looking — and the indent-2 fail-closed check never fires. Before
#: the `saw_jobs` check this parsed to zero jobs and PASSed.
_FOUR_SPACE = """
name: W
on:
    pull_request:
        branches: [main]
jobs:
    a:
        name: Alpha
        runs-on: ubuntu-latest
"""

#: A folded block scalar as a job name. The reader would take the literal `>-`
#: as the context name and demand that be required, while the real context
#: ("Folded Name") goes unrequired — failure mode 1, caused by the gate.
_BLOCK_SCALAR_NAME = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: >-
      Folded Name
    runs-on: ubuntu-latest
"""

_TEMPLATED = """
name: W
on:
  pull_request:
    branches: [main]
jobs:
  a:
    name: Alpha ${{ matrix.target }}
    strategy:
      matrix:
        target: [x, y]
    runs-on: ubuntu-latest
"""

_WRONG_BRANCH = """
name: W
on:
  pull_request:
    branches: [release/*]
jobs:
  a:
    name: Alpha
    runs-on: ubuntu-latest
"""

SELF_TESTS = [
    (
        "clean: one gateable job, required",
        _CLEAN, ["Alpha"], 0,
        "the checker can return 0 at all — without this the other cases are "
        "consistent with a script that always fails",
    ),
    (
        "gateable job missing from the list",
        _CLEAN, [], 1,
        "guard 1: a job that runs on every PR and cannot fail open is not "
        "silently left out of the gate (the osate-corpus shape)",
    ),
    (
        "stale entry: required context no job produces",
        _CLEAN, ["Alpha", "Ghost"], 1,
        "guard 2a: a renamed or deleted job leaves a context stuck at "
        "\"Expected — waiting for status\" forever",
    ),
    (
        "advisory job required",
        _ADVISORY_MARKED, ["Alpha", "Beta (advisory)"], 1,
        "guard 2b: requiring a continue-on-error job enforces nothing — it "
        "reports success whatever happened",
    ),
    (
        "advisory job not marked in its name",
        _ADVISORY_UNMARKED, ["Alpha"], 1,
        "guard 3: an unmarked advisory job renders a green tick a reader takes "
        "as a verdict",
    ),
    (
        "advisory job marked and not required",
        _ADVISORY_MARKED, ["Alpha"], 0,
        "guard 3 accepts the remedy it demands, rather than being unsatisfiable",
    ),
    (
        "path-scoped workflow required",
        _PATH_SCOPED, ["Alpha"], 1,
        "THE DEADLOCK TRAP: a non-triggered workflow reports nothing, so this "
        "context never arrives and every PR missing those paths is unmergeable",
    ),
    (
        "path-scoped workflow NOT required",
        _PATH_SCOPED, [], 0,
        "the trap is not 'fixed' by demanding the context — guard 1 must not "
        "fire here, which is precisely what a mechanical 'every job must be "
        "required' rule would get wrong",
    ),
    (
        "paths-ignore-scoped workflow required",
        _PATHS_IGNORE_SCOPED, ["Alpha"], 1,
        "`paths-ignore` deadlocks identically to `paths`. Without this case the "
        "paths-ignore branch survives every other fixture — deleting it reds "
        "nothing, which is what a guard nothing exercises is worth",
    ),
    (
        "flow-mapping pull_request refused",
        _FLOW_MAPPING, ["Alpha"], 2,
        "the reader cannot see a flow mapping's `paths`, so it would rule the "
        "job GATEABLE and order you to require an undeliverable context — the "
        "gate handing you the deadlock it exists to prevent. Refusal, not a guess",
    ),
    (
        "four-space indentation refused",
        _FOUR_SPACE, [], 2,
        "fail-OPEN, the one direction that must not exist: jobs at indent 4 are "
        "invisible, zero jobs means zero violations means PASS on a file the "
        "reader never read",
    ),
    (
        "block-scalar job name refused",
        _BLOCK_SCALAR_NAME, ["Folded Name"], 2,
        "a folded name reads as the literal '>-'; requiring that string leaves "
        "the real context ungated",
    ),
    (
        "templated matrix name required",
        _TEMPLATED, ["Alpha ${{ matrix.target }}"], 1,
        "a matrix name renders to N runtime contexts, none equal to the literal",
    ),
    (
        "workflow scoped to a different branch",
        _WRONG_BRANCH, [], 0,
        "a workflow that never runs against main is out of scope, not a miss",
    ),
]


def self_test() -> int:
    import tempfile

    failures = 0
    for name, wf_text, required, want, proves in SELF_TESTS:
        with tempfile.TemporaryDirectory() as tmp:
            wf_dir = os.path.join(tmp, "wf")
            os.makedirs(wf_dir)
            # Deliberately `.yaml`, the extension the original `*.yml` glob could
            # not see. Every fixture therefore also exercises discovery: narrow
            # the glob back and all of them go red at once, loudly. `.yml` needs
            # no fixture — all ten real workflows use it, so the live run covers
            # that leg on every PR.
            with open(os.path.join(wf_dir, "w.yaml"), "w") as fh:
                fh.write(wf_text)
            list_path = os.path.join(tmp, "required.txt")
            with open(list_path, "w") as fh:
                fh.write("\n".join(required) + "\n")

            import io

            buf = io.StringIO()
            # `.y*ml`, matching the real glob — a fixture written as `.yaml`
            # must be discovered, since Actions loads both extensions.
            try:
                got = run_check(os.path.join(wf_dir, "*.y*ml"), list_path, out=buf)
            except WorkflowSyntaxError as exc:
                # 2 == "refused to guess", the fail-closed exit main() renders.
                # Distinct from 1 (a real violation) on purpose: one says the
                # gate found a problem, the other says the gate cannot see.
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

    # The fixtures above run against a glob this function builds, so they cannot
    # see WORKFLOW_GLOB itself — narrowing that constant back to `*.yml` would
    # leave every fixture green while the production run stopped reading half the
    # directory. Assert on the constant directly.
    if not fnmatch.fnmatch(".github/workflows/x.yaml", WORKFLOW_GLOB):
        print(f"  FAIL WORKFLOW_GLOB does not match `.yaml`: {WORKFLOW_GLOB!r}")
        print("       proves: Actions loads .yml AND .yaml; a glob that misses "
              "one reports PASS on a directory it never fully read")
        failures += 1
    else:
        print("  ok   WORKFLOW_GLOB matches both .yml and .yaml")
        print("       proves: no workflow file is invisible to the scan by extension")

    total = len(SELF_TESTS) + 1
    if failures:
        print(f"\n{failures} of {total} self-test(s) FAILED.")
        return 1
    print(f"\n{total} self-test(s) passed.")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--workflows", default=WORKFLOW_GLOB)
    p.add_argument("--list", dest="list_path", default=LIST_PATH)
    p.add_argument("--self-test", action="store_true")
    p.add_argument("--print", dest="do_print", action="store_true")
    p.add_argument("--against-api", action="store_true")
    p.add_argument("--cross-check", action="store_true")
    p.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "pulseengine/spar"))
    args = p.parse_args()

    if args.self_test:
        return self_test()

    if args.cross_check:
        return cross_check(args.workflows)

    if args.do_print:
        for name, (bucket, *_rest) in sorted(scan(args.workflows).items()):
            if bucket == GATEABLE:
                print(name)
        return 0

    if args.against_api:
        return against_api(args.list_path, args.repo)

    return run_check(args.workflows, args.list_path)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except WorkflowSyntaxError as exc:
        # Fail closed. A workflow shape the reader does not fully recognise is
        # a workflow whose jobs it cannot classify, and an unclassifiable job is
        # exactly the one that would slip through unrequired.
        print(f"::error::{exc}", file=sys.stderr)
        print(
            "::error::The workflow reader is deliberately restricted (see the "
            "module docstring) and stops rather than guessing. Either simplify "
            "the construct or extend the reader — and re-run --cross-check.",
            file=sys.stderr,
        )
        raise SystemExit(2)
