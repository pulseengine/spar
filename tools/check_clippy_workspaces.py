#!/usr/bin/env python3
"""Run clippy in EVERY workspace, not just the one cargo defaults to.

REQ-GUARD-GATE-EVIDENCE-002 (g). The clippy half of #383.

WHY THIS EXISTS
===============

`cargo clippy --workspace --all-targets -- -D warnings` means "all members of
*this* workspace" — the same scope `cargo fmt --all` had. spar has four:

    Cargo.toml
    codegen-exec-oracle/Cargo.toml
    codegen-kiln-oracle/Cargo.toml
    fuzz/Cargo.toml

so the `Clippy` required check has been linting one of them and reporting green
for the repo. Three workspaces have been unlinted while a required gate said
otherwise. That is #383's second half; `check_fmt_workspaces.py` fixed the first
and left this one, and the requirement is explicit that the obligation is
"every whole-repo tool uses [discovery], not just the one that was caught".

WHY THIS FILE IS THIN
=====================

The discovery, the `--min-workspaces` floor, the broken-scan-vs-removed-
workspace distinction, and the always-print-a-count contract already exist and
are already self-tested in `check_fmt_workspaces.py`. This imports them and
supplies a clippy runner.

Copying that logic instead would put workspace discovery in two files, and a
fact duplicated in two places is exactly what made #381 possible — the report
path was written twice and the two copies disagreed for four months. One
definition, two callers.

WHAT IT DOES NOT DO
===================

It does not pick the lint set. `-D warnings` and any `[lints]` tables are the
repo's business; this only ensures the tool is pointed at every workspace. A
workspace with no lints configured will pass here and still be under-linted —
that is a different obligation.

Measured 2026-08-06 before wiring: all four workspaces are already clippy-clean
under `-D warnings`, so turning this on is not a latent red. If a future
workspace lands dirty, that is the gate working, not this script mis-scoping.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from check_fmt_workspaces import (  # noqa: E402  (path set above)
    EXIT_CANNOT_CHECK,
    MIN_WORKSPACES,
    check,
    discover,
)


def _cargo_clippy(root, manifest):
    """Real runner: (exit_code, combined_output). Raises OSError if cargo is absent."""
    proc = subprocess.run(
        [
            "cargo",
            "clippy",
            "--manifest-path",
            manifest,
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        cwd=root,
        capture_output=True,
        text=True,
    )
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


def self_test() -> int:
    """Assert the clippy wiring, not the discovery (which check_fmt_workspaces owns).

    Two cases, and the pair is the point: the SAME tree passes when the runner
    reports clean and fails when it reports dirty. A wrapper that ignored its
    runner's exit code — the way to make this gate vacuous — passes the first
    and fails the second.
    """
    import io
    import tempfile

    passed = failed = 0

    def case(desc, want, runner):
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            for sub in ("", "a", "b", "c"):
                d = os.path.join(td, sub)
                os.makedirs(d, exist_ok=True)
                with open(os.path.join(d, "Cargo.toml"), "w", encoding="utf-8") as fh:
                    fh.write('[workspace]\nmembers = []\n')
            buf = io.StringIO()
            got = check(
                td,
                runner=runner,
                out=buf,
                tool="clippy",
                verb="lint-clean",
                fix_hint="cargo clippy --manifest-path {manifest} --all-targets -- -D warnings",
            )
        if got == want:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: got {got}, want {want}")
            for line in buf.getvalue().splitlines()[:5]:
                print(f"         {line}")

    print("check_clippy_workspaces self-test")
    case("all workspaces lint-clean -> pass", 0, lambda r, m: (0, ""))
    case("a dirty workspace -> FAIL (runner exit is honoured)", 1,
         lambda r, m: (101, "error: unused variable"))
    case("cargo missing is CANNOT_CHECK, not a pass", EXIT_CANNOT_CHECK,
         lambda r, m: (_ for _ in ()).throw(OSError("no cargo")))
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", default=".")
    ap.add_argument("--min-workspaces", type=int, default=MIN_WORKSPACES)
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    print("== clippy-scope guardrail ==")
    print(f"workspaces discovered: {len(discover(a.root))}")
    return check(
        a.root,
        min_workspaces=a.min_workspaces,
        runner=_cargo_clippy,
        tool="clippy",
        verb="lint-clean",
        fix_hint="cargo clippy --manifest-path {manifest} --all-targets -- -D warnings",
    )


if __name__ == "__main__":
    sys.exit(main())
