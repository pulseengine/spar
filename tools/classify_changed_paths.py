#!/usr/bin/env python3
"""Decide whether a PR diff is code, and never answer "no" by accident.

REQ-GUARD-GATE-EVIDENCE-002 (e). Closes #384.

WHY THIS EXISTS
===============

`Detect changed paths` gates 10 of the 18 required status checks; the identical
logic cloned into `proofs.yml` gates an 11th (`Lean proof typecheck`). It could
be made to answer "no code changed" two different ways, and in both the error
path produced the *permissive* verdict with the step exiting 0.

`skipped` satisfies a required context. A skipped required check and a passed
one are the same green on the merge button.

Mechanism A — the allow-list was scoped by PREFIX, not by file type
------------------------------------------------------------------

    allow='^(artifacts/.*\\.ya?ml|safety/.*|docs/.*|rivet\\.yaml|[^/]*\\.md|.*/.*\\.md)$'

`artifacts/` and the two `*.md` entries name the extensions they admit.
`safety/.*` and `docs/.*` do not — they admit **anything** under those trees,
including `.rs`, `.sh`, `.toml`. `docs/research/forge-replication/` already
holds `run_trial.sh`, `tasks.json`, `descriptor.sexp` and `descriptor.aadl`, so
this was one `git mv` away from live rather than purely theoretical.

Mechanism B — the error path failed OPEN
----------------------------------------

    n_code=$(printf '%s\\n' "$changed" | grep -cvE "$allow" || true)
    if [ "$n_code" -gt 0 ]; then ... else echo "code=false" ...

If `grep` failed for any reason other than "zero matches" — an unbalanced group
after an edit, `grep` missing from `PATH` — it wrote nothing, `n_code` became
the empty string, and `[ "" -gt 0 ]` exited 2. Inside an `if` condition `set -e`
is inert, so bash read that non-zero as *false* and took the `code=false`
branch. Two error messages printed to the log and the step still exited 0,
having skipped the suite.

The `|| true` was not simply removable: `grep -c` exits 1 on a zero count, which
is the legitimate "everything was allow-listed" outcome. That is the trap — the
suppression was load-bearing for the success path and catastrophic for the
failure path, because both arrive as a non-zero exit.

WHAT REPLACES IT
================

One script, called from both workflows.

* **Type-scoped allow-list.** Every entry names its extensions. A path that
  matches nothing is CODE. Unknown is not a licence to skip — a classifier that
  cannot recognise a file has not shown it is inert.
* **No error path can emit `code=false`.** Any failure raises, `main` catches,
  prints, and exits non-zero *without emitting a verdict at all*.
* **The script owns `$GITHUB_OUTPUT`.** It opens and appends the file itself.
  This is the only emission mode on purpose. Had the workflow done

      echo "code=$(python3 tools/classify_changed_paths.py ...)" >> "$GITHUB_OUTPUT"

  mechanism B would be back verbatim in the glue: the script dies, `echo`
  succeeds, `code=` lands empty, every `== 'true'` dependent skips, exit 0. The
  decision and its emission must not be separated by a shell that fails open.
* **Every file prints with its verdict**, on every path including success, so
  "these 4 files were skipped" is readable rather than inferred from silence.

WHY EXITING NON-ZERO IS SAFE HERE, AND WHY THAT IS NOT OBVIOUS
--------------------------------------------------------------

A failing `changes` job means its dependents (`if: needs.changes.outputs.code ==
'true'`) are **skipped**, and skipped satisfies a required context. So exiting
non-zero would normally be *worse* than failing open.

It is safe only because `Detect changed paths` and `Detect changed paths
(proofs)` are themselves required contexts (verified against the branch
protection API, not assumed). The classifier going red therefore blocks the
merge on its own. Two further facts were checked rather than trusted:

* every consumer of `outputs.code` tests `== 'true'`, so no job runs on the
  inverse and an absent output cannot trigger anything;
* `python3` is present on `[self-hosted, linux, x64, light]` — `ci.yml::fmt`
  already runs `check_fmt_workspaces.py` on that exact label.

**If `Detect changed paths` is ever removed from the required contexts, this
file becomes unsafe.** `check_required_contexts.py` is what keeps that honest.

ONE DELIBERATE BEHAVIOUR CHANGE
-------------------------------

The two copies had already drifted: `ci.yml` gained `rivet\\.yaml` (#379),
`proofs.yml` never did. Single-sourcing gives the proofs side that entry, so a
rivet.yaml-only PR now also skips the Lean job. That is safe for the same reason
#379 gave for the CI side — nothing under `proofs/` reads `rivet.yaml` — and it
is exactly the class of divergence single-sourcing exists to end. It is a
loosening, so it is called out here rather than absorbed silently.

NOT CLAIMED: that skipping is right for any particular file. This only ensures
the *reason* for a skip is a match against a declared, type-scoped pattern, and
never a crash.
"""

