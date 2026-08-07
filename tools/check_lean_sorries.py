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

* Count every unproven obligation, with NO comment-based exemption. A `-- TODO`
  is a note to humans, not a permit.

  "Obligation" is broader than the first revision of this gate allowed, and the
  widening is #385's defect (b), fixed separately from its defect (a). That
  revision matched only a bare `sorry` alone on its line, so `:= by sorry`,
  `:= sorry`, `admit` and `axiom cheat : 7 = 8` all reported zero — the four
  forms the issue had named explicitly. Worse, its self-test contained a case
  ASSERTING that a non-bare occurrence does not count, which made the hole look
  deliberate. All five forms (including `sorryAx`) now count, matched over
  comment-stripped text so prose still does not.

  MEASURED before widening, so this is not a latent red: the count is 12 either
  way — the tree contains no inline `sorry`, no `admit`, no `axiom` and no
  `sorryAx` today. The declared floor is unchanged; only the escape hatches are
  closed.
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

# Every way this tree can carry an unproven obligation.
#
# The first revision of this gate matched ONLY `^[ \t]*sorry[ \t]*(--.*)?$` — a
# bare `sorry` alone on its line. That fixed #385's defect (a), the self-service
# `-- TODO` exemption, and left its defect (b) untouched: the issue named
# `:= by sorry`, `:= sorry`, `admit` and `axiom` explicitly, and all four exited
# 0. The self-exemption had been replaced with a narrower one that was just as
# self-service — an author could assert a falsehood via `axiom` and the declared
# floor never moved.
#
# Detection runs over COMMENT-STRIPPED text, which is what lets the patterns be
# word-bounded rather than line-anchored: `-- mentions sorry in prose` must not
# count, and it no longer can, because the comment is gone before matching.
#
# `sorryAx` is counted separately and deliberately. It is Lean's underlying
# escape term; `\bsorry\b` does not match inside it (word boundary), so without
# its own entry it would be invisible. There are zero occurrences today, so
# counting it costs nothing now and closes the hatch later.
_OBLIGATIONS: list[tuple[str, re.Pattern[str]]] = [
    ("sorry", re.compile(r"\bsorry\b")),
    ("sorryAx", re.compile(r"\bsorryAx\b")),
    ("admit", re.compile(r"\badmit\b")),
    # An axiom asserts its statement without proof — `axiom cheat : 7 = 8` is
    # strictly worse than a `sorry`, because nothing marks the proof as
    # incomplete. Anchored at line start so `Classical.axiom_of_choice`-style
    # references in expressions are not swept up.
    ("axiom", re.compile(r"^[ \t]*axiom[ \t]+")),
]


