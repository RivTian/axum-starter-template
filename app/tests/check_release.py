#!/usr/bin/env python3
"""Build and execute an actual release artifact; never substitute the test profile."""
from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import subprocess
import sys


def run(command: list[str], cwd: Path, timeout: int) -> subprocess.CompletedProcess[str]:
    child = subprocess.Popen(
        command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, start_new_session=os.name == "posix",
    )
    try:
        stdout, stderr = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name == "posix":
            os.killpg(child.pid, signal.SIGKILL)
        else:
            child.kill()
        stdout, stderr = child.communicate()
        raise RuntimeError(f"command exceeded {timeout}s: {command}\n{stdout}\n{stderr}") from None
    return subprocess.CompletedProcess(command, child.returncode, stdout, stderr)


def main() -> None:
    root = Path(__file__).resolve().parents[2]
    cargo = os.environ.get("CARGO", "cargo")
    build = run([
        cargo, "build", "--locked", "--release", "--example", "m1-release-panic",
        "--message-format=json-render-diagnostics",
    ], root, 900)
    sys.stderr.write(build.stderr)
    if build.returncode:
        sys.stdout.write(build.stdout)
        raise RuntimeError("release fixture build failed")
    executables = []
    for line in build.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message["target"]["name"] == "m1-release-panic"
                and message.get("executable")):
            executables.append(message["executable"])
    if len(executables) != 1:
        raise RuntimeError(f"expected one release executable, got {executables!r}")
    outcome = run([executables[0]], root, 15)
    expected = ["M1:panic-observed", "M1:worker-joined", "M1:runtime-teardown-returned"]
    if outcome.returncode != 1 or outcome.stdout.splitlines() != expected:
        raise RuntimeError(f"incorrect release outcome: {outcome.returncode}\n{outcome.stdout}\n{outcome.stderr}")
    if "M1 controlled release task panic" not in outcome.stderr:
        raise RuntimeError("expected panic hook output was not captured")
    print("M1 release panic: observed, sibling joined, runtimes torn down, exit=1")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError) as error:
        print(f"release check failed: {error}", file=sys.stderr)
        sys.exit(1)
