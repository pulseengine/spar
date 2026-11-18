#!/usr/bin/env python3
"""The per-PR Verification Gate's default filter, and an independent count of it.

REQ-VERIFY-GATE-SCOPE-001, owner decision 20261117T184412-effaa3 (option
`union`): the per-PR default is every `type: feature` artifact in the frozen
v093/v0100 slice OR whose `release:` is the version in Cargo.toml. The weekly
workflow runs all of them.

The release half is spelled as explicit ids, `(or (= id "A") (= id "B") ...)`,
read from the YAML: the gate's pinned rivet (v0.7.0, b7a17bef) has no `release`
field in its filter language, so `(= release "v0.49.0")` matched 0 there while
rivet 0.41.1 matched 5 (measured 2026-11-18).

The count is taken from the artifact YAML with PyYAML, not from rivet, and the
gate passes it to run_verification.py as --min-matched. So if rivet drops part
of the filter or fails to load an artifact, the gate matches fewer than this
count and fails instead of quietly checking a smaller set.

Usage:
    tools/verification_default_filter.py [--root .] [--release vX.Y.Z]
        prints `release=`, `filter=`, `release_filter=` and `expected=` lines,
        ready to append to $GITHUB_OUTPUT
    tools/verification_default_filter.py --self-test

Exit 0 on success, 2 when Cargo.toml has no version, PyYAML is missing, or an
artifact file cannot be parsed (a file this reader cannot read could hide
matches, so no count is printed).
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys
import tempfile

try:
    import yaml
except ImportError:  # pragma: no cover
    print("::error::PyYAML is unavailable; the default count cannot be taken.",
          file=sys.stderr)
    sys.exit(2)

FROZEN_TAGS = ("v093", "v0100")
GLOBS = ("artifacts/*.yaml", "safety/**/*.yaml")


class Unreadable(Exception):
    pass


def cargo_release(root: str) -> str:
    """'v' + the first top-level `version = "..."` in Cargo.toml."""
    try:
        text = open(os.path.join(root, "Cargo.toml"), encoding="utf-8").read()
    except OSError as e:
        raise Unreadable(f"Cargo.toml: {e}")
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        raise Unreadable("Cargo.toml has no version line")
    return "v" + m.group(1)


NO_MATCH = '(= id "__no-feature-carries-this-release__")'


def _ids(ids: list[str]) -> str:
    return " ".join(f'(= id "{i}")' for i in ids)


def release_filter(ids: list[str]) -> str:
    """The release half alone; matches nothing when no feature carries the release."""
    if not ids:
        return NO_MATCH
    return f'(and (= type "feature") (or {_ids(ids)}))'


def default_filter(ids: list[str]) -> str:
    tags = " ".join(f'(has-tag "{t}")' for t in FROZEN_TAGS)
    extra = f" {_ids(ids)}" if ids else ""
    return f'(and (= type "feature") (or {tags}{extra}))'


def selected(art: dict, release: str) -> bool:
    if art.get("type") != "feature":
        return False
    tags = art.get("tags") or []
    if isinstance(tags, list) and any(t in tags for t in FROZEN_TAGS):
        return True
    return art.get("release") == release


def _features(root: str):
    """Every (artifact dict) in the scanned YAML; Unreadable on a file PyYAML cannot read."""
    for pattern in GLOBS:
        for path in sorted(glob.glob(os.path.join(root, pattern), recursive=True)):
            try:
                doc = yaml.safe_load(open(path, encoding="utf-8"))
            except (OSError, yaml.YAMLError) as e:
                raise Unreadable(f"{os.path.relpath(path, root)}: {e}")
            if not isinstance(doc, dict):
                continue
            for value in doc.values():
                if not isinstance(value, list):
                    continue
                for art in value:
                    if isinstance(art, dict) and art.get("type") == "feature":
                        yield art


def release_ids(root: str, release: str) -> list[str]:
    return sorted({str(a.get("id")) for a in _features(root) if a.get("release") == release})


def count(root: str, release: str) -> int:
    return len({str(a.get("id")) for a in _features(root) if selected(a, release)})


def self_test() -> int:
    passed = failed = 0

    def check(desc: str, ok: bool, got=None) -> None:
        nonlocal passed, failed
        passed, failed = (passed + 1, failed) if ok else (passed, failed + 1)
        print(f"  {'ok  ' if ok else 'FAIL'} {desc}" + ("" if ok else f" (got {got!r})"))

    print("verification_default_filter self-test")
    arts = """artifacts:
  - {id: F-TAG, type: feature, tags: [v093]}
  - {id: F-TAG2, type: feature, tags: [x, v0100]}
  - {id: F-REL, type: feature, release: v1.2.0}
  - {id: F-BOTH, type: feature, tags: [v093], release: v1.2.0}
  - {id: F-OLD, type: feature, release: v1.1.0}
  - {id: F-NONE, type: feature}
  - {id: R-REL, type: requirement, release: v1.2.0, tags: [v093]}
