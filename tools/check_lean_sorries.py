#!/usr/bin/env python3
"""Gate Lean `sorry` count against a declared, shrinking floor.

REQ-GUARD-GATE-EVIDENCE-002 (f). Closes #385.

WHY THIS EXISTS
===============

`Fail on sorry (post-build gate)` in `proofs.yml` is part of the required
`Lean proof typecheck (lake build)` context, and it permitted EVERY sorry in the
tree while printing `No sorrys in proofs/Proofs/ — gate green.`

Its pipeline was:

    grep -rn -E '^[[:space:]]*sorry[[:space:]]*(--.*)?$' proofs/Proofs/ \\
      | grep -v -E 'sorry[[:space:]]*--[[:space:]]*TODO'

12 sorries in, 0 out, `if` never fires. Every one carries `sorry -- TODO(v1.0.0)`,
so the exemption pattern matched all of them. **The gate was satisfied by exactly
the thing it exists to prevent.**

HOW IT DRIFTED, which is the part worth keeping
-----------------------------------------------

The step's own comment says:

    # The five tracked sorrys do NOT carry a same-line comment; the
    # TODO sits on the prior line, so this gate flags all five.

That was TRUE when written. The exclusion was deliberately a no-op — a defensive
filter against a form nobody used. Since then the sorries grew 5 -> 12 and moved
their TODO onto the same line, and the no-op silently became universal. Nobody
edited the guard; the code drifted out from under its comment.

So the lesson is not "someone wrote a bad filter". It is that **a self-exemption
keyed on a pattern the exempted code itself controls cannot hold** — the proof
author writes both the `sorry` and the `-- TODO` that excuses it. An exemption
must be declared somewhere the exempted party does not edit in the same commit.

THE REPLACEMENT
===============

A ratchet, the same shape as the mutants gate:

* Count every bare `sorry`, with NO comment-based exemption. A `-- TODO` is a
  note to humans, not a permit.
* Compare against `--max-sorries`, a floor declared in the workflow.
* count > floor  -> FAIL. New sorries cannot be admitted by writing a comment.
* count < floor  -> PASS, and say loudly that the floor should be lowered. That
  is how the ratchet walks down. Improving must never fail.
* The count and the per-file breakdown print on EVERY path, success included,
  so "12 permitted" is visible rather than inferred from silence.

An empty scan exits 2. Zero files matched means the walk is broken (wrong path,
renamed directory), and zero-sorries-because-nothing-was-read is the exact
failure this whole requirement is about: the error path producing the ideal
reading.

NOT CLAIMED: that the remaining sorries are acceptable. The floor records how
many exist, not that they are fine. Discharging them is REQ-PROOF-NC-MINPLUS-001
(v0.38.0); this only stops the number rising unnoticed.
"""

from __future__ import annotations

import argparse
import io
import re
import sys
import tempfile
from pathlib import Path

# A bare `sorry` on its own line, with or without a trailing comment. The
# trailing comment is CAPTURED, not excluded — that was the bug.
_SORRY = re.compile(r"^[ \t]*sorry[ \t]*(--.*)?$")


def scan(root: Path) -> dict[str, list[tuple[int, str]]]:
    """{relative path: [(line_no, line_text)]} for every bare sorry."""
    found: dict[str, list[tuple[int, str]]] = {}
    for path in sorted(root.rglob("*.lean")):
        hits = []
        for n, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
            if _SORRY.match(line):
                hits.append((n, line.strip()))
        if hits:
            found[str(path)] = hits
    return found


def check(root: Path, max_sorries: int, out=sys.stdout) -> int:
    if not root.is_dir():
        print(f"::error::not a directory: {root} — the scan cannot run, so it "
              f"reports nothing rather than zero sorries.", file=out)
        return 2

    lean_files = list(root.rglob("*.lean"))
    if not lean_files:
        print(f"::error::no .lean files under {root} — a broken scan, not a "
              f"sorry-free tree. Zero-because-nothing-was-read is the failure "
              f"this gate exists to prevent.", file=out)
        return 2

    found = scan(root)
    total = sum(len(v) for v in found.values())

    print("== lean-sorry guardrail ==", file=out)
    print(f".lean files scanned: {len(lean_files)}", file=out)
    print(f"sorries found:       {total}   (declared floor: {max_sorries})", file=out)
    for path, hits in found.items():
        print(f"  {len(hits):3}  {path}", file=out)
        for n, text in hits:
            print(f"       :{n}  {text}", file=out)

    if total > max_sorries:
        print("", file=out)
        print(f"::error::{total} sorries exceed the declared floor of {max_sorries}.", file=out)
        print("::error::A `-- TODO` comment is a note, not a permit — it does not "
              "exempt a sorry from this count (#385). Discharge the proof, or "
              "raise the floor in proofs.yml deliberately and say why.", file=out)
        return 1

    if total < max_sorries:
        print("", file=out)
        print(f"::notice::{total} sorries is BELOW the floor of {max_sorries} — "
              f"lower --max-sorries to {total} in proofs.yml so the ratchet holds "
              f"the ground that was gained.", file=out)

    print(f"\n{total} sorries, at or under the declared floor of {max_sorries}.", file=out)
    return 0


def self_test() -> int:
    passed = failed = 0

    def case(desc: str, want: int, files: dict[str, str], floor: int) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for name, body in files.items():
                p = root / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(body, encoding="utf-8")
            buf = io.StringIO()
            got = check(root, floor, out=buf)
        if got == want:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want {want}")
            for line in buf.getvalue().splitlines()[:6]:
                print(f"         {line}")

    PLAIN = "theorem t : True := by\n  sorry\n"
    TODO = "theorem t : True := by\n  sorry -- TODO(v1.0.0)\n"
    DONE = "theorem t : True := by\n  trivial\n"

    print("check_lean_sorries self-test")
    # THE BUG. A `-- TODO` sorry must COUNT. Under the old gate this was exempt,
    # which is how all 12 became invisible.
    case("REGRESSION #385: `sorry -- TODO` COUNTS and can breach the floor", 1,
         {"A.lean": TODO}, 0)
    case("...and the same file passes when the floor admits it", 0,
         {"A.lean": TODO}, 1)
    # Plain sorries behave identically — the comment is not signal either way.
    case("bare sorry over floor fails", 1, {"A.lean": PLAIN}, 0)
    case("bare sorry at floor passes", 0, {"A.lean": PLAIN}, 1)
    # The ratchet direction: improving must never fail.
    case("below floor PASSES (ratchet may be tightened, not a failure)", 0,
         {"A.lean": DONE}, 3)
    # Counting across files.
    case("counts across multiple files", 1,
         {"A.lean": PLAIN, "sub/B.lean": TODO}, 1)
    # Broken scans must not read as clean.
    case("no .lean files is a broken scan, not a clean tree", 2, {"README.md": "x"}, 0)
    # A `sorry` inside a word or mid-expression is not a bare sorry.
    case("`sorryAx` / inline text is not a bare sorry", 0,
         {"A.lean": "def sorryAx := 1\n-- mentions sorry in prose\n"}, 0)

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", default="proofs/Proofs")
    ap.add_argument("--max-sorries", type=int, default=12)
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return check(Path(a.root), a.max_sorries)


if __name__ == "__main__":
    sys.exit(main())
