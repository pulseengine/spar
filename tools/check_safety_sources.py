#!/usr/bin/env python3
"""Fail if an artifact YAML in the safety tree is not under any `sources:` path.

rivet loads artifacts from the paths listed under `sources:` in `rivet.yaml` and
from nowhere else. A YAML file that carries an `artifacts:` list but sits outside
every one of those paths is never loaded: `rivet validate` cannot see its ids,
`rivet list` never returns them, and no status/release/vocabulary guard can reach
them. The file *reads* as tracked traceability — it has ids, statuses, `links:` —
while being tracked by nothing.

WHY THIS EXISTS (#375, hole 2)
    `safety/requirements.yaml` (23 ids) and `safety/analysis.yaml` (71 ids) sat
    directly under `safety/`, which is not a `sources:` path — only `safety/stpa`
    is. 94 artifacts rivet never loaded. They were a frozen 2026-03-10 snapshot:
    every id in each also existed in the `safety/stpa/` file rivet DOES load, and
    the loaded copy was strictly ahead (later `links:`, later `status:`). So the
    "21 off-vocabulary statuses" #371 flagged in the shadow copy were the March
    values, already migrated in the copy rivet reads — nothing to fix in them,
    only a file to delete. This guard makes that deletion durable: a shadow safety
    file cannot silently return.

    It is scoped to `safety/` on purpose. That is the tree where this defect lives
    and where an un-loaded artifact file masquerades as tracked safety evidence;
    a repo-wide scan would red on the many artifact-shaped fixtures under `tools/`
    and elsewhere that are deliberately not sources.

WHY IT CHECKS rivet.yaml, NOT A HARDCODED "safety/stpa"
    The oracle is coverage by the ACTUAL `sources:` config. If the maintainer
    later adds `safety/` (or a specific file) to `sources:`, the same file stops
    being a shadow and this guard correctly stops flagging it — no code change
    here. A hardcoded allow-list would drift from rivet.yaml and become its own
    second, weaker source of truth.

WHY NO PyYAML
    Same reason as check_release_plane.py / check_human_scoped.py: no workflow
    here installs a Python package, so a required check must not import an
    unverified runner package — a guardrail that cannot run reads as approval.
    Stdlib only, and fail-closed: a rivet.yaml whose `sources:` this parser cannot
    read exits 2, not 0.

Usage:
    tools/check_safety_sources.py [--rivet-yaml FILE] [--safety-dir DIR]

Exit code:
    0  every artifact-bearing file under the safety tree is covered by `sources:`
    1  at least one is a shadow — artifact YAML rivet never loads (listed on stderr)
    2  INCONCLUSIVE — rivet.yaml unreadable, its `sources:` unparseable, or a
       scanned file unreadable. Deliberately distinct from 1: "there is a shadow"
       and "could not check" are different facts and must not be conflated.
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys

#: A top-level mapping key (column 0), e.g. `sources:`, `docs:`, `results:`. Used
#: to find where the `sources:` block ends.
_TOP_KEY_RE = re.compile(r"^(?P<key>[A-Za-z_][A-Za-z0-9_-]*):")

#: A `- path: VALUE` sequence entry at any indent inside the `sources:` block.
_PATH_RE = re.compile(r"^\s*-\s+path:\s*(?P<val>\S.*?)\s*$")


class Unsupported(Exception):
    """rivet.yaml uses shape this parser does not handle — never guess past it."""


def parse_source_paths(text: str) -> list[str]:
    """Return the `path:` values listed under the top-level `sources:` key.

    Stdlib line parser, not a YAML load. It finds the `sources:` line, then
    collects every `- path: X` until the next top-level key (a `word:` at column
    0). Comments and blank lines inside the block are skipped, so the large
    comment sitting between rivet.yaml's two source entries does not truncate the
    list — a parser that stopped at the first comment would miss `safety/stpa`
    and then flag every file in it as a shadow, which the `sources-interleaved`
    self-test rules out.

    Raises Unsupported if no `sources:` block or no paths are found — a config
    this cannot read must fail closed, not report "nothing outside sources".
    """
    lines = text.splitlines()
    in_sources = False
    paths: list[str] = []
    for line in lines:
        stripped = line.strip()
        top = _TOP_KEY_RE.match(line)
        if not in_sources:
            # Only a genuine top-level `sources:` key (column 0, exact name) opens
            # the block — not an indented `sources:` nor a `sources_extra:`.
            if top and top.group("key") == "sources":
                in_sources = True
            continue
        # Inside the block. A comment or blank line is not the end of it.
        if not stripped or stripped.startswith("#"):
            continue
        # A new top-level key at column 0 ends the block.
        if top:
            break
        m = _PATH_RE.match(line)
        if m:
            paths.append(_normalise(m.group("val")))

    if not in_sources:
        raise Unsupported("no top-level `sources:` key found in rivet.yaml")
    if not paths:
        raise Unsupported("`sources:` block contained no `- path:` entries")
    return paths


def _normalise(path: str) -> str:
    """Strip quotes, trailing slash, and normalise separators to POSIX."""
    path = path.strip().strip('"').strip("'")
    path = path.replace(os.sep, "/").rstrip("/")
    return path


def is_covered(relpath: str, source_paths: list[str]) -> bool:
    """True if *relpath* is one of the source paths or lives inside one.

    Coverage is by directory prefix, not exact match, because a source `path:`
    naming a directory (`safety/stpa`) loads every `*.yaml` under it. Exact-match
    only would flag `safety/stpa/analysis.yaml` as a shadow — the
    `covered-is-under-dir` self-test kills that mutation.
    """
    relpath = _normalise(relpath)
    for src in source_paths:
        if relpath == src or relpath.startswith(src + "/"):
            return True
    return False


def classify(
    files: list[tuple[str, bool]], source_paths: list[str]
) -> tuple[list[str], list[str]]:
    """Split *files* into (covered, shadow).

    *files* is a list of (relpath, has_artifacts). Only artifact-bearing files are
    classified; a file without an `artifacts:` list is neither a shadow nor a
    covered artifact and is ignored, so an incidental README or notes YAML in the
    tree does not red the build (the `non-artifact-ignored` self-test). Split out
    from the filesystem walk so the self-test asserts the SET, not a process exit.
    """
    covered: list[str] = []
    shadow: list[str] = []
    for relpath, has_artifacts in files:
        if not has_artifacts:
            continue
        (covered if is_covered(relpath, source_paths) else shadow).append(
            _normalise(relpath)
        )
    return sorted(covered), sorted(shadow)


def has_artifacts_key(path: str) -> bool:
    """True if *path* declares a top-level `artifacts:` list (a rivet artifact file).

    Raises OSError to the caller if the file cannot be read — fail closed rather
    than treat an unreadable file as "not an artifact file" and skip it silently.
    """
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.rstrip() == "artifacts:" or re.match(r"^artifacts:\s*$", line):
                return True
    return False


def collect(
    rivet_yaml: str, safety_dir: str
) -> tuple[list[str], list[str], list[str]]:
    """Return (covered, shadow, source_paths) for the real tree.

    Reads `sources:` from *rivet_yaml* and walks *safety_dir* for `**/*.yaml`.
    Paths are made relative to the directory holding rivet.yaml so they compare
    against the `sources:` paths, which are written relative to it.
    """
    with open(rivet_yaml, encoding="utf-8") as handle:
        source_paths = parse_source_paths(handle.read())

    root = os.path.dirname(os.path.abspath(rivet_yaml)) or "."
    pattern = os.path.join(safety_dir, "**", "*.yaml")
    files: list[tuple[str, bool]] = []
    for path in sorted(glob.glob(pattern, recursive=True)):
        rel = os.path.relpath(os.path.abspath(path), root)
        files.append((rel, has_artifacts_key(path)))

    covered, shadow = classify(files, source_paths)
    return covered, shadow, source_paths


def run_check(rivet_yaml: str, safety_dir: str, out=None, err=None) -> int:
    """The whole check. Returns the process exit code (see module docstring)."""
    out = out or sys.stdout
    err = err or sys.stderr

    try:
        covered, shadow, source_paths = collect(rivet_yaml, safety_dir)
    except (OSError, Unsupported) as exc:
        print(f"INCONCLUSIVE: {exc}", file=err)
        print(
            "This check could not read its input, which is NOT the same as the "
            "property holding. Treat it as red.",
            file=err,
        )
        return 2

    print("== safety-sources guardrail ==", file=out)
    print(f"sources: {', '.join(source_paths)}", file=out)
    print(f"artifact files under {safety_dir}/ covered by sources: {len(covered)}", file=out)
    print(f"artifact files under {safety_dir}/ NOT loaded (shadow): {len(shadow)}", file=out)

    if len(covered) == 0:
        # A guardrail with nothing to guard passes trivially, which is how a check
        # silently becomes decorative. Say so rather than printing a reassuring
        # nothing — a safety tree with zero LOADED artifact files is itself odd.
        print(
            f"WARNING: no artifact-bearing file under {safety_dir}/ is covered by "
            f"`sources:`, so this check asserted nothing about a real tree.",
            file=err,
        )

    if shadow:
        print(file=err)
        print(
            f"error: {len(shadow)} artifact file(s) under {safety_dir}/ are not "
            f"under any `sources:` path, so rivet never loads them:",
            file=err,
        )
        for rel in shadow:
            print(f"  {rel}", file=err)
        print(file=err)
        print(
            "Either add the file's directory to `sources:` in rivet.yaml (and fix "
            "what then surfaces), or delete the file if it is a dead copy. It must "
            "not sit in the safety tree looking tracked while rivet loads nothing "
            "from it. Verify with `rivet list`: a loaded file's ids appear there.",
            file=err,
        )
        return 1

    return 0


#: (name, source_paths, files, want_shadow, want_covered, proves). `files` is a
#: list of (relpath, has_artifacts). Asserted on the SET sizes from classify(),
#: not merely an exit code: a checker that flagged every safety file, or none,
#: would still exit 1/0 on some fixture — the (shadow, covered) pair is what
#: pins that it discriminates on COVERAGE. Mutation-checked in the comments.
_CLASSIFY_TESTS = [
    (
        "shadow-vs-covered",
        ["safety/stpa"],
        [("safety/stpa/analysis.yaml", True), ("safety/analysis.yaml", True)],
        1,
        1,
        "a bare `safety/analysis.yaml` is a shadow while `safety/stpa/analysis.yaml` "
        "in the same tree is NOT (shadow=1, covered=1, from two artifact files "
        "differing only in directory) — the check discriminates on COVERAGE, not "
        "on being a safety YAML. This is the #375 layout exactly.",
    ),
    (
        "covered-is-under-dir",
        ["safety/stpa"],
        [
            ("safety/stpa/analysis.yaml", True),
            ("safety/stpa/requirements.yaml", True),
        ],
        0,
        2,
        "a `sources:` entry naming a DIRECTORY covers every artifact file under "
        "it (covered=2, shadow=0). An exact-match-only coverage test would flag "
        "both as shadows — this fixture kills that mutation.",
    ),
    (
        "non-artifact-ignored",
        ["safety/stpa"],
        [("safety/stpa/analysis.yaml", True), ("safety/NOTES.yaml", False)],
        0,
        1,
        "a YAML in the tree with no `artifacts:` list is neither shadow nor "
        "covered (shadow=0, covered=1) — an incidental notes file outside "
        "`sources:` does not red the build. A checker ignoring has_artifacts "
        "would flag NOTES.yaml as a shadow.",
    ),
]

#: (name, rivet_yaml_text, want_paths_or_None, proves). None means parse must
#: raise Unsupported (fail-closed). The interleaved-comment case is the load-
#: bearing one: rivet.yaml's real `sources:` has a ~25-line comment between its
#: two entries, so a parser that ended the block at the first comment would drop
#: `safety/stpa` and turn every file under it into a false shadow.
_PARSE_TESTS = [
    (
        "sources-interleaved",
        "project:\n  name: x\nsources:\n  - path: artifacts\n    format: generic-yaml\n"
        "  # a long comment\n  # spanning several lines\n  - path: safety/stpa\n"
        "    format: stpa-yaml\n\ndocs:\n  - docs\n",
        ["artifacts", "safety/stpa"],
        "comments and blank lines inside `sources:` do not end the block, and the "
        "block ends at the next top-level key (`docs:`) — both paths are read.",
    ),
    (
        "sources-quoted-trailing-slash",
        'sources:\n  - path: "safety/stpa/"\n',
        ["safety/stpa"],
        "quotes and a trailing slash are normalised so coverage compares cleanly.",
    ),
    (
        "sources-decoy-keys",
        "project:\n  sources:\n    - path: not-this\nsources_extra:\n  - path: not-this-either\n"
        "sources:\n  - path: artifacts\n  - path: safety/stpa\ndocs:\n  - docs\n",
        ["artifacts", "safety/stpa"],
        "an indented `sources:` under `project:` and a top-level `sources_extra:` "
        "are both decoys — only the genuine column-0 `sources:` key with the exact "
        "name opens the block, so neither decoy's `path:` leaks in.",
    ),
    (
        "no-sources-key",
        "project:\n  name: x\ndocs:\n  - docs\n",
        None,
        "a rivet.yaml with no `sources:` block fails closed (exit 2), not 'nothing "
        "is outside sources'.",
    ),
    (
        "empty-sources",
        "sources:\ndocs:\n  - docs\n",
        None,
        "a `sources:` key with no `- path:` entries fails closed rather than "
        "declaring every safety file a shadow.",
    ),
]


def self_test() -> int:
    """Assert the classifier and the sources parser still behave as claimed."""
    failures = 0

    for name, srcs, files, want_shadow, want_covered, proves in _CLASSIFY_TESTS:
        covered, shadow = classify(files, srcs)
        ok = len(shadow) == want_shadow and len(covered) == want_covered
        failures += not ok
        print(
            f"[{'PASS' if ok else 'FAIL'}] {name}: "
            f"shadow {len(shadow)} (want {want_shadow}), "
            f"covered {len(covered)} (want {want_covered})"
        )
        print(f"       proves: {proves}")

    for name, text, want_paths, proves in _PARSE_TESTS:
        try:
            got = parse_source_paths(text)
            raised = False
        except Unsupported:
            got = None
            raised = True
        if want_paths is None:
            ok = raised
            detail = f"raised Unsupported {raised} (want True)"
        else:
            ok = not raised and got == want_paths
            detail = f"paths {got} (want {want_paths})"
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] {name}: {detail}")
        print(f"       proves: {proves}")

    if failures:
        print(
            f"\nerror: {failures} self-test(s) failed — the guardrail is not "
            f"detecting what it claims to detect.",
            file=sys.stderr,
        )
        return 1
    total = len(_CLASSIFY_TESTS) + len(_PARSE_TESTS)
    print(f"\n{total} self-test(s) passed.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--rivet-yaml",
        default="rivet.yaml",
        metavar="FILE",
        help="rivet config whose `sources:` defines what is loaded (default: rivet.yaml)",
    )
    parser.add_argument(
        "--safety-dir",
        default="safety",
        metavar="DIR",
        help="the safety artifact tree to scan for shadow files (default: safety)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="verify the classifier and sources parser, then exit",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    return run_check(args.rivet_yaml, args.safety_dir)


if __name__ == "__main__":
    sys.exit(main())
