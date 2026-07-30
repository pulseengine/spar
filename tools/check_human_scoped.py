#!/usr/bin/env python3
"""Fail if a `human-scoped` rivet artifact is recorded as done.

An artifact tagged `human-scoped` is one whose *scoping* needs a human: first
wiring of an external dependency, narrowing a research-grade property to
something a solver can actually decide, or hand-designing a falsification
kill-criterion. The autonomous issue-hunt cron must not carry such an artifact
to completion on its own — it will over-scope the spike or wing the
integration.

This check makes that boundary mechanical. A `human-scoped` artifact may sit at
`draft` / `proposed` / `approved` (planning is fine) and may be `rejected` /
`deprecated` / `withdrawn` (retracting is fine), but it may NOT be recorded as
`implemented`, `verified`, `released`, `accepted`, or `passing`.

To ship one, remove the `human-scoped` tag **in the same change**. The removal
is then a single explicit line in the diff instead of a status promotion buried
in a large PR.

WHY THIS EXISTS
    v0.35.0's REQ-CODEGEN-LAYOUT-CERT-001 was picked up by the autonomous cron,
    implemented, self-recorded `status: verified`, and merged in about an hour —
    asserting a proof that did not exist (the SMT goal constant-folded, so every
    input produced a byte-identical certificate). The requirement had been
    designated human-scoped in a planning note, and nothing anywhere enforced
    it. A guardrail that lives only in prose is not a guardrail.

LIMITATION — READ THIS BEFORE TRUSTING IT
    This is a speed bump, not an authorization boundary. `main` requires zero
    approving reviews, and that is forced rather than sloppy: with
    `enforce_admins: true` on a single-maintainer repo, GitHub's ban on
    self-approval would make any required review an unbreakable deadlock. Nor
    can commit signatures discriminate, since squash-merges are re-signed by
    GitHub's own key.

    So an autonomous agent with push access can delete the tag in the same PR
    and green this check. What the check buys is that doing so becomes
    explicit, diffable, and logged rather than invisible. The only real gate is
    on the agent's own prompt and permissions, which lives outside this repo.

WHY NO PyYAML — AND WHY THE PARSER REFUSES WHAT IT CANNOT READ
    No workflow in this repo installs a Python package, and every other
    `tools/*.py` is stdlib-only; `python3` itself is proven present because
    `verification-gate.yml` invokes `run_verification.py` by shebang. Importing
    `yaml` would make a *required* check depend on a runner package nobody has
    verified — and a guardrail that cannot run is worse than none, because its
    absence reads as approval.

    So this parses the narrow subset the artifact files actually use (measured:
    records at one list indent, scalar keys one level in, `tags:` exclusively as
    an inline flow sequence — 354 occurrences, zero block-form, zero quoted
    entries). Crucially it is **fail-closed**: anything outside that subset —
    block-form `tags:`, a file that yields no records — exits 2 rather than
    quietly scanning past it. A best-effort scanner would degrade to a false
    green, which is precisely the vacuity that let #354 through.

Usage:
    tools/check_human_scoped.py [--artifacts-dir DIR ...] [--tag TAG]

Exit code:
    0  no `human-scoped` artifact claims a done status
    1  at least one does (each is listed on stderr)
    2  INCONCLUSIVE — the input could not be read, or uses syntax this parser
       does not fully understand. Deliberately distinct from 1: "violated" and
       "could not check" are different facts and must not be conflated.
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys

#: Statuses that assert the work is finished. A `human-scoped` artifact reaching
#: any of these is the exact event this check exists to surface.
#:
#: `passing` is included because it is this repo's convention for a `type:
#: feature` verification artifact and is accepted by the rivet pinned in CI
#: (v0.4.3), even though newer rivet releases reject it. Leaving it out would
#: let a human-scoped *test* artifact be marked green without challenge.
DONE_STATUSES = frozenset(
    {"implemented", "verified", "released", "accepted", "passing"}
)

DEFAULT_TAG = "human-scoped"

#: A sequence entry: leading indent, then `- `. The *shallowest* such indent in a
#: file is the artifact-list level; anything deeper is a nested sequence (e.g. a
#: verification artifact's `fields.steps`).
_ITEM_RE = re.compile(r"^(?P<indent> *)-(?: +(?P<rest>\S.*))?$")

#: A scalar mapping key. Values are taken verbatim; comments are stripped only
#: for the two keys this check reads, both of which are comment-free in practice.
_KEY_RE = re.compile(r"^(?P<indent> *)(?P<key>[A-Za-z_][A-Za-z0-9_-]*): *(?P<val>.*)$")


class Unsupported(Exception):
    """Input uses YAML this parser does not fully handle — never guess past it."""


def _strip_scalar(raw: str) -> str:
    """Strip a trailing comment and surrounding quotes from a scalar value."""
    val = raw.strip()
    if val and not val[0] in "\"'":
        val = val.split(" #", 1)[0].rstrip()
    if len(val) >= 2 and val[0] == val[-1] and val[0] in "\"'":
        val = val[1:-1]
    return val


def _parse_tags(raw: str, where: str) -> list[str]:
    """Parse an inline flow sequence: `[a, b, c]`.

    Anything else — most importantly an empty value, which means a block-form
    sequence follows on subsequent lines — is refused rather than read as "no
    tags", because reading it as "no tags" would silently exempt the artifact.
    """
    val = raw.strip()
    if not val:
        raise Unsupported(
            f"{where}: block-form `tags:` is not supported by this parser. "
            f"Rewrite as an inline list (`tags: [a, b]`) or teach the parser."
        )
    if not (val.startswith("[") and val.endswith("]")):
        raise Unsupported(f"{where}: cannot read `tags: {val}` as an inline list.")
    return [t for t in (_strip_scalar(p) for p in val[1:-1].split(",")) if t]


def parse_file(path: str) -> list[dict]:
    """Return one dict per artifact record in *path*.

    Only keys at the record's own indent are collected, so nested maps (a
    verification artifact's `fields:` block) cannot shadow a record's `status`.
    Block scalars are skipped for free: their continuation lines are indented
    deeper than the key that introduces them.
    """
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()

    indents = [
        m.group("indent") for m in (_ITEM_RE.match(ln) for ln in lines) if m
    ]
    if not indents:
        raise Unsupported(f"{path}: no sequence entries found — is this artifact YAML?")
    item_indent = min(len(i) for i in indents)
    key_indent = item_indent + 2

    records: list[dict] = []
    current: dict | None = None
    for lineno, line in enumerate(lines, 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        where = f"{os.path.basename(path)}:{lineno}"

        item = _ITEM_RE.match(line)
        if item and len(item.group("indent")) == item_indent:
            current = {"_source": os.path.basename(path), "_line": lineno}
            records.append(current)
            inline = item.group("rest")
            if not inline:
                continue
            # `- id: FOO` — the first key sits on the item line itself.
            kv = _KEY_RE.match(inline)
            if kv:
                _absorb(current, kv.group("key"), kv.group("val"), where)
            continue

        if current is None:
            continue
        kv = _KEY_RE.match(line)
        if kv and len(kv.group("indent")) == key_indent:
            _absorb(current, kv.group("key"), kv.group("val"), where)

    if not records:
        raise Unsupported(f"{path}: parsed zero artifacts.")
    return records


def _absorb(record: dict, key: str, raw: str, where: str) -> None:
    if key == "id":
        record["id"] = _strip_scalar(raw)
    elif key == "status":
        record["status"] = _strip_scalar(raw)
    elif key == "tags":
        record["tags"] = _parse_tags(raw, where)


def load_artifacts(dirs: list[str]) -> list[dict]:
    """Return every artifact record found under each of *dirs*."""
    found: list[dict] = []
    for artifacts_dir in dirs:
        paths = sorted(glob.glob(os.path.join(artifacts_dir, "*.yaml")))
        if not paths:
            raise Unsupported(f"no *.yaml under {artifacts_dir!r}")
        for path in paths:
            found.extend(parse_file(path))
    return found


def label(art: dict) -> str:
    """A name for the artifact even when it has no `id` — never drop a record."""
    return art.get("id") or f"<unnamed at {art['_source']}:{art['_line']}>"


def violations(artifacts: list[dict], tag: str) -> list[tuple[str, str, str]]:
    """Return (label, status, source) for each tagged artifact claiming done."""
    out = []
    for art in artifacts:
        if tag not in (art.get("tags") or []):
            continue
        if str(art.get("status", "")).strip().lower() in DONE_STATUSES:
            out.append((label(art), art["status"], art["_source"]))
    return out


def run_check(dirs: list[str], tag: str, out=None, err=None) -> int:
    """The whole check. Returns the process exit code (see module docstring)."""
    out = out or sys.stdout
    err = err or sys.stderr

    try:
        artifacts = load_artifacts(dirs)
    except (OSError, Unsupported) as exc:
        print(f"INCONCLUSIVE: {exc}", file=err)
        print(
            "This check could not read its input, which is NOT the same as the "
            "property holding. Treat it as red.",
            file=err,
        )
        return 2

    tagged = [a for a in artifacts if tag in (a.get("tags") or [])]
    bad = violations(artifacts, tag)
    bad_labels = {b[0] for b in bad}

    print("== human-scoped guardrail ==", file=out)
    print(
        f"artifacts scanned: {len(artifacts)} from "
        f"{', '.join(d + '/' for d in dirs)}",
        file=out,
    )
    print(f"tagged {tag!r}: {len(tagged)}", file=out)
    for art in tagged:
        marker = "VIOLATION" if label(art) in bad_labels else "ok"
        print(f"  [{marker:9s}] {label(art)} status={art.get('status')}", file=out)

    if not tagged:
        # A guardrail with nothing to guard passes trivially, which is how a
        # check silently becomes decorative. Say so out loud rather than
        # printing a reassuring nothing.
        print(
            f"WARNING: no artifact is tagged {tag!r}, so this check "
            f"asserted nothing.",
            file=err,
        )

    if bad:
        print(file=err)
        print(
            f"error: {len(bad)} {tag!r} artifact(s) record a completed "
            f"status. Such an artifact needs a human to scope it.",
            file=err,
        )
        for art_label, status, source in bad:
            print(f"  {art_label}: status={status}  ({source})", file=err)
        print(file=err)
        print(
            f"If a human really did scope this, remove the {tag!r} tag in "
            f"this same change so the promotion is one explicit line of diff.",
            file=err,
        )
        return 1

    return 0


#: (fixture directory, required exit code, what it proves).
#:
#: These run on every CI invocation via --self-test. A guardrail exercised only
#: against compliant input cannot distinguish "the property holds" from "the
#: check does nothing" — the distinction #354 turned on. Asserting the codes here
#: means a refactor that makes the checker toothless turns the gate red.
SELF_TESTS = [
    (
        "human-scoped-violation",
        1,
        "a human-scoped artifact at status=verified is reported, "
        "while an otherwise-identical one at status=proposed is not "
        "(so the check discriminates on status, not merely on the tag)",
    ),
    (
        "human-scoped-unsupported",
        2,
        "block-form `tags:` is refused as INCONCLUSIVE rather than read as "
        '"no tags", which would silently exempt the artifact and exit 0',
    ),
    (
        "human-scoped-duplicate-id",
        1,
        "a duplicate id cannot shadow a violation — the violating copy is "
        "reported even though a compliant copy of the same id follows it, so "
        "deduplicating by id (last-write-wins) would break this",
    ),
]


def self_test() -> int:
    """Assert the checker still fails on input it must fail on."""
    fixtures = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")
    devnull = open(os.devnull, "w", encoding="utf-8")
    failures = 0
    for name, want, proves in SELF_TESTS:
        got = run_check([os.path.join(fixtures, name)], DEFAULT_TAG, devnull, devnull)
        ok = got == want
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] {name}: exit {got} (want {want})")
        print(f"       proves: {proves}")
    devnull.close()
    if failures:
        print(
            f"\nerror: {failures} self-test(s) failed — the guardrail is not "
            f"detecting what it claims to detect.",
            file=sys.stderr,
        )
        return 1
    print(f"\n{len(SELF_TESTS)} self-test(s) passed.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--artifacts-dir",
        action="append",
        dest="dirs",
        metavar="DIR",
        help="directory of rivet artifact YAML; repeatable (default: artifacts)",
    )
    parser.add_argument(
        "--tag",
        default=DEFAULT_TAG,
        help="tag marking human-scoped artifacts (default: %(default)s)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="verify the checker still rejects the fixtures, then exit",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    return run_check(args.dirs or ["artifacts"], args.tag)


if __name__ == "__main__":
    sys.exit(main())
