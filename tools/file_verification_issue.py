#!/usr/bin/env python3
"""File, or comment on, the one open issue that tracks a red weekly verification run.

REQ-VERIFY-GATE-SCOPE-001 (#358, #513). The per-PR Verification Gate checks a
slice of the stepped feature artifacts; `.github/workflows/verification-weekly.yml`
runs all of them once a week. A red weekly run is reported here as an issue,
because a scheduled run has no PR to turn red and nobody reads it otherwise.

One issue, not one per week: the open issue whose body carries MARKER gets a
comment; only when none is open is a new one created. Pure stdlib (urllib).

Fail-closed where `post_verification_comment.py` is not: a missing or unreadable
results file is reported as its own failure (the gate died before writing it),
never as "nothing to post", and any API error exits 2.

Usage:
    tools/file_verification_issue.py --run-url URL [--results-json PATH] [--repo OWNER/NAME]
    tools/file_verification_issue.py --self-test

Required env (not for --self-test): GH_TOKEN or GITHUB_TOKEN with `issues: write`.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

MARKER = "<!-- rivet-verification-weekly -->"
TITLE = "Verification Weekly: stepped feature artifacts are failing"
API = "https://api.github.com"


def github_request(method: str, path: str, token: str, body: dict | None = None):
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(
        f"{API}{path}",
        data=data,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "spar-verification-weekly",
            "Content-Type": "application/json" if data else "application/octet-stream",
        },
    )
    try:
        with urllib.request.urlopen(req) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def load_results(path: Path) -> dict | None:
    """The results dict, or None when the file is missing or not a results object."""
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict) or "failed" not in data:
        return None
    return data


def render_body(results: dict | None, run_url: str) -> str:
    if results is None:
        summary = ("**No results file.** The run failed before "
                   "`tools/run_verification.py` wrote `verification-results.json`, "
                   "so nothing below says which artifacts failed. Read the run log.")
        failed_section = "_(unknown)_"
    else:
        summary = (f"**{results.get('failed_count', len(results['failed']))}** failed, "
                   f"{results.get('passed_count', '?')} passed, "
                   f"{results.get('skipped_count', '?')} skipped (no steps), "
                   f"of {results.get('total', '?')} matched.")
        failed_section = ("\n".join(f"- `{i}`" for i in results["failed"])
                          or "_(none listed; the run failed on scope or setup, see the log)_")
    return (f"{MARKER}\nWeekly run: {run_url}\n\n{summary}\n\n"
            f"Failed artifacts:\n\n{failed_section}\n\n"
            f"<sub>Filed by `tools/file_verification_issue.py` from "
            f"`.github/workflows/verification-weekly.yml`.</sub>")


def pick_issue(items: list[dict]) -> int | None:
    """Number of the open ISSUE (not a PR) whose body carries the marker."""
    for it in items:
        if "pull_request" in it:
            continue
        if it.get("state", "open") != "open":
            continue
        if MARKER in (it.get("body") or ""):
            return it["number"]
    return None


def find_open_issue(repo: str, token: str) -> tuple[bool, int | None]:
    """(ok, number). ok is False on an API error, so the caller exits red."""
    page = 1
    while True:
        status, body = github_request(
            "GET", f"/repos/{repo}/issues?state=open&per_page=100&page={page}", token)
        if status != 200:
            print(f"GET issues failed: {status} {body[:200]!r}", file=sys.stderr)
            return False, None
        items = json.loads(body)
        if not isinstance(items, list):
            print(f"GET issues returned {type(items).__name__}, not a list", file=sys.stderr)
            return False, None
        found = pick_issue(items)
        if found is not None or len(items) < 100:
            return True, found
        page += 1


def self_test() -> int:
    passed = failed = 0

    def check(desc: str, ok: bool) -> None:
        nonlocal passed, failed
        passed, failed = (passed + 1, failed) if ok else (passed, failed + 1)
        print(f"  {'ok  ' if ok else 'FAIL'} {desc}")

    print("file_verification_issue self-test")
    m = {"number": 7, "state": "open", "body": f"x\n{MARKER}\ny"}
    check("the marked open issue is picked", pick_issue([m]) == 7)
    check("a PR carrying the marker is NOT picked",
          pick_issue([{**m, "number": 8, "pull_request": {}}]) is None)
    check("an issue without the marker is NOT picked",
          pick_issue([{"number": 9, "state": "open", "body": "unrelated"}]) is None)
    check("a closed marked issue is NOT picked", pick_issue([{**m, "state": "closed"}]) is None)
    check("a null body does not crash", pick_issue([{"number": 1, "body": None}]) is None)
    check("the issue is picked past a marked PR",
          pick_issue([{**m, "number": 8, "pull_request": {}}, m]) == 7)

    r = {"failed": ["TEST-A", "TEST-B"], "failed_count": 2, "passed_count": 5,
         "skipped_count": 1, "total": 8}
    b = render_body(r, "https://example/run/1")
    check("the body carries the marker (or next week files a duplicate)", MARKER in b)
    check("the body names every failed artifact", "`TEST-A`" in b and "`TEST-B`" in b)
    check("the body links the run", "https://example/run/1" in b)
    nb = render_body(None, "u")
    check("a missing results file is reported, not posted as 'none failed'",
          "No results file" in nb and MARKER in nb)

    tmp = Path(os.environ.get("TMPDIR", "/tmp")) / f"fvi-selftest-{os.getpid()}.json"
    try:
        tmp.write_text("[]")
        check("a results file that is not an object reads as missing", load_results(tmp) is None)
        tmp.write_text("{not json")
        check("an unparsable results file reads as missing", load_results(tmp) is None)
        tmp.write_text(json.dumps(r))
        check("a real results file loads", load_results(tmp) == r)
    finally:
        tmp.unlink(missing_ok=True)
    check("an absent results file reads as missing",
          load_results(Path("/nonexistent/verification-results.json")) is None)

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--results-json", type=Path, default=Path("verification-results.json"))
    ap.add_argument("--run-url", default="")
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "pulseengine/spar"))
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()

    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not token:
        print("GH_TOKEN or GITHUB_TOKEN required", file=sys.stderr)
        return 2
    body = render_body(load_results(args.results_json), args.run_url or "(no run URL given)")

    ok, number = find_open_issue(args.repo, token)
    if not ok:
        return 2
    if number is not None:
        status, resp = github_request(
            "POST", f"/repos/{args.repo}/issues/{number}/comments", token, {"body": body})
        what = f"commented on #{number}"
    else:
        status, resp = github_request(
            "POST", f"/repos/{args.repo}/issues", token, {"title": TITLE, "body": body})
        what = "created a new issue"
    if status not in (200, 201):
        print(f"issue write failed: {status} {resp[:300]!r}", file=sys.stderr)
        return 2
    print(what)
    return 0


if __name__ == "__main__":
    sys.exit(main())
