#!/usr/bin/env python3
"""Fail when a proof depends on a trust escape the committed ledger does not list.

REQ-PROOF-AXIOM-LEDGER-001.

WHY THIS EXISTS
===============

`check_lean_sorries.py` counts `sorry`, `sorryAx`, `admit` and `axiom` in the
TEXT of `proofs/Proofs/` and holds the total to a ratchet. A count cannot say
WHICH declarations are unproven, and text cannot see what the kernel sees:

* Moving a `sorry` from one theorem to another keeps the count at 12, so the
  ratchet stays green while a theorem that was proven no longer is.
* A theorem that USES a sorried lemma depends on `sorryAx` without containing
  the word `sorry`. Only the elaborated term shows that.
* `native_decide` is not in the text gate's pattern set at all. It trusts the
  compiler: on the pinned Lean 4.29 each use adds an auxiliary axiom named
  `<decl>._native.native_decide.ax_N_M` (older Lean: `Lean.ofReduceBool`).

`check_generated_proofs.py` already runs `#print axioms` on the theorems
`spar codegen --verify proof` writes. This tool does the same for the
hand-written library, by name.

WHAT IT DOES
============

Lean half. Writes a throwaway Lean file that imports every module under
`proofs/Proofs/`, walks the elaborated environment, and prints the axioms
(`Lean.collectAxioms`, transitive) of every constant those modules declare:
theorems, defs and private declarations alike. The names come from Lean, not
from a regex over the source. `propext`, `Quot.sound` and `Classical.choice`
are the standard axioms; any other axiom a declaration depends on is a trust
escape and becomes a ledger entry `lean <module> <kind> <decl> <axiom>`.

Verus half. Scans tracked `.rs` files for `#[verifier::external_body]`,
`assume_specification`, `assume(...)` and `admit()`, each attributed to its
function, as `verus <path> <fn> <kind>` (one line per occurrence). Kani's
`kani::assume` is a harness precondition, not a trust escape, and is excluded.

The committed ledger `.github/trust-ledger.txt` lists today's set. The check
fails (exit 1) on an escape the ledger does not list. An entry the ledger lists
that no longer occurs is a ::notice, never a failure: removing a trust escape
is always allowed, and the notice says which line to delete.

`--negative-control` adds canary declarations (`by sorry`, `native_decide`, a
custom `axiom` and a theorem using it) to the same Lean run and fails unless
every canary is reported as an unlisted escape. The step therefore shows on
every run that it can go red.

Exit codes: 0 = every escape is listed, 1 = an unlisted escape, 2 = the check
could not judge (Lean failed, empty report, malformed ledger).

    tools/check_trust_ledger.py --negative-control   # CI, after lake build
    tools/check_trust_ledger.py --write              # regenerate the ledger
    tools/check_trust_ledger.py --self-test
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path
from typing import Iterable, List, Optional, TextIO, Tuple

REPO = Path(__file__).resolve().parent.parent
LEDGER = REPO / ".github" / "trust-ledger.txt"
PROOFS = REPO / "proofs"

#: The axioms Mathlib-style classical mathematics rests on. Everything else a
#: declaration depends on (sorryAx, Lean.ofReduceBool, Lean.trustCompiler, a
#: user `axiom`) is a trust escape.
STANDARD_AXIOMS = frozenset({"propext", "Quot.sound", "Classical.choice"})

CANARY_NS = "TrustLedgerCanary"
CANARY_MODULE = "<canary>"
CANARY_SOURCE = f"""
namespace {CANARY_NS}
theorem by_sorry : (1 : Nat) = 2 := by sorry
theorem by_native_decide : (2 : Nat) + 2 = 4 := by native_decide
axiom cheat : (7 : Nat) = 8
theorem uses_cheat : (7 : Nat) = 8 := cheat
end {CANARY_NS}
"""
CANARY_DECLS = frozenset(
    f"{CANARY_NS}.{n}" for n in ("by_sorry", "by_native_decide", "cheat", "uses_cheat"))

# The metaprogram. One line per constant of a Proofs.* module (and, with the
# canary, per canary constant), then a SUMMARY line with the denominators.
REPORT_PROGRAM = r"""
open Lean Elab Command in
#eval show CommandElabM Unit from do
  let env ← getEnv
  let mut out : Array String := #[]
  let mut nall := 0
  let mut nthm := 0
  let mut mods : Std.HashSet Name := {}
  let mut cands : Array (Name × String) := #[]
  for (n, _) in env.constants.map₁.toList do
    let some idx := env.getModuleIdxFor? n | continue
    let mod := env.header.moduleNames[idx.toNat]!
    if (`Proofs).isPrefixOf mod then
      cands := cands.push (n, mod.toString)
      mods := mods.insert mod
  for (n, _) in env.constants.map₂.toList do
    if (`__CANARY__).isPrefixOf n then
      cands := cands.push (n, "<canary>")
  for (n, mod) in cands do
    let some ci := env.find? n | continue
    let kind := match ci with
      | .thmInfo _ => "theorem"
      | .axiomInfo _ => "axiom"
      | .defnInfo _ => "def"
      | .opaqueInfo _ => "opaque"
      | _ => "other"
    if mod != "<canary>" then
      nall := nall + 1
      if kind == "theorem" then nthm := nthm + 1
    let axs ← liftCoreM (Lean.collectAxioms n)
    let user := (privateToUserName? n).getD n
    let axstr := String.intercalate "," (axs.toList.map toString)
    out := out.push s!"DECL\t{mod}\t{kind}\t{user}\t{axstr}"
  for l in out.qsort (· < ·) do IO.println l
  IO.println s!"SUMMARY\t{mods.size}\t{nall}\t{nthm}"
