#!/usr/bin/env python3
"""Fail early, under a step that names podman, when rootless podman cannot start.

REQ-CI-FIXTURE-PODMAN-PREFLIGHT-001. Refs #425.

The Trace-Topology Fixture workflow failed for 13 nights because the runner's
/run/user/<uid> was read-only. Rootless podman needs a writable
XDG_RUNTIME_DIR, so its first call died. That call was inside the step named
'Require /dev/kvm openable by a Nix BUILD process', so the run summary blamed
/dev/kvm, and the KVM advice that step prints never ran (it lives inside the
container). This check runs before it and asks the two podman questions
directly:

1. Is XDG_RUNTIME_DIR (default /run/user/<uid>) a directory this user can
   write? Tested by CREATING a file in it, not by os.access: a read-only
   mount is the failure this exists for, and the real operation is the only
   reading that cannot disagree with podman's.
2. Does `podman info` succeed? It is the cheapest call that starts the same
   rootless machinery the KVM probe's `podman run` needs.

Exit 0 when both hold, 1 when either does not (with the reason), 2 on a usage
error. NOT CLAIMED: that the KVM probe will then pass; that is its own step.
"""

from __future__ import annotations

import argparse
import io
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


def runtime_dir(env: dict[str, str]) -> Path:
    return Path(env.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}")


def check(env: dict[str, str], podman: str = "podman", out=sys.stdout) -> int:
    print("== podman preflight ==", file=out)
    d = runtime_dir(env)
    print(f"XDG_RUNTIME_DIR: {d}", file=out)
    if not d.is_dir():
        print(f"::error::XDG_RUNTIME_DIR {d} does not exist, so rootless podman cannot "
              f"start. This is a podman/runner problem, not a /dev/kvm one (#425).", file=out)
        return 1
    try:
        with tempfile.NamedTemporaryFile(dir=d, prefix=".podman-preflight-"):
            pass
    except OSError as e:
        print(f"::error::XDG_RUNTIME_DIR {d} is not writable ({e.strerror}), so rootless "
              f"podman cannot start. A read-only /run/user/<uid> caused the 13-night "
              f"failure in #425; this is not a /dev/kvm problem.", file=out)
        return 1
    try:
        p = subprocess.run([podman, "info", "--format", "{{.Host.OCIRuntime.Name}}"],
                           capture_output=True, text=True, env=env, timeout=120)
    except FileNotFoundError:
        print(f"::error::`{podman}` is not installed on this runner.", file=out)
        return 1
    except subprocess.TimeoutExpired:
        print("::error::`podman info` did not return within 120 s.", file=out)
        return 1
    if p.returncode != 0:
        print(f"::error::`podman info` failed (exit {p.returncode}), so podman cannot start "
              f"here. Its output follows; this is not a /dev/kvm problem.", file=out)
        print((p.stdout + p.stderr).strip(), file=out)
        return 1
    print(f"podman starts; OCI runtime: {p.stdout.strip() or '(unreported)'}", file=out)
    return 0


def self_test() -> int:
    passed = failed = 0

    def case(desc: str, want: int, env_dir: str | None, podman_body: str | None,
             want_msg: str | None = None, readonly: bool = False) -> None:
        nonlocal passed, failed
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            env = {"PATH": os.environ.get("PATH", "")}
            if env_dir is not None:
                d = root / env_dir
                if env_dir != "missing":
                    d.mkdir()
                    if readonly:
                        d.chmod(stat.S_IRUSR | stat.S_IXUSR)
                env["XDG_RUNTIME_DIR"] = str(d)
            podman = str(root / "nope")
            if podman_body is not None:
                fake = root / "podman"
                fake.write_text("#!/bin/sh\n" + podman_body + "\n")
                fake.chmod(0o755)
                podman = str(fake)
            buf = io.StringIO()
            got = check(env, podman, out=buf)
            if readonly:
                (root / env_dir).chmod(0o755)
        text = buf.getvalue()
        ok = got == want and (want_msg is None or want_msg in text)
        passed, failed = (passed + 1, failed) if ok else (passed, failed + 1)
        print(f"  {'ok  ' if ok else 'FAIL'} {desc}" + ("" if ok else f": exit {got}\n{text}"))

    print("check_podman_preflight self-test")
    case("writable dir + podman info ok -> 0", 0, "run", "echo crun")
    case("the OCI runtime is reported", 0, "run", "echo crun", want_msg="OCI runtime: crun")
    if os.geteuid() == 0:
        # root writes through a 0500 directory, so the #425 case cannot be built here.
        print("  skip a READ-ONLY XDG_RUNTIME_DIR fails (running as root)")
    else:
        case("REGRESSION #425: a READ-ONLY XDG_RUNTIME_DIR fails, naming podman", 1,
             "run", "echo crun", want_msg="not a /dev/kvm problem", readonly=True)
    case("a missing XDG_RUNTIME_DIR fails", 1, "missing", "echo crun", want_msg="does not exist")
    case("podman info failing fails, with its output", 1, "run",
         "echo 'cannot set up namespace' >&2; exit 125", want_msg="cannot set up namespace")
    case("podman not installed fails", 1, "run", None, want_msg="not installed")
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--podman", default="podman")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    return check(dict(os.environ), a.podman)


if __name__ == "__main__":
    sys.exit(main())
