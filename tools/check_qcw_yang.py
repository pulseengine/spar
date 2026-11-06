#!/usr/bin/env python3
"""Validate spar's 802.1Qcw export against the published YANG modules.

REQ-TSN-YANG-DEPLOYABLE-001. Refs #475, #476.

`GateSchedule::to_qcw_yang` emitted, up to v0.48.2, a document no switch could
apply: the gate table at the wrong path and in a namespace that defines no data
nodes, a `uint32` rational with a 1e12 denominator, a bare identityref, no
mandatory interface `type`, and nothing to activate. Every one of those is a
schema fact, so the oracle is the schema, run on spar's REAL output.

What this does:

1. Reads `tools/yang/modules.txt`: a YangModels/yang commit, and per module its
   sha256 and path. The modules are fetched at that commit and checked against
   the sums, never vendored (YangModels' standard/ieee/README grants free
   reproduction for experimental/ieee/ only).
2. Runs `yanglint -t edit` (NETCONF edit-config content, which is what the
   export is: a full datastore would demand device state such as
   supported-list-max) on every document in `--docs`. Each must be accepted.
3. NEGATIVE CONTROL: the first document with its identity prefix removed
   (`sched:set-gate-states` -> `set-gate-states`) must be REJECTED. A
   validator that accepts that accepts anything, and its green is no evidence.

Exit 0 only when every document is accepted AND the control is rejected.
Exit 1 when a document is rejected. Exit 2 when the measurement cannot be
trusted: no documents, a checksum mismatch, a fetch failure, no yanglint, or a
control that passed.

NOT CLAIMED: that a switch accepts the output. The schema does not require
`admin-base-time` or `config-change`, so spar's own tests assert those.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

RAW = "https://raw.githubusercontent.com/YangModels/yang/{commit}/{path}"
GOOD_IDENTITY = "sched:set-gate-states"
BAD_IDENTITY = "set-gate-states"


class Untrusted(Exception):
    """The measurement cannot be trusted; exit 2."""


def parse_modules(text: str) -> tuple[str, list[tuple[str, str]]]:
    """(commit, [(sha256, path)]) from modules.txt."""
    commit = ""
    mods: list[tuple[str, str]] = []
    for n, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if parts[0] == "commit" and len(parts) == 2:
            commit = parts[1]
        elif len(parts) == 2 and len(parts[0]) == 64:
            mods.append((parts[0].lower(), parts[1]))
        else:
            raise Untrusted(f"modules.txt:{n}: not `commit <sha>` or `<sha256> <path>`: {line!r}")
    if len(commit) != 40:
        raise Untrusted("modules.txt names no 40-hex YangModels commit")
    if not mods:
        raise Untrusted("modules.txt lists no modules")
    return commit, mods


def module_file(path: str) -> str:
    """`standard/iana/iana-if-type@2026-10-23.yang` -> `iana-if-type.yang`."""
    return Path(path).name.split("@", 1)[0].removesuffix(".yang") + ".yang"


def verify(data: bytes, want: str, path: str) -> None:
    got = hashlib.sha256(data).hexdigest()
    if got != want:
        raise Untrusted(f"{path}: sha256 {got} != pinned {want}")


def fetch(commit: str, mods: list[tuple[str, str]], into: Path) -> list[Path]:
    files = []
    for sha, path in mods:
        url = RAW.format(commit=commit, path=path)
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                data = r.read()
        except OSError as e:
            raise Untrusted(f"fetch {url}: {e}") from e
        verify(data, sha, path)
        f = into / module_file(path)
        f.write_bytes(data)
        files.append(f)
    return files


def decide(doc_results: dict[str, int], control_rc: int | None) -> tuple[int, str]:
    """Exit code and reason from yanglint's exit codes."""
    if not doc_results:
        return 2, "no documents to validate: an empty run is not a valid export"
    if control_rc is None:
        return 2, "the negative control did not run"
    if control_rc == 0:
        return 2, ("the negative control (a bare identityref) was ACCEPTED: this "
                   "validator does not discriminate, so its acceptance is no evidence")
    bad = sorted(d for d, rc in doc_results.items() if rc != 0)
    if bad:
        return 1, f"rejected by the schema: {', '.join(bad)}"
    return 0, f"{len(doc_results)} document(s) accepted; the negative control was rejected"


def yanglint(yl: str, moddir: Path, modules: list[Path], doc: Path) -> tuple[int, str]:
    p = subprocess.run(
        [yl, "-p", str(moddir), "-t", "edit", *map(str, modules), str(doc)],
        capture_output=True, text=True,
    )
    return p.returncode, (p.stdout + p.stderr).strip()


