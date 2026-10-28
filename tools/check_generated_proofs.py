#!/usr/bin/env python3
"""Lake-build the Lean proofs `spar codegen --verify proof` writes.

REQ-PROOF-GEN-COMPILES-001. Refs #542.

WHY THIS EXISTS
===============

`spar codegen --verify proof` wrote Lean files that `import
Proofs.Scheduling.RTA` and then called `compute_response_time`, a function no
module under `proofs/Proofs` defines. Nothing compiled a generated proof, so
the generator and the library it imports drifted apart and no gate noticed.

This tool is that gate. It:

1. runs `spar codegen --verify proof` on one fixture model (`FIXTURE` below,
   or `--model`);
2. rejects the output if it holds `sorry`, `admit` or `native_decide`, or has
   no theorem at all (an empty file builds green and proves nothing);
3. wraps the files in a throwaway Lake project that `require`s the in-repo
   `proofs/` library by path and shares its already-built `.lake/packages`, so
   Mathlib is not fetched or rebuilt a second time;
4. runs `lake build` with `warningAsError`, so a `sorry` warning also fails;
5. prints the axioms of every generated theorem and fails on `sorryAx` or
   `Lean.ofReduceBool`;
6. with `--negative-control`, rebuilds three more times with a broken
   theorem (a deadline below the response time; a wrong fixed-point witness;
   a proof replaced by `sorry`, bypassing step 2 so `warningAsError` itself is
   tested) and fails unless ALL of those builds fail. A build that accepts a
   false theorem is not checking anything.

Run it after `lake build` in `proofs/`, from the repo root:

    tools/check_generated_proofs.py --spar target/debug/spar --negative-control
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_ROOT = "Rta::Top.impl"

# The model the gate generates from. It lives here, not as a .aadl under
# test-data/: every first-party .aadl must carry committed OSATE and Ocarina
# verdicts (crates/spar-cli/tests/first_party_legality.rs and the interop
# baselines), and CI has neither tool. proof_gen.rs's unit tests hold the same
# text as `RTA_FIXTURE` and a test there fails if the two copies differ.
#
# Rate-monotonic order and the response times the generator computes:
#   fast  T=5ms  D=5ms  C=1ms                  R=1ms  meets
#   mid   T=10ms D=10ms C=2ms  hp {fast}        R=3ms  meets
#   slow  T=20ms D=20ms C=5ms  hp {fast,mid}    R=9ms  meets (two steps)
#   late  T=40ms D=12ms C=6ms  hp {fast,mid,slow}: 6 -> 15ms > 12ms, misses
# so both theorem shapes (`_meets_deadline`, `_misses_deadline`) are built.
# BEGIN RTA_FIXTURE
FIXTURE = """\
package Rta
public
  processor Cpu
  end Cpu;
  thread Fast
    properties
      Dispatch_Protocol => Periodic;
      Period => 5 ms;
      Deadline => 5 ms;
      Compute_Execution_Time => 1 ms .. 1 ms;
  end Fast;
  thread Mid
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end Mid;
  thread Slow
    properties
      Dispatch_Protocol => Periodic;
      Period => 20 ms;
      Deadline => 20 ms;
      Compute_Execution_Time => 5 ms .. 5 ms;
  end Slow;
  thread Late
    properties
      Dispatch_Protocol => Periodic;
      Period => 40 ms;
      Deadline => 12 ms;
      Compute_Execution_Time => 6 ms .. 6 ms;
  end Late;
  process App
  end App;
  process implementation App.impl
    subcomponents
      fast : thread Fast;
      mid : thread Mid;
      slow : thread Slow;
      late : thread Late;
  end App.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      app : process App.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.fast;
      Actual_Processor_Binding => (reference (cpu)) applies to app.mid;
      Actual_Processor_Binding => (reference (cpu)) applies to app.slow;
      Actual_Processor_Binding => (reference (cpu)) applies to app.late;
  end Top.impl;
