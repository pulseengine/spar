#!/usr/bin/env python3
"""Format-check every workspace in the repo, not just the one cargo defaults to (#383).

THE FAILURE
-----------
The required `Format` status check ran

    cargo fmt --all -- --check

and reported green while `fuzz/fuzz_targets/fuzz_codegen_roundtrip.rs` was
unformatted on main. `--all` does not mean "all crates in the repo". It means
"all members of *this* workspace", and this repo has four workspaces:

    Cargo.toml                      the 23-crate root
    fuzz/Cargo.toml                 own [workspace] table
    codegen-exec-oracle/Cargo.toml  own [workspace] table
    codegen-kiln-oracle/Cargo.toml  own [workspace] table

A nested crate with its own `[workspace]` table is a separate workspace, and
the root's `--all` never opens it. Measured on main:

    cargo fmt --all -- --check                            -> 0
    cargo fmt --manifest-path fuzz/Cargo.toml -- --check  -> 1  (13 diff lines)

WHY IT SURVIVED
---------------
The same property as #381: the narrow answer is byte-identical to the broad
one. `cargo fmt --all --check` exiting 0 over 23 of 26 crates prints exactly
what it prints over all 26 — nothing. A gate that says nothing on success
cannot distinguish "checked everything" from "checked a subset". So this
script's contract is that it always prints the count of what it examined:

    4 workspaces checked, 4 formatted

A count is not producible by a broken scan. That is the entire trick.

DISCOVERY, NOT ENUMERATION
--------------------------
A hard-coded list of four manifests would fix today's bug and re-open it the
day someone adds a fifth workspace. So the set is discovered by walking the
tree for `Cargo.toml` files containing a `[workspace]` table, and the count is
floored at --min-workspaces. Discovery that silently returns fewer manifests
than expected is the failure mode that would restore the original bug, so it
is the one thing checked before any formatting happens.

THE PRUNE LIST IS LOAD-BEARING
------------------------------
`target/` fills up with vendored dependency sources, and plenty of crates.io
packages ship a `[workspace]` table. A naive walk finds dozens of them on any
machine that has built once, sails past a `>= 4` floor, and then asks cargo to
format other people's code out of the build cache. The floor check would still
be green, which is what makes this worth a self-test case rather than a
comment: the bug it prevents is one that *passes*.

EXIT CODES
----------
    0  every discovered workspace is formatted
    1  at least one is not (or discovery came up short)
    2  the check could not run at all (cargo missing, root unreadable)

2 exists so that "I could not measure" is not spelled the same way as "I
measured and it was fine". Collapsing those two into 0 is how #381 lasted four
months.
"""

from __future__ import annotations

import argparse
import io
import os
import re
import subprocess
import sys
import tempfile

EXIT_OK = 0
EXIT_VIOLATION = 1
EXIT_CANNOT_CHECK = 2

# Four today. This is a floor, not an expectation: adding a fifth workspace
# should not require touching this file, but losing one should be loud.
MIN_WORKSPACES = 4

# Directories never descended into. `target` is the one that matters (see the
# module docstring); the rest are conventional build/VCS residue.
PRUNE = {"target", ".git", "node_modules", ".direnv", "result"}

# A `[workspace]` table header, alone on its line. Deliberately NOT matching
# `[workspace.package]` / `[workspace.dependencies]`, which appear in the root
# manifest and in members that inherit from it — those do not create a
# workspace, and counting them would inflate the total past the floor.
WORKSPACE_TABLE = re.compile(r"^\[workspace\]\s*$", re.MULTILINE)


def discover(root):
    """Return the sorted repo-relative manifests that declare a [workspace] table."""
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in PRUNE)
        if "Cargo.toml" not in filenames:
            continue
        path = os.path.join(dirpath, "Cargo.toml")
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError:
            continue
        if WORKSPACE_TABLE.search(text):
            found.append(os.path.relpath(path, root))
    return sorted(found)


