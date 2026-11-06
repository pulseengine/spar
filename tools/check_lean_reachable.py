#!/usr/bin/env python3
"""Fail when a first-party Lean file is not reachable from a Lake target CI builds.

REQ-PROOF-LAKE-REACHABLE-001. Refs #397.

WHY THIS EXISTS
===============

`Lean proof typecheck (lake build)` is a required context, and it compiled
nothing in `proofs/Proofs/Network/MinPlusPwa.lean`. Lake builds a target and
the transitive closure of its imports, and no file imported MinPlusPwa. Its
eight theorem statements were therefore never elaborated, and when this guard
was written they no longer could be: they called `List.minimum?` and
`List.maximum?`, which the pinned Lean does not have. Five hard errors sat in
a tree whose required typecheck was green.

The sorry ratchet (`check_lean_sorries.py`) did not see it either, and could
not: it reads TEXT under `proofs/Proofs/`, so it counted the file's eight
sorries all along. Two gates over the same directory measured two different
populations, the files that exist and the files that compile, and neither
compared them. This guard is that comparison.

WHAT "REACHABLE" MEANS HERE
===========================

The roots are what CI actually compiles, derived rather than declared:

* every entry of `defaultTargets` in `lakefile.toml`, which is what a bare
  `lake build` builds (a `lean_lib` contributes its root module, a `lean_exe`
  its `root`); and
* every `lean_exe` named by a `lake exe NAME` line in `.github/workflows/`,
  because running an exe compiles it.

A `lean_lib` that is NOT a default target is not a root. Today `Proofs` is
reached only because `Main` (the default exe) says `import Proofs`; delete that
line and `lake build` compiles none of `Proofs/`. Counting every declared lib as
a root would keep this guard green in exactly that case.

The guard takes no root arguments, so its bare run and its CI run measure the
same thing.

FAIL-CLOSED CHOICES
===================

Exit 2, never 0, when the measurement cannot be trusted:

* no `.lean` files found (a wrong directory reads as "nothing unreachable");
* a `lakefile.lean`, or a lakefile key this parser does not model (`globs`,
  `roots`, `srcDir`, ...), since each can change what Lake compiles;
* a default target that names no declared lib or exe;
* an `import` header line in a form the parser does not recognise.

`.lake/` (Mathlib and the build output) is not first-party and is excluded.

NOT CLAIMED: that a reachable file is proven. Reachable means typechecked;
MinPlusPwa is reachable and every one of its theorems is still `sorry`.
"""

from __future__ import annotations

import argparse
import io
import re
import sys
import tempfile
from pathlib import Path

# Keys this parser understands. Anything else in a target table, or at top
# level, exits 2: `globs = ["Proofs.+"]` would make Lake build every module
# under a namespace, `srcDir` moves where modules resolve, and guessing at
# either would make the reachable set wrong in one direction or the other.
_TOP_KEYS = {"name", "version", "defaultTargets"}
_TABLE_KEYS = {
    "lean_lib": {"name"},
    "lean_exe": {"name", "root"},
    "require": None,  # dependency declarations; not first-party, contents ignored
}

_LAKE_EXE = re.compile(r"\blake\s+exe\s+([A-Za-z_][\w]*)")

# One header line. Lean 4's module system adds `module`, `public`/`private`,
# `meta`, and `import all`; several modules may follow one `import`.
_HEADER_IMPORT = re.compile(
    r"^(?:(?:public|private)\s+)?(?:meta\s+)?import\s+(?:all\s+)?"
    r"(?P<mods>[A-Za-z_«][\w.«»']*(?:\s+[A-Za-z_«][\w.«»']*)*)\s*$"
)
_HEADER_OTHER = re.compile(r"^(?:module|prelude)\s*$")
# Anything that looks like it is trying to be an import. If it does not match
# _HEADER_IMPORT exactly, the parser does not understand it and says so.
_IMPORTISH = re.compile(r"^(?:(?:public|private|meta)\s+)*import\b")


class Unmodelled(Exception):
    """The input uses something this guard does not model; exit 2."""