end Rta;
"""
# END RTA_FIXTURE

FORBIDDEN = re.compile(r"\b(sorry|admit|native_decide)\b")
THEOREM = re.compile(r"^theorem\s+(\S+)", re.MULTILINE)
AXIOMS = re.compile(r"'([^']+)' depends on axioms: \[([^\]]*)\]")
# `'X' does not depend on any axioms` is also a valid (clean) answer.
NO_AXIOMS = re.compile(r"'([^']+)' does not depend on any axioms")
BAD_AXIOMS = {"sorryAx", "Lean.ofReduceBool", "Lean.trustCompiler"}


def run(cmd: list[str], cwd: Path, env: dict[str, str] | None = None) -> tuple[int, str]:
    proc = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def generate(spar: Path, model: Path, root: str, out: Path) -> list[Path]:
    rc, log = run([str(spar), "codegen", "--root", root, "--verify", "proof",
                   "--output", str(out), str(model)], cwd=REPO)
    if rc != 0:
        print(log)
        print(f"::error::spar codegen exited {rc}")
        sys.exit(1)
    files = sorted((out / "proofs").glob("*.lean"))
    if not files:
        print(f"::error::spar codegen wrote no .lean file under {out / 'proofs'}")
        sys.exit(1)
    return files


def lint(files: list[Path]) -> list[str]:
    """Theorem names across the files; exits on a forbidden token or none."""
    theorems: list[str] = []
    for f in files:
        text = f.read_text(encoding="utf-8")
        for n, line in enumerate(text.splitlines(), 1):
            code = line.split("--", 1)[0]
            m = FORBIDDEN.search(code)
            if m:
                print(f"::error::{f.name}:{n}: generated proof uses `{m.group(1)}`: {line.strip()}")
                sys.exit(1)
        theorems += THEOREM.findall(text)
    if not theorems:
        print("::error::the generated proofs hold no theorem; a build of them proves nothing")
        sys.exit(1)
    return theorems


def namespace_of(text: str) -> str:
    m = re.search(r"^namespace\s+(\S+)", text, re.MULTILINE)
    return m.group(1) + "." if m else ""


def make_project(proj: Path, proofs: Path, files: list[Path]) -> list[str]:
    """Write the wrapper project; returns the fully qualified theorem names."""
    if proj.exists():
        shutil.rmtree(proj)
    (proj / "Generated").mkdir(parents=True)
    shutil.copy(proofs / "lean-toolchain", proj / "lean-toolchain")
    (proj / "lakefile.toml").write_text(
        'name = "generated"\n'
        f'packagesDir = "{proofs / ".lake" / "packages"}"\n'
        'defaultTargets = ["Generated"]\n\n'
        "[leanOptions]\nwarningAsError = true\n\n"
        f'[[require]]\nname = "proofs"\npath = "{proofs}"\n\n'
        '[[lean_lib]]\nname = "Generated"\nglobs = ["Generated.+"]\n',
        encoding="utf-8",
    )
    qualified: list[str] = []
    modules: list[str] = []
    for f in files:
        text = f.read_text(encoding="utf-8")
        (proj / "Generated" / f.name).write_text(text, encoding="utf-8")
        modules.append("Generated." + f.stem)
        ns = namespace_of(text)
        qualified += [ns + t for t in THEOREM.findall(text)]
    axioms = "".join(f"import {m}\n" for m in modules) + "\n"
    axioms += "".join(f"#print axioms {q}\n" for q in qualified)
    (proj / "Generated" / "ZAxioms.lean").write_text(axioms, encoding="utf-8")
    return qualified


def lake_build(proj: Path) -> tuple[int, str]:
    env = dict(os.environ)
    # The wrapper has no manifest yet, so Lake resolves it on first build and
    # runs Mathlib's post-update hook, which would `lake exe cache get` into the
    # wrong directory. The packages are already there and built.
    env["MATHLIB_NO_CACHE_ON_UPDATE"] = "1"
    return run(["lake", "build"], cwd=proj, env=env)


def check_axioms(log: str, qualified: list[str]) -> int:
    seen: dict[str, set[str]] = {}
    for name, axs in AXIOMS.findall(log):
        seen[name] = {a.strip() for a in axs.split(",") if a.strip()}
    for name in NO_AXIOMS.findall(log):
        seen[name] = set()
    rc = 0
    for q in qualified:
        if q not in seen:
            print(f"::error::no axiom report for {q}; the build did not elaborate it")
            rc = 1
        elif seen[q] & BAD_AXIOMS:
            print(f"::error::{q} depends on {sorted(seen[q] & BAD_AXIOMS)}")
            rc = 1
        else:
            print(f"  {q}: axioms {sorted(seen[q])}")
    return rc


# Negative controls: each turns ONE generated theorem false (or its proof
# wrong). Each is applied to a fresh copy; the build MUST fail.
def control_deadline_below_wcrt(text: str) -> str | None:
    """`_meets_deadline` with the task's deadline set one below its fixed point."""
    m = re.search(
        r"(theorem \S+_meets_deadline :\n    let task : Task := \{ exec := \d+, period := \d+, "
        r"deadline := )(\d+)(.*?have hfp : isFixedPoint task hp )(\d+)",
        text, re.DOTALL)
    if not m:
        return None
    wcrt = int(m.group(4))
    return text[:m.start(2)] + str(wcrt - 1) + text[m.end(2):]


