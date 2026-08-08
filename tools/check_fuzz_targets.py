#!/usr/bin/env python3
"""Fail when the fuzz targets RUN are not exactly the fuzz targets DECLARED.

REQ-GUARD-GATE-EVIDENCE-002 (h) — the advisory nightly.
REQ-GUARD-FUZZ-SMOKE-001     — the REQUIRED `Fuzz smoke` context (#406).

WHY THIS EXISTS
===============

`fuzz/Cargo.toml` declares the harnesses as `[[bin]]` entries. TWO workflows run
them from separately-maintained lists, and this gate compares the declaration to
BOTH:

  * `fuzz-nightly.yml` runs them from a hand-written `matrix.include` list — one
    entry per target, because each carries its own `extra_args` (only
    `fuzz_scheduler_solver` needs `-max_len=128`, and a flat list would force
    that cap onto two healthy fuzzers; see #361). That per-target detail is a
    good reason for the `include` form and is staying. This workflow is
    ADVISORY — its job name is templated (`Fuzz ${{ matrix.target }}`), which by
    `check_required_contexts.py`'s rules can never be a required context.

  * `ci.yml`'s `fuzz-smoke` job hand-writes the SAME targets as one
    `cargo +nightly fuzz run … <target>` step each. THIS job is a REQUIRED
    context (`Fuzz smoke (60s/target)` in `.github/required-contexts.txt`), so
    it is the one that actually blocks merges.

#406: the original tool audited only the nightly matrix — the advisory list —
and left the required `fuzz-smoke` job compared against nothing. Add a `[[bin]]`
and a nightly matrix entry but forget the `fuzz-smoke` step, and this tool would
report `All 4 declared fuzz targets are run` while the required gate fuzzed 3 of
4, green, forever. Auditing the advisory list and not the blocking one is the
requirement's own defect shape: the covered list does not gate, and the gating
list was uncovered.

The cost of both list forms is the same: the two lists are maintained separately
and nothing compared them. Add a harness to `fuzz/Cargo.toml` and forget a
workflow, or delete an entry, and the job stays GREEN having fuzzed a strict
subset. There is no error, no warning, and no smaller number anywhere in the
log — the run simply says `success` for the legs it did run.

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

Declared-set equality against EACH run-set, both directions, because the two
failure modes are different bugs:

  * declared but not run  -> a harness exists and is never exercised.
  * run but not declared  -> the workflow names a target cargo-fuzz cannot
    build; the leg fails, but as a build error that reads like a toolchain
    problem rather than a stale matrix.

It prints both sets and the count on every path, success included — a gate that
says nothing on success cannot distinguish "compared three" from "compared
none", the same reason `check_fmt_workspaces.py` always prints its count.

stdlib-only, and all three parsers are deliberately narrow: `[[bin]] name = "…"`
from the manifest, `- target: …` from the nightly matrix, and the target
positional of a `cargo … fuzz run … <target> [-- …]` command from `ci.yml`.

The fuzz-smoke extractor is SCOPED to the `fuzz-smoke:` job block (from its
2-space-indented key to the next top-level job) and SKIPS full-line comments —
because the string `fuzz run` also occurs in prose comments and could occur in
other jobs, and a whole-file scan that matched one of those would resolve to
several positional tokens and wrongly read the required gate as a broken scan.
That is not hypothetical: the comment above the guard step in this same `ci.yml`
contains `cargo … fuzz run … <target>`, and an unscoped scan turned the required
`Rivet validate (artifacts)` context red on the very PR that added this tool.
Within the block it drops the toolchain token (`+nightly`), the value-taking
flags a command may carry (`--target <triple>`, so the triple is never mistaken
for a harness), and the libfuzzer args after `--`; a `fuzz run` command that does
not resolve to exactly one target positional is a broken scan (exit 2), not a
silent skip, so an unrecognised flag form fails loud and is fixed here
deliberately rather than dropping a target unnoticed. None of the three files is
arbitrary YAML/TOML in practice, and a dependency this gate cannot install is a
gate that can fail to run — which reads as approval.
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
# `- target: fuzz_x` inside the nightly workflow matrix.
_MATRIX_TARGET = re.compile(r"^\s*-\s+target:\s*(\S+)\s*$", re.M)
# The `fuzz-smoke:` job key (2-space indent) and any other top-level job key, so
# the smoke scan can be confined to that one job — the string `fuzz run` also
# appears in prose comments and could appear in other jobs.
_SMOKE_JOB = re.compile(r"^  fuzz-smoke:\s*$", re.M)
_TOP_JOB = re.compile(r"^  \S")
# `cargo … fuzz run …` inside a step of that job. Everything after `fuzz run`
# is captured and picked apart in code.
_FUZZ_RUN = re.compile(r"\bfuzz\s+run\b(.*)$")
# cargo-fuzz `run` options that take a following value token; their argument
# must not be mistaken for the target positional. `--target x86_64-…` is the one
# actually used, but the run subcommand accepts these too, and an equals form
# (`--target=…`) is handled separately as a single hyphen-prefixed token.
_VALUE_FLAGS = frozenset({
    "--target", "-s", "--sanitizer", "-j", "--jobs", "--features",
    "--build-dir", "--target-dir", "-D", "--dev", "--release",
})


def declared_targets(manifest_text: str) -> set[str]:
    """Every `[[bin]]` name in fuzz/Cargo.toml."""
    out: set[str] = set()
    for m in _BIN_HEADER.finditer(manifest_text):
        nm = _NAME.search(manifest_text, m.end())
        if nm:
            out.add(nm.group(1))
    return out


def matrix_targets(workflow_text: str) -> set[str]:
    """Every `- target:` in the nightly fuzz workflow matrix."""
    return {m.group(1).strip('"\'') for m in _MATRIX_TARGET.finditer(workflow_text)}


def _one_smoke_target(after_run: str) -> str | None:
    """The single target positional of one `cargo … fuzz run …` command.

    Returns the harness name, or None if the command does not resolve to
    EXACTLY one positional — which is a broken scan, handled loudly by the
    caller rather than dropped. Drops the libfuzzer args after `--`, the
    `+toolchain` token, value-taking flags together with their argument, and any
    remaining hyphen-prefixed flag (covering the `--flag=value` form).
    """
    head = after_run.split(" -- ", 1)[0]
    toks = head.split()
    positionals: list[str] = []
    i = 0
    while i < len(toks):
        t = toks[i]
        if t in _VALUE_FLAGS:
            i += 2  # skip the flag and its value
            continue
        if t.startswith("-") or t.startswith("+"):
            i += 1  # a bare flag, or the `--flag=value` / `+toolchain` form
            continue
        positionals.append(t)
        i += 1
    return positionals[0] if len(positionals) == 1 else None


def _smoke_job_block(workflow_text: str) -> str | None:
    """The `fuzz-smoke:` job block, key line to the next top-level job key.

    None if there is no `fuzz-smoke:` job — which the caller treats as a broken
    scan (the required job vanished), not an empty pass.
    """
    lines = workflow_text.splitlines()
    start = None
    for i, ln in enumerate(lines):
        if _SMOKE_JOB.match(ln):
            start = i
            break
    if start is None:
        return None
    end = len(lines)
    for j in range(start + 1, len(lines)):
        if _TOP_JOB.match(lines[j]):
            end = j
            break
    return "\n".join(lines[start:end])


def smoke_targets(workflow_text: str) -> set[str] | None:
    """Every target run by a `cargo … fuzz run` step in ci.yml's fuzz-smoke job.

    Scoped to the `fuzz-smoke:` job block and skipping full-line comments, so a
    `fuzz run` string in a prose comment or an unrelated job cannot poison the
    scan (an earlier whole-file version matched the guard step's own comment and
    turned the required gate red, #406).

    None signals a broken scan: no `fuzz-smoke:` job at all, or a `fuzz run`
    command that did not resolve to exactly one target positional (an
    unrecognised flag form, say). Either must fail the gate rather than silently
    drop a leg — the whole point of #406.
    """
    block = _smoke_job_block(workflow_text)
    if block is None:
        return None
    out: set[str] = set()
    for raw in block.splitlines():
        if raw.lstrip().startswith("#"):
            continue  # a full-line YAML/shell comment is never a run command
        m = _FUZZ_RUN.search(raw)
        if not m:
            continue
        tgt = _one_smoke_target(m.group(1))
        if tgt is None:
            return None
        out.add(tgt)
    return out


def _compare(declared: set[str], run: set[str] | None, manifest: Path,
             run_src: str, run_desc: str, out=sys.stdout) -> int:
    print(f"== fuzz-target guardrail: {run_desc} ==", file=out)
    print(f"declared in {manifest}: {len(declared)}  {sorted(declared)}", file=out)
    if run is None:
        print(f"run by      {run_src}: <unparsable>", file=out)
        print(f"::error::a `fuzz run` step in {run_src} did not resolve to "
              "exactly one target — a broken scan, not a disabled job.", file=out)
        return 2
    print(f"run by      {run_src}: {len(run)}  {sorted(run)}", file=out)

    # An empty side is a broken scan, not a finding. Without this, deleting the
    # [[bin]] section or renaming the matrix key / step form would make both
    # sets empty and the equality check would pass — the gate reporting perfect
    # agreement about nothing.
    if not declared:
        print("::error::no `[[bin]]` targets found — the manifest parse read "
              "nothing, which is a broken scan, not an empty fuzz suite.", file=out)
        return 2
    if not run:
        print(f"::error::no fuzz targets found in {run_src} — the parse read "
              "nothing, which is a broken scan, not a disabled job.", file=out)
        return 2

    missing = sorted(declared - run)
    extra = sorted(run - declared)
    if missing:
        for t in missing:
            print(f"::error::{t} is declared in fuzz/Cargo.toml but NOT run by "
                  f"{run_desc} — it is never fuzzed and nothing says so.", file=out)
    if extra:
        for t in extra:
            print(f"::error::{t} is run by {run_desc} but NOT declared in "
                  f"fuzz/Cargo.toml — that leg cannot build.", file=out)
    if missing or extra:
        return 1

    print(f"\nAll {len(declared)} declared fuzz targets are run by {run_desc}.",
          file=out)
    return 0


def check(manifest: Path, workflow: Path, out=sys.stdout) -> int:
    """Declared set vs the nightly matrix `include` list."""
    declared = declared_targets(manifest.read_text(encoding="utf-8"))
    run = matrix_targets(workflow.read_text(encoding="utf-8"))
    return _compare(declared, run, manifest, str(workflow),
                    "the nightly matrix", out=out)


def check_smoke(manifest: Path, ci_workflow: Path, out=sys.stdout) -> int:
    """Declared set vs the REQUIRED ci.yml fuzz-smoke steps (#406)."""
    declared = declared_targets(manifest.read_text(encoding="utf-8"))
    run = smoke_targets(ci_workflow.read_text(encoding="utf-8"))
    return _compare(declared, run, manifest, str(ci_workflow),
                    "the fuzz-smoke job", out=out)


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

# ci.yml's fuzz-smoke job form: one `cargo … fuzz run … <target>` step each,
# carrying the real `--target <triple>` flag whose value must not be read as a
# harness, and libfuzzer args after `--`. Matches the two _MANIFEST bins.
_CI = """  fuzz-smoke:
    name: Fuzz smoke (60s/target)
    steps:
      - name: fuzz_aadl_parse (60s)
        run: cargo +nightly fuzz run --target x86_64-unknown-linux-gnu fuzz_aadl_parse -- -max_total_time=60 -timeout=10
      - name: fuzz_scheduler_solver (60s)
        run: cargo +nightly fuzz run --target x86_64-unknown-linux-gnu fuzz_scheduler_solver -- -max_total_time=60 -timeout=5
"""


def self_test() -> int:
    passed = failed = 0

    def case(desc: str, want: int, manifest: str, workflow: str,
             checker=check) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            m = Path(td) / "Cargo.toml"
            w = Path(td) / "fuzz.yml"
            m.write_text(manifest, encoding="utf-8")
            w.write_text(workflow, encoding="utf-8")
            buf = io.StringIO()
            got = checker(m, w, out=buf)
        if got == want:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want {want}")
            for line in buf.getvalue().splitlines()[:6]:
                print(f"         {line}")

    print("check_fuzz_targets self-test")
    print(" nightly matrix (REQ-GUARD-GATE-EVIDENCE-002 h)")
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

    print(" fuzz-smoke steps (REQ-GUARD-FUZZ-SMOKE-001, #406)")
    # Base case doubles as the non-vacuity proof for the extractor: it only
    # passes if the `--target x86_64-unknown-linux-gnu` triple is NOT read as a
    # third harness (that would be an `extra`, exit 1) and the libfuzzer args
    # after `--` are dropped.
    case("declared set == fuzz-smoke run set", 0, _MANIFEST, _CI,
         checker=check_smoke)
    # THE #406 ACCEPTANCE CRITERION: a [[bin]] added to fuzz/Cargo.toml but not
    # to ci.yml's fuzz-smoke job must fail a gate. Distinct input (3 declared),
    # distinct output (exit 1) from the base case (2 declared, exit 0) — the
    # property the old advisory-only tool lacked.
    case("REGRESSION #406: declared but NOT in fuzz-smoke must FAIL", 1,
         _MANIFEST + '\n[[bin]]\nname = "fuzz_codegen_roundtrip"\n', _CI,
         checker=check_smoke)
    case("fuzz-smoke runs a target NOT declared must FAIL", 1, _MANIFEST,
         _CI + "      - name: fuzz_ghost\n        run: cargo +nightly fuzz run "
               "--target x86_64-unknown-linux-gnu fuzz_ghost -- -max_total_time=60\n",
         checker=check_smoke)
    # No `fuzz run` step at all is a broken scan, not a disabled job.
    case("no fuzz-smoke steps is a broken scan", 2, _MANIFEST,
         "  fuzz-smoke:\n    steps:\n      - name: checkout\n        run: echo hi\n",
         checker=check_smoke)
    # A `fuzz run` step that does not resolve to one target fails loud (exit 2)
    # rather than dropping the leg — e.g. an unrecognised value-flag form that
    # leaves two positionals.
    case("fuzz run with an ambiguous command is a broken scan", 2, _MANIFEST,
         "  fuzz-smoke:\n    steps:\n      - run: cargo +nightly fuzz run "
         "--unknown-value-flag stray_value fuzz_aadl_parse\n",
         checker=check_smoke)
    # The `--flag=value` equals form is a single hyphen token, correctly a flag.
    case("equals-form flag does not leak a positional", 0, _MANIFEST,
         _CI.replace("--target x86_64-unknown-linux-gnu",
                     "--target=x86_64-unknown-linux-gnu"),
         checker=check_smoke)
    # THE REGRESSION AN EARLIER VERSION SHIPPED: a `fuzz run` string in a prose
    # comment of ANOTHER job (here `guard:`) must not poison the scan. A
    # whole-file scan matched exactly this and read the required gate as a
    # broken scan; the block scoping must exit 0 here, not 2.
    case("a `fuzz run` comment in another job does not poison the scan", 0,
         _MANIFEST,
         _CI + "  guard:\n    steps:\n      # one `cargo fuzz run <target>` "
               "step per harness — poisoning comment\n      - run: echo audit\n",
         checker=check_smoke)
    # A comment INSIDE the fuzz-smoke block is skipped too (defense in depth).
    case("a `fuzz run` comment inside the block is skipped", 0, _MANIFEST,
         _CI.replace("    steps:\n",
                     "    steps:\n      # cargo fuzz run <target> — inside-block "
                     "comment must be skipped\n"),
         checker=check_smoke)
    # No fuzz-smoke job at all is a broken scan (the required job vanished), not
    # an empty pass.
    case("ci.yml with no fuzz-smoke job is a broken scan", 2, _MANIFEST,
         "jobs:\n  other:\n    steps:\n      - run: echo hi\n",
         checker=check_smoke)

    # Real-tree guard. The synthetic fixtures above cannot catch a `fuzz run`
    # string in a COMMENT of the ACTUAL committed ci.yml — the exact regression
    # an earlier whole-file scan shipped (#406). If the repo files are reachable
    # from cwd, assert the shipped tree is clean under BOTH audits; this is the
    # row that fails loudly the moment a poisoning comment lands in ci.yml.
    print(" real committed tree (if reachable)")

    def real_case(desc: str, checker, run_path: str) -> None:
        nonlocal passed, failed
        manifest = Path("fuzz/Cargo.toml")
        wf = Path(run_path)
        if not (manifest.exists() and wf.exists()):
            print(f"  skip {desc} (repo files not reachable from cwd)")
            return
        buf = io.StringIO()
        got = checker(manifest, wf, out=buf)
        if got == 0:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want 0")
            for line in buf.getvalue().splitlines()[:6]:
                print(f"         {line}")

    real_case("real ci.yml fuzz-smoke is clean (no comment poisons it)",
              check_smoke, ".github/workflows/ci.yml")
    real_case("real fuzz-nightly matrix is clean",
              check, ".github/workflows/fuzz-nightly.yml")
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--manifest", default="fuzz/Cargo.toml")
    ap.add_argument("--workflow", default=".github/workflows/fuzz-nightly.yml",
                    help="advisory nightly workflow (matrix include form)")
    ap.add_argument("--ci-workflow", default=".github/workflows/ci.yml",
                    help="workflow carrying the REQUIRED fuzz-smoke job (#406)")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    # Audit the declaration against BOTH run-lists; the worst exit wins so a
    # broken scan (2) is never masked by a clean sibling (0).
    rc_nightly = check(Path(a.manifest), Path(a.workflow))
    rc_smoke = check_smoke(Path(a.manifest), Path(a.ci_workflow))
    return max(rc_nightly, rc_smoke)


if __name__ == "__main__":
    sys.exit(main())