def strip_comments(text: str) -> str:
    """Blank out Lean comments, preserving line structure.

    The same algorithm as `check_lean_sorries.strip_comments`, copied rather
    than imported: the potency harness runs each mutant of this file alone in
    an empty temp directory, where a sibling import would turn every mutant
    into an ERROR instead of a verdict. Nested `/- -/` blocks and `--` to end
    of line are both handled.
    """
    out, i, depth = [], 0, 0
    while i < len(text):
        if text.startswith("/-", i):
            depth += 1
            out.append("  ")
            i += 2
            continue
        if text.startswith("-/", i) and depth:
            depth -= 1
            out.append("  ")
            i += 2
            continue
        if depth:
            out.append("\n" if text[i] == "\n" else " ")
            i += 1
            continue
        out.append(text[i])
        i += 1
    return "\n".join(re.sub(r"--.*$", "", ln) for ln in "".join(out).splitlines())


def _toml_value(raw: str):
    raw = raw.strip()
    if raw.startswith('"') and raw.endswith('"'):
        return raw[1:-1]
    if raw.startswith("[") and raw.endswith("]"):
        return re.findall(r'"([^"]*)"', raw)
    raise Unmodelled(f"lakefile value not modelled: {raw!r}")


def parse_lakefile(path: Path) -> tuple[dict, list[tuple[str, dict]]]:
    """(top-level keys, [(table kind, keys)]) for the subset Lake uses here.

    Python 3.9 is the floor (no tomllib), so this reads only flat
    `key = "string"` / `key = ["a", "b"]` lines and `[[table]]` headers, and
    refuses anything else.
    """
    top: dict = {}
    tables: list[tuple[str, dict]] = []
    current: dict | None = None
    kind = ""
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        m = re.fullmatch(r"\[\[([\w]+)\]\]", line)
        if m:
            kind = m.group(1)
            if kind not in _TABLE_KEYS:
                raise Unmodelled(f"{path}:{n}: table [[{kind}]] is not modelled")
            current = {}
            tables.append((kind, current))
            continue
        if line.startswith("["):
            raise Unmodelled(f"{path}:{n}: table {line} is not modelled")
        m = re.fullmatch(r"([\w]+)\s*=\s*(.+)", line)
        if not m:
            raise Unmodelled(f"{path}:{n}: line not modelled: {line!r}")
        key, val = m.group(1), m.group(2)
        if current is None:
            if key not in _TOP_KEYS:
                raise Unmodelled(f"{path}:{n}: top-level key `{key}` is not modelled; "
                                 f"it may change what Lake compiles")
            top[key] = _toml_value(val)
        else:
            allowed = _TABLE_KEYS[kind]
            if allowed is not None and key not in allowed:
                raise Unmodelled(f"{path}:{n}: [[{kind}]] key `{key}` is not modelled; "
                                 f"it may change what Lake compiles")
            current[key] = _toml_value(val) if allowed is not None else val
    return top, tables


def roots(proofs: Path, workflows: Path, out) -> list[str]:
    if (proofs / "lakefile.lean").exists():
        raise Unmodelled(f"{proofs}/lakefile.lean exists; only lakefile.toml is modelled")
    lakefile = proofs / "lakefile.toml"
    if not lakefile.is_file():
        raise Unmodelled(f"no {lakefile}")
    top, tables = parse_lakefile(lakefile)
    libs = {t["name"]: t["name"] for k, t in tables if k == "lean_lib"}
    exes = {t["name"]: t.get("root", t["name"]) for k, t in tables if k == "lean_exe"}

    defaults = top.get("defaultTargets")
    if not defaults:
        raise Unmodelled(f"{lakefile} declares no defaultTargets; what a bare "
                         f"`lake build` compiles is then not modelled")
    found: list[str] = []
    for t in defaults:
        if t in exes:
            found.append(exes[t])
            print(f"root  {exes[t]:<12} default target lean_exe `{t}`", file=out)
        elif t in libs:
            found.append(libs[t])
            print(f"root  {libs[t]:<12} default target lean_lib `{t}`", file=out)
        else:
            raise Unmodelled(f"default target `{t}` names no declared lean_lib or lean_exe")

    ran = set()
    if workflows.is_dir():
        for wf in sorted(list(workflows.glob("*.yml")) + list(workflows.glob("*.yaml"))):
            ran.update(_LAKE_EXE.findall(wf.read_text(encoding="utf-8")))
    for name in sorted(ran):
        if name in exes and exes[name] not in found:
            found.append(exes[name])
            print(f"root  {exes[name]:<12} lean_exe `{name}`, run by `lake exe` in CI", file=out)
    return found


