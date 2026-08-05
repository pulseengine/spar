#!/usr/bin/env python3
"""Gate the declared MSRV against the resolved dependency closure (#369).

THE FAILURE
-----------
spar has a minimum supported Rust version whether or not anyone writes it
down, because cargo enforces one: a build fails if the toolchain is below the
maximum `rust-version` over the entire *resolved dependency closure*. Before
this gate, `[workspace.package]` declared no `rust-version` and there was no
`rust-toolchain.toml`, so the real floor was

    max(rust-version) over every package in Cargo.lock

— an emergent property of the lockfile. It moves on any `cargo update` with no
diff anywhere in this repo, and it was discovered at build time by whichever
consumer happened to have the oldest toolchain.

WHY IT IS WORTH A GATE RATHER THAN A DECLARATION
------------------------------------------------
It already caused a defect. In #364 the tools/fixture-vm nixpkgs pin was moved
24.05 -> 25.05 and recorded as "cannot compile -> can compile". The reasoning
was: edition 2024 needs rustc >= 1.85, 25.05 ships 1.86.0, 1.86 >= 1.85, done.
Every step of that is true. The conclusion was still wrong:

    error: rustc 1.86.0 is not supported by the following package:
      smol_str@0.3.6 requires rustc 1.89

The edition floor is a *necessary* condition that got mistaken for *the*
condition. The failure mode worth naming, because it is not "forgot to check":
the probe genuinely ran and genuinely returned true. A one-variable check that
comes back green is the most convincing way to not verify something — it
produces the feeling of verification while silently scoping the claim to the
one variable you happened to think of.

A bare `rust-version = "..."` line does not fix that. A declaration is just
another number that can go stale against the closure. So the declaration is the
subject of this check, not the answer to it.

WHAT IT CHECKS
--------------
1. Every workspace member has an effective `rust-version`.  A new crate that
   forgets `rust-version.workspace = true` is silently exempt from the floor,
   and would publish to crates.io claiming to build on toolchains it cannot.
2. All members agree.  A per-crate override that diverges from the workspace
   makes "the MSRV" ambiguous.
3. **The closure maximum does not exceed the declared floor.**  This is the
   drift gate: the `cargo update` that raises the real floor goes red on the PR
   that introduces it, naming the package responsible, instead of surfacing two
   months later inside a nix derivation.

WHY IT READS `cargo metadata` RATHER THAN PARSING Cargo.toml
------------------------------------------------------------
`rust-version.workspace = true` means the value a member actually carries is
computed by cargo, not written in its manifest. Re-parsing TOML would check
what the files say; `cargo metadata` reports what cargo resolved, which is what
is actually enforced. It also makes guard 1 fall out for free — a member that
failed to inherit simply has no `rust_version` in the metadata.

`--locked` is deliberate: the check must describe the committed lockfile, not
whatever a resolver would pick today.

THE DELIBERATE ASYMMETRY
------------------------
declared < closure  -> FAIL.  Consumers on the declared floor cannot build.
declared > closure  -> report, do not fail.  An inflated floor excludes
consumers unnecessarily, but it can also be a legitimate choice: spar's own
source may use a feature newer than anything its dependencies require, and
nothing here can tell those two cases apart. Failing would block the
legitimate one. It is printed so it stays visible rather than silently drifting.

WHAT THIS DOES NOT CLAIM
------------------------
- It does NOT verify that spar's own source compiles on the declared floor.
  That needs a CI job pinned to that toolchain, which this is not. The claim
  here is narrower and exact: the declared floor is not below what the
  dependency closure demands.
- It does not check the nix channel's rustc. That comparison lives in the
  fixture-vm workflow, where a nix evaluation is already paid for, and is what
  turns a 14-minute build failure into a several-second one.

Stdlib only, matching the constraint documented above the human-scoped
guardrail in ci.yml: no workflow here installs a Python package, so importing
one would make a *required* check depend on an unverified runner package.
"""

from __future__ import annotations

import argparse
import io
import json
import subprocess
import sys

EXIT_OK = 0
EXIT_VIOLATION = 1
EXIT_CANNOT_CHECK = 2


def parse_version(text):
    """`1.89` -> (1, 89, 0). Zero-padded so `1.89` and `1.89.0` compare equal.

    Numeric, never lexicographic. The trap this avoids is real: as strings,
    "1.100" < "1.99", so a naive comparison would pass a closure floor that is
    actually higher than the declared one. `sort -V` gets this right but is a
    GNU extension; this does not depend on which sort is installed.
    """
    parts = text.strip().split(".")
    out = []
    for p in parts[:3]:
        digits = ""
        for ch in p:
            if not ch.isdigit():
                break
            digits += ch
        if not digits:
            raise ValueError(f"not a version: {text!r}")
        out.append(int(digits))
    while len(out) < 3:
        out.append(0)
    return tuple(out)


