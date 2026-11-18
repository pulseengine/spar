#!/usr/bin/env python3
"""Gate the load-bearing claims in README.md and COMPLIANCE.md against evidence.

REQ-CLAIM-LEDGER-001. A claim nothing checks drifts: the README said
"16 subcommands" while `COMMANDS` in spar-cli held 19, and COMPLIANCE.md still
calls spar-sysml2 "production-grade" while REQ-SYSML2-CONFORMANCE-001 measures
the parser rejecting the specification's own Annex A model. This reads the
ledger `claims.yaml` and fails when:

  1. a WATCHED phrase (a maturity word, a competitive claim, a stated count, a
     badge) appears on a line no ledger entry covers. A new claim cannot land
     without being bound or explicitly marked as narrative;
  2. a ledger entry's text is no longer in its document. Rewording a claim
     without updating the ledger leaves evidence bound to a sentence nobody
     reads any more;
  3. a claim's evidence no longer holds (a count moved, a file went away);
  4. the number of CONTESTED claims differs from CONTESTED. A contested claim
     is one the repo's own measurements contradict and whose rewording is the
     maintainer's call. EXACT and two-sided like DEFAULT_MAX in
     check_stale_scope.py: a new one fails, and so does a fixed one until the
     constant is lowered.

Mark the assertions, never the narrative. Motivation, rationale and examples
stay free; only lines matching a `watch` pattern need a ledger entry.

NOT CLAIMED: that a gated claim is true. The gate proves the stated evidence
still holds, not that it supports the wording. Nor that the watch list finds
every claim: it is an allow-list that grows, and a claim phrased in words it
does not list passes unseen.

Exit codes: 0 every check holds; 1 a claim is unledgered, stale or unbacked,
or the contested count moved; 2 the ledger or a document could not be read,
or a predicate could not be evaluated (a glob matching nothing, a region
marker that is gone). A measurement that could not be made never reads as a
clean one.

Usage:
    tools/check_claims.py [--ledger claims.yaml] [--root .] [--contested N]
    tools/check_claims.py --self-test
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from pathlib import Path

try:
    import yaml
except ModuleNotFoundError:  # pragma: no cover - reported, never silently skipped
    print("::error::PyYAML is unavailable, so the claims ledger cannot be read. "
          "This check reports nothing rather than reporting zero violations.",
          file=sys.stderr)
    sys.exit(2)

# Claims the repo's own measurements contradict, kept visible until the
# maintainer rewords them. Re-derived from claims.yaml on 2026-11-17: the SysML
# v2 badge, "replaces the Eclipse/Java OSATE2 toolchain", "reaches well past
# what OSATE ... covers", and three v0.9.1 snapshots in COMPLIANCE.md: its
# "production-grade", its header count of 18 crates (23 today) and its "10
# analysis passes", three of which no longer exist as registered passes.
CONTESTED = 6


class Inconclusive(Exception):
    """A predicate or input that could not be evaluated: exit 2, never a pass."""


def _texts(entry: dict) -> list[str]:
    t = entry.get("text")
    texts = [t] if isinstance(t, str) else t
    if not texts or not all(isinstance(x, str) and x for x in texts):
        raise Inconclusive(f"ledger entry {entry.get('id', '?')!r} has no `text` "
                           f"(a non-empty string or list of strings)")
    return texts


def _region(lines: list[str], spec: dict | None, where: str) -> list[str]:
    if not spec:
        return lines
    start = re.compile(spec["from"])
    end = re.compile(spec["to"])
    for i, line in enumerate(lines):
        if start.search(line):
            for j in range(i + 1, len(lines)):
                if end.search(lines[j]):
                    return lines[i + 1:j]
            raise Inconclusive(f"{where}: region end {spec['to']!r} not found")
    raise Inconclusive(f"{where}: region start {spec['from']!r} not found")


def evaluate(ev: dict, root: Path, where: str) -> str | None:
    """None when the predicate holds, else why it does not."""
    kind = ev.get("kind")
    if kind == "file-exists":
        return None if (root / ev["path"]).is_file() else f"{ev['path']} does not exist"
    if kind != "count":
        raise Inconclusive(f"{where}: unknown evidence kind {kind!r}")

    files = sorted(p for p in root.glob(ev["glob"]) if p.is_file())
    if not files:
        # A count over nothing is the fail-open this guard class keeps producing:
        # a renamed directory satisfies any `max` and reads as a clean result.
        raise Inconclusive(f"{where}: glob {ev['glob']!r} matched no file")
    if "pattern" in ev:
        pat = re.compile(ev["pattern"])
        n = 0
        for f in files:
            lines = f.read_text(encoding="utf-8").splitlines()
            n += sum(1 for ln in _region(lines, ev.get("region"), f"{where} {f.name}")
                     if pat.search(ln))
        what = f"lines matching {ev['pattern']!r} in {ev['glob']}"
    else:
        n = len(files)
        what = f"files matching {ev['glob']}"

    bounds = [k for k in ("eq", "min", "max") if k in ev]
    if not bounds:
        raise Inconclusive(f"{where}: a count needs eq, min or max")
    if "eq" in ev and n != ev["eq"]:
        return f"{n} {what}, the claim needs exactly {ev['eq']}"
    if "min" in ev and n < ev["min"]:
        return f"{n} {what}, the claim needs at least {ev['min']}"
    if "max" in ev and n > ev["max"]:
        return f"{n} {what}, the claim allows at most {ev['max']}"
    return None


def check(ledger_path: Path, root: Path, contested_declared: int, out=sys.stdout) -> int:
    try:
        ledger = yaml.safe_load(ledger_path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as exc:
        print(f"::error::cannot read the claims ledger {ledger_path}: {exc}", file=out)
        return 2
    if not isinstance(ledger, dict) or not ledger.get("docs") or not ledger.get("claims"):
        print(f"::error::{ledger_path} needs non-empty `docs` and `claims`", file=out)
        return 2

    failures: list[str] = []
    try:
        docs: dict[str, list[str]] = {}
        for d in ledger["docs"]:
            try:
                docs[d] = (root / d).read_text(encoding="utf-8").splitlines()
            except OSError as exc:
                raise Inconclusive(f"cannot read watched document {d}: {exc}")
        watch = [(re.compile(w["pattern"]), w.get("why", "")) for w in ledger.get("watch", [])]

        entries = [("claim", e) for e in ledger["claims"]] + \
                  [("narrative", e) for e in ledger.get("narrative", []) or []]
        covering: dict[str, list[str]] = {d: [] for d in docs}
        ids = [e.get("id") for k, e in entries if k == "claim"]
        if len(ids) != len(set(ids)) or None in ids:
            raise Inconclusive("every claim needs a unique `id`")

        contested = []
        held = 0
        for kind, e in entries:
            label = e.get("id") or f"narrative {e.get('text')!r}"
            doc = e.get("doc")
            if doc not in docs:
                raise Inconclusive(f"{label}: doc {doc!r} is not in `docs`")
            texts = _texts(e)
            covering[doc].extend(texts)
            body = "\n".join(docs[doc])
            for t in texts:
                if t not in body:
                    failures.append(f"STALE      {label}: {t!r} is no longer in {doc}; "
                                    f"the claim was reworded or removed without the ledger")
            if kind == "narrative":
                continue
            if "contested" in e:
                if e.get("evidence"):
                    raise Inconclusive(f"{label}: contested and evidenced at once")
                contested.append(label)
                print(f"  contested  {label}: {e['contested']}", file=out)
                continue
            if not e.get("evidence"):
                raise Inconclusive(f"{label}: a claim needs `evidence` or `contested`")
            for ev in e["evidence"]:
                why = evaluate(ev, root, label)
                if why is not None:
                    failures.append(f"UNBACKED   {label}: {why}")
            held += 1

        unledgered = 0
        for d, lines in docs.items():
            for no, line in enumerate(lines, 1):
                for pat, why in watch:
                    if pat.search(line) and not any(t in line for t in covering[d]):
                        unledgered += 1
                        failures.append(f"UNLEDGERED {d}:{no}: {line.strip()[:120]!r} "
                                        f"matches {pat.pattern!r} ({why}) and no "
                                        f"ledger entry covers it")
                        break
    except Inconclusive as exc:
        print(f"::error::{exc}", file=out)
        return 2
    except (KeyError, TypeError, re.error) as exc:
        print(f"::error::malformed claims ledger: {exc!r}", file=out)
        return 2

    print(f"claims checked: {held} held by evidence, {len(contested)} contested "
          f"(declared {contested_declared}), {len(docs)} documents, "
          f"{len(watch)} watch patterns", file=out)
    if len(contested) != contested_declared:
        failures.append(
            f"CONTESTED  {len(contested)} contested claims, declared {contested_declared}. "
            + ("A new contested claim is an overclaim landing knowingly; bind it to "
               "evidence or reword it." if len(contested) > contested_declared else
               "That is a win: lower CONTESTED so it is locked in."))
    for f in failures:
        print(f"::error::{f}", file=out)
    return 1 if failures else 0


# ── self-test ────────────────────────────────────────────────────────────────

_BASE_LEDGER = """
docs: [README.md]
watch:
  - pattern: '\\bproduction[- ]grade\\b'
    why: maturity word
  - pattern: '\\b\\d+\\+?\\s+(\\w+\\s+)?(crates|subcommands)\\b'
    why: stated count