def imports_of(path: Path) -> list[str]:
    """Module names in the file's header, or Unmodelled on a form not understood."""
    mods: list[str] = []
    in_header = True
    for n, line in enumerate(strip_comments(path.read_text(encoding="utf-8")).splitlines(), 1):
        s = line.strip()
        if not s:
            continue
        m = _HEADER_IMPORT.match(s)
        if in_header and m:
            mods.extend(m.group("mods").split())
            continue
        if in_header and _HEADER_OTHER.match(s):
            continue
        if _IMPORTISH.match(s):
            where = "header" if in_header else "body (after the header ended)"
            raise Unmodelled(f"{path}:{n}: import line in the {where} not understood: {s!r}")
        in_header = False
    return mods


def population(proofs: Path) -> list[Path]:
    return sorted(p for p in proofs.rglob("*.lean")
                  if ".lake" not in p.relative_to(proofs).parts)


def check(proofs: Path, workflows: Path, out=sys.stdout) -> int:
    print("== lean reachability guardrail ==", file=out)
    if not proofs.is_dir():
        print(f"::error::not a directory: {proofs}. The scan cannot run, so it "
              f"reports nothing rather than nothing unreachable.", file=out)
        return 2
    files = population(proofs)
    if not files:
        print(f"::error::no first-party .lean files under {proofs}. A broken scan, "
              f"not a fully reachable tree.", file=out)
        return 2
    try:
        start = roots(proofs, workflows, out)
        reached: set[Path] = set()
        todo = list(start)
        while todo:
            mod = todo.pop()
            f = proofs.joinpath(*mod.split(".")).with_suffix(".lean")
            if f in reached or not f.is_file():
                continue  # already seen, or not first-party (Mathlib, Lean core)
            reached.add(f)
            todo.extend(imports_of(f))
    except Unmodelled as e:
        print(f"::error::{e}", file=out)
        return 2

    unreachable = [f for f in files if f not in reached]
    print(f"first-party .lean files: {len(files)}", file=out)
    print(f"reachable from roots:    {len(files) - len(unreachable)}", file=out)
    for f in unreachable:
        print(f"  UNREACHABLE  {f}", file=out)
    if unreachable:
        print("", file=out)
        print(f"::error::{len(unreachable)} first-party Lean file(s) are imported by no "
              f"target CI builds, so the required `lake build` never typechecks them. "
              f"Import each from a reachable module (Proofs.lean), or move it out of "
              f"{proofs}/ if it is dead.", file=out)
        return 1
    print("every first-party .lean file is compiled by a CI-built Lake target.", file=out)
    return 0