def load_metadata():
    """Run `cargo metadata`, or exit 2. A gate that cannot run is not a pass."""
    try:
        proc = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        print("::error::cargo not found; cannot derive the closure MSRV.", file=sys.stderr)
        sys.exit(EXIT_CANNOT_CHECK)
    if proc.returncode != 0:
        print("::error::`cargo metadata --locked` failed. If this is a lockfile", file=sys.stderr)
        print("::error::mismatch, commit the updated Cargo.lock.", file=sys.stderr)
        sys.stderr.write(proc.stderr[-2000:])
        sys.exit(EXIT_CANNOT_CHECK)
    return json.loads(proc.stdout)


def analyse(meta):
    """Split the resolved graph into workspace members and everything else.

    Returns (members, closure) as lists of (name, version, rust_version|None).
    """
    member_ids = set(meta.get("workspace_members", []))
    members, closure = [], []
    for p in meta.get("packages", []):
        row = (p.get("name"), p.get("version"), p.get("rust_version"))
        (members if p.get("id") in member_ids else closure).append(row)
    return members, closure


def run_check(meta, verbose=True):
    members, closure = analyse(meta)
    problems = []

    if not members:
        print("::error::no workspace members in cargo metadata — refusing to", file=sys.stderr)
        print("::error::report a floor derived from nothing.", file=sys.stderr)
        return EXIT_CANNOT_CHECK

    # Guard 1: a member with no effective rust-version is exempt from the floor.
    missing = [f"{n}@{v}" for n, v, rv in members if not rv]
    if missing:
        problems.append(
            "::error::%d workspace member(s) declare no rust-version:\n%s\n"
            "::error::Add `rust-version.workspace = true` to each, next to\n"
            "::error::`edition.workspace = true`. Without it the crate publishes\n"
            "::error::claiming to build on toolchains the workspace does not support."
            % (len(missing), "\n".join("::error::  " + m for m in missing))
        )

    # Guard 2: members must agree, or "the MSRV" has no referent.
    declared_set = {rv for _, _, rv in members if rv}
    if len(declared_set) > 1:
        problems.append(
            "::error::workspace members declare conflicting rust-versions: %s\n"
            "::error::Set it once in [workspace.package] and inherit it."
            % ", ".join(sorted(declared_set))
        )

    if problems:
        for p in problems:
            print(p, file=sys.stderr)
        return EXIT_VIOLATION

    declared_text = next(iter(declared_set))
    declared = parse_version(declared_text)

    # Guard 3: the drift gate.
    rated = [(parse_version(rv), n, v, rv) for n, v, rv in closure if rv]
    if not rated:
        print("::error::no dependency in the closure declares a rust-version.", file=sys.stderr)
        print("::error::That is implausible for this lockfile — refusing to pass", file=sys.stderr)
        print("::error::on a comparison against an empty set.", file=sys.stderr)
        return EXIT_CANNOT_CHECK

    top = max(rated)
    closure_max, top_name, top_ver, top_text = top
    raisers = sorted({f"{n}@{v}" for pv, n, v, _ in rated if pv == closure_max})

    if verbose:
        print(
            f"declared floor : {declared_text}  ([workspace.package] rust-version, "
            f"inherited by {len(members)} member(s))"
        )
        print(
            f"closure maximum: {top_text}  (set by {', '.join(raisers)}; "
            f"{len(rated)} of {len(closure)} dependencies declare one)"
        )

    if closure_max > declared:
        print(
            "::error::the dependency closure requires rustc %s, but "
            "[workspace.package] rust-version declares %s." % (top_text, declared_text),
            file=sys.stderr,
        )
        for r in raisers:
            print(f"::error::  raised by: {r}", file=sys.stderr)
        print(
            "::error::Anything built against the declared floor will fail with\n"
            "::error::`rustc %s is not supported by the following package`.\n"
            "::error::Either raise rust-version to %s, or pin the dependency back."
            % (declared_text, top_text),
            file=sys.stderr,
        )
        return EXIT_VIOLATION

    if closure_max < declared and verbose:
        print(
            f"note: the declared floor is above the closure maximum "
            f"({declared_text} > {top_text}). Not an error — spar's own source may "
            f"need it — but nothing here verifies that, so it is reported rather "
            f"than assumed."
        )

    if verbose:
        print(f"PASS: declared floor {declared_text} covers the closure.")
    return EXIT_OK


# ---------------------------------------------------------------------------
# Self-tests. Fixtures are cargo-metadata-shaped dicts, so they run without a
# toolchain and without a lockfile. Each records the failure it proves is still
# caught; a test whose `proves` string does not name a real failure is a test
# that will survive the deletion of the logic it claims to cover.
# ---------------------------------------------------------------------------


