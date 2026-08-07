#!/usr/bin/env python3
"""Fail when the fuzz targets RUN are not exactly the fuzz targets DECLARED.

REQ-GUARD-GATE-EVIDENCE-002 (h).

WHY THIS EXISTS
===============

`fuzz/Cargo.toml` declares the harnesses as `[[bin]]` entries. `fuzz-nightly.yml`
runs them from a hand-written `matrix.include` list — one entry per target,
because each carries its own `extra_args` (only `fuzz_scheduler_solver` needs
`-max_len=128`, and a flat list would force that cap onto two healthy fuzzers;
see #361). That per-target detail is a good reason for the `include` form and is
staying.

The cost of the `include` form is that the two lists are maintained separately
and nothing compares them. Add a harness to `fuzz/Cargo.toml` and forget the
workflow, or delete a matrix entry, and the job stays GREEN having fuzzed a
strict subset. There is no error, no warning, and no smaller number anywhere in
the log — the run simply says `success` for the legs it did run.

That is the family this requirement is about: the shortfall renders as the ideal
reading. A fuzz job that fuzzed two of three targets looks exactly like a fuzz
job that fuzzed three, because success is reported per leg and nobody counts the
legs.

Nightly fuzzing is also where this hides longest — it is not on the PR gate, so
a dropped target can go unnoticed for months. `fuzz_scheduler_solver` was red
97/97 runs from 2026-04-25 before anyone looked (#361); a *silently absent* leg
is strictly harder to notice than a loudly failing one.

WHAT IT CHECKS
==============

Set equality, both directions, because the two failure modes are different bugs:

  * declared but not run  -> a harness exists and is never exercised.
  * run but not declared  -> the workflow names a target cargo-fuzz cannot
    build; the leg fails, but as a build error that reads like a toolchain
    problem rather than a stale matrix.

It prints both sets and the count on every path, success included — a gate that
says nothing on success cannot distinguish "compared three" from "compared
none", the same reason `check_fmt_workspaces.py` always prints its count.

stdlib-only, and both parsers are deliberately narrow: `[[bin]] name = "..."`
from the manifest, `- target: ...` from the matrix. Neither file is arbitrary
YAML/TOML in practice, and a dependency this gate cannot install is a gate that
can fail to run — which reads as approval.
"""

from __future__ import annotations

import argparse
import io
import re
import sys
import tempfile
from pathlib import Path

# `[[bin]]` … `name = "fuzz_x"` — the first `name` after each [[bin]] header.
_BIN_HEADER = re.compile(r"^\s*\[\[bin\]\]\s*$", re.M)
_NAME = re.compile(r'^\s*name\s*=\s*"([^"]+)"\s*$', re.M)
# `- target: fuzz_x` inside the workflow matrix.
_MATRIX_TARGET = re.compile(r"^\s*-\s+target:\s*(\S+)\s*$", re.M)


def declared_targets(manifest_text: str) -> set[str]:
    """Every `[[bin]]` name in fuzz/Cargo.toml."""
    out: set[str] = set()
    for m in _BIN_HEADER.finditer(manifest_text):
        nm = _NAME.search(manifest_text, m.end())
        if nm:
            out.add(nm.group(1))
    return out


def matrix_targets(workflow_text: str) -> set[str]:
    """Every `- target:` in the fuzz workflow matrix."""
    return {m.group(1).strip('"\'') for m in _MATRIX_TARGET.finditer(workflow_text)}


def check(manifest: Path, workflow: Path, out=sys.stdout) -> int:
    declared = declared_targets(manifest.read_text(encoding="utf-8"))
    run = matrix_targets(workflow.read_text(encoding="utf-8"))

    print("== fuzz-target guardrail ==", file=out)
    print(f"declared in {manifest}: {len(declared)}  {sorted(declared)}", file=out)
    print(f"run by      {workflow}: {len(run)}  {sorted(run)}", file=out)

    # An empty side is a broken scan, not a finding. Without this, deleting the
    # [[bin]] section or renaming the matrix key would make both sets empty and
    # the equality check would pass — the gate reporting perfect agreement
    # about nothing.
    if not declared:
        print("::error::no `[[bin]]` targets found — the manifest parse read "
              "nothing, which is a broken scan, not an empty fuzz suite.", file=out)
        return 2
    if not run:
        print("::error::no `- target:` entries found — the workflow parse read "
              "nothing, which is a broken scan, not a disabled job.", file=out)
        return 2

    missing = sorted(declared - run)
    extra = sorted(run - declared)
    if missing:
        for t in missing:
            print(f"::error::{t} is declared in fuzz/Cargo.toml but NOT run by "
                  f"the workflow — it is never fuzzed and nothing says so.", file=out)
    if extra:
        for t in extra:
            print(f"::error::{t} is run by the workflow but NOT declared in "
                  f"fuzz/Cargo.toml — that leg cannot build.", file=out)
    if missing or extra:
        return 1

    print(f"\nAll {len(declared)} declared fuzz targets are run.", file=out)
    return 0


_MANIFEST = """[package]
name = "spar-fuzz"

[[bin]]
name = "fuzz_aadl_parse"
path = "fuzz_targets/fuzz_aadl_parse.rs"

[[bin]]
name = "fuzz_scheduler_solver"
path = "fuzz_targets/fuzz_scheduler_solver.rs"
"""

_WORKFLOW = """jobs:
  fuzz:
    strategy:
      matrix:
        include:
          - target: fuzz_aadl_parse
            extra_args: ""
          - target: fuzz_scheduler_solver
            extra_args: "-max_len=128"
"""


def self_test() -> int:
    passed = failed = 0

    def case(desc: str, want: int, manifest: str, workflow: str) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            m = Path(td) / "Cargo.toml"
            w = Path(td) / "fuzz.yml"
            m.write_text(manifest, encoding="utf-8")
            w.write_text(workflow, encoding="utf-8")
            buf = io.StringIO()
            got = check(m, w, out=buf)
        if got == want:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want {want}")
            for line in buf.getvalue().splitlines()[:5]:
                print(f"         {line}")

    print("check_fuzz_targets self-test")
    case("declared set == run set", 0, _MANIFEST, _WORKFLOW)
    # The bug this exists for: a harness that is never fuzzed.
    case("REGRESSION: declared but NOT run must FAIL", 1,
         _MANIFEST + '\n[[bin]]\nname = "fuzz_codegen_roundtrip"\n', _WORKFLOW)
    case("run but NOT declared must FAIL", 1,
         _MANIFEST, _WORKFLOW + "          - target: fuzz_ghost\n")
    # Broken scans must not read as agreement.
    case("empty manifest is a broken scan, not an empty suite", 2, "", _WORKFLOW)
    case("empty workflow matrix is a broken scan, not a disabled job", 2,
         _MANIFEST, "jobs:\n  fuzz:\n")
    # Order and quoting are not signal.
    case("matrix order does not matter", 0, _MANIFEST,
         """jobs:
  fuzz:
    strategy:
      matrix:
        include:
          - target: fuzz_scheduler_solver
          - target: fuzz_aadl_parse
""")
    case("quoted matrix target is unquoted before comparing", 0, _MANIFEST,
         _WORKFLOW.replace("- target: fuzz_aadl_parse", '- target: "fuzz_aadl_parse"'))
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--manifest", default="fuzz/Cargo.toml")
    ap.add_argument("--workflow", default=".github/workflows/fuzz-nightly.yml")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return check(Path(a.manifest), Path(a.workflow))


if __name__ == "__main__":
    sys.exit(main())