def self_test() -> int:
    passed = failed = 0

    def case(desc: str, want: int, files: dict, want_msg: str | None = None,
             no_msg: str | None = None) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for rel, body in files.items():
                p = root / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(body, encoding="utf-8")
            buf = io.StringIO()
            got = check(root / "proofs", root / "wf", out=buf)
        text = buf.getvalue()
        bad = None
        if got != want:
            bad = f"exit {got}, want {want}"
        elif want_msg is not None and want_msg not in text:
            bad = f"exit correct, but output lacks {want_msg!r}"
        elif no_msg is not None and no_msg in text:
            bad = f"exit correct, but output contains {no_msg!r}"
        if bad:
            failed += 1
            print(f"  FAIL {desc}: {bad}")
            for line in text.splitlines()[:8]:
                print(f"         {line}")
        else:
            passed += 1
            print(f"  ok   {desc}")

    LAKE = ('name = "proofs"\ndefaultTargets = ["proofs"]\n\n'
            '[[lean_lib]]\nname = "Proofs"\n\n'
            '[[lean_exe]]\nname = "proofs"\nroot = "Main"\n\n'
            '[[lean_exe]]\nname = "codegen"\nroot = "Codegen"\n\n'
            '[[require]]\nname = "mathlib"\nscope = "leanprover-community"\n')
    BASE = {
        "proofs/lakefile.toml": LAKE,
        "proofs/Main.lean": "import Proofs\n\ndef main : IO Unit := pure ()\n",
        "proofs/Proofs.lean": "import Proofs.A\n",
        "proofs/Proofs/A.lean": "import Mathlib.Tactic\ntheorem a : True := trivial\n",
        "proofs/Codegen.lean": "import Lean\nimport Proofs.Core\n",
        "proofs/Proofs/Core.lean": "def c := 1\n",
        "wf/proofs.yml": "      - run: lake exe codegen > out.rs\n",
    }

    def with_(**over):
        d = dict(BASE)
        for k, v in over.items():
            key = k.replace("__", "/").replace("_DOT_", ".")
            if v is None:
                d.pop(key, None)
            else:
                d[key] = v
        return d

    print("check_lean_reachable self-test")
    case("the reference tree is fully reachable", 0, BASE)
    # THE BUG (#397): a file under Proofs/ that nothing imports.
    case("REGRESSION #397: an unimported module FAILS and is named", 1,
         {**BASE, "proofs/Proofs/Network/MinPlusPwa.lean": "theorem t : True := sorry\n"},
         want_msg="UNREACHABLE  ")
    case("...and passes once Proofs.lean imports it", 0,
         {**BASE, "proofs/Proofs.lean": "import Proofs.A\nimport Proofs.Network.MinPlusPwa\n",
          "proofs/Proofs/Network/MinPlusPwa.lean": "theorem t : True := sorry\n"})
    # A declared lib is not a root unless it is a default target: with Main no
    # longer importing Proofs, bare `lake build` compiles none of Proofs/.
    case("a lean_lib that is not a default target is NOT a root", 1,
         {**BASE, "proofs/Main.lean": "def main : IO Unit := pure ()\n"},
         want_msg="UNREACHABLE  ")
    # An exe CI runs via `lake exe` is a root; without that run it is not.
    case("an exe run by `lake exe` in CI is a root", 0, BASE)
    case("an exe CI never runs is NOT a root", 1,
         {**BASE, "wf/proofs.yml": "      - run: lake build\n"},
         want_msg="Codegen.lean")
    # Comments are not imports.
    case("an `import` inside a block comment does not reach the module", 1,
         {**BASE, "proofs/Proofs.lean": "/- import Proofs.B -/\nimport Proofs.A\n",
          "proofs/Proofs/B.lean": "def b := 1\n"},
         want_msg="B.lean")
    case("an `import` after `--` does not reach the module", 1,
         {**BASE, "proofs/Proofs.lean": "import Proofs.A -- import Proofs.B\n",
          "proofs/Proofs/B.lean": "def b := 1\n"},
         want_msg="B.lean")
    case("several modules on one import line are all reached", 0,
         {**BASE, "proofs/Proofs.lean": "import Proofs.A Proofs.B\n",
          "proofs/Proofs/B.lean": "def b := 1\n"})
    case("module-system header forms are parsed", 0,
         {**BASE, "proofs/Proofs.lean":
          "module\n\npublic import Proofs.A\nmeta import Proofs.B\nimport all Proofs.C\n",
          "proofs/Proofs/B.lean": "def b := 1\n", "proofs/Proofs/C.lean": "def c := 1\n"})
    case("an import line the parser does not understand fails closed", 2,
         {**BASE, "proofs/Proofs.lean": "import Proofs.A\nimport (Proofs.B)\n"})
    # Not first-party, not counted.
    case(".lake/ (Mathlib, build output) is not first-party", 0,
         {**BASE, "proofs/.lake/packages/mathlib/Mathlib/X.lean": "def x := 1\n"},
         no_msg="UNREACHABLE")
    # Broken measurements are not clean ones.
    case("no .lean files is a broken scan, not a reachable tree", 2,
         {"proofs/lakefile.toml": LAKE, "proofs/README.md": "x\n"})
    case("a missing proofs directory is a broken scan", 2, {"wf/x.yml": ""})
    case("an unmodelled lakefile key (globs) fails closed", 2,
         {**BASE, "proofs/lakefile.toml": LAKE.replace(
             'name = "Proofs"\n', 'name = "Proofs"\nglobs = ["Proofs.+"]\n')})
    case("a default target naming nothing declared fails closed", 2,
         {**BASE, "proofs/lakefile.toml": LAKE.replace(
             'defaultTargets = ["proofs"]', 'defaultTargets = ["nope"]')})
    case("no defaultTargets fails closed", 2,
         {**BASE, "proofs/lakefile.toml": LAKE.replace('defaultTargets = ["proofs"]\n', '')})
    case("a lakefile.lean is not modelled", 2,
         {**BASE, "proofs/lakefile.lean": "import Lake\n"})

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--proofs-dir", default="proofs")
    ap.add_argument("--workflows-dir", default=".github/workflows")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return check(Path(a.proofs_dir), Path(a.workflows_dir))


if __name__ == "__main__":
    sys.exit(main())
