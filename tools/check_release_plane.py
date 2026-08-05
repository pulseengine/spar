#!/usr/bin/env python3
"""Fail if a `release:` assignment is nested where rivet cannot read it.

rivet has two field planes. `release` is a **core** field, read by the CLI's own
query surface — `rivet list --release vX.Y` is documented as *"the
release-planning view"*. Separately, each artifact may carry a `fields:` bag for
schema-declared custom keys (for `requirement` the declared set is `baseline`,
`category`, `cited-source`, `priority`, `upstream-ref`).

Writing `release:` into the bag is valid YAML, is tolerated by the schema, and is
completely invisible to the query:

    - id: REQ-A            - id: REQ-B
      status: proposed       status: proposed
      release: v0.36.0       fields:
                               release: v0.23.0
      ^ rivet sees this               ^ rivet does not

WHY THIS EXISTS
    59 of 85 release assignments were on the wrong plane (#370). `rivet list`
    reported 25 artifacts with a release and 856 without, and **eleven shipped
    releases** — v0.13.0 through v0.21.0, v0.31.0, v0.32.0 — returned zero
    artifacts each. Release readiness in this project is supposed to be a query
    ("what must we implement for vX.Y" = `release: vX.Y` + status != verified);
    that query was answering "nothing to do" for eleven releases' worth of scope.

    The failure renders identically to a genuinely empty release. That is the
    recurring shape here: an operation that can produce "nothing happened" must
    render differently from one that worked.

    rivet did emit `INFO: field 'release' is not defined in schema` for all 59 —
    correct, precise, and buried under 276 unrelated `ERROR:` lines from #371. A
    diagnostic nobody can see is not a diagnostic, which is why this is a gate.

WHY NOT JUST GREP FOR SIX SPACES
    Because `release:` also appears inside `description: >` prose, where deeper
    indentation is meaningless and a grep would red the build for a sentence.
    This parser tracks block scalars and skips their bodies. There is no such
    line in the tree today; the handling exists so that adding one is not a
    trap. See the `release-plane-block-scalar` self-test.

WHY NO PyYAML
    Same reason as check_human_scoped.py: no workflow here installs a Python
    package, so a required check must not depend on an unverified runner
    package. A guardrail that cannot run reads as approval. Stdlib only, and
    fail-closed — input this parser cannot fully read exits 2, not 0.

Usage:
    tools/check_release_plane.py [--artifacts-dir DIR ...]

Exit code:
    0  every `release:` assignment is on the artifact's own key plane
    1  at least one is nested where rivet cannot read it (each listed on stderr)
    2  INCONCLUSIVE — input unreadable, or syntax this parser does not fully
       understand. Deliberately distinct from 1: "violated" and "could not
       check" are different facts and must not be conflated.
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys

#: A sequence entry. The *shallowest* such indent in a file is the artifact-list
#: level; anything deeper is a nested sequence (e.g. `fields.steps`).
_ITEM_RE = re.compile(r"^(?P<indent> *)-(?: +(?P<rest>\S.*))?$")

#: A scalar mapping key.
_KEY_RE = re.compile(r"^(?P<indent> *)(?P<key>[A-Za-z_][A-Za-z0-9_-]*): *(?P<val>.*)$")

#: A block-scalar introducer: `>`, `|`, with optional chomping/indent modifiers
#: (`>-`, `|+`, `>2`) and nothing else on the line.
_BLOCK_RE = re.compile(r"^[>|][+-]?\d*$")

FIELD = "release"


class Unsupported(Exception):
    """Input uses YAML this parser does not fully handle — never guess past it."""


def scan_file(path: str) -> tuple[list[tuple[str, int, int]], int]:
    """Return (findings, visible_count) for *path*.

    A finding is (artifact_id, lineno, indent) for a `release:` key nested
    deeper than the artifact's own key plane. `visible_count` counts the ones
    correctly placed, so the caller can tell "all good" from "nothing to check".
    """
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()

    indents = [m.group("indent") for m in (_ITEM_RE.match(ln) for ln in lines) if m]
    if not indents:
        raise Unsupported(f"{path}: no sequence entries found — is this artifact YAML?")
    item_indent = min(len(i) for i in indents)
    key_indent = item_indent + 2

    findings: list[tuple[str, int, int]] = []
    visible = 0
    current_id: str | None = None
    seen_record = False
    #: While non-None, we are inside a block scalar introduced by a key at this
    #: indent; every line indented deeper is prose and must not be parsed.
    block_at: int | None = None

    for lineno, line in enumerate(lines, 1):
        if not line.strip():
            continue

        stripped_indent = len(line) - len(line.lstrip(" "))
        if block_at is not None:
            if stripped_indent > block_at:
                continue  # prose inside a block scalar
            block_at = None

        if line.lstrip().startswith("#"):
            continue

        item = _ITEM_RE.match(line)
        if item and len(item.group("indent")) == item_indent:
            seen_record = True
            current_id = None
            inline = item.group("rest")
            if inline:
                kv = _KEY_RE.match(inline)
                if kv and kv.group("key") == "id":
                    current_id = kv.group("val").strip()
            continue

        kv = _KEY_RE.match(line)
        if not kv:
            continue

        indent = len(kv.group("indent"))
        key = kv.group("key")
        val = kv.group("val").strip()

        if key == "id" and indent == key_indent:
            current_id = val

        if _BLOCK_RE.match(val):
            block_at = indent
            continue

        if key == FIELD and seen_record:
            if indent == key_indent:
                visible += 1
            elif indent > key_indent:
                findings.append((current_id or f"<unnamed>", lineno, indent))

    if not seen_record:
        raise Unsupported(f"{path}: parsed zero artifacts.")
    return findings, visible


def collect(dirs: list[str]) -> tuple[list[tuple[str, str, int, int]], int, int]:
    """Return (findings, visible_count, files_scanned) across *dirs*.

    Split out from run_check so the self-test can assert on the finding SET
    rather than on the process's single-integer summary. `exit 1` is necessary
    for "reported the nested one" but not sufficient — it is equally consistent
    with reporting every `release:` in the file, which is the specific bug the
    violation fixture exists to rule out.
    """
    all_findings: list[tuple[str, str, int, int]] = []
    total_visible = 0
    scanned = 0
    for artifacts_dir in dirs:
        paths = sorted(glob.glob(os.path.join(artifacts_dir, "*.yaml")))
        if not paths:
            raise Unsupported(f"no *.yaml under {artifacts_dir!r}")
        for path in paths:
            findings, visible = scan_file(path)
            scanned += 1
            total_visible += visible
            base = os.path.basename(path)
            all_findings.extend((base, i, ln, ind) for i, ln, ind in findings)
    return all_findings, total_visible, scanned


def run_check(dirs: list[str], out=None, err=None) -> int:
    """The whole check. Returns the process exit code (see module docstring)."""
    out = out or sys.stdout
    err = err or sys.stderr

    try:
        all_findings, total_visible, scanned = collect(dirs)
    except (OSError, Unsupported) as exc:
        print(f"INCONCLUSIVE: {exc}", file=err)
        print(
            "This check could not read its input, which is NOT the same as the "
            "property holding. Treat it as red.",
            file=err,
        )
        return 2

    print("== release-plane guardrail ==", file=out)
    print(f"files scanned: {scanned} from {', '.join(d + '/' for d in dirs)}", file=out)
    print(f"`{FIELD}:` on the artifact key plane (rivet sees these): {total_visible}", file=out)
    print(f"`{FIELD}:` nested deeper (rivet is blind to these):     {len(all_findings)}", file=out)

    if total_visible == 0 and not all_findings:
        # A guardrail with nothing to guard passes trivially, which is how a
        # check silently becomes decorative. Say so rather than printing a
        # reassuring nothing.
        print(
            f"WARNING: no `{FIELD}:` assignment found anywhere, so this check "
            f"asserted nothing.",
            file=err,
        )

    if all_findings:
        print(file=err)
        print(
            f"error: {len(all_findings)} `{FIELD}:` assignment(s) are nested "
            f"below the artifact's own keys, where rivet cannot read them.",
            file=err,
        )
        for source, art_id, lineno, indent in all_findings:
            print(f"  {art_id}: {source}:{lineno} (indent {indent})", file=err)
        print(file=err)
        print(
            f"Move `{FIELD}:` to the artifact's own key level — a sibling of "
            f"`status:` and `tags:`, not a member of `fields:`. Verify with "
            f"`rivet list --release <version>`: it must return the artifact.",
            file=err,
        )
        return 1

    return 0


#: (fixture directory, required exit code, required nested count, required
#: visible count, what it proves). `None` for a count means "not asserted"
#: — used only where collect() raises and there are no counts to assert.
#:
#: These run on every CI invocation via --self-test. A guardrail exercised only
#: against compliant input cannot distinguish "the property holds" from "the
#: check does nothing".
#:
#: The counts are asserted, not just the exit code, because the exit code is
#: too coarse to carry the claim. `exit 1` on the violation fixture is equally
#: consistent with a checker that reports EVERY `release:` it sees — which
#: would be a checker that does not discriminate on plane at all, i.e. exactly
#: the regression this fixture is here to catch. (nested=1, visible=1) is the
#: assertion that actually means what the `proves` text says.
SELF_TESTS = [
    (
        "release-plane-violation",
        1,
        1,
        1,
        "a `fields:`-nested release is reported, while a top-level one in the "
        "same file is NOT (nested=1, visible=1, from two artifacts differing "
        "only in indentation) — so the check discriminates on PLANE, not "
        "merely on the presence of a `release:` key",
    ),
    (
        "release-plane-block-scalar",
        0,
        0,
        1,
        "`release:` appearing inside a `description: >` block scalar is prose, "
        "not an assignment, and does not trip the check — a plain indentation "
        "grep would red the build here. visible=1 pins that the file was still "
        "genuinely parsed, so exit 0 is not the silence of a dead parser",
    ),
    (
        "release-plane-unsupported",
        2,
        None,
        None,
        "an artifact layout the parser cannot read is refused as INCONCLUSIVE "
        "rather than passing vacuously — note the fixture's only `release:` is "
        "fields-nested, so a checker without the guard would exit 0, not 1",
    ),
    (
        "release-plane-no-artifacts",
        2,
        None,
        None,
        "the SECOND fail-closed guard fires: sequence syntax was found, an "
        "artifact plane was inferred from it, and zero artifacts parsed at "
        "that plane. Added because a mutation test showed deleting this raise "
        "left the other three self-tests green",
    ),
]


def self_test() -> int:
    """Assert the checker still fails on input it must fail on."""
    fixtures = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")
    devnull = open(os.devnull, "w", encoding="utf-8")
    failures = 0
    for name, want, want_nested, want_visible, proves in SELF_TESTS:
        path = os.path.join(fixtures, name)
        got = run_check([path], devnull, devnull)
        detail = f"exit {got} (want {want})"
        ok = got == want
        if want_nested is not None:
            try:
                findings, visible, _ = collect([path])
                nested = len(findings)
            except (OSError, Unsupported):
                nested, visible = None, None
            ok = ok and nested == want_nested and visible == want_visible
            detail += (
                f", nested {nested} (want {want_nested})"
                f", visible {visible} (want {want_visible})"
            )
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] {name}: {detail}")
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
        "--self-test",
        action="store_true",
        help="verify the checker still rejects the fixtures, then exit",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    return run_check(args.dirs or ["artifacts"])


if __name__ == "__main__":
    sys.exit(main())
