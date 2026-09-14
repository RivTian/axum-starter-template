#!/usr/bin/env python3
"""Real stdout PTY/pipe/file checks, without mutating the parent environment."""
from __future__ import annotations

import argparse
import errno
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

from check_service import build_binary, configuration, has_field, require


SGR = re.compile(rb"\x1b\[[0-9;]*m")


def drain(fd: int, sink: str, output: bytearray) -> None:
    while True:
        try:
            chunk = os.read(fd, 65536)
        except BlockingIOError:
            return
        except OSError as error:
            # Linux PTY masters report EIO when the final slave closes; macOS
            # can report EOF. Do not hide any other read failure.
            if sink == "tty" and error.errno == errno.EIO:
                return
            raise
        if not chunk:
            return
        output.extend(chunk)


def capture(binary: Path, directory: Path, sink: str, no_color: str | None,
            term: str) -> bytes:
    directory.mkdir()
    config = directory / "settings/service.toml"
    config.parent.mkdir()
    config.write_text(configuration())
    env = os.environ.copy()
    env.pop("NO_COLOR", None)
    if no_color is not None:
        env["NO_COLOR"] = no_color
    env.update({"TERM": term, "RUST_LOG": "info"})
    if sink == "tty":
        read_fd, write_fd = os.openpty()
    elif sink == "pipe":
        read_fd, write_fd = os.pipe()
    else:
        log = directory / "stdout.log"
        write_fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        # Separate opens: duplicating a regular-file fd shares its seek offset.
        try:
            read_fd = os.open(log, os.O_RDONLY)
        except BaseException:
            os.close(write_fd)
            raise
    with os.fdopen(read_fd, "rb", buffering=0) as reader, os.fdopen(write_fd, "wb", buffering=0) as writer:
        os.set_blocking(reader.fileno(), False)
        child = subprocess.Popen(
            [str(binary), "--config", str(config)], cwd=directory, env=env,
            stdin=subprocess.DEVNULL, stdout=writer, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        writer.close()  # Only the child may keep the pipe/PTY writer alive.
        output = bytearray()
        stopping = False
        deadline = time.monotonic() + 15
        try:
            while True:
                drain(reader.fileno(), sink, output)
                if child.poll() is not None:
                    # Include all final logs even if exit raced the previous read.
                    drain(reader.fileno(), sink, output)
                    break
                if not stopping and has_field(SGR.sub(b"", output).decode(), "event", "service_started"):
                    child.send_signal(signal.SIGTERM)
                    stopping = True
                    deadline = time.monotonic() + 15
                require(time.monotonic() < deadline, f"{sink} logging check timed out\n{output!r}")
                time.sleep(0.01)
            require(stopping and child.returncode == 0,
                    f"{sink} service did not start and shut down normally: {child.returncode}\n{output!r}")
            return bytes(output)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=5)


def verify(binary: Path) -> list[str]:
    cases = []
    with tempfile.TemporaryDirectory(prefix="service-logging-") as temporary:
        root = Path(temporary)
        for name, sink, no_color, term, color in (
            ("tty-auto-color", "tty", None, "xterm-256color", True),
            ("tty-empty-no-color", "tty", "", "xterm-256color", True),
            ("tty-no-color", "tty", "1", "xterm-256color", False),
            ("tty-dumb", "tty", None, "dumb", False),
            ("pipe-plain", "pipe", None, "xterm-256color", False),
            ("file-plain", "file", None, "xterm-256color", False),
        ):
            raw = capture(binary, root / name, sink, no_color, term)
            require(bool(SGR.search(raw)) == color, f"{name}: incorrect ANSI policy\n{raw!r}")
            text = SGR.sub(b"", raw).decode()
            require("\x1b" not in text, f"{name}: unexpected non-style escape\n{text!r}")
            # A cosmetic change must not introduce banners/multiline pretty logs
            # or discard the fields consumed by diagnostics and process checks.
            lines = text.splitlines()
            require(all(re.match(r"^\S+\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s", line) for line in lines),
                    f"{name}: not compact single-line logs\n{text}")
            for event in ("runtime_topology", "task_polled", "service_started", "shutdown_started", "shutdown_complete"):
                require(any(has_field(line, "event", event) for line in lines), f"{name}: missing {event}\n{text}")
            shutdowns = [line for line in lines if has_field(line, "event", "shutdown_complete")]
            require(len(shutdowns) == 1 and all(has_field(shutdowns[0], key, value) for key, value in (
                ("cause", "signal"), ("forced", "false"), ("storage", "closed"),
                ("unreaped_tasks", "0"), ("cleanup_failed", "false"), ("exit_code", "0"),
            )), f"{name}: shutdown lost cleanup evidence\n{text}")
            cases.append(name)
    return cases


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=["debug", "release"], default="debug")
    args = parser.parse_args()
    require(os.name == "posix", "PTY/signal logging checks currently require POSIX; this platform is not silently skipped")
    root = Path(__file__).resolve().parents[2]
    binary, _ = build_binary(root, args.profile)
    cases = verify(binary)
    print(json.dumps({"suite": "terminal-logging", "profile": args.profile, "passed": cases, "result": "passed"}))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"logging check failed: {error}", file=sys.stderr)
        sys.exit(1)
