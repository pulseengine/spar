#!/usr/bin/env python3
"""Assert no artifact is scoped to a release that has already gone by.

An artifact carrying `release: vX.Y.Z` with a status short of `verified`, where
X.Y.Z is BELOW the version currently in Cargo.toml, is a scheduling failure the
board cannot see: the release it was promised to has shipped, and nothing said
so. Two distinct shapes hide in that one count, and they need different work:

  * `proposed`    — scope planned and never built. v0.23.0 alone carries 26.
  * `implemented` — work that shipped but never closed the right side of the V.

`rivet validate` does not catch either: both are schema-valid, and a release
field is just a string. The only thing that ever noticed was a human reading
the board, which is how 90 of them accumulated.

## Why this is a ratchet and not a zero

There are 90 today. A guard that demanded zero would be red on every PR from
the moment it landed, get `|| true`-d within a week, and join the set of checks
people route around. So it is EXACT, like `MAX_TOO_PERMISSIVE`: above the
declared count fails as a regression, below it fails until the constant is
lowered, so a cleanup is locked in rather than left as slack someone re-spends.

## What this deliberately does NOT do

It does not decide which of the 90 are worth finishing. An artifact assigned to
a release that shipped might be real work to schedule, or might be scope that
should be deleted; that is a maintainer judgement and the guard has no opinion.
It only insists the number is known and only falls.

Nor does it look at tags. The comparison is against Cargo.toml, because that is
the release being PREPARED — anything assigned below it should already be
finished. Comparing against the newest tag would have let the whole
bumped-but-untagged window (v0.41.0, v0.42.0, v0.43.0) hide artifacts.

Usage:
    tools/check_stale_scope.py [--max N] [--artifacts-dir DIR]...
    tools/check_stale_scope.py --self-test
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - environment problem, not a verdict
    print("::error::PyYAML is required", file=sys.stderr)
    sys.exit(2)

# Re-measured 2026-10-13 against Cargo.toml 0.45.0, scanning BOTH artifacts/ and
# safety/stpa/ — the same two directories the other plane guards scan, and now
# also this tool's DEFAULT_DIRS, so the local default and the CI invocation
# cannot disagree. They used to: `--artifacts-dir` defaulted to artifacts/ alone,
# which counts 89 against CI's 90, and the below-floor branch then advised
# lowering DEFAULT_MAX to a number that would have gone red on the next CI run.
# A guard that measures a narrower tree than it governs gives confident wrong
# advice, so the default now IS the governed scope.
#
# 90 -> 80 is REQ-GUARD-STALE-SCOPE-002: ten TEST-GUARD-* verification artifacts
# were counted as debt and are not debt (see IS_SETTLED_AT_IMPLEMENTED).
DEFAULT_MAX = 80

# The two directories this guard governs. CI passes them explicitly; this is the
# same list so an operator running the tool bare measures what the gate measures.
DEFAULT_DIRS = (Path("artifacts"), Path("safety/stpa"))

# A status at or past this point has closed the V and is not stale whatever its
# release says.
SETTLED = {"verified", "accepted", "rejected"}

# ── Why `implemented` settles a VERIFICATION artifact but not a requirement ──
#
# For a REQUIREMENT, `implemented` is the guard's main finding and must stay
# counted: the left side of the V closed and the right side did not. v0.23.0
# alone carries 26 of those.
#
# For a VERIFICATION artifact it is wrong, and the reason is structural rather
# than a matter of convention: a test is not verified by another test, which
# would regress infinitely. A verification artifact closes its V by existing and
# running, and this repo records that as `implemented`. The convention is
# unambiguous — of 129 TEST-* artifacts, 111 are `implemented` and ZERO are
# `verified`. There is no higher state in use, so counting them as debt counts a
# state nothing can ever leave.
#
# THE MARKER DISCRIMINATES, which is what makes this a fix and not an exemption.
# `fields.method` names HOW the artifact verifies — `automated-test`,
# `unit-test`, `formal-proof`. Paired with a status it separates two different
# things that look alike:
#
#   method + implemented  -> a test that exists and runs      -> SETTLED
#   method + proposed     -> a test that was never built       -> real debt
#
# It is deliberately not an id-prefix question: VAL-* and FUZZ-* carry
# `automated-test` too, and TEST-GUARD-STATUS-VOCAB carries no `fields` at all,
# so it stays counted for lacking the marker rather than for its status.
STATUS_SETTLING_A_VERIFICATION_ARTIFACT = "implemented"


def verification_method(a: dict) -> str | None:
    """`fields.method` if the artifact declares one, else None.

    The marker that says "this artifact verifies something, and here is how".
    Read defensively: `fields` is absent on most artifacts and has been seen as
    an explicit null, and neither may raise.
    """
    fields = a.get("fields")
    if not isinstance(fields, dict):
        return None
    method = fields.get("method")
    return str(method) if method else None


def has_verifies_link(a: dict) -> bool:
    """Whether the artifact carries a `links:` entry of type `verifies`.

    The SECOND, independent record of the same fact. Note the key is `type`,
    not `relation` — `{type: verifies, target: REQ-...}`. It is read only as a
    printed cross-check and never changes the count; see `settles_as_verification`.
    """
    for link in a.get("links") or []:
        if isinstance(link, dict) and link.get("type") == "verifies":
            return True
    return False


def settles_as_verification(a: dict, status: str) -> bool:
    """A verification artifact that exists and runs has no right side left to close.

    THE COUNT IS DECIDED BY `fields.method` ALONE, deliberately, even though
    `has_verifies_link` records the same fact and selects the identical ten
    artifacts in today's stale set. Requiring BOTH would be the conservative
    reading, and it is the wrong one here: a missing `links` entry is a routine
    data-entry omission, so demanding it would turn a real, running test into
    phantom debt — and the way phantom debt gets "fixed" is by moving
    DEFAULT_MAX, which pockets a false reading into the ratchet. That is the
    failure this requirement exists to prevent.

    The optimistic direction is already defended, which is why one marker is
    enough: DEFAULT_MAX is an EXACT two-sided bound, so a genuinely new stale
    artifact pushes the count above the constant and reds CI regardless. This
    guard does not need to be the thing that catches it.
    """
    return status == STATUS_SETTLING_A_VERIFICATION_ARTIFACT and verification_method(a) is not None


def parse_version(text: str) -> tuple[int, ...] | None:
    """`v0.23.0` / `0.23.0` -> (0, 23, 0). None when it is not a version."""
    m = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)", text.strip())
    return tuple(int(g) for g in m.groups()) if m else None


def current_version(cargo_toml: Path) -> tuple[int, ...] | None:
    """The version being prepared, from `[workspace.package]`."""
    in_pkg = False
    for line in cargo_toml.read_text().splitlines():
        s = line.strip()
        if s.startswith("["):
            in_pkg = s == "[workspace.package]"
            continue
        if in_pkg and (m := re.match(r'version\s*=\s*"([^"]+)"', s)):
            return parse_version(m.group(1))
    return None


def stale_artifacts(
    dirs: list[Path], current: tuple[int, ...]
) -> tuple[list[tuple[str, str, str]], list[tuple[str, str, bool]]]:
    """Two lists: the stale artifacts, and the verification artifacts excused.

    The second return value is the whole reason this is a fix and not a silent
    threshold change. Ten artifacts stopped being counted, and a reader has to
    be able to see WHICH ten and on what marker — an excusal that prints nothing
    is indistinguishable from a bug that loses rows.
    """
    out: list[tuple[str, str, str]] = []
    settled_verifications: list[tuple[str, str, bool]] = []
    for d in dirs:
        for path in sorted(d.glob("*.yaml")):
            try:
                doc = yaml.safe_load(path.read_text())
            except yaml.YAMLError as exc:
                # An unreadable artifact file is NOT zero stale artifacts. The
                # whole point of this guard is that silence must not read as a
                # clean result.
                print(f"::error::cannot parse {path}: {exc}", file=sys.stderr)
                sys.exit(2)
            if not isinstance(doc, dict):
                continue
            for items in doc.values():
                if not isinstance(items, list):
                    continue
                for a in items:
                    if not isinstance(a, dict) or "id" not in a:
                        continue
                    status, release = a.get("status"), a.get("release")

                    # No release is backlog, not a missed commitment.
                    if not release:
                        continue

                    # A release with NO status is unclassifiable, and the three
                    # conditions below all silently read as "not stale" if it is
                    # waved past: the guard cannot tell a missed commitment from a
                    # closed one. 322 of the artifacts here carry no top-level
                    # status, so this is a shape the tree can genuinely grow.
                    if not status:
                        print(f"::error::{a['id']} declares release {release!r} with no "
                              f"status. The guard cannot tell a missed commitment from a "
                              f"closed one and must not guess.", file=sys.stderr)
                        sys.exit(2)

                    # Settled before parsing: for a closed artifact the release
                    # field is history, and history need not be orderable.
                    if status in SETTLED:
                        continue

                    # A verification artifact at `implemented` has closed the
                    # only V it has. Checked BEFORE parsing the release, for the
                    # same reason SETTLED is: for a closed artifact the release
                    # field is history, and history need not be orderable.
                    if settles_as_verification(a, status):
                        settled_verifications.append(
                            (a["id"], str(verification_method(a)), has_verifies_link(a))
                        )
                        continue

                    rel = parse_version(str(release))
                    if rel is None:
                        print(f"::error::{a['id']} declares an unparseable release "
                              f"{release!r}. A release the guard cannot order is not a "
                              f"release above the line; it is a measurement that was not "
                              f"made.", file=sys.stderr)
                        sys.exit(2)

                    if rel < current:
                        out.append((str(release), status, a["id"]))
    return out, settled_verifications


def check(dirs: list[Path], cargo_toml: Path, max_stale: int) -> int:
    current = current_version(cargo_toml)
    if current is None:
        print(f"::error::no [workspace.package] version in {cargo_toml}", file=sys.stderr)
        return 2

    stale, excused = stale_artifacts(dirs, current)
    n = len(stale)
    cur_s = ".".join(str(x) for x in current)

    print("== stale-scope guardrail ==")
    print(f"preparing: {cur_s}")
    print(f"artifacts scoped below it and not settled: {n} (declared {max_stale})")

    by_release: dict[str, int] = {}
    for rel, _, _ in stale:
        by_release[rel] = by_release.get(rel, 0) + 1
    for rel in sorted(by_release, key=lambda r: parse_version(r) or (0, 0, 0)):
        print(f"  {rel:>9}  {by_release[rel]}")

    if excused:
        print(
            f"not counted: {len(excused)} verification artifact(s) at "
            f"{STATUS_SETTLING_A_VERIFICATION_ARTIFACT} — a test is not verified by "
            f"another test, so this is the end of its V, not a missed commitment"
        )
        # THE CROSS-CHECK. `fields.method` decided the count; the verifies link
        # is the independent second record of the same fact. Where they disagree
        # the artifact is still excused — saying so is the point, because a
        # divergence is a data-quality signal and not a verdict, and a guard that
        # printed only the number it liked would hide it.
        for ident, method, linked in sorted(excused):
            mark = "" if linked else "   (no verifies link — marker disagreement)"
            print(f"  {ident:<40} {method}{mark}")
        disagree = [e for e in excused if not e[2]]
        if disagree:
            print(
                f"::notice::{len(disagree)} of {len(excused)} excused artifact(s) carry "
                f"`fields.method` but no `links: {{type: verifies}}`. Both markers record "
                f"the same fact; adding the link makes the trace navigable."
            )

    if n > max_stale:
        print(
            f"::error::{n} artifacts are scoped to a release below {cur_s}, above the "
            f"declared {max_stale}. Something was assigned to a release that has already "
            f"gone by. Either finish it, re-scope it to a release still ahead, or delete "
            f"it — but do not raise this number.",
            file=sys.stderr,
        )
        for rel, status, ident in stale[: max_stale + 10][-10:]:
            print(f"  {rel:>9}  {status:12} {ident}", file=sys.stderr)
        return 1

    if n < max_stale:
        print(
            f"::error::{n} artifacts are stale-scoped, BELOW the declared {max_stale}. "
            f"That is a win: lower DEFAULT_MAX to {n} so it is locked in rather than "
            f"left as slack the next pass can re-spend.",
            file=sys.stderr,
        )
        return 1

    print("ok")
    return 0


# ── self-test ────────────────────────────────────────────────────────────────

def _write(tmp: Path, name: str, artifacts: list[dict]) -> None:
    (tmp / name).write_text(yaml.safe_dump({"artifacts": artifacts}, sort_keys=False))


def self_test() -> int:
    import tempfile

    cases: list[tuple[str, list[dict], str, int, str]] = [
        (
            "an artifact below the prepared version is stale",
            [{"id": "A", "status": "proposed", "release": "v0.23.0"}],
            "0.43.0", 1,
            "the whole point; if this passes the guard detects nothing",
        ),
        (
            "a verified artifact below it is NOT stale",
            [{"id": "A", "status": "verified", "release": "v0.23.0"}],
            "0.43.0", 0,
            "the V is closed — the release field is history, not debt",
        ),
        (
            "an artifact at the prepared version is not stale",
            [{"id": "A", "status": "proposed", "release": "v0.43.0"}],
            "0.43.0", 0,
            "it is scope for the release being prepared, which is normal",
        ),
        (
            "an artifact ahead of it is not stale",
            [{"id": "A", "status": "proposed", "release": "v0.47.0"}],
            "0.43.0", 0,
            "planned work, the thing a roadmap is made of",
        ),
        (
            "an artifact with no release is not stale",
            [{"id": "A", "status": "proposed"}],
            "0.43.0", 0,
            "backlog, by the release-planning definition — not a missed commitment",
        ),
        (
            "rejected counts as settled",
            [{"id": "A", "status": "rejected", "release": "v0.23.0"}],
            "0.43.0", 0,
            "a withdrawn requirement is decided, not outstanding",
        ),
        # ── REQ-GUARD-STALE-SCOPE-002: the discriminator, both arms ──────────
        #
        # These four are a MATRIX over (has a method marker) x (status), and the
        # pair is the point in each direction. The first case alone would pass a
        # build that excused `implemented` unconditionally; the third is what
        # refuses that build. The second alone would pass a build that excused
        # any artifact carrying a method; the first is what refuses that one.
        (
            "a verification artifact at implemented is settled",
            [{"id": "TEST-X", "status": "implemented", "release": "v0.23.0",
              "fields": {"method": "automated-test"}}],
            "0.43.0", 0,
            "a test is not verified by another test — `implemented` is the end of "
            "its V, and 111 of 129 TEST-* sit there with ZERO ever reaching verified",
        ),
        (
            "a verification artifact at proposed is STILL debt",
            [{"id": "TEST-X", "status": "proposed", "release": "v0.23.0",
              "fields": {"method": "automated-test"}}],
            "0.43.0", 1,
            "the marker alone must not excuse: a test that was never built is the "
            "realest debt there is. NOTE this arm has NO instance in the tree today, "
            "so it exists only here — the measurement that said otherwise named "
            "TEST-GUARD-STATUS-VOCAB, which carries no `fields` at all and is counted "
            "for lacking the marker, not for its status",
        ),
        (
            "a REQUIREMENT at implemented is NOT settled",
            [{"id": "REQ-X", "status": "implemented", "release": "v0.23.0"}],
            "0.43.0", 1,
            "the guard's main finding, and the thing the new arm must not swallow: "
            "left side of the V closed, right side open. v0.23.0 alone carries 26",
        ),
        (
            "an empty or null fields block does not excuse",
            [{"id": "TEST-X", "status": "implemented", "release": "v0.23.0",
              "fields": None},
             {"id": "TEST-Y", "status": "implemented", "release": "v0.23.0",
              "fields": {}},
             {"id": "TEST-Z", "status": "implemented", "release": "v0.23.0",
              "fields": {"method": ""}}],
            "0.43.0", 3,
            "`fields: null` is real in this tree and an absent method must read as "
            "absent, not as a truthy marker — an excusal keyed on a block that merely "
            "EXISTS would settle every artifact that happens to carry fields",
        ),
        (
            "the verifies link does not excuse on its own",
            [{"id": "TEST-X", "status": "implemented", "release": "v0.23.0",
              "links": [{"type": "verifies", "target": "REQ-Y"}]}],
            "0.43.0", 1,
            "the link is a printed CROSS-CHECK, not a second way to be excused. "
            "Pinning that keeps the count decided by exactly one marker, so a "
            "reader of the requirement can predict the number",
        ),
    ]

    failures = 0
    for name, artifacts, version, want_count, proves in cases:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            _write(tmp, "a.yaml", artifacts)
            cur = parse_version(version)
            assert cur is not None
            got = len(stale_artifacts([tmp], cur)[0])
            ok = got == want_count
            failures += not ok
            print(f"[{'PASS' if ok else 'FAIL'}] {name}: {got} stale (want {want_count})")
            print(f"       proves: {proves}")

    # The ratchet must bite in BOTH directions, or a "win" leaks back out.
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        _write(tmp, "a.yaml", [{"id": "A", "status": "proposed", "release": "v0.1.0"}])
        (tmp / "Cargo.toml").write_text('[workspace.package]\nversion = "0.43.0"\n')
        above = check([tmp], tmp / "Cargo.toml", 0)
        below = check([tmp], tmp / "Cargo.toml", 5)
        exact = check([tmp], tmp / "Cargo.toml", 1)
        for label, got, want in (("above", above, 1), ("below", below, 1), ("exact", exact, 0)):
            ok = got == want
            failures += not ok
            print(f"[{'PASS' if ok else 'FAIL'}] ratchet {label}-the-bound: exit {got} (want {want})")
        print("       proves: the bound is EXACT — a cleanup must be recorded, not pocketed")

    # A release the guard cannot CLASSIFY must be inconclusive, the same as one it
    # cannot read. Both of these were silent skips: `if not status or not release`
    # and `if rel is not None` each sent an unclassifiable artifact down the
    # not-stale path, which is the guard's own ERROR path yielding its IDEAL
    # reading. Inert today (0 of 937 artifacts), but 322 carry no top-level
    # status, so it is a shape the tree can grow into — and an inert path with no
    # case is how the unparseable-FILE hole above survived its own code review.
    for name, artifact, why in (
        (
            "a release with no status",
            {"id": "A", "release": "v0.23.0"},
            "cannot tell a missed commitment from a closed one",
        ),
        (
            "an unparseable release string",
            {"id": "A", "status": "proposed", "release": "next-sprint"},
            "a release that cannot be ordered is not a release above the line",
        ),
    ):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            _write(tmp, "a.yaml", [artifact])
            (tmp / "Cargo.toml").write_text('[workspace.package]\nversion = "0.43.0"\n')
            try:
                rc = check([tmp], tmp / "Cargo.toml", 0)
            except SystemExit as exc:
                rc = exc.code
            ok = rc == 2
            failures += not ok
            print(f"[{'PASS' if ok else 'FAIL'}] {name} is exit 2, not 0 stale: {rc}")
            print(f"       proves: {why}")

    # The DELIBERATE bound on the pair above, kept explicit so a later tightening
    # does not quietly swallow it: for a SETTLED artifact the release field is
    # history, and history need not be orderable. Without this case, moving the
    # settled check after the parse would look like a harmless reordering.
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        _write(tmp, "a.yaml", [{"id": "A", "status": "verified", "release": "whenever"}])
        (tmp / "Cargo.toml").write_text('[workspace.package]\nversion = "0.43.0"\n')
        try:
            rc = check([tmp], tmp / "Cargo.toml", 0)
        except SystemExit as exc:
            rc = exc.code
        ok = rc == 0
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] a SETTLED artifact's release need not parse: {rc}")
        print("       proves: the V is closed — the release field is history, not debt")

    # A corrupt ARTIFACT FILE must be inconclusive too, and this is the case the
    # suite shipped without: the `except yaml.YAMLError` branch was asserted in a
    # source comment and pinned by nothing. Replacing its `sys.exit(2)` with
    # `continue` — a truncated artifacts file scanned past as clean — passed the
    # whole suite (confirmed SURVIVED before this case was written). It is now the
    # `ss-swallow-bad-yaml` mutant in check_self_test_potency.py, so the case
    # cannot be dropped again silently.
    #
    # `max_stale=0` is deliberate: under the mutant the file yields zero
    # artifacts, 0 == declared 0, and the run reports `ok`. The ideal reading
    # produced by the error path is the red flag this whole guard is about.
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        (tmp / "broken.yaml").write_text("artifacts:\n  - id: A\n   bad-indent: [\n")
        (tmp / "Cargo.toml").write_text('[workspace.package]\nversion = "0.43.0"\n')
        try:
            rc = check([tmp], tmp / "Cargo.toml", 0)
        except SystemExit as exc:  # stale_artifacts exits directly
            rc = exc.code
        ok = rc == 2
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] an unparseable artifact file is exit 2, not 0 stale: {rc}")
        print("       proves: a file the guard cannot read must not be scanned past as clean")

    # A version this script cannot read must be INCONCLUSIVE, never "0 stale".
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        _write(tmp, "a.yaml", [{"id": "A", "status": "proposed", "release": "v0.1.0"}])
        (tmp / "Cargo.toml").write_text("[package]\nname = 'x'\n")
        rc = check([tmp], tmp / "Cargo.toml", 99)
        ok = rc == 2
        failures += not ok
        print(f"[{'PASS' if ok else 'FAIL'}] unreadable version is exit 2, not a pass: {rc}")
        print("       proves: a guard that cannot measure must not report clean")

    print(f"\n{failures} self-test(s) failed." if failures else "\nAll self-tests passed.")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--artifacts-dir", action="append", default=[], type=Path)
    ap.add_argument("--cargo-toml", type=Path, default=Path("Cargo.toml"))
    ap.add_argument("--max", type=int, default=DEFAULT_MAX)
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    dirs = args.artifacts_dir or list(DEFAULT_DIRS)
    return check(dirs, args.cargo_toml, args.max)


if __name__ == "__main__":
    sys.exit(main())