""".replace("__CANARY__", CANARY_NS)


class Unjudgeable(Exception):
    """The check cannot reach a verdict (exit 2)."""


# ── Lean half ──────────────────────────────────────────────────────────────


def proof_modules(proofs: Path) -> List[str]:
    """Module names of every first-party Lean file under proofs/Proofs."""
    mods = []
    root = proofs / "Proofs.lean"
    if root.is_file():
        mods.append("Proofs")
    for f in sorted((proofs / "Proofs").rglob("*.lean")):
        rel = f.relative_to(proofs).with_suffix("")
        mods.append(".".join(rel.parts))
    return mods


def lean_source(modules: List[str], canary: bool) -> str:
    src = "".join(f"import {m}\n" for m in modules) + "import Lean\n"
    if canary:
        src += CANARY_SOURCE
    return src + REPORT_PROGRAM


def run_lean(proofs: Path, canary: bool) -> str:
    modules = proof_modules(proofs)
    if not modules:
        raise Unjudgeable(f"no Lean module under {proofs}/Proofs; nothing to judge")
    with tempfile.TemporaryDirectory(prefix="trust-ledger-") as tmp:
        f = Path(tmp) / "TrustLedgerReport.lean"
        f.write_text(lean_source(modules, canary), encoding="utf-8")
        proc = subprocess.run(["lake", "env", "lean", str(f)], cwd=proofs,
                              capture_output=True, text=True)
    if proc.returncode != 0:
        raise Unjudgeable(f"`lake env lean` exited {proc.returncode} (run `lake build` "
                          f"in {proofs} first):\n{proc.stdout}{proc.stderr}")
    return proc.stdout


def parse_report(text: str) -> Tuple[List[Tuple[str, str, str, List[str]]], Tuple[int, int, int]]:
    """(declarations, (modules, constants, theorems)) from the metaprogram output."""
    decls = []
    summary = None
    for line in text.splitlines():
        parts = line.split("\t")
        if parts[0] == "DECL" and len(parts) == 5:
            axs = [a for a in parts[4].split(",") if a]
            decls.append((parts[1], parts[2], parts[3], axs))
        elif parts[0] == "SUMMARY" and len(parts) == 4:
            summary = (int(parts[1]), int(parts[2]), int(parts[3]))
    if summary is None:
        raise Unjudgeable("the Lean report has no SUMMARY line; the metaprogram did not run")
    if summary[2] == 0:
        raise Unjudgeable("the Lean report saw 0 theorems; an empty report proves nothing")
    return decls, summary


def lean_entries(decls) -> List[str]:
    out = []
    for mod, kind, name, axs in decls:
        for a in axs:
            if a not in STANDARD_AXIOMS:
                out.append("\t".join(("lean", mod, kind, name, a)))
    return out


# ── Verus half ─────────────────────────────────────────────────────────────

VERUS_PATTERNS = [
    ("external_body", re.compile(r"#\[\s*verifier\s*(?:::\s*external_body|\(\s*external_body\s*\))\s*\]")),
    ("assume_specification", re.compile(r"\bassume_specification\b")),
    # `kani::assume(` is preceded by `:`, `fn assume(` by `fn `; neither counts.
    ("assume", re.compile(r"(?<![\w:])(?<!fn )assume\s*\(")),
    ("admit", re.compile(r"(?<![\w:])(?<!fn )admit\s*\(\s*\)")),
]
FN = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")


def strip_line_comment(line: str) -> str:
    return line.split("//", 1)[0]


def verus_entries_in(path: str, text: str) -> List[str]:
    lines = [strip_line_comment(ln) for ln in text.splitlines()]
    out = []
    last_fn = "<top>"
    for i, line in enumerate(lines):
        for kind, pat in VERUS_PATTERNS:
            for _ in pat.finditer(line):
                if kind == "external_body":
                    # The attribute precedes the function it trusts.
                    fn = next((m.group(1) for ln in lines[i:] for m in [FN.search(ln)] if m),
                              "<top>")
                else:
                    m = FN.search(line)
                    fn = m.group(1) if m else last_fn
                out.append("\t".join(("verus", path, fn, kind)))
        m = FN.search(line)
        if m:
            last_fn = m.group(1)
    return out


def verus_entries(repo: Path) -> List[str]:
    proc = subprocess.run(["git", "ls-files", "*.rs"], cwd=repo, capture_output=True,
                          text=True)
    if proc.returncode != 0:
        raise Unjudgeable(f"git ls-files exited {proc.returncode}: {proc.stderr}")
    out = []
    for rel in proc.stdout.splitlines():
        p = repo / rel
        if p.is_file():
            out += verus_entries_in(rel, p.read_text(encoding="utf-8", errors="replace"))
    return out


# ── Ledger ─────────────────────────────────────────────────────────────────

LEDGER_HEADER = """\
# Trust escapes in spar's proofs: every declaration that depends on an axiom
# beyond propext, Quot.sound and Classical.choice, and every Verus
# external_body / assume_specification / assume / admit.
#
# Checked by tools/check_trust_ledger.py in the required
# `Lean proof typecheck (lake build)` job. A new escape fails the build until
# it is proven or listed here in the same PR, where review sees it. Removing a
# line is always allowed. Regenerate with `tools/check_trust_ledger.py --write`.
#
# lean   <module> <kind> <declaration> <axiom>
# verus  <path> <fn> <kind>        (one line per occurrence)
"""


def parse_ledger(text: str) -> Counter:
    entries: Counter = Counter()
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.rstrip("\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        parts = line.split("\t")
        ok = (parts[0] == "lean" and len(parts) == 5) or (parts[0] == "verus" and len(parts) == 4)
        if not ok or not all(parts):
            raise Unjudgeable(f"ledger line {n} is malformed (want tab-separated "
                              f"`lean mod kind decl axiom` or `verus path fn kind`): {line!r}")
        entries[line] += 1
    return entries


def render_ledger(entries: Iterable[str]) -> str:
    return LEDGER_HEADER + "".join(e + "\n" for e in sorted(entries))


def compare(current: Counter, ledger: Counter, out: TextIO) -> int:
    added = current - ledger
    stale = ledger - current
    for e, k in sorted(added.items()):
        print(f"::error::unlisted trust escape{'' if k == 1 else f' (x{k})'}: "
              f"{e.replace(chr(9), ' ')}", file=out)
    for e, k in sorted(stale.items()):
        print(f"::notice::no longer occurs, delete from the ledger: "
              f"{e.replace(chr(9), ' ')}", file=out)
    if added:
        print(f"{sum(added.values())} trust escape(s) not in {LEDGER.relative_to(REPO)}. "
              f"Prove them, or list them there in this PR.", file=out)
        return 1
    return 0


def check_canary(decls, out: TextIO) -> int:
    """Every canary must surface as a non-standard dependency."""
    flagged = {name for mod, _, name, axs in decls
               if mod == CANARY_MODULE and any(a not in STANDARD_AXIOMS for a in axs)}
    missing = sorted(CANARY_DECLS - flagged)
    if missing:
        print(f"::error::negative control: canary declaration(s) {missing} were NOT "
              f"reported as trust escapes. The check cannot fail.", file=out)
        return 1
    print(f"negative control: all {len(CANARY_DECLS)} canaries reported as trust escapes "
          f"(expected)", file=out)
    return 0


def main(argv: Optional[List[str]] = None, out: TextIO = sys.stdout) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    ap.add_argument("--ledger", type=Path, default=LEDGER)
    ap.add_argument("--proofs-dir", type=Path, default=PROOFS)
    ap.add_argument("--write", action="store_true", help="regenerate the ledger")
    ap.add_argument("--negative-control", action="store_true",
                    help="also check that canary escapes are reported")
    ap.add_argument("--self-test", action="store_true", help="run the decision table")
    a = ap.parse_args(argv)
    if a.self_test:
        return self_test()
    try:
        decls, (nmod, nall, nthm) = parse_report(run_lean(a.proofs_dir.resolve(),
                                                          a.negative_control))
        real = [d for d in decls if d[0] != CANARY_MODULE]
        lean = lean_entries(real)
        verus = verus_entries(REPO)
        print(f"lean: {len(proof_modules(a.proofs_dir.resolve()))} module(s) imported, "
              f"{nmod} declaring {nall} constant(s), {nthm} of them theorems; "
              f"{len(lean)} non-standard axiom dependency(ies)", file=out)
        print(f"verus: {len(verus)} escape(s) in tracked .rs files", file=out)
        current = Counter(lean + verus)
        if a.write:
            a.ledger.write_text(render_ledger(current.elements()), encoding="utf-8")
            print(f"wrote {sum(current.values())} entr(ies) to {a.ledger}", file=out)
            return 0
        if not a.ledger.is_file():
            raise Unjudgeable(f"{a.ledger} is missing; create it with --write")
        rc = compare(current, parse_ledger(a.ledger.read_text(encoding="utf-8")), out)
        if a.negative_control:
            rc = max(rc, check_canary(decls, out))
        if rc == 0:
            print("every trust escape is in the ledger.", file=out)
        return rc
    except Unjudgeable as e:
        print(f"::error::{e}", file=out)
        return 2


# ── Self-test ──────────────────────────────────────────────────────────────


def self_test() -> int:
    import io
    cases = []

    def case(name, fn):
        cases.append((name, fn))

    def report(*decl_lines, summary="SUMMARY\t1\t3\t2"):
        return "\n".join(list(decl_lines) + ([summary] if summary else [])) + "\n"

    def expect_unjudgeable(fn):
        try:
            fn()
        except Unjudgeable:
            return True
        return False

    case("standard axioms only produce no entry",
         lambda: lean_entries(parse_report(report(
             "DECL\tProofs.A\ttheorem\tA.t\tpropext,Quot.sound,Classical.choice"))[0]) == [])
    case("sorryAx is not standard",
         lambda: lean_entries(parse_report(report(
             "DECL\tProofs.A\ttheorem\tA.t\tpropext,sorryAx"))[0])
         == ["lean\tProofs.A\ttheorem\tA.t\tsorryAx"])
    case("native_decide's Lean.ofReduceBool is not standard",
         lambda: lean_entries(parse_report(report(
             "DECL\tProofs.A\ttheorem\tA.t\tLean.ofReduceBool"))[0])
         == ["lean\tProofs.A\ttheorem\tA.t\tLean.ofReduceBool"])
    case("a def that uses sorry counts, not only theorems",
         lambda: lean_entries(parse_report(report(
             "DECL\tProofs.A\tdef\tA.f\tsorryAx"))[0])
         == ["lean\tProofs.A\tdef\tA.f\tsorryAx"])
    case("a report with no SUMMARY is unjudgeable",
         lambda: expect_unjudgeable(lambda: parse_report(report(
             "DECL\tProofs.A\ttheorem\tA.t\t", summary=None))))
    case("a report with 0 theorems is unjudgeable",
         lambda: expect_unjudgeable(lambda: parse_report(report(summary="SUMMARY\t1\t0\t0"))))

    def cmp(cur, led):
        buf = io.StringIO()
        return compare(Counter(cur), Counter(led), buf), buf.getvalue()

    e1 = "lean\tProofs.A\ttheorem\tA.t\tsorryAx"
    e2 = "lean\tProofs.A\ttheorem\tA.u\tsorryAx"
    case("an unlisted escape FAILS", lambda: cmp([e1, e2], [e1])[0] == 1)
    case("moving a sorry to another theorem FAILS (same count, new name)",
         lambda: cmp([e2], [e1])[0] == 1)
    case("a removed escape passes with a notice",
         lambda: cmp([], [e1]) == (0, "::notice::no longer occurs, delete from the ledger: "
                                      "lean Proofs.A theorem A.t sorryAx\n"))
    case("an exact match passes", lambda: cmp([e1], [e1])[0] == 0)
    v = "verus\tcrates/a.rs\tf\tassume"
    case("a second occurrence in the same fn FAILS", lambda: cmp([v, v], [v])[0] == 1)

    case("malformed ledger line is unjudgeable",
         lambda: expect_unjudgeable(lambda: parse_ledger("lean\tonly-two\n")))
    case("ledger comments and blanks are ignored",
         lambda: parse_ledger("# c\n\n" + e1 + "\n") == Counter([e1]))
    case("render then parse round-trips",
         lambda: parse_ledger(render_ledger([e2, e1, v])) == Counter([e1, e2, v]))

    rs = (
        "fn harness() {\n"
        "    kani::assume(x < 3);\n"
        "    // assume(commented)\n"
        "}\n"
        "#[verifier::external_body]\n"
        "fn trusted() {}\n"
        "fn proof_fn() {\n"
        "    assume(y > 0);\n"
        "    admit();\n"
        "}\n"
        "pub assume_specification[ core::cmp::max ];\n"
        "fn assume(b: bool) {}\n"
    )
    got = verus_entries_in("x.rs", rs)
    case("kani::assume is not a trust escape",
         lambda: not any(e.endswith("\tassume") and "\tharness\t" in e for e in got))
    case("a commented-out assume does not count",
         lambda: sum(1 for e in got if e.endswith("\tassume")) == 1)
    case("external_body is attributed to the function it precedes",
         lambda: "verus\tx.rs\ttrusted\texternal_body" in got)
    case("assume() and admit() inside a fn are attributed to it",
         lambda: {"verus\tx.rs\tproof_fn\tassume", "verus\tx.rs\tproof_fn\tadmit"} <= set(got))
    case("assume_specification counts",
         lambda: any(e.endswith("\tassume_specification") for e in got))
    case("a fn NAMED assume is not a call",
         lambda: len(got) == 4)

    def canary(decls):
        return check_canary(decls, io.StringIO())

    all_flagged = [(CANARY_MODULE, "theorem", n, ["sorryAx"]) for n in CANARY_DECLS]
    case("negative control passes when every canary is flagged",
         lambda: canary(all_flagged) == 0)
    case("negative control FAILS when a canary is reported clean",
         lambda: canary([d if d[2] != f"{CANARY_NS}.by_native_decide"
                         else (d[0], d[1], d[2], ["propext"]) for d in all_flagged]) == 1)
    case("negative control FAILS when a canary is absent",
         lambda: canary(all_flagged[1:]) == 1)
    case("the Lean source imports every module and carries the canary only on request",
         lambda: ("import Proofs.X\n" in lean_source(["Proofs", "Proofs.X"], False)
                  and CANARY_NS not in lean_source(["Proofs"], False).split("open Lean")[0]
                  and "theorem by_sorry" in lean_source(["Proofs"], True)))

    failed = 0
    for name, fn in cases:
        try:
            ok = bool(fn())
        except Exception as e:  # a crashing case is a failing case
            ok = False
            name += f" (raised {e!r})"
        print(f"  {'ok  ' if ok else 'FAIL'} {name}")
        failed += not ok
    print(f"\n{len(cases) - failed} of {len(cases)} self-test(s) passed.")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