def control_wrong_witness(text: str) -> str | None:
    """`_meets_deadline` whose fixed-point witness is off by one."""
    m = re.search(r"have hfp : isFixedPoint task hp (\d+)", text)
    if not m:
        return None
    r = int(m.group(1)) + 1
    out = text[:m.start(1)] + str(r) + text[m.end(1):]
    # Keep the two sibling `have`s consistent so only the fixed point is wrong.
    rest = out[m.start(1):]
    rest = re.sub(r"(have hge : task\.exec ≤ )\d+", rf"\g<1>{r}", rest, count=1)
    rest = re.sub(r"(have hle : \()\d+", rf"\g<1>{r}", rest, count=1)
    return out[:m.start(1)] + rest


def control_sorry_proof(text: str) -> str | None:
    """`_meets_deadline` whose whole proof is `sorry`.

    `lint()` would already reject this text; the control skips `lint()` and
    checks the layer below it, that `warningAsError` turns the `declaration uses
    'sorry'` warning into a failed build.
    """
    m = re.search(r"(theorem \S+_meets_deadline :.*?:= by\n)(.*?)(\n\n)", text, re.DOTALL)
    if not m:
        return None
    return text[:m.start(2)] + "  sorry" + text[m.end(2):]


CONTROLS = [control_deadline_below_wcrt, control_wrong_witness, control_sorry_proof]


def negative_controls(work: Path, proofs: Path, files: list[Path]) -> int:
    rc = 0
    target = files[0]
    for control in CONTROLS:
        mutated = control(target.read_text(encoding="utf-8"))
        if mutated is None:
            print(f"::error::negative control {control.__name__} found no "
                  f"`_meets_deadline` theorem in {target.name} to falsify")
            rc = 1
            continue
        mdir = work / ("mut-" + control.__name__)
        (mdir / "proofs").mkdir(parents=True, exist_ok=True)
        mfile = mdir / "proofs" / target.name
        mfile.write_text(mutated, encoding="utf-8")
        make_project(mdir / "project", proofs, [mfile])
        brc, log = lake_build(mdir / "project")
        if brc == 0:
            print(log)
            print(f"::error::negative control {control.__name__}: lake build ACCEPTED a "
                  f"false theorem (exit 0). The gate cannot fail.")
            rc = 1
        else:
            errs = [ln for ln in log.splitlines() if "error" in ln][:3]
            print(f"  negative control {control.__name__}: lake build exit {brc} (expected)")
            for ln in errs:
                print(f"    {ln}")
    return rc


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    ap.add_argument("--spar", required=True, type=Path, help="spar binary")
    ap.add_argument("--proofs-dir", type=Path, default=REPO / "proofs",
                    help="the in-repo Lean library, already `lake build`-ed")
    ap.add_argument("--model", type=Path, default=None,
                    help="AADL model (default: the built-in FIXTURE)")
    ap.add_argument("--root", default=DEFAULT_ROOT)
    ap.add_argument("--workdir", type=Path, default=None,
                    help="scratch directory (default: a fresh temp dir)")
    ap.add_argument("--negative-control", action="store_true",
                    help="also check that a false theorem fails the build")
    a = ap.parse_args()

    proofs = a.proofs_dir.resolve()
    if not (proofs / ".lake" / "packages").is_dir():
        print(f"::error::{proofs}/.lake/packages is missing; `lake build` in {proofs} first")
        return 2
    work = (a.workdir or Path(tempfile.mkdtemp(prefix="spar-genproof-"))).resolve()
    work.mkdir(parents=True, exist_ok=True)

    model = a.model
    if model is None:
        model = work / "rta_proof.aadl"
        model.write_text(FIXTURE, encoding="utf-8")
    files = generate(a.spar.resolve(), model.resolve(), a.root, work / "out")
    theorems = lint(files)
    print(f"generated {len(files)} file(s), {len(theorems)} theorem(s): {', '.join(theorems)}")

    qualified = make_project(work / "project", proofs, files)
    rc, log = lake_build(work / "project")
    print(log)
    if rc != 0:
        print(f"::error::lake build of the generated proofs exited {rc}")
        return 1
    if check_axioms(log, qualified) != 0:
        return 1
    print("generated proofs build against proofs/ with no sorry.")

    if a.negative_control and negative_controls(work, proofs, files) != 0:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
