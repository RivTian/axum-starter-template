#!/usr/bin/env python3
"""probe.sh 的进程驱动：起进程、等第一条日志、发 SIGINT、有界等待退出。

单独放一个文件（而不是内联 heredoc）是为了让 shell 与 Python 的语法互不干扰。
用法：probe_run.py <binary> <cwd> <log-path>
退出码：子进程的退出码（<125）；超时 5；异常退出码 6。
"""

import signal
import subprocess
import sys
import time


def main() -> int:
    binary, cwd, log_path = sys.argv[1], sys.argv[2], sys.argv[3]
    with open(log_path, "wb") as log:
        proc = subprocess.Popen([binary], cwd=cwd, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.time() + 20
        while time.time() < deadline:
            with open(log_path, "rb") as handle:
                if b"build=" in handle.read():
                    break
            time.sleep(0.1)
        time.sleep(1.0)
        proc.send_signal(signal.SIGINT)
        try:
            code = proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()
            print("probe_run: 进程在 SIGINT 后 30s 内没有退出", file=sys.stderr)
            return 5
    return code if code < 125 else 6


if __name__ == "__main__":
    sys.exit(main())