def run(modules_txt: Path, docs: list[Path], yl: str) -> int:
    print("== qcw yang schema validation ==")
    try:
        if not shutil.which(yl):
            raise Untrusted(f"`{yl}` not found; install libyang's yanglint")
        ver = subprocess.run([yl, "--version"], capture_output=True, text=True).stdout.strip()
        print(f"validator: {ver}")
        if not docs:
            raise Untrusted("no documents given")
        commit, mods = parse_modules(modules_txt.read_text(encoding="utf-8"))
        print(f"modules:   {len(mods)} at YangModels/yang@{commit[:12]}, sha256-checked")
        with tempfile.TemporaryDirectory() as td:
            moddir = Path(td)
            files = fetch(commit, mods, moddir)
            results: dict[str, int] = {}
            for doc in docs:
                rc, out = yanglint(yl, moddir, files, doc)
                results[str(doc)] = rc
                print(f"  {'ok  ' if rc == 0 else 'FAIL'} {doc}")
                if rc:
                    print("       " + out.replace("\n", "\n       "))
            text = docs[0].read_text(encoding="utf-8")
            if GOOD_IDENTITY not in text:
                raise Untrusted(f"{docs[0]} has no `{GOOD_IDENTITY}` to strip for the control")
            ctl = moddir / "control.xml"
            ctl.write_text(text.replace(GOOD_IDENTITY, BAD_IDENTITY), encoding="utf-8")
            crc, cout = yanglint(yl, moddir, files, ctl)
            print(f"  control (bare identityref): {'REJECTED' if crc else 'ACCEPTED'}"
                  + (f" — {cout.splitlines()[0]}" if crc and cout else ""))
    except Untrusted as e:
        print(f"::error::{e}")
        return 2
    code, why = decide(results, crc)
    print(("::error::" if code else "") + why)
    return code


def self_test() -> int:
    passed = failed = 0

    def check(desc: str, ok: bool) -> None:
        nonlocal passed, failed
        passed, failed = (passed + 1, failed) if ok else (passed, failed + 1)
        print(f"  {'ok  ' if ok else 'FAIL'} {desc}")

    print("check_qcw_yang self-test")
    check("all accepted + control rejected -> 0", decide({"a": 0, "b": 0}, 1)[0] == 0)
    check("a rejected document -> 1", decide({"a": 0, "b": 1}, 1)[0] == 1)
    check("rejected document is named", "b" in decide({"a": 0, "b": 1}, 1)[1])
    # The vacuity guard: a validator that accepts the control proves nothing.
    check("control ACCEPTED -> 2 even when every doc passes", decide({"a": 0}, 0)[0] == 2)
    check("control ACCEPTED outranks a rejected doc", decide({"a": 1}, 0)[0] == 2)
    check("no documents -> 2", decide({}, 1)[0] == 2)
    check("control not run -> 2", decide({"a": 0}, None)[0] == 2)

    sha = "a" * 64
    good = f"# c\ncommit {'b' * 40}\n{sha} standard/iana/iana-if-type@2026-10-23.yang\n"
    commit, mods = parse_modules(good)
    check("modules.txt parses commit and entries", commit == "b" * 40 and mods == [
        (sha, "standard/iana/iana-if-type@2026-10-23.yang")])
    for desc, text in [
        ("no commit line is untrusted", f"{sha} x.yang\n"),
        ("no modules is untrusted", f"commit {'b' * 40}\n"),
        ("a malformed line is untrusted", f"commit {'b' * 40}\nnot a line\n"),
    ]:
        try:
            parse_modules(text)
            check(desc, False)
        except Untrusted:
            check(desc, True)
    check("revisioned path saves under the module name",
          module_file("standard/iana/iana-if-type@2026-10-23.yang") == "iana-if-type.yang")
    check("plain path keeps its name",
          module_file("standard/ietf/RFC/ietf-interfaces.yang") == "ietf-interfaces.yang")
    try:
        verify(b"x", hashlib.sha256(b"y").hexdigest(), "p")
        check("a checksum mismatch is untrusted", False)
    except Untrusted:
        check("a checksum mismatch is untrusted", True)
    verify(b"x", hashlib.sha256(b"x").hexdigest(), "p")
    check("a matching checksum passes", True)

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--modules", default="tools/yang/modules.txt", type=Path)
    ap.add_argument("--yanglint", default="yanglint")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("docs", nargs="*", type=Path)
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return run(a.modules, a.docs, a.yanglint)


if __name__ == "__main__":
    sys.exit(main())
