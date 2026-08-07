#!/usr/bin/env python3
"""Fail when a verification step's test filter selects no tests.

REQ-GUARD-GATE-EVIDENCE-002 (c).

WHY THIS EXISTS
===============

`cargo test -p X -- some_filter` EXITS 0 WHEN THE FILTER MATCHES NOTHING. It
prints

    running 0 tests
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 47 filtered out

and returns success. `tools/run_verification.py` scores a step with

    return proc.returncode == 0

and sends both streams to DEVNULL, so a filter naming a test that does not
exist is indistinguishable from one naming a test that passes. The V closes on
paper either way.

This is not hypothetical. Executed on 2026-07-30:

    $ cargo test -p spar-wasm -- layout_is_deterministic
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out
    EXIT=0

VAL-R1..VAL-R5 are `status: implemented` and between them close the V for six
`RENDER-REQ-*` and four `ARCH-R*`. Every one of their filters selects nothing.

The shape is the one REQ-GUARD-GATE-EVIDENCE-001 names: the error path produces
the ideal reading. "No tests matched" and "all matched tests passed" are the
same exit code, and the second is the better-looking of the two.

HOW IT DECIDES
==============

libtest's filter is a SUBSTRING match over the full test path (`--exact` opts
out; nothing here uses it). So the question "does this filter select anything?"
is answerable exactly, without running any test, given the inventory of test
names — which `cargo test -- --list` prints as

    module::tests::name: test

The inventory is collected PER PACKAGE — `cargo test -p X --all-targets --
--list` for each package a step names — and each filter is checked against its
own package's names in memory. The listings share one build, so this is still
O(one build) rather than O(one invocation per step).

Per package is not a refinement, it is the correctness condition. The first
revision collected one flat workspace-wide list while parsing (and discarding)
the `-p`, so it asked "does this substring occur ANYWHERE?" where
`run_verification.py` runs "does it occur in package P?". #388's own executed
proof case survived that: `cargo test -p spar-wasm -- topology` matches three
tests, all of them in other crates (#404).

A filter that selects nothing fails, naming the artifact, the step and the
filter. A count is printed on every path, success included — for the same
reason `check_fmt_workspaces.py` prints one: a gate that says nothing on
success cannot distinguish "checked everything" from "checked nothing".

WHAT IT DOES NOT CLAIM
======================

* It does not check that the selected tests PASS. That is
  `run_verification.py`'s job and it does it correctly. This answers only the
  prior question — whether there was anything to run.
* A non-empty selection does not mean the right test. A filter may select a
  test that has nothing to do with the requirement. This is a lower bound on
  honesty, not a proof of relevance.
* Steps that are not `cargo test` (scripts, `lake`, shell) are counted and
  reported as unclassified rather than silently ignored. Silence is what this
  file exists to remove.

stdlib-only: no workflow in this repo installs a Python package, and a required
check that depends on an unverified runner package is a check that can fail to
run — which reads as approval.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

# `- run: <cmd>` inside a verification artifact's steps list.
_RUN = re.compile(r"^\s*-\s+run:\s*(?P<cmd>.+?)\s*$", re.M)
# `  - id: <ID>` at artifact level.
_ID = re.compile(r"^  - id:\s*(?P<id>\S+)\s*$", re.M)
# `    status: <value>` at artifact level.
_STATUS = re.compile(r"^    status:\s*(?P<status>\S+)\s*$", re.M)
# Statuses that ASSERT the evidence exists. A `proposed` or `draft` artifact
# naming a test that does not exist yet is a plan, not a false claim, and
# failing it would push authors to write the artifact only after the test —
# which inverts the order this methodology depends on. `rejected` and
# `withdrawn` assert nothing.
_CLAIMING = {"implemented", "verified", "accepted"}
# `cargo test [-p PKG] [...] -- FILTER [more]`. The filter is the first
# non-flag token after the bare `--`.
_CARGO_TEST = re.compile(r"\bcargo\s+test\b")
# `path::to::test_name: test` from `cargo test -- --list`.
_LIST_LINE = re.compile(r"^(?P<name>\S+):\s+test\s*$", re.M)


def parse_steps(yaml_text: str) -> list[tuple[str, str, str]]:
    """[(artifact_id, status, run_cmd)] in file order, without a YAML dependency.

    Deliberately narrow, and it FAILS rather than guesses: every `- run:` is
    attributed to the most recent `  - id:` above it, and a `- run:` appearing
    before any id is an error, not an orphan silently dropped.
    """
    marks = [(m.start(), "id", m.group("id")) for m in _ID.finditer(yaml_text)]
    marks += [(m.start(), "status", m.group("status")) for m in _STATUS.finditer(yaml_text)]
    marks += [(m.start(), "run", m.group("cmd")) for m in _RUN.finditer(yaml_text)]
    marks.sort()
    out: list[tuple[str, str, str]] = []
    current: str | None = None
    status = "<none>"
    pending: list[tuple[str, str]] = []
    for _pos, kind, val in marks:
        if kind == "id":
            out.extend((aid, status, cmd) for aid, cmd in pending)
            pending = []
            current, status = val, "<none>"
        elif kind == "status":
            status = val
        elif current is None:
            raise ValueError(f"`- run: {val}` appears before any artifact id")
        else:
            pending.append((current, _unquote(val)))
    out.extend((aid, status, cmd) for aid, cmd in pending)
    return out


def _unquote(val: str) -> str:
    """Strip YAML's surrounding quotes from a scalar.

    Steps in this repo are quoted deliberately: an unquoted `- run:` ending in
    `plp::` breaks serde_yaml, so several steps are written
    `- run: "cargo test -p spar-network --lib -- cqf"`.

    Without this, the regex hands back a command whose last token is `cqf"` and
    the filter check reports a vacuous filter for a step that is fine. The
    first run of this gate did exactly that for 8 of the 18 it flagged — a
    detector manufacturing findings out of its own misparse, which is the same
    species as everything it was written to catch. Only the direction of the
    error differed: it failed loudly rather than flatteringly, which is the one
    reason it was caught in the first read.
    """
    v = val.strip()
    if len(v) >= 2 and v[0] == v[-1] and v[0] in ("'", '"'):
        return v[1:-1]
    return v


def extract_filter(cmd: str) -> tuple[str | None, str | None]:
    """(package, filter) for a cargo-test command; (None, None) otherwise.

    Returns (pkg, None) when the command runs a whole package with no filter —
    that selects everything, so it can never be vacuous in this sense.
    """
    if not _CARGO_TEST.search(cmd):
        return (None, None)
    toks = cmd.split()
    pkg = None
    for i, t in enumerate(toks):
        if t in ("-p", "--package") and i + 1 < len(toks):
            pkg = toks[i + 1]
        elif t.startswith("--package="):
            pkg = t.split("=", 1)[1]
    if "--" not in toks:
        return (pkg, None)
    tail = toks[toks.index("--") + 1 :]
    for t in tail:
        if not t.startswith("-"):
            return (pkg, t)
    return (pkg, None)


def collect_inventory(manifest_dir: Path, packages: set[str]) -> dict[str, set[str]]:
    """{package: {test names}} — one listing PER PACKAGE.

    The first revision collected a single flat, workspace-wide list. It also
    parsed the `-p` out of each command and then never used it, so the gate
    asked "does this substring occur anywhere in the workspace?" while
    `run_verification.py` runs "does it occur in package P?" — strictly weaker,
    and #388's own executed proof case survived it: `cargo test -p spar-wasm --
    topology` matches three tests, all of them in spar-solver and spar-network
    (#404).

    Listing per package costs 10 invocations here, but they share one build, and
    it is the same question the consumer asks. A gate that answers an easier
    question than the thing it guards is the defect this whole requirement is
    about.
    """
    inv: dict[str, set[str]] = {}
    for pkg in sorted(packages):
        proc = subprocess.run(
            ["cargo", "test", "-p", pkg, "--all-targets", "--", "--list"],
            cwd=manifest_dir,
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            # Two different failures, and they deserve different reports.
            #
            # "did not match any packages" means the artifact names a package
            # that does not exist — a finding ABOUT that artifact. Omit it from
            # the inventory so `check()` can name the affected artifacts and
            # exit 2, rather than dying here with a traceback that names only
            # the first one. (The live instance: four steps said
            # `-p spar-cli`, but `crates/spar-cli/Cargo.toml` declares
            # `name = "spar"` and no package `spar-cli` exists. The old flat
            # inventory never ran `-p`, so it never noticed.)
            #
            # Anything else is a broken build, which invalidates every verdict,
            # not one artifact's — that stays fatal.
            if "did not match any packages" in proc.stderr:
                continue
            raise RuntimeError(
                f"cargo test -p {pkg} --list failed; cannot build a test "
                f"inventory for it, so its filters cannot be judged. Refusing "
                f"to report success.\n" + proc.stderr[-2000:]
            )
        names = {m.group("name") for m in _LIST_LINE.finditer(proc.stdout)}
        if not names:
            # A package with no tests at all is possible, but a package named by
            # a verification step and listing nothing is far more likely to be a
            # broken invocation. Fail closed rather than judge every one of its
            # filters vacuous (or, worse, non-vacuous against an empty pool).
            raise RuntimeError(
                f"cargo listed ZERO tests for package {pkg}, which a "
                f"verification step claims to filter. That is a broken scan, "
                f"not a finding about the filters."
            )
        inv[pkg] = names
    return inv


def check(yaml_path: Path, inventory: dict[str, set[str]], out=sys.stdout) -> int:
    steps = parse_steps(yaml_path.read_text(encoding="utf-8"))
    # (artifact id, status, command, filter, package)
    filtered: list[tuple[str, str, str, str, str | None]] = []
    whole_pkg = 0
    other = 0
    for aid, status, cmd in steps:
        pkg, filt = extract_filter(cmd)
        if pkg is None and filt is None:
            other += 1
        elif filt is None:
            whole_pkg += 1
        else:
            filtered.append((aid, status, cmd, filt, pkg))

    # Each filter is judged against ITS OWN package's inventory, not the
    # workspace's. `libtest` filters are substring matches over the test paths
    # *of the binary being run*, and `cargo test -p X` runs only X's binaries —
    # so a filter matching a test in another crate selects nothing here (#404).
    # A step with no `-p` really does run everything, so it uses the union.
    union: set[str] = set()
    for names in inventory.values():
        union |= names

    def pool_for(pkg: str | None) -> set[str] | None:
        return union if pkg is None else inventory.get(pkg)

    unlistable = [e for e in filtered if pool_for(e[4]) is None]
    empty = [
        e for e in filtered
        if (pool := pool_for(e[4])) is not None
        and not any(e[3] in name for name in pool)
    ]
    for aid, _status, _cmd, filt, pkg in unlistable:
        # No inventory for the named package means the question could not be
        # asked. "Could not judge" must not read as "fine".
        print(f"::error::{aid}: no test inventory for package {pkg!r}, so its "
              f"filter {filt!r} cannot be judged", file=out)
    # `<none>` is NOT lenient. An artifact whose status this reader cannot find
    # is unclassifiable, and an unclassifiable artifact with a filter that
    # selects nothing is exactly the case that must not pass quietly. Same
    # fail-closed rule check_required_contexts.py uses for input its narrow
    # parser cannot represent. TEST-STPA-PARSER-COVERAGE is the live instance:
    # no `status:` at artifact indent, its fields nested a level deeper.
    vacuous = [e for e in empty if e[1] in _CLAIMING or e[1] == "<none>"]
    planned = [e for e in empty if e[1] not in _CLAIMING and e[1] != "<none>"]

    print("== verification-filter guardrail ==", file=out)
    print(f"steps read:               {len(steps)}", file=out)
    print(f"  cargo test + filter:    {len(filtered)}", file=out)
    print(f"  cargo test, whole pkg:  {whole_pkg}", file=out)
    print(f"  not a cargo test:       {other}", file=out)
    print(f"packages inventoried:     {len(inventory)}"
          f"  ({', '.join(f'{p}={len(n)}' for p, n in sorted(inventory.items()))})", file=out)
    print(f"test names, all packages: {len(union)}", file=out)
    print(f"filters selecting nothing: {len(empty)}", file=out)
    print(f"  claiming evidence (FAIL): {len(vacuous)}", file=out)
    print(f"  proposed/draft (ok):      {len(planned)}", file=out)

    if not union:
        print(
            "::error::the test inventory is empty, so every filter would look "
            "vacuous. That is a broken scan, not a finding.",
            file=out,
        )
        return 2

    # A step whose package could not be inventoried is unjudgeable, and
    # unjudgeable must not read as fine. Errors were already printed above.
    if unlistable:
        print(f"\n{len(unlistable)} filtered step(s) name a package with no "
              f"inventory — refusing to report success over an unasked question.",
              file=out)
        return 2

    for aid, status, cmd, filt, pkg in planned:
        print(f"  note: {aid} ({status}) plans a test that does not exist yet: {filt!r}", file=out)

    if vacuous:
        print("", file=out)
        for aid, status, cmd, filt, pkg in vacuous:
            scope = f"package {pkg}" if pkg else "the workspace"
            print(f"::error::{aid} (status={status}): filter {filt!r} selects no "
                  f"test in {scope}", file=out)
            print(f"    step: {cmd}", file=out)
        print(
            "\nA filter that matches nothing exits 0. These steps have been "
            "scored as passing without running anything. Note the scope: a "
            "filter matching a test in a DIFFERENT package still selects "
            "nothing here, because `cargo test -p X` runs only X's binaries.",
            file=out,
        )
        return 1

    print(
        f"\nAll {len(filtered) - len(planned)} evidence-claiming filtered steps "
        f"select at least one test.",
        file=out,
    )
    return 0


def self_test() -> int:
    """Decision table. Includes the #388 regression itself."""
    import io

    passed = failed = 0

    def case(desc: str, want: int, yaml_text: str, inv: dict[str, set[str]],
             want_msg: str | None = None) -> None:
        """`want_msg` pins the DIAGNOSIS as well as the exit code.

        Several distinct broken inputs reach the same exit code by different
        routes — an empty inventory and an un-inventoried package both exit 2 —
        so asserting the code alone cannot say which check fired, and removing
        either leaves the suite green. Pinning the message is what makes each
        branch load-bearing.
        """
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "v.yaml"
            p.write_text(yaml_text, encoding="utf-8")
            buf = io.StringIO()
            got = check(p, inv, out=buf)
        text = buf.getvalue()
        if got != want:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want {want}")
            for line in text.splitlines()[:6]:
                print(f"         {line}")
        elif want_msg is not None and want_msg not in text:
            failed += 1
            print(f"  FAIL {desc}: exit {got} correct, but the diagnosis is "
                  f"wrong — expected {want_msg!r}")
            for line in text.splitlines()[:6]:
                print(f"         {line}")
        else:
            passed += 1
            print(f"  ok   {desc}")

    # Inventories are PER PACKAGE, because that is the scope `cargo test -p X`
    # actually searches. `spar-solver` deliberately owns a test whose name would
    # match a spar-wasm filter — that is the #404 counterexample, and under the
    # old flat inventory it made a vacuous filter look fine.
    INV = {
        "spar-wasm": {"render::tests::render_basic_aadl",
                      "graph::tests::test_graph_with_connections"},
        "spar-solver": {"tests::topology_is_deterministic"},
        "spar-parser": {"parse::tests::basic"},
    }

    Y_GOOD = """artifacts:
  - id: TEST-OK
    status: implemented
    fields:
      steps:
        - run: cargo test -p spar-wasm -- render_basic
"""
    Y_BAD = """artifacts:
  - id: VAL-R1
    status: implemented
    fields:
      steps:
        - run: cargo test -p spar-wasm -- layout_is_deterministic
"""
    Y_PLANNED = """artifacts:
  - id: VAL-FUTURE
    status: proposed
    fields:
      steps:
        - run: cargo test -p spar-wasm -- not_written_yet
"""
    Y_WHOLE = """artifacts:
  - id: TEST-WHOLE
    status: implemented
    fields:
      steps:
        - run: cargo test -p spar-parser
"""
    Y_OTHER = """artifacts:
  - id: TEST-SCRIPT
    status: implemented
    fields:
      steps:
        - run: tools/check_msrv.py --self-test
"""
    Y_MIXED = Y_GOOD + Y_BAD.replace("artifacts:\n", "")

    print("check_verification_filters self-test")
    # The bug itself.
    case("REGRESSION #388: filter selecting nothing must FAIL", 1, Y_BAD, INV)
    # THE #404 COUNTEREXAMPLE. `topology` matches a real test — in spar-solver,
    # not in the spar-wasm this step runs. Under the old flat workspace
    # inventory this passed, which is how #388's own executed proof case stayed
    # green after #388 was closed.
    Y_CROSS = """artifacts:
  - id: TEST-STPA-SVG-TOPOLOGY
    status: implemented
    fields:
      steps:
        - run: cargo test -p spar-wasm -- topology
"""
    case("REGRESSION #404: a filter matching another PACKAGE's test is vacuous",
         1, Y_CROSS, INV)
    # ...and the same filter is fine when the step names the package that owns
    # the test. Distinct inputs, distinct outputs: a checker that ignored `-p`
    # gives both the same verdict and cannot fail the first.
    case("...and it PASSES when -p names the package that owns the test",
         0, Y_CROSS.replace("spar-wasm", "spar-solver"), INV)
    # A package nobody could list is unjudgeable, which must not read as fine.
    case("a step naming an un-inventoried package is CANNOT-JUDGE, not a pass",
         2, Y_GOOD.replace("spar-wasm", "spar-ghost"), INV)
    # ...and it names the artifact rather than dying, which is what turns a
    # traceback into a finding. LIVE INSTANCE: four steps said `-p spar-cli`
    # while `crates/spar-cli/Cargo.toml` declares `name = "spar"`.
    case("...and it NAMES the artifact and package", 2,
         Y_GOOD.replace("spar-wasm", "spar-ghost"), INV,
         want_msg="no test inventory for package 'spar-ghost'")
    # Normal operation.
    case("filter selecting a real test passes", 0, Y_GOOD, INV)
    case("whole-package step is never vacuous", 0, Y_WHOLE, INV)
    case("non-cargo step is counted, not judged", 0, Y_OTHER, INV)
    case("one good + one vacuous still fails", 1, Y_MIXED, INV)
    # Broken scan must not read as clean.
    case("empty inventory is a broken scan, not a pass", 2, Y_GOOD, {},
         want_msg="the test inventory is empty")
    # Status semantics: a plan is not a false claim.
    case("proposed artifact planning a future test PASSES", 0, Y_PLANNED, INV)
    case("NO status is unclassifiable, so it fails closed", 1,
         Y_PLANNED.replace("    status: proposed\n", ""), INV)
    case("...but the SAME step claiming implemented FAILS", 1,
         Y_PLANNED.replace("proposed", "implemented"), INV)
    # Substring semantics, which is what libtest does.
    case("filter matching as a SUBSTRING passes", 0,
         Y_GOOD.replace("render_basic", "basic_aadl"), INV)
    # A `::`-bearing filter, because 16 of the 60 real filters carry one and no
    # case did. The discriminator: the FULL filter must be matched, not its last
    # segment. Here `nonexistent::render_basic` selects nothing even though
    # `render_basic` alone would — a checker that split on `::` and matched only
    # the tail would pass this and could not fail it.
    case("a `::` filter is matched WHOLE, not by its last segment", 1,
         Y_GOOD.replace("render_basic", "nonexistent::render_basic"), INV)
    case("...and a real module-path filter still passes", 0,
         Y_GOOD.replace("render_basic", "render::tests::render_basic"), INV)
    case("near-miss substring still fails", 1,
         Y_GOOD.replace("render_basic", "render_basicX"), INV)
    # A QUOTED step is YAML syntax, not part of the filter. The first run of
    # this gate flagged 8 steps as vacuous purely because the closing quote was
    # read as the last character of the filter (`cqf"` instead of `cqf`).
    # Without this case the parser could regress to that and the suite would
    # still be green, because every other fixture here is unquoted.
    Y_QUOTED = """artifacts:
  - id: TEST-QUOTED
    status: implemented
    fields:
      steps:
        - run: "cargo test -p spar-wasm --lib -- render_basic"
"""
    case("double-quoted step: quotes are YAML, not filter", 0, Y_QUOTED, INV)
    case("single-quoted step: same", 0,
         Y_QUOTED.replace('"cargo', "'cargo").replace('basic"', "basic'"), INV)

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--verification", default="artifacts/verification.yaml")
    ap.add_argument("--manifest-dir", default=".")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument(
        "--inventory-json",
        help="read the test inventory from a JSON array instead of running cargo",
    )
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    verification = Path(a.verification)
    if a.inventory_json:
        # {"pkg": ["name", ...]} — a flat array is no longer accepted, because a
        # flat inventory is exactly the defect (#404): it cannot express which
        # package owns a test, so every filter is judged against the wrong pool.
        raw = json.loads(Path(a.inventory_json).read_text(encoding="utf-8"))
        if not isinstance(raw, dict):
            print("::error::--inventory-json must be an object mapping package "
                  "-> [test names]. A flat list cannot say which package owns a "
                  "test, which is the #404 defect.", file=sys.stderr)
            return 2
        inv = {pkg: set(names) for pkg, names in raw.items()}
    else:
        # Only the packages some step actually names — listing the rest would
        # build crates nothing asks about.
        packages = {
            pkg
            for _aid, _status, cmd in parse_steps(verification.read_text(encoding="utf-8"))
            if (pkg := extract_filter(cmd)[0]) is not None
        }
        inv = collect_inventory(Path(a.manifest_dir), packages)
    return check(verification, inv)


if __name__ == "__main__":
    sys.exit(main())
