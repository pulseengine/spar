#!/usr/bin/env python3
"""Fail if a core field is nested where rivet's query surface cannot read it.

rivet has two field planes. `release` and `status` are **core** fields, read by
the CLI's own query surface — `rivet list --release vX.Y` is documented as *"the
release-planning view"*, and `status` is what every lifecycle query filters on.
Separately, each artifact may carry a `fields:` bag for schema-declared custom
keys (for `requirement` the declared set is `baseline`, `category`,
`cited-source`, `priority`, `upstream-ref`).

Writing a core field into the bag is valid YAML, is tolerated by the schema, and
is completely invisible to the query:

    - id: REQ-A            - id: REQ-B
      status: proposed       fields:
      release: v0.36.0         release: v0.23.0
                               status: planned
      ^ rivet sees these       ^ rivet sees neither

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

WHY `status` TOO (#375)
    `status:` has the same failure mode as `release:`, one field over. #371's
    status-vocabulary reconciliation measured six `status:` values in
    `safety/stpa/architecture.yaml` sitting inside the `fields:` bag (ARCH-026,
    -027, -028, -030, -031, -032), invisible to `status-allowed-values` and to
    every lifecycle query — the #370 defect exactly, on `status` instead of
    `release`. They were named as an open hole in REQ-GUARD-STATUS-VOCAB-001
    rather than silently absorbed. The parser, the block-scalar handling and the
    fail-closed guards were already field-agnostic; only the constant and the
    report strings assumed a single name. So the gate now walks a LIST of core
    fields, reporting each plane separately.

WHY NOT JUST GREP FOR SIX SPACES
    Because a core field's name also appears inside `description: >` prose, where
    deeper indentation is meaningless and a grep would red the build for a
    sentence. This parser tracks block scalars and skips their bodies. There is
    no such line in the tree today; the handling exists so that adding one is not
    a trap. See the `release-plane-block-scalar` self-test.

WHY NO PyYAML
    Same reason as check_human_scoped.py: no workflow here installs a Python
    package, so a required check must not depend on an unverified runner
    package. A guardrail that cannot run reads as approval. Stdlib only, and
    fail-closed — input this parser cannot fully read exits 2, not 0.

Usage:
    tools/check_release_plane.py [--artifacts-dir DIR ...]

Exit code:
    0  every checked field's assignments are on the artifact's own key plane
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

#: The core fields rivet's query surface reads directly. Each is checked
#: independently on its own plane; the report and the self-test counts are kept
#: per-field so "release is clean" and "status is clean" remain distinct claims.
FIELDS = ("release", "status")


class Unsupported(Exception):
    """Input uses YAML this parser does not fully handle — never guess past it."""


def scan_file(
    path: str, fields: tuple[str, ...] = FIELDS
) -> tuple[list[tuple[str, str, int, int]], dict[str, int]]:
    """Return (findings, visible) for *path*.

    A finding is (field, artifact_id, lineno, indent) for a core field key
    nested deeper than the artifact's own key plane. `visible` maps each field
    to the count of correctly-placed assignments, so the caller can tell "all
    good" from "nothing to check" per field.
    """
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()

    indents = [m.group("indent") for m in (_ITEM_RE.match(ln) for ln in lines) if m]
    if not indents:
        raise Unsupported(f"{path}: no sequence entries found — is this artifact YAML?")
    item_indent = min(len(i) for i in indents)
    key_indent = item_indent + 2

    findings: list[tuple[str, str, int, int]] = []
    visible: dict[str, int] = {f: 0 for f in fields}
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

        if key in fields and seen_record:
            if indent == key_indent:
                visible[key] += 1
            elif indent > key_indent:
                findings.append((key, current_id or "<unnamed>", lineno, indent))

    if not seen_record:
        raise Unsupported(f"{path}: parsed zero artifacts.")
    return findings, visible


def collect(
    dirs: list[str], fields: tuple[str, ...] = FIELDS
) -> tuple[list[tuple[str, str, str, int, int]], dict[str, int], int]:
    """Return (findings, visible, files_scanned) across *dirs*.

    A finding is (source_file, field, artifact_id, lineno, indent). Split out
    from run_check so the self-test can assert on the finding SET per field
    rather than on the process's single-integer summary. `exit 1` is necessary
    for "reported the nested one" but not sufficient — it is equally consistent
    with reporting every assignment in the file, which is the specific bug the
    violation fixtures exist to rule out.
    """
    all_findings: list[tuple[str, str, str, int, int]] = []
    total_visible: dict[str, int] = {f: 0 for f in fields}
    scanned = 0
    for artifacts_dir in dirs:
        paths = sorted(glob.glob(os.path.join(artifacts_dir, "*.yaml")))
        if not paths:
            raise Unsupported(f"no *.yaml under {artifacts_dir!r}")
        for path in paths:
            findings, visible = scan_file(path, fields)
            scanned += 1
            for f in fields:
                total_visible[f] += visible[f]
            base = os.path.basename(path)
            all_findings.extend((base, fld, i, ln, ind) for fld, i, ln, ind in findings)
    return all_findings, total_visible, scanned


def run_check(dirs: list[str], out=None, err=None, fields: tuple[str, ...] = FIELDS) -> int:
    """The whole check. Returns the process exit code (see module docstring)."""
    out = out or sys.stdout
    err = err or sys.stderr

    try:
        all_findings, total_visible, scanned = collect(dirs, fields)
    except (OSError, Unsupported) as exc:
        print(f"INCONCLUSIVE: {exc}", file=err)
        print(
            "This check could not read its input, which is NOT the same as the "
            "property holding. Treat it as red.",
            file=err,
        )
        return 2

    print("== field-plane guardrail ==", file=out)
    print(f"files scanned: {scanned} from {', '.join(d + '/' for d in dirs)}", file=out)
    for field in fields:
        nested = sum(1 for _, fld, _, _, _ in all_findings if fld == field)
        print(
            f"`{field}:` on the artifact key plane (rivet sees these): {total_visible[field]}",
            file=out,
        )
        print(
            f"`{field}:` nested deeper (rivet is blind to these):     {nested}",
            file=out,
        )
        if total_visible[field] == 0 and nested == 0:
            # A guardrail with nothing to guard passes trivially, which is how a
            # check silently becomes decorative. Say so rather than printing a
            # reassuring nothing.
            print(
                f"WARNING: no `{field}:` assignment found anywhere, so this check "
                f"asserted nothing about that field.",
                file=err,
            )

    if all_findings:
        print(file=err)
        print(
            f"error: {len(all_findings)} core-field assignment(s) are nested "
            f"below the artifact's own keys, where rivet cannot read them.",
            file=err,
        )
        for source, field, art_id, lineno, indent in all_findings:
            print(f"  {art_id}: `{field}:` at {source}:{lineno} (indent {indent})", file=err)
        print(file=err)
        print(
            "Move the field to the artifact's own key level — a sibling of "
            "`id:` and `type:`, not a member of `fields:`. Verify with "
            "`rivet list` / `rivet list --release <version>`: the query must "
            "return the artifact.",
            file=err,
        )
        return 1

    return 0


#: (fixture directory, field to assert, required exit code, required nested
#: count, required visible count, what it proves). `None` for a count means "not
#: asserted" — used only where collect() raises and there are no counts to
#: assert. The `field` column names which plane's counts the fixture pins; the
#: exit code is a whole-run property (any field's finding reds it).
#:
#: These run on every CI invocation via --self-test. A guardrail exercised only
#: against compliant input cannot distinguish "the property holds" from "the
#: check does nothing".
#:
#: The counts are asserted, not just the exit code, because the exit code is
#: too coarse to carry the claim. `exit 1` on a violation fixture is equally
#: consistent with a checker that reports EVERY assignment it sees — which
#: would be a checker that does not discriminate on plane at all, i.e. exactly
#: the regression these fixtures are here to catch. (nested=1, visible=1) is the
#: assertion that actually means what the `proves` text says.
SELF_TESTS = [
    (
        "release-plane-violation",
        "release",
        1,
        1,
        1,
        "a `fields:`-nested release is reported, while a top-level one in the "
        "same file is NOT (nested=1, visible=1, from two artifacts differing "
        "only in indentation) — so the check discriminates on PLANE, not "
        "merely on the presence of a `release:` key",
    ),
    (
        "status-plane-violation",
        "status",
        1,
        1,
        1,
        "the SAME discrimination holds for the second core field: a "
        "`fields:`-nested `status:` (the ARCH-026-class #375 defect) is "
        "reported, a top-level `status:` in the same file is NOT (nested=1, "
        "visible=1). Deleting `status` from FIELDS makes this fixture exit 0 "
        "and reds the self-test, so the field list is an exercised claim",
    ),
    (
        "release-plane-block-scalar",
        "release",
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
        "release",
        2,
        None,
        None,
        "an artifact layout the parser cannot read is refused as INCONCLUSIVE "
        "rather than passing vacuously — note the fixture's only `release:` is "
        "fields-nested, so a checker without the guard would exit 0, not 1",
    ),
    (
        "release-plane-no-artifacts",
        "release",
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
    for name, field, want, want_nested, want_visible, proves in SELF_TESTS:
        path = os.path.join(fixtures, name)
        got = run_check([path], devnull, devnull)
        detail = f"exit {got} (want {want})"
        ok = got == want
        if want_nested is not None:
            try:
                findings, visible, _ = collect([path])
                nested = sum(1 for _, fld, _, _, _ in findings if fld == field)
                vis = visible.get(field)
            except (OSError, Unsupported):
                nested, vis = None, None
            ok = ok and nested == want_nested and vis == want_visible
            detail += (
                f", {field} nested {nested} (want {want_nested})"
                f", {field} visible {vis} (want {want_visible})"
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