"""
    with tempfile.TemporaryDirectory() as d:
        os.makedirs(os.path.join(d, "artifacts"))
        os.makedirs(os.path.join(d, "safety", "stpa"))
        open(os.path.join(d, "Cargo.toml"), "w").write(
            '[workspace]\nmembers = []\n\n[workspace.dependencies]\n'
            'foo = { version = "9.9.9" }\n\n[workspace.package]\nversion = "1.2.0"\n')
        open(os.path.join(d, "artifacts", "a.yaml"), "w").write(arts)
        open(os.path.join(d, "safety", "stpa", "s.yaml"), "w").write(
            "artifacts:\n  - {id: S-REL, type: feature, release: v1.2.0}\n")
        rel = cargo_release(d)
        check("the release is the workspace version, not a dependency's", rel == "v1.2.0", rel)
        n = count(d, rel)
        # F-TAG, F-TAG2, F-REL, F-BOTH, S-REL
        check("the count is the union: tagged, released, nested safety/ files", n == 5, n)
        n_frozen = count(d, "v0.0.0")
        check("with no artifact in the release, the frozen slice alone is counted",
              n_frozen == 3, n_frozen)
        ids = release_ids(d, rel)
        check("the release half names each feature carrying the release",
              ids == ["F-BOTH", "F-REL", "S-REL"], ids)
        check("a requirement carrying the tag and release is not counted",
              not selected({"id": "R", "type": "requirement", "tags": ["v093"],
                            "release": rel}, rel))
        open(os.path.join(d, "artifacts", "bad.yaml"), "w").write("artifacts: [\n")
        try:
            count(d, rel)
            check("an unparsable artifact file is an error, not a lower count", False)
        except Unreadable:
            check("an unparsable artifact file is an error, not a lower count", True)
        open(os.path.join(d, "Cargo.toml"), "w").write("[workspace]\n")
        try:
            cargo_release(d)
            check("a Cargo.toml without a version is an error", False)
        except Unreadable:
            check("a Cargo.toml without a version is an error", True)
    f = default_filter(["F-REL"])
    check("the filter carries both frozen tags and the release ids",
          '(has-tag "v093")' in f and '(has-tag "v0100")' in f
          and '(= id "F-REL")' in f, f)
    check("with no release ids the filter is the frozen slice alone",
          default_filter([]) == '(and (= type "feature") (or (has-tag "v093") (has-tag "v0100")))',
          default_filter([]))
    check("with no release ids the release half matches nothing",
          release_filter([]) == NO_MATCH, release_filter([]))
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=".")
    ap.add_argument("--release", default=None,
                    help="override the release (default: v + Cargo.toml's version)")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    try:
        release = args.release or cargo_release(args.root)
        n = count(args.root, release)
        ids = release_ids(args.root, release)
    except Unreadable as e:
        print(f"::error::{e}", file=sys.stderr)
        return 2
    print(f"release={release}")
    print(f"filter={default_filter(ids)}")
    print(f"release_filter={release_filter(ids)}")
    print(f"expected={n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