def _meta(members, deps):
    """members/deps: list of (name, version, rust_version|None)."""
    pkgs, ids = [], []
    for i, (n, v, rv) in enumerate(members):
        pid = f"member {i}"
        ids.append(pid)
        pkgs.append({"id": pid, "name": n, "version": v, "rust_version": rv})
    for i, (n, v, rv) in enumerate(deps):
        pkgs.append({"id": f"dep {i}", "name": n, "version": v, "rust_version": rv})
    return {"workspace_members": ids, "packages": pkgs}


SELF_TESTS = [
    (
        "clean: declared equals closure max",
        _meta([("spar-cli", "0.35.0", "1.89")], [("smol_str", "0.3.6", "1.89")]),
        EXIT_OK,
        "the checker can return 0 at all — without this the rest is consistent "
        "with a script that always fails",
    ),
    (
        "closure above declared (the #364 defect shape)",
        _meta([("spar-cli", "0.35.0", "1.86")], [("smol_str", "0.3.6", "1.89")]),
        EXIT_VIOLATION,
        "the exact drift that broke the fixture-vm build: a dependency demands "
        "more than the declared floor",
    ),
    (
        "member missing rust-version",
        _meta(
            [("spar-cli", "0.35.0", "1.89"), ("spar-new", "0.35.0", None)],
            [("smol_str", "0.3.6", "1.89")],
        ),
        EXIT_VIOLATION,
        "a new crate that forgets `rust-version.workspace = true` is exempt from "
        "the floor and would publish a claim it cannot honour",
    ),
    (
        "members disagree",
        _meta(
            [("spar-cli", "0.35.0", "1.89"), ("spar-core", "0.35.0", "1.85")],
            [("smol_str", "0.3.6", "1.85")],
        ),
        EXIT_VIOLATION,
        "with two different floors declared, 'the MSRV' has no referent and the "
        "comparison in guard 3 would be against an arbitrary one of them",
    ),
    (
        "declared above closure max is allowed",
        _meta([("spar-cli", "0.35.0", "1.90")], [("smol_str", "0.3.6", "1.89")]),
        EXIT_OK,
        "the asymmetry is deliberate: spar's own source may need more than its "
        "dependencies, and failing here would block that legitimate case",
    ),
    (
        "1.89 vs 1.89.0 are the same floor",
        _meta([("spar-cli", "0.35.0", "1.89")], [("dep", "1.0.0", "1.89.0")]),
        EXIT_OK,
        "crates.io mixes both spellings (this lockfile has '1.89' and '1.87.0'); "
        "treating them as different would red the build over formatting",
    ),
    (
        "1.100 is above 1.99, not below it",
        _meta([("spar-cli", "0.35.0", "1.99")], [("dep", "1.0.0", "1.100")]),
        EXIT_VIOLATION,
        "as strings '1.100' < '1.99', so a lexicographic compare passes a floor "
        "that is actually too low — the comparison must be numeric",
    ),
    (
        "no dependency declares a rust-version",
        _meta([("spar-cli", "0.35.0", "1.89")], [("dep", "1.0.0", None)]),
        EXIT_CANNOT_CHECK,
        "an empty closure means the metadata was not what we think it is; "
        "passing on a comparison against nothing reads as coverage",
    ),
    (
        "no workspace members",
        _meta([], [("smol_str", "0.3.6", "1.89")]),
        EXIT_CANNOT_CHECK,
        "same fail-closed rule at the other end: no members means no declared "
        "floor to compare against",
    ),
]


def self_test():
    failures = 0
    for name, meta, want, proves in SELF_TESTS:
        # Capture the fixtures' stderr rather than letting it through. Six of
        # these cases are *supposed* to fail, and their messages are
        # `::error::` lines — which are GitHub Actions workflow commands, not
        # plain text. Emitted from a passing self-test they would stamp eight
        # red annotations onto the PR's diff view while the step exits 0. That
        # is the same defect this whole gate exists to prevent: output that
        # renders as a claim it does not support. The captured text is printed
        # only when a case actually fails, where it is the debugging evidence.
        buf, real = io.StringIO(), sys.stderr
        sys.stderr = buf
        try:
            got = run_check(meta, verbose=False)
        finally:
            sys.stderr = real
        ok = got == want
        failures += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} {name}: exit {got} (want {want})")
        print(f"       proves: {proves}")
        if not ok:
            for line in buf.getvalue().splitlines():
                print(f"       | {line.replace('::error::', '')}")
    print()
    if failures:
        print(f"{failures} self-test(s) FAILED.", file=sys.stderr)
        return EXIT_VIOLATION
    print(f"{len(SELF_TESTS)} self-test(s) passed.")
    return EXIT_OK


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="run the inline fixtures; no cargo or lockfile needed",
    )
    ap.add_argument(
        "--print",
        dest="print_only",
        action="store_true",
        help="print the derived floors and exit 0 regardless of the verdict",
    )
    args = ap.parse_args()

    if args.self_test:
        sys.exit(self_test())

    meta = load_metadata()
    rc = run_check(meta)
    sys.exit(EXIT_OK if args.print_only else rc)


if __name__ == "__main__":
    main()