from __future__ import annotations

import argparse
import io
import re
import subprocess
import sys
import tempfile
from pathlib import Path

# The allow-list: a changed file is non-code only if it FULL-matches one of
# these. Every entry names the extensions it admits — a bare directory prefix is
# mechanism A and must never reappear here. `_no_pattern_admits_code` in the
# self-test enforces that mechanically for any pattern added later.
_ALLOW: list[tuple[str, str]] = [
    ("rivet artifact YAML", r"artifacts/(?:.*/)?[^/]+\.ya?ml"),
    ("STPA safety YAML", r"safety/(?:.*/)?[^/]+\.ya?ml"),
    # Root-level rivet manifest (#379). Not under artifacts/, so it needs its
    # own entry; nothing that compiles reads it.
    ("rivet manifest", r"rivet\.yaml"),
    # Markdown anywhere. Checked: no .md in the tree is consumed as build or
    # test data — the `docs/design/*.md` filters in spar-codegen's golden tests
    # match GENERATED output paths, not repo files.
    ("markdown", r"(?:.*/)?[^/]+\.md"),
]

_ALLOW_RE: list[tuple[str, re.Pattern[str]]] = [(k, re.compile(p)) for k, p in _ALLOW]


def classify_one(path: str) -> str | None:
    """Return the allow-list label that admits `path`, or None if it is code."""
    for label, rx in _ALLOW_RE:
        if rx.fullmatch(path):
            return label
    return None


def classify(paths: list[str]) -> tuple[bool, list[tuple[str, str | None]]]:
    """(is_code, [(path, label_or_None)]). is_code once ANY path is unmatched."""
    rows = [(p, classify_one(p)) for p in paths]
    return any(label is None for _, label in rows), rows


def changed_paths(base: str, head: str, cwd: str | None = None) -> list[str]:
    """`git diff --name-only base...head`. Raises — never returns a short list."""
    proc = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...{head}"],
        capture_output=True,
        text=True,
        cwd=cwd,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"git diff {base}...{head} failed (exit {proc.returncode}): "
            f"{proc.stderr.strip() or '(no stderr)'}"
        )
    return [line for line in proc.stdout.splitlines() if line.strip()]


def emit(is_code: bool, github_output: str) -> None:
    """Append the verdict. The ONLY way a verdict leaves this process."""
    with open(github_output, "a", encoding="utf-8") as fh:
        fh.write(f"code={'true' if is_code else 'false'}\n")


def run(
    event_name: str,
    base_sha: str,
    head: str,
    github_output: str,
    cwd: str | None = None,
    out=sys.stdout,
) -> int:
    print("== changed-path classifier ==", file=out)
    print(f"event: {event_name}", file=out)

    if event_name != "pull_request":
        print(f"non-PR event -> full suite.", file=out)
        emit(True, github_output)
        return 0

    if not base_sha:
        raise RuntimeError(
            "pull_request event with an empty base sha — the diff cannot be "
            "computed, so no file can be shown to be inert."
        )

    paths = changed_paths(base_sha, head, cwd=cwd)
    if not paths:
        # A PR with a genuinely empty diff is odd; so is a base sha that makes
        # the diff look empty. Both get the full suite.
        print("empty diff -> full suite (fail-safe).", file=out)
        emit(True, github_output)
        return 0

    is_code, rows = classify(paths)
    for path, label in rows:
        print(f"  {'CODE' if label is None else 'skip'}  {path}"
              + (f"   [{label}]" if label else ""), file=out)

    n_code = sum(1 for _, label in rows if label is None)
    print(f"\n{len(rows)} changed, {n_code} classified CODE.", file=out)
    print(f"-> code={'true' if is_code else 'false'}", file=out)
    emit(is_code, github_output)
    return 0


# --------------------------------------------------------------------------
# self-test
# --------------------------------------------------------------------------

# Extensions that must never be admitted by a directory-scoped pattern.
_HOSTILE = ["evil.rs", "evil.sh", "Cargo.toml", "evil.py", "evil.json", "Makefile"]