claims:
  - id: CRATES
    doc: README.md
    text: "2 crates in total"
    evidence:
      - {kind: count, glob: "crates/*/Cargo.toml", eq: 2}
  - id: CMDS
    doc: README.md
    text: ["3 subcommands", "3 cli subcommands"]
    evidence:
      - kind: count
        glob: "main.rs"
        pattern: '^\\s+"[a-z]+",$'
        region: {from: '^const COMMANDS', to: '^\\];'}
        min: 3
  - id: LICENSE
    doc: README.md
    text: "MIT licensed"
    evidence:
      - {kind: file-exists, path: LICENSE}
  - id: OLD
    doc: README.md
    text: "an old boast"
    contested: "contradicted by a measurement"
narrative:
  - doc: README.md
    text: "unlike a production-grade parser"
    why: a comparison, not a claim about spar
"""

_BASE_README = """# x
2 crates in total.
It has 3 subcommands, or 3 cli subcommands.
MIT licensed. an old boast.
We wrote it unlike a production-grade parser would.
"""

_BASE_MAIN = """fn x() {}
const COMMANDS: &[&str] = &[
    "parse",
    "items",
    "lsp",
];
    "outside",
"""


def _fixture(td: Path, ledger: str = _BASE_LEDGER, readme: str = _BASE_README,
             main: str = _BASE_MAIN, crates: int = 2, license: bool = True) -> Path:
    for i in range(crates):
        (td / "crates" / f"c{i}").mkdir(parents=True, exist_ok=True)
        (td / "crates" / f"c{i}" / "Cargo.toml").write_text("[package]\n")
    (td / "README.md").write_text(readme)
    (td / "main.rs").write_text(main)
    if license:
        (td / "LICENSE").write_text("MIT\n")
    (td / "claims.yaml").write_text(ledger)
    return td / "claims.yaml"


def self_test() -> int:
    import io

    cases = [
        # (description, fixture kwargs, contested declared, expected exit, text the output must hold)
        ("the clean ledger holds", {}, 1, 0, "3 held by evidence, 1 contested"),
        ("an unledgered maturity word fails",
         {"readme": _BASE_README + "spar is production-grade.\n"}, 1, 1, "UNLEDGERED README.md:6"),
        ("an unledgered stated count fails",
         {"readme": _BASE_README + "It ships 40 analysis crates.\n"}, 1, 1, "UNLEDGERED"),
        ("a narrative entry covers a second hit on its own line",
         {"readme": _BASE_README.replace("parser would.", "parser would. production-grade.")},
         1, 0, "3 held"),
        ("a narrative entry does not cover another line",
         {"readme": _BASE_README + "Now production-grade.\n"}, 1, 1, "UNLEDGERED README.md:6"),
        ("a reworded claim is STALE",
         {"readme": _BASE_README.replace("MIT licensed", "MIT-licensed")}, 1, 1,
         "STALE      LICENSE"),
        ("a count eq mismatch is UNBACKED", {"crates": 3}, 1, 1, "3 files matching"),
        ("a count below min is UNBACKED",
         {"main": _BASE_MAIN.replace('    "lsp",\n', "")}, 1, 1, "UNBACKED   CMDS"),
        ("lines outside the region are not counted",
         {"main": _BASE_MAIN.replace('    "lsp",\n', "") + '    "late",\n'}, 1, 1, "UNBACKED   CMDS"),
        ("a missing file is UNBACKED", {"license": False}, 1, 1, "LICENSE does not exist"),
        ("a glob matching nothing is inconclusive", {"crates": 0}, 1, 2, "matched no file"),
        ("a region marker that is gone is inconclusive",
         {"main": _BASE_MAIN.replace("const COMMANDS", "static CMDS")}, 1, 2, "region start"),
        ("one more contested claim than declared fails", {}, 0, 1, "declared 0"),
        ("one fewer contested claim than declared fails", {}, 2, 1, "lower CONTESTED"),
        ("a claim with neither evidence nor contested is inconclusive",
         {"ledger": _BASE_LEDGER.replace('    contested: "contradicted by a measurement"\n', "")},
         1, 2, "needs `evidence` or `contested`"),
        ("an unparseable ledger is inconclusive", {"ledger": "docs: [\n"}, 1, 2, "cannot read"),
        ("a watched document that is missing is inconclusive",
         {"ledger": _BASE_LEDGER.replace("docs: [README.md]", "docs: [README.md, GONE.md]")},
         1, 2, "GONE.md"),
        ("duplicate claim ids are inconclusive",
         {"ledger": _BASE_LEDGER.replace("id: CMDS", "id: CRATES")}, 1, 2, "unique"),
    ]
    passed = failed = 0
    print("check_claims self-test")
    for desc, kw, declared, want_rc, want_text in cases:
        with tempfile.TemporaryDirectory() as t:
            td = Path(t)
            ledger = _fixture(td, **kw)
            buf = io.StringIO()
            rc = check(ledger, td, declared, out=buf)
            ok = rc == want_rc and want_text in buf.getvalue()
            passed, failed = (passed + 1, failed) if ok else (passed, failed + 1)
            print(f"  {'ok  ' if ok else 'FAIL'} {desc} (exit {rc}, want {want_rc})")
            if not ok:
                print("       " + buf.getvalue().replace("\n", "\n       "))
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ledger", type=Path, default=Path("claims.yaml"))
    ap.add_argument("--root", type=Path, default=Path("."))
    ap.add_argument("--contested", type=int, default=CONTESTED)
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return check(a.ledger, a.root, a.contested)


if __name__ == "__main__":
    sys.exit(main())