def _cargo_fmt(root, manifest):
    """Real runner: (exit_code, combined_output). Raises OSError if cargo is absent."""
    proc = subprocess.run(
        ["cargo", "fmt", "--manifest-path", manifest, "--all", "--", "--check"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


def check(
    root,
    *,
    min_workspaces=MIN_WORKSPACES,
    runner=None,
    out=None,
    tool="fmt",
    verb="formatted",
    fix_hint="cargo fmt --manifest-path {manifest} --all",
):
    """Discover every workspace, run `runner` in each, and always print the count.

    `tool`/`verb`/`fix_hint` exist so the SAME discovery + floor + always-print-a-
    count logic backs more than one gate. REQ-GUARD-GATE-EVIDENCE-002 (g) is
    explicit that the obligation is "every whole-repo tool uses it, not just the
    one that was caught" — clippy inherits `--workspace` exactly as `cargo fmt
    --all` did, covering 1 of this repo's 4 workspaces (#383). Copying this
    function for clippy would put the discovery in two places, which is the
    drift that made #381 possible; injecting the wording keeps it in one.

    Defaults reproduce the fmt wording verbatim, so existing callers and the
    self-test below are unchanged by this parameterisation.
    """
    out = out or sys.stdout
    runner = runner or _cargo_fmt

    if not os.path.isdir(root):
        print(f"::error::not a directory: {root}", file=sys.stderr)
        return EXIT_CANNOT_CHECK

    manifests = discover(root)

    if not manifests:
        # Distinct from the floor message below on purpose. Zero means the walk
        # itself is broken (wrong root, prune list too greedy); fewer-than-N
        # means a workspace was removed. Same exit code, different repair.
        print(
            "::error::discovered NO workspace manifests — the scan is broken, "
            f"not the tree (root: {root})",
            file=sys.stderr,
        )
        return EXIT_CANNOT_CHECK

    if len(manifests) < min_workspaces:
        print(
            f"::error::discovered {len(manifests)} workspace(s), expected at least "
            f"{min_workspaces}: {', '.join(manifests)}",
            file=sys.stderr,
        )
        print(
            f"::error::a workspace that vanishes from discovery is silently exempt "
            f"from {tool} — that is #383. Lower --min-workspaces only when a "
            f"workspace was deliberately removed.",
            file=sys.stderr,
        )
        return EXIT_VIOLATION

    dirty = []
    for manifest in manifests:
        try:
            code, output = runner(root, manifest)
        except OSError as exc:
            print(f"::error::could not run cargo {tool} for {manifest}: {exc}", file=sys.stderr)
            return EXIT_CANNOT_CHECK
        if code == 0:
            print(f"  ok   {manifest}", file=out)
        else:
            dirty.append((manifest, output))
            print(f"  DIRTY {manifest} (exit {code})", file=out)

    # The positive count, always, on every path. This line is the fix.
    print(
        f"{len(manifests)} workspace(s) checked, {len(manifests) - len(dirty)} {verb}.",
        file=out,
    )

    if dirty:
        for manifest, output in dirty:
            print(
                f"::error::{manifest} is not {verb} — run: "
                + fix_hint.format(manifest=manifest),
                file=sys.stderr,
            )
            for line in output.splitlines()[:40]:
                print(f"  | {line}", file=sys.stderr)
        return EXIT_VIOLATION

    return EXIT_OK


# ── self-test ────────────────────────────────────────────────────────────
# Every case is a real directory tree under a temp root. The runner is faked so
# the table exercises discovery and reporting without needing a toolchain; the
# real cargo invocation is one subprocess.run and is not what broke.

def _tree(root, spec):
    """spec: {relative_dir: manifest_text}. Creates Cargo.toml files."""
    for rel, text in spec.items():
        d = os.path.join(root, rel) if rel else root
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "Cargo.toml"), "w", encoding="utf-8") as fh:
            fh.write(text)


WS = '[workspace]\nmembers = ["a"]\n'
MEMBER = '[package]\nname = "a"\n'
INHERIT = '[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "a"\n'

FOUR = {"": WS, "fuzz": WS, "codegen-exec-oracle": WS, "codegen-kiln-oracle": WS}