def _allow_prefixes() -> list[str]:
    """Literal leading directory of each pattern, e.g. 'artifacts', 'safety'."""
    out = set()
    for _, pattern in _ALLOW:
        m = re.match(r"^([A-Za-z0-9_.-]+)/", pattern)
        if m:
            out.add(m.group(1))
    return sorted(out)


def self_test() -> int:
    passed = failed = 0

    def ok(cond: bool, desc: str, detail: str = "") -> None:
        nonlocal passed, failed
        if cond:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}" + (f"\n         {detail}" if detail else ""))

    print("classify_changed_paths self-test")

    # -- pure classification -------------------------------------------------
    def verdict(paths):
        return classify(paths)[0]

    ok(verdict(["docs/architecture.md"]) is False, "a docs .md alone is not code")
    ok(verdict(["README.md"]) is False, "a root .md alone is not code")
    ok(verdict(["artifacts/requirements.yaml"]) is False, "artifact YAML is not code")
    ok(verdict(["safety/stpa/hazards.yaml"]) is False, "STPA YAML is not code")
    ok(verdict(["rivet.yaml"]) is False, "rivet.yaml is not code (#379)")

    # MECHANISM A. These are the exact paths from #384's executed repro.
    ok(verdict(["docs/nested/evil.rs"]) is True,
       "REGRESSION #384-A: a .rs under docs/ is CODE")
    ok(verdict(["safety/scripts/deploy.sh"]) is True,
       "REGRESSION #384-A: a .sh under safety/ is CODE")
    # The live instance of the same hole.
    ok(verdict(["docs/research/forge-replication/run_trial.sh"]) is True,
       "the real docs/research/*.sh in the tree is CODE")
    ok(verdict(["docs/research/forge-replication/descriptor.aadl"]) is True,
       "a .aadl under docs/ is CODE (spar parses .aadl)")
    ok(verdict(["artifacts/evil.rs"]) is True,
       "a .rs under artifacts/ is CODE (that entry was already type-scoped)")

    # Mixed diffs must run the suite — one code file poisons the whole diff.
    ok(verdict(["artifacts/requirements.yaml", "crates/spar-cli/src/main.rs"]) is True,
       "artifacts + code together is CODE")
    ok(verdict(["crates/spar-analysis/src/scheduling.rs"]) is True, "plain .rs is CODE")
    ok(verdict([".github/workflows/ci.yml"]) is True,
       "a workflow YAML is CODE (only artifacts/ and safety/ YAML are exempt)")

    # -- PROPERTY: no allow-list pattern admits code -------------------------
    # A future editor who re-broadens `safety/.*\.ya?ml` back to `safety/.*`
    # fails here without having to remember to add a case.
    bad = []
    for prefix in _allow_prefixes():
        for name in _HOSTILE:
            for path in (f"{prefix}/{name}", f"{prefix}/deep/nested/{name}"):
                label = classify_one(path)
                if label is not None:
                    bad.append(f"{path} admitted by [{label}]")
    ok(not bad,
       f"PROPERTY: no allow pattern admits code under {_allow_prefixes()}",
       "; ".join(bad[:4]))

    # -- emission honours its argument ---------------------------------------
    # Without this, an emit() that ignored `is_code` would pass every test
    # above: distinct inputs must produce distinct outputs.
    with tempfile.TemporaryDirectory() as td:
        f_true, f_false = Path(td) / "t", Path(td) / "f"
        emit(True, str(f_true))
        emit(False, str(f_false))
        ok(f_true.read_text().strip() == "code=true", "emit(True) writes code=true")
        ok(f_false.read_text().strip() == "code=false", "emit(False) writes code=false")

    # -- PROPERTY: no error path emits code=false ----------------------------
    # This is the requirement's own detector turned on this script: a guard
    # whose ERROR path yields its IDEAL reading. Every induced failure must
    # exit non-zero AND leave no verdict behind.
    def induce(desc: str, argv_fn, expect_written: str | None = None) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            gh = Path(td) / "gh_output"
            gh.write_text("")
            argv = argv_fn(str(gh))
            err = io.StringIO()
            real_err, sys.stderr = sys.stderr, err
            try:
                try:
                    rc = main(argv)
                except SystemExit as e:  # argparse
                    rc = e.code if isinstance(e.code, int) else 1
            finally:
                sys.stderr = real_err
            written = gh.read_text() if gh.exists() else ""
        if expect_written is None:
            good = rc != 0 and "code=false" not in written
            detail = f"rc={rc}, wrote {written!r}"
        else:
            good = rc == 0 and written.strip() == expect_written
            detail = f"rc={rc}, wrote {written!r}"
        if good:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}\n         {detail}")

    induce("ERROR: nonexistent base sha -> non-zero, no code=false",
           lambda gh: ["--event-name", "pull_request", "--base-sha", "0" * 40,
                       "--github-output", gh])
    induce("ERROR: empty base sha on a PR -> non-zero, no code=false",
           lambda gh: ["--event-name", "pull_request", "--base-sha", "",
                       "--github-output", gh])
    induce("ERROR: unwritable --github-output -> non-zero, no code=false",
           lambda _: ["--event-name", "push", "--github-output",
                      "/nonexistent-dir-84f2/out"])
    induce("ERROR: missing --github-output -> non-zero, no code=false",
           lambda _: ["--event-name", "push"])
    induce("ERROR: missing --event-name -> non-zero, no code=false",
           lambda gh: ["--github-output", gh])
    # ...and the success control, so the above is not passing because the tool
    # is simply always broken.
    induce("push event emits code=true and exits 0",
           lambda gh: ["--event-name", "push", "--github-output", gh],
           expect_written="code=true")

    # -- end-to-end over a real git diff --------------------------------------
    # Closes the seam between classify() and emit(): a run() that emitted a
    # constant would satisfy every test above.
    def e2e(desc: str, files: list[str], want: str) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            g = ["git", "-C", td, "-c", "user.name=t", "-c", "user.email=t@t",
                 # A throwaway fixture repo, not a spar commit: signing is
                 # switched OFF in this repo's own config rather than skipped on
                 # the commit, and identity is supplied so a runner with no
                 # global git config still works.
                 "-c", "commit.gpgsign=false",
                 # This self-test is step 2 of a required context that gates 11
                 # others. A machine with a global `core.hooksPath` would
                 # otherwise run foreign hooks inside the fixture repo and could
                 # stall CI for every PR in the repo.
                 "-c", "core.hooksPath=/dev/null"]
            try:
                subprocess.run(g + ["init", "-q", "-b", "main"], check=True,
                               capture_output=True)
                Path(td, "seed").write_text("seed")
                subprocess.run(g + ["add", "seed"], check=True, capture_output=True)
                subprocess.run(g + ["commit", "-qm", "seed"], check=True,
                               capture_output=True)
                base = subprocess.run(g + ["rev-parse", "HEAD"], check=True,
                                      capture_output=True, text=True).stdout.strip()
                for rel in files:
                    p = Path(td, rel)
                    p.parent.mkdir(parents=True, exist_ok=True)
                    p.write_text("x")
                    subprocess.run(g + ["add", rel], check=True, capture_output=True)
                subprocess.run(g + ["commit", "-qm", "change"], check=True,
                               capture_output=True)
            except (subprocess.CalledProcessError, OSError) as e:
                failed += 1
                print(f"  FAIL {desc}: fixture repo could not be built: {e}")
                return
            gh = Path(td, "gh_output")
            gh.write_text("")
            rc = run("pull_request", base, "HEAD", str(gh), cwd=td, out=io.StringIO())
            written = gh.read_text().strip()
        if rc == 0 and written == want:
            passed += 1
            print(f"  ok   {desc}")
        else:
            failed += 1
            print(f"  FAIL {desc}: rc={rc}, wrote {written!r}, want {want!r}")

    e2e("E2E: a docs-only diff really does emit code=false",
        ["docs/guide.md"], "code=false")
    e2e("E2E: a .rs diff emits code=true", ["crates/x/src/lib.rs"], "code=true")
    e2e("E2E: REGRESSION #384-A end-to-end — docs/*.rs emits code=true",
        ["docs/nested/evil.rs"], "code=true")

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--self-test" in argv:
        return self_test()

    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--event-name", required=True, help="github.event_name")
    ap.add_argument("--base-sha", default="",
                    help="github.event.pull_request.base.sha")
    ap.add_argument("--head", default="HEAD")
    # Required, and the only emission mode. See the module docstring: an
    # optional stdout mode is a mode a workflow will interpolate through a
    # shell, which is how #384 mechanism B comes back.
    ap.add_argument("--github-output", required=True,
                    help="path to $GITHUB_OUTPUT; this script appends to it")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args(argv)

    try:
        return run(a.event_name, a.base_sha, a.head, a.github_output)
    except Exception as exc:  # noqa: BLE001 — every failure must land here
        print(f"::error::changed-path classifier failed: {exc}", file=sys.stderr)
        print("::error::No verdict was emitted. This job is RED and it is a "
              "required context, so the merge is blocked rather than the suite "
              "silently skipped (#384).", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
