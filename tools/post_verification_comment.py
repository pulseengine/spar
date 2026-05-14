#!/usr/bin/env python3
"""Post (or update) a sticky PR comment summarising rivet verification results.

Reads the JSON written by `tools/run_verification.py` and uses the `gh` CLI to
upsert a single marker-tagged comment on the PR. Re-running on the same PR
replaces the prior body rather than appending another comment.

Usage:
    tools/post_verification_comment.py <pr-number> [--results-json PATH] [--repo OWNER/NAME]

Required:
    GH_TOKEN (or GITHUB_TOKEN) with `pull-requests: write`.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

MARKER = "<!-- rivet-verification-gate -->"


def render_body(results: dict) -> str:
    passed = results["passed_count"]
    failed = results["failed_count"]
    skipped = results["skipped_count"]
    total = results["total"]
    failed_ids = results["failed"]
    flt = results["filter"]

    if failed == 0:
        status = f"✅ **{passed}/{total}** passed"
    else:
        status = f"❌ **{passed}/{total}** passed — **{failed}** failed"

    failed_section = (
        "\n".join(f"- `{i}`" for i in failed_ids) if failed_ids else "_(none)_"
    )

    return f"""{MARKER}
## Rivet verification gate

{status}

| | count |
|---|---:|
| Passed  | {passed} |
| Failed  | {failed} |
| Skipped (no steps) | {skipped} |

**Filter:** `{flt}`

<details><summary>Failed artifacts</summary>

{failed_section}

</details>

<sub>Updated automatically by `tools/post_verification_comment.py`. Source of truth: `artifacts/verification.yaml`.</sub>"""


def find_marker_comment(repo: str, pr: int) -> int | None:
    proc = subprocess.run(
        [
            "gh",
            "api",
            f"repos/{repo}/issues/{pr}/comments",
            "--paginate",
            "--jq",
            f'.[] | select(.body | contains("{MARKER}")) | .id',
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    out = proc.stdout.strip()
    if not out:
        return None
    return int(out.splitlines()[0])


def upsert_comment(repo: str, pr: int, body: str) -> None:
    existing = find_marker_comment(repo, pr)
    if existing is not None:
        print(f"updating comment {existing}", file=sys.stderr)
        subprocess.run(
            [
                "gh",
                "api",
                "-X",
                "PATCH",
                f"repos/{repo}/issues/comments/{existing}",
                "-f",
                f"body={body}",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    else:
        print("creating new comment", file=sys.stderr)
        subprocess.run(
            [
                "gh",
                "api",
                f"repos/{repo}/issues/{pr}/comments",
                "-f",
                f"body={body}",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pr", type=int, help="pull-request number")
    parser.add_argument(
        "--results-json",
        default="verification-results.json",
        type=Path,
        help="path to the JSON summary (default: %(default)s)",
    )
    parser.add_argument(
        "--repo",
        default=os.environ.get("GH_REPO", "pulseengine/spar"),
        help="OWNER/NAME (default: %(default)s)",
    )
    args = parser.parse_args()

    if not args.results_json.is_file():
        print(f"no {args.results_json} found; nothing to post", file=sys.stderr)
        return 0

    results = json.loads(args.results_json.read_text())
    body = render_body(results)
    upsert_comment(args.repo, args.pr, body)
    return 0


if __name__ == "__main__":
    sys.exit(main())