def strip_comments(text: str) -> str:
    """Blank out Lean comments, preserving line structure.

    Handles nested `/- -/` blocks (Lean permits nesting) and `--` to
    end-of-line. Newlines are preserved so reported line numbers stay true to
    the original file.
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


def scan(root: Path) -> dict[str, list[tuple[int, str, str]]]:
    """{path: [(line_no, kind, original_line_text)]} for every obligation."""
    found: dict[str, list[tuple[int, str, str]]] = {}
    for path in sorted(root.rglob("*.lean")):
        raw = path.read_text(encoding="utf-8", errors="replace")
        raw_lines = raw.splitlines()
        hits: list[tuple[int, str, str]] = []
        for n, line in enumerate(strip_comments(raw).splitlines(), 1):
            for kind, rx in _OBLIGATIONS:
                for _ in rx.finditer(line):
                    original = raw_lines[n - 1].strip() if n <= len(raw_lines) else line.strip()
                    hits.append((n, kind, original))
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
    by_kind: dict[str, int] = {}
    for hits in found.values():
        for _, kind, _text in hits:
            by_kind[kind] = by_kind.get(kind, 0) + 1

    print("== lean-sorry guardrail ==", file=out)
    print(f".lean files scanned: {len(lean_files)}", file=out)
    print(f"obligations found:   {total}   (declared floor: {max_sorries})", file=out)
    # Printed on every path, success included, and broken out by kind: the
    # forms are not interchangeable, and a shift from `sorry` to `axiom` at a
    # constant total would otherwise be invisible.
    kinds = ", ".join(f"{k}={by_kind[k]}" for k, _ in _OBLIGATIONS if k in by_kind)
    print(f"by kind:             {kinds or '(none)'}", file=out)
    for path, hits in found.items():
        print(f"  {len(hits):3}  {path}", file=out)
        for n, kind, text in hits:
            print(f"       :{n}  [{kind}] {text}", file=out)

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

    def case(desc: str, want: int, files: dict[str, str], floor: int,
             want_msg: str | None = None) -> None:
        """`want_msg` pins the DIAGNOSIS, not just the exit code.

        The ratchet notice and the per-kind breakdown are output-only: deleting
        either changes no verdict, so an exit-code assertion cannot tell whether
        they still work. An adversarial review found exactly that — removing the
        below-floor branch left 8/8 green. Anything whose whole purpose is to
        say something needs a case that reads what it said.
        """
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for name, body in files.items():
                p = root / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(body, encoding="utf-8")
            buf = io.StringIO()
            got = check(root, floor, out=buf)
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
         {"A.lean": DONE}, 3,
         want_msg="lower --max-sorries to 0")
    # Counting across files.
    case("counts across multiple files", 1,
         {"A.lean": PLAIN, "sub/B.lean": TODO}, 1)
    # Broken scans must not read as clean.
    case("no .lean files is a broken scan, not a clean tree", 2, {"README.md": "x"}, 0)
    # ── #385 defect (b): the forms the first revision could not see ──
    #
    # The case that used to sit here read:
    #
    #     case("`sorryAx` / inline text is not a bare sorry", 0,
    #          {"A.lean": "def sorryAx := 1\n-- mentions sorry in prose\n"}, 0)
    #
    # It PINNED THE HOLE SHUT rather than finding it — asserting as intended
    # behaviour that a non-bare occurrence does not count, at a floor of 0. The
    # issue had already enumerated four forms by name; none had a case, and all
    # four exited 0. A test that affirms the gap is worse than no test, because
    # it makes the gap look deliberate.
    case("REGRESSION #385(b): inline `:= by sorry` COUNTS", 1,
         {"A.lean": "theorem cheat : 1 = 2 := by sorry\n"}, 0)
    case("REGRESSION #385(b): inline `:= sorry` COUNTS", 1,
         {"A.lean": "theorem cheat : 1 = 2 := sorry\n"}, 0)
    case("REGRESSION #385(b): `admit` COUNTS", 1,
         {"A.lean": "theorem cheat : 1 = 2 := by\n  admit\n"}, 0)
    case("REGRESSION #385(b): `axiom` COUNTS (asserts without proof)", 1,
         {"A.lean": "axiom cheat : 7 = 8\n"}, 0,
         want_msg="axiom=1")
    case("REGRESSION #385(b): `sorryAx` COUNTS (the underlying escape term)", 1,
         {"A.lean": "theorem cheat : 1 = 2 := sorryAx _ true\n"}, 0)

    # ...and the legitimate half of the old case still holds: prose must not
    # count. This is what makes the widening safe rather than merely stricter —
    # detection runs over comment-stripped text, so the word in a comment is
    # gone before any pattern is applied.
    case("a `sorry` mentioned in a line comment does NOT count", 0,
         {"A.lean": "theorem t : True := by\n  trivial  -- unlike sorry, this closes\n"}, 0)
    case("a `sorry` inside a /- block comment -/ does NOT count", 0,
         {"A.lean": "/- we could sorry this, or:\n   admit it -/\ntheorem t : True := by\n  trivial\n"}, 0)
    case("a nested /- /- -/ -/ block is fully stripped", 0,
         {"A.lean": "/- outer /- inner sorry -/ still comment -/\ntheorem t : True := by\n  trivial\n"}, 0)
    case("an identifier merely CONTAINING sorry is not a bare sorry", 0,
         {"A.lean": "def sorryless := 1\n"}, 0)

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
