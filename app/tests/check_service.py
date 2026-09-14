#!/usr/bin/env python3
"""Real-process checks. No fixed ports, ambient proxies or global env mutations."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import queue
import re
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
if sys.version_info < (3, 11):
    raise SystemExit("Python 3.11+ required; select it with make PYTHON=/path/to/python3")
import tomllib
import urllib.error
import urllib.request

from check_release import run


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def has_field(text: str, name: str, value: str) -> bool:
    # Compact tracing quotes string values but not numeric/display fields.
    return re.search(r"\b" + re.escape(name) + r'=(?:"' + re.escape(value) + r'"|' + re.escape(value) + r')(?:\s|$)', text) is not None


TOPOLOGIES = {
    "main": {"ticker": "main", "http": "main"},
    "worker": {"ticker": "compute", "http": "main"},
    "http": {"ticker": "main", "http": "network"},
    "shared": {"ticker": "isolated", "http": "isolated"},
    "split": {"ticker": "compute", "http": "network"},
}


def configuration(listen: str = "127.0.0.1:0", topology: str = "main") -> str:
    bindings = TOPOLOGIES[topology]
    http_binding = "" if bindings["http"] == "main" else f'runtime = "{bindings["http"]}"\n'
    ticker_binding = "" if bindings["ticker"] == "main" else f'runtime = "{bindings["ticker"]}"\n'
    body = f'''[http]
listen = {json.dumps(listen)}
{http_binding}request_timeout_ms = 300
ready_probe_timeout_ms = 100
[storage]
path = "../data/service.sqlite3"
max_connections = 2
acquire_timeout_ms = 2000
busy_timeout_ms = 1000
[runtime]
worker_threads = 2
max_blocking_threads = 4
[ticker]
{ticker_binding}interval_ms = 1000
[lifecycle]
startup_timeout_ms = 5000
grace_ms = 1000
abort_reap_ms = 250
storage_close_ms = 3000
runtime_shutdown_ms = 1000
'''
    for name in sorted(set(bindings.values()) - {"main"}):
        body += f"[runtime.extra.{name}]\nworker_threads = 1\nmax_blocking_threads = 2\n"
    return body


class Service:
    def __init__(self, binary: Path, directory: Path, *, env_only: bool = False,
                 body: str | None = None, topology: str = "main"):
        self.topology = topology
        self.directory = directory
        self.config = directory / "settings/service.toml"
        self.config.parent.mkdir(parents=True, exist_ok=True)
        self.config.write_text(body if body is not None else configuration(topology=topology))
        cwd = directory / "elsewhere/deep"
        cwd.mkdir(parents=True, exist_ok=True)
        env = os.environ.copy()
        env["RUST_LOG"] = "info"
        config_key = binary.name.upper() + "_CONFIG"
        env[config_key] = str(self.config) if env_only else str(directory / "must-not-be-read.toml")
        args = [str(binary)] if env_only else [str(binary), "--config", str(self.config)]
        # The bad env path proves explicit CLI selection wins over environment.
        self.process = subprocess.Popen(args, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, text=True, bufsize=1,
                                        start_new_session=True)
        self.lines: list[str] = []
        self.events: queue.Queue[str | None] = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.cursor = 0
        self.address: str | None = None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.lines.append(line)
            self.events.put(line)
        self.events.put(None)

    def wait_line(self, *parts: str, timeout: float = 15, after: int | None = None) -> str:
        # Publication and consumption logs may be emitted on different threads.
        # Keep history and allow an explicit checkpoint, rather than losing an
        # early consumer event while waiting for the publisher's log line.
        start = self.cursor if after is None else after
        deadline = time.monotonic() + timeout
        while True:
            for index, line in enumerate(self.lines[start:], start):
                if all(part in line for part in parts):
                    self.cursor = max(self.cursor, index + 1)
                    return line
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(f"waiting for {parts} timed out\n{''.join(self.lines)}")
            try:
                line = self.events.get(timeout=remaining)
            except queue.Empty:
                raise RuntimeError(f"no {parts} event\n{''.join(self.lines)}") from None
            if line is None:
                raise RuntimeError(f"process exited before {parts}\n{''.join(self.lines)}")

    def replace_config(self, body: str) -> None:
        temporary = self.config.with_suffix(".next")
        temporary.write_text(body)
        os.replace(temporary, self.config)

    def reload(self, body: str | None, outcome: str, generation: int) -> int:
        checkpoint = len(self.lines)
        if body is not None:
            self.replace_config(body)
        self.process.send_signal(signal.SIGHUP)
        line = self.wait_line("config_reload", f'result="{outcome}"', after=checkpoint)
        require(has_field(line, "generation", str(generation)), f"unexpected reload generation: {line}")
        return checkpoint

    def started(self) -> None:
        line = self.wait_line("service_started")
        match = re.search(r"listen_addr=(127\.0\.0\.1:\d+)", line)
        require(match is not None, f"missing actual listen address: {line}")
        self.address = match.group(1)
        require(self.directory.joinpath("data/service.sqlite3").is_file(), "DB path was not relative to the configuration directory")
        bindings = TOPOLOGIES[self.topology]
        for task, runtime in bindings.items():
            require(any("task_polled" in line and has_field(line, "task", task)
                        and has_field(line, "thread", f"service-{runtime}") for line in self.lines),
                    f"{task} did not run on {runtime}\n{''.join(self.lines)}")
        count = 1 + len(set(bindings.values()) - {"main"})
        require(any("runtime_topology" in line and has_field(line, "runtime_count", str(count)) for line in self.lines),
                "runtime count does not match configured topology")

    def request(self, path: str, method: str = "GET", headers: dict[str, str] | None = None) -> tuple[int, dict, bytes]:
        require(self.address is not None, "service has not confirmed startup")
        request = urllib.request.Request(f"http://{self.address}{path}", method=method, headers=headers or {})
        try:
            response = self.opener.open(request, timeout=5)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, dict(response.headers), response.read()

    def wait_exit(self, expected: int, timeout: float = 15) -> str:
        try:
            code = self.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.kill()
            raise RuntimeError(f"service exceeded exit deadline\n{''.join(self.lines)}") from None
        self.reader.join(timeout=2)
        require(not self.reader.is_alive(), "output reader outlived process exit")
        text = "".join(self.lines)
        require(code == expected, f"expected exit {expected}, got {code}\n{text}")
        require("\x1b" not in text, f"ANSI escapes leaked into piped logs\n{text!r}")
        return text

    def stop(self, sig: int = signal.SIGTERM) -> str:
        require(self.process.poll() is None, f"service exited unexpectedly\n{''.join(self.lines)}")
        self.process.send_signal(sig)
        text = self.wait_exit(0)
        require("shutdown_complete" in text and has_field(text, "storage", "closed") and "unreaped_tasks=0" in text,
                f"normal shutdown lacked cleanup evidence\n{text}")
        expected = sorted(set(TOPOLOGIES[self.topology].values()) - {"main"}, reverse=True) + ["main"]
        shutdowns = [line for line in self.lines if has_field(line, "event", "runtime_shutdown_returned")]
        require(len(shutdowns) == len(expected), f"runtime shutdown count differs: {shutdowns}")
        for line, name in zip(shutdowns, expected, strict=True):
            require(has_field(line, "runtime", name), f"incorrect runtime shutdown order: {line}")
        storage_index = next(i for i, line in enumerate(self.lines) if has_field(line, "event", "storage_close_result"))
        runtime_index = next(i for i, line in enumerate(self.lines) if has_field(line, "event", "runtime_shutdown_returned"))
        require(storage_index < runtime_index, "runtime teardown preceded storage close receipt")
        return text

    def kill(self) -> None:
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait(timeout=5)
        self.reader.join(timeout=2)
        if self.process.stdout is not None:
            self.process.stdout.close()

    def __enter__(self) -> Service:
        return self

    def __exit__(self, *_: object) -> None:
        self.kill()


def build_binary(root: Path, profile: str) -> tuple[Path, str]:
    app = tomllib.loads((root / "app/Cargo.toml").read_text())
    name = app["bin"][0]["name"]
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    args = [os.environ.get("CARGO", "cargo"), "build", "--locked", "--bin", name, "--message-format=json-render-diagnostics"]
    if profile == "release":
        args.append("--release")
    outcome = run(args, root, 900)
    sys.stderr.write(outcome.stderr)
    require(outcome.returncode == 0, "application build failed\n" + outcome.stdout)
    artifacts = [message["executable"] for line in outcome.stdout.splitlines()
                 if (message := json.loads(line)).get("reason") == "compiler-artifact"
                 and message.get("executable") and message["target"]["name"] == name]
    require(len(artifacts) == 1, "could not identify the application executable")
    return Path(artifacts[0]), version


def verify(binary: Path, version: str, topology: str) -> list[str]:
    cases = []
    def launch(directory: Path, **kwargs: object) -> Service:
        return Service(binary, directory, topology=topology, **kwargs)
    with tempfile.TemporaryDirectory(prefix="service-m4-") as temporary:
        root = Path(temporary)
        with launch(root / "normal") as service:
            service.started()
            for endpoint in ("health", "ready"):
                code, headers, data = service.request(f"/v1/service/{endpoint}")
                require(code == 200 and json.loads(data) == {"status": "success", "code": 200, "description": ""}, endpoint)
                require(headers.get("content-type", headers.get("Content-Type", "")).startswith("application/json"), "not JSON")
            code, _, data = service.request("/v1/service/info?token=PROCESS_SECRET", headers={"authorization": "Bearer PROCESS_HEADER_SECRET"})
            require(code == 200 and json.loads(data) == {"service": binary.name, "version": version}, "info identity")
            for path in ("/", "/v1", "/v1/", "/v1/unknown", "/elsewhere"):
                code, _, data = service.request(path)
                require(code == 404 and json.loads(data)["code"] == 404, "fallback envelope")
            code, headers, data = service.request("/v1/service/health", "POST")
            require(code == 405 and json.loads(data)["code"] == 405, "method envelope")
            require("GET" in headers.get("allow", headers.get("Allow", "")), "missing Allow header")
            code, _, data = service.request("/v1/service/health", "HEAD")
            require(code == 200 and not data, "HEAD body contract")
            service.reload(None, "no_change", 1)
            invalid = "this must fail configuration loading"
            service.reload(invalid, "invalid", 1)
            require(service.config.read_text() == invalid, "reload rewrote the operator's file")
            hot = configuration(topology=topology).replace("interval_ms = 1000", "interval_ms = 100")
            checkpoint = service.reload(hot, "published", 2)
            service.wait_line("ticker_config_applied", "config_generation=2", "interval_ms=100", after=checkpoint)
            service.wait_line("ticker tick", "config_generation=2", "interval_ms=100", after=checkpoint)
            service.reload(None, "no_change", 2)
            mixed = configuration(topology=topology).replace("worker_threads = 2", "worker_threads = 3").replace("interval_ms = 1000", "interval_ms = 50")
            checkpoint = service.reload(mixed, "requires_restart", 2)
            require(service.config.read_text() == mixed, "cold rejection rewrote disk state")
            service.wait_line("ticker tick", "config_generation=2", "interval_ms=100", after=checkpoint)
            service.reload(hot.replace("request_timeout_ms = 300", "request_timeout_ms = 400"), "requires_restart", 2)
            updated = configuration(topology=topology).replace("interval_ms = 1000", "interval_ms = 200")
            checkpoint = service.reload(updated, "published", 3)
            service.wait_line("ticker_config_applied", "config_generation=3", "interval_ms=200", after=checkpoint)
            service.wait_line("ticker tick", "config_generation=3", "interval_ms=200", after=checkpoint)
            service.wait_line("ticker tick", "config_generation=3", "interval_ms=200")
            require(service.request("/v1/service/ready")[0] == 200, "reload broke readiness")
            alternate = "shared" if topology == "split" else "split"
            service.reload(configuration(topology=alternate).replace("interval_ms = 1000", "interval_ms = 50"), "requires_restart", 3)
            require(service.request("/v1/service/ready")[0] == 200, "cold topology change affected the live service")
            # A completion racing a stop must never publish after the coordinator
            # has observed shutdown. A publication before that point is legal.
            service.replace_config(hot)
            service.process.send_signal(signal.SIGHUP)
            text = service.stop()
            stopped = text.split("shutdown_started", 1)[1]
            require('result="published"' not in stopped, "published after shutdown started")
            require("http_request" in text and "matched_route" in text and "request completed" in text, "request logging was not observed")
            require("PROCESS_SECRET" not in text and "PROCESS_HEADER_SECRET" not in text, "request secret leaked to logs")
        cases.extend(["runtime-placement", "runtime-shutdown-order", "cold-topology-reload-rejected", "system-endpoints-and-errors", "reload-no-change", "reload-invalid-keeps-state", "reload-hot-publish-and-consume", "reload-mixed-rejects-all", "reload-cold-requires-restart", "reload-second-period", "reload-shutdown-race", "sigterm-drain", "trace-redaction"])

        with launch(root / "environment", env_only=True) as left, launch(root / "cli-priority") as right:
            left.started(); right.started()
            require(left.address != right.address, "instances unexpectedly share a listener")
            left.stop(signal.SIGINT)
            require(right.request("/v1/service/ready")[0] == 200, "stopping one instance affected the other")
            right.stop()
        cases.extend(["config-location-and-cli-priority", "sigint-drain", "two-independent-processes"])

        with launch(root / "slow-headers") as service:
            service.started()
            host, port = service.address.rsplit(":", 1)
            with socket.create_connection((host, int(port)), timeout=2) as peer:
                peer.sendall(b"GET /v1/service/ready HTTP/1.1\r\nHost: localhost\r\nX-Pending:")
                peer.settimeout(0.1)
                try:
                    peer.recv(1)
                except socket.timeout:
                    pass  # Keep the incomplete request open during shutdown.
                else:
                    raise RuntimeError("incomplete request ended before the shutdown probe")
                service.process.send_signal(signal.SIGTERM)
                code = service.process.wait(timeout=8)  # independent process watchdog
                text = service.wait_exit(code)
                summaries = [line for line in text.splitlines() if has_field(line, "event", "shutdown_complete")]
                require(code in (0, 1) and len(summaries) == 1, "slow connection did not exit with a cleanup report")
                summary = summaries[0]
                if code == 0:
                    require(has_field(summary, "forced", "false") and has_field(summary, "storage", "closed"),
                            "slow request falsely reported graceful cleanup")
                else:
                    require(has_field(summary, "forced", "true") and has_field(summary, "storage", "skipped_unproven"),
                            "forced HTTP shutdown falsely reported normal pool closure")
                peer.settimeout(2)
                try:
                    require(peer.recv(1024) == b"", "connection outlived the service process")
                except ConnectionResetError:
                    pass
        cases.append("slow-header-shutdown-is-bounded-and-honest")

        with launch(root / "disconnected-clients") as service:
            service.started()
            host, port = service.address.rsplit(":", 1)
            for _ in range(4):
                with socket.create_connection((host, int(port)), timeout=2) as peer:
                    peer.sendall(b"GET /v1/service/ready HTTP/1.1\r\nHost:")
            require(service.request("/v1/service/ready")[0] == 200, "disconnected peers killed the HTTP face")
            service.stop()
        cases.append("disconnected-client-does-not-kill-the-face")

        with launch(root / "bad-config", body="[http]\nlisten='PRIVATE_CONFIG_VALUE'") as failed:
            text = failed.wait_exit(1)
            require("service_started" not in text and "PRIVATE_CONFIG_VALUE" not in text, "invalid config published or leaked")
        cases.append("invalid-config-fail-fast")
        base = configuration(topology="main")
        invalid_topologies = [
            base.replace("[ticker]", "[ticker]\nruntime = 'typo'"),
            base + "[runtime.extra.unused]\nworker_threads=1\nmax_blocking_threads=1\n",
            base + "[runtime.extra.main]\nworker_threads=1\nmax_blocking_threads=1\n",
            configuration(topology="worker").replace("worker_threads = 2", "worker_threads = 256"),
        ]
        for index, invalid in enumerate(invalid_topologies):
            with launch(root / f"bad-topology-{index}", body=invalid) as failed:
                text = failed.wait_exit(1)
                require("runtime_built" not in text and "service_started" not in text,
                        "invalid topology created runtime resources")
        cases.append("topology-fail-fast-before-construction")

        fifo = root / "not-a-regular-config.toml"
        os.mkfifo(fifo)
        rejected = run([str(binary), "--config", str(fifo)], root, 5)
        require(rejected.returncode == 1 and "regular file" in rejected.stderr,
                "a FIFO configuration was not rejected promptly")
        cases.append("nonregular-config-fail-fast")

        with socket.socket() as occupied:
            occupied.bind(("127.0.0.1", 0)); occupied.listen()
            address = f"127.0.0.1:{occupied.getsockname()[1]}"
            with launch(root / "bind-failure", body=configuration(address, topology)) as failed:
                text = failed.wait_exit(1)
                require("service_started" not in text and "HTTP listener bind failed" in text and has_field(text, "storage", "closed"),
                        "bind failure lost its cause or cleanup")
        cases.append("bind-failure-and-rollback")

        bad_storage = root / "bad-storage"
        bad_storage.mkdir(); (bad_storage / "data").write_text("this is a file, not a directory")
        with launch(bad_storage) as failed:
            text = failed.wait_exit(1)
            require("service_started" not in text and "data_directory" in text, "storage path did not fail fast")
        cases.append("storage-path-fail-fast")

        interrupted = root / "interrupted-startup"
        (interrupted / "data").mkdir(parents=True)
        with sqlite3.connect(interrupted / "data/service.sqlite3") as lock:
            lock.execute("PRAGMA journal_mode=DELETE")
            lock.execute("CREATE TABLE test_only_lock (id INTEGER)")
            lock.commit()
            lock.execute("BEGIN EXCLUSIVE")
            with launch(interrupted) as service:
                service.wait_line("startup_stage", "storage")
                service.process.send_signal(signal.SIGTERM)
                text = service.wait_exit(0)
                require("service_started" not in text and has_field(text, "storage", "closed"), "startup cancellation published a service or lost its pool")
            lock.rollback()
        cases.append("signal-during-storage-initialization")

        dirty = root / "dirty-migration"
        with launch(dirty) as service:
            service.started(); service.stop()
        with sqlite3.connect(dirty / "data/service.sqlite3") as database:
            database.execute("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (1, 'test-only', 0, ?, 0)", (bytes(48),))
        with launch(dirty) as failed:
            text = failed.wait_exit(1)
            require("service_started" not in text and "storage migrate failed" in text and has_field(text, "storage", "closed"), "dirty migration was accepted or not cleaned up")
        cases.append("migration-fail-fast")
    return cases


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=["debug", "release"], default="debug")
    parser.add_argument("--topology", choices=list(TOPOLOGIES), default="main")
    parser.add_argument("--identity-only", action="store_true")
    args = parser.parse_args()
    require(os.name == "posix", "real signal/pipe process checks currently require POSIX; this platform is not silently skipped")
    root = Path(__file__).resolve().parents[2]
    binary, version = build_binary(root, args.profile)
    if args.identity_only:
        with tempfile.TemporaryDirectory(prefix="service-identity-") as temporary:
            with Service(binary, Path(temporary), env_only=True) as service:
                service.started()
                code, _, data = service.request("/v1/service/info")
                require(code == 200 and json.loads(data) == {"service": binary.name, "version": version}, "generated identity differs")
                service.stop()
        cases = ["generated-environment-prefix-and-build-identity"]
    else:
        cases = verify(binary, version, args.topology)
    print(json.dumps({"suite": "service", "scope": "identity" if args.identity_only else "full", "profile": args.profile,
                      "topology": args.topology, "passed": cases, "result": "passed"}))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"service check failed: {error}", file=sys.stderr)
        sys.exit(1)