CASES = [
    (
        "the repo's four workspaces are all discovered",
        FOUR,
        lambda r, m: (0, ""),
        4,
        EXIT_OK,
        ["4 workspace(s) checked, 4 formatted"],
        [],
        "discovery finds nested [workspace] tables, not just the root",
    ),
    (
        "REGRESSION #383: a dirty NON-ROOT workspace fails the gate",
        FOUR,
        lambda r, m: (1, "Diff in x.rs") if m.startswith("fuzz") else (0, ""),
        4,
        EXIT_VIOLATION,
        ["DIRTY fuzz/Cargo.toml", "4 workspace(s) checked, 3 formatted"],
        [],
        "the exact live bug: root clean, fuzz/ dirty, old gate said 0",
    ),
    (
        "a dirty ROOT workspace still fails (no regression the other way)",
        FOUR,
        lambda r, m: (1, "Diff in y.rs") if m == "Cargo.toml" else (0, ""),
        4,
        EXIT_VIOLATION,
        ["DIRTY Cargo.toml"],
        [],
        "widening scope did not lose the coverage that already worked",
    ),
    (
        "PRUNE: vendored [workspace] under target/ is not discovered",
        {**FOUR, "target/package/somecrate-1.0": WS},
        lambda r, m: (0, ""),
        4,
        EXIT_OK,
        ["4 workspace(s) checked"],
        ["target/"],
        "a built machine does not format other people's cached sources",
    ),
    (
        "PRUNE: nested crate's own target/ is skipped too",
        {**FOUR, "fuzz/target/debug/build/dep-abc": WS},
        lambda r, m: (0, ""),
        4,
        EXIT_OK,
        ["4 workspace(s) checked"],
        ["fuzz/target/"],
        "the walk prunes by directory name at every depth, not just the top",
    ),
    (
        "[workspace.package] is NOT a workspace root",
        {**FOUR, "crates/spar-hir": INHERIT},
        lambda r, m: (0, ""),
        4,
        EXIT_OK,
        ["4 workspace(s) checked"],
        ["crates/spar-hir"],
        "inheriting members must not inflate the count past the floor",
    ),
    (
        "a member crate with no [workspace] is not counted",
        {**FOUR, "crates/spar-parser": MEMBER},
        lambda r, m: (0, ""),
        4,
        EXIT_OK,
        ["4 workspace(s) checked"],
        ["spar-parser"],
        "the 23 root members are formatted BY the root, not counted separately",
    ),
    (
        "a workspace going missing trips the floor",
        {"": WS, "fuzz": WS, "codegen-exec-oracle": WS},
        lambda r, m: (0, ""),
        4,
        EXIT_VIOLATION,
        [],
        [],
        "silently checking fewer workspaces than yesterday is #383 returning",
    ),
    (
        "discovery finding nothing is CANNOT_CHECK, not a pass",
        {"crates/spar-parser": MEMBER},
        lambda r, m: (0, ""),
        4,
        EXIT_CANNOT_CHECK,
        [],
        [],
        "an empty scan must not render as 'everything is formatted'",
    ),
    (
        "cargo absent is CANNOT_CHECK, not a pass",
        FOUR,
        lambda r, m: (_ for _ in ()).throw(OSError("No such file or directory: 'cargo'")),
        4,
        EXIT_CANNOT_CHECK,
        [],
        [],
        "the #381 lesson: a tool that never ran scores neither 0 nor green",
    ),
]


def self_test():
    failures = 0
    print("check_fmt_workspaces self-test")
    for name, spec, runner, floor, want_exit, want_in, want_not_in, proves in CASES:
        with tempfile.TemporaryDirectory() as root:
            _tree(root, spec)
            buf, real = io.StringIO(), sys.stderr
            sys.stderr = buf
            try:
                got = check(root, min_workspaces=floor, runner=runner, out=buf)
            except Exception as exc:  # a crash is a failed case, not a crashed run
                sys.stderr = real
                print(f"  FAIL {name}: raised {exc!r}")
                failures += 1
                continue
            finally:
                sys.stderr = real
            text = buf.getvalue()

        ok = got == want_exit
        for needle in want_in:
            if needle not in text:
                ok = False
        for needle in want_not_in:
            if needle in text:
                ok = False
        failures += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} {name}: exit {got} (want {want_exit})")
        print(f"       proves: {proves}")
        if not ok:
            for line in text.splitlines():
                print(f"       | {line.replace('::error::', '')}")

    print()
    if failures:
        print(f"{failures} self-test(s) FAILED.", file=sys.stderr)
        return EXIT_VIOLATION
    print(f"{len(CASES)} self-test(s) passed.")
    return EXIT_OK


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", default=".", help="repository root (default: .)")
    ap.add_argument(
        "--min-workspaces",
        type=int,
        default=MIN_WORKSPACES,
        help=f"fail if discovery finds fewer than this many (default: {MIN_WORKSPACES})",
    )
    ap.add_argument("--self-test", action="store_true", help="run the decision table and exit")
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    return check(args.root, min_workspaces=args.min_workspaces)


if __name__ == "__main__":
    sys.exit(main())
