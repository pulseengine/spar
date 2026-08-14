#!/usr/bin/env python3
"""Re-derive the subcomponent-category legality table from OSATE.

Generates one minimal probe package per (container, subcomponent) pair -- 14
categories, so 196 files -- then prints the table implied by OSATE's verdicts.
This is the provenance behind `crates/spar-parser/src/grammar/categories.rs`
(#420 category 1); run it after an OSATE upgrade to check the table still
holds.

    python3 tools/osate-conformance/headless/category-probe.py --generate /tmp/probe
    # then, per tools/osate-conformance/README.md:
    "$JAVA" -jar "$LAUNCHER" -application spar.osate.headless.app \
        -data /tmp/ws -configuration "$ECLIPSE/configuration" -nosplash \
        validate /tmp/probe > /tmp/probe-verdicts.txt
    python3 tools/osate-conformance/headless/category-probe.py --table /tmp/probe-verdicts.txt

Why the table is measured rather than transcribed from AS5506B: being
*stricter* than the standard makes spar reject valid AADL, which
`osate_agreement.rs` treats as an invariant rather than a ratchet. A
transcription slip in the forbidding direction breaks that invariant silently.
The measured table cannot slip, and `--table` also cross-checks it against the
vendored corpus, where any observed pair is legal by construction.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

CATEGORIES = [
    "abstract", "bus", "data", "device", "memory", "process", "processor",
    "subprogram", "subprogram group", "system", "thread", "thread group",
    "virtual bus", "virtual processor",
]


def tag(container: str, sub: str) -> str:
    return f"{container}_{sub}".replace(" ", "")


def generate(out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    for c in CATEGORIES:
        for s in CATEGORIES:
            t = tag(c, s)
            # No classifier on the subcomponent: legal AADL, and it keeps the
            # category pairing the only thing under test.
            (out / f"P{t}.aadl").write_text(
                f"package P{t}\npublic\n"
                f"  {c} Container\n  end Container;\n"
                f"  {c} implementation Container.i\n"
                f"    subcomponents\n      x : {s};\n"
                f"  end Container.i;\n"
                f"end P{t};\n",
                encoding="utf-8",
            )
            n += 1
    print(f"generated {n} probe packages in {out}")
    return 0


def corpus_pairs(root: Path) -> Counter:
    alt = "|".join(c.replace(" ", r"\s+") for c in CATEGORIES)
    impl = re.compile(rf"\b({alt})\s+implementation\s+[\w.]+(.*?)\bend\b", re.I | re.S)
    subs = re.compile(
        r"\bsubcomponents\b(.*?)(?=\b(?:connections|calls|flows|modes|properties|annex|end)\b)",
        re.I | re.S,
    )
    decl = re.compile(rf"^\s*\w+\s*:\s*(?:refined\s+to\s+)?({alt})\b", re.I | re.M)
    norm = lambda s: re.sub(r"\s+", " ", s.strip().lower())
    found: Counter = Counter()
    for p in root.rglob("*.aadl"):
        text = re.sub(r"--[^\n]*", "", p.read_text(encoding="utf-8", errors="replace"))
        for m in impl.finditer(text):
            sm = subs.search(m.group(2))
            if sm:
                for d in decl.finditer(sm.group(1)):
                    found[(norm(m.group(1)), norm(d.group(1)))] += 1
    return found


def table(verdicts: Path, corpus: Path | None) -> int:
    lookup = {tag(c, s): (c, s) for c in CATEGORIES for s in CATEGORIES}
    legal, illegal = set(), set()
    for line in verdicts.read_text().splitlines():
        if not line.startswith("VERDICT|"):
            continue
        _, fname, verdict, _ = line.split("|")
        pair = lookup.get(fname[1:-5])
        if pair is None:
            print(f"::warning::unrecognised probe file {fname}", file=sys.stderr)
            continue
        (legal if verdict == "ACCEPT" else illegal).add(pair)

    total = len(legal) + len(illegal)
    if total != len(CATEGORIES) ** 2:
        # Missing verdicts would silently shrink the legal set, which is the
        # direction that makes spar too strict.
        print(f"::error::{total} verdicts, expected {len(CATEGORIES) ** 2}. "
              f"An incomplete probe run must not be turned into a table.",
              file=sys.stderr)
        return 2

    if corpus is not None:
        observed = corpus_pairs(corpus)
        contradictions = [(p, n) for p, n in observed.items() if p in illegal]
        if contradictions:
            print("::error::the corpus uses pairs this probe calls illegal — "
                  "the probe is malformed, not the corpus:", file=sys.stderr)
            for (c, s), n in contradictions:
                print(f"    {c} contains {s}  ({n}x)", file=sys.stderr)
            return 2
        print(f"cross-check OK: all {len(observed)} corpus pairs are legal here")

    print(f"{len(legal)} legal, {len(illegal)} illegal\n")
    for c in CATEGORIES:
        allowed = sorted(s for (cc, s) in legal if cc == c)
        print(f"  {c:<18} -> {', '.join(allowed) if allowed else '(nothing)'}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--generate", metavar="DIR", type=Path)
    ap.add_argument("--table", metavar="VERDICTS", type=Path)
    ap.add_argument("--corpus", metavar="DIR", type=Path,
                    default=Path("test-data/interop"),
                    help="corpus to cross-check the table against")
    a = ap.parse_args()
    if a.generate:
        return generate(a.generate)
    if a.table:
        return table(a.table, a.corpus if a.corpus.is_dir() else None)
    ap.print_help()
    return 2


if __name__ == "__main__":
    sys.exit(main())
