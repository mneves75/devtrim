#!/usr/bin/env python3
"""Isolated PTY checks and optional live process-CPU samples for read-only views."""

import argparse
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time


class View:
    def __init__(self, binary, home, arguments):
        self.master, self.slave = pty.openpty()
        self.output = bytearray()
        self.resize(40, 120)
        self.original = termios.tcgetattr(self.slave)
        environment = {"HOME": str(home), "PATH": str(home / "bin"),
                       "TERM": "xterm-256color", "LANG": "en_US.UTF-8"}
        for name in ("CONFIG", "CACHE", "STATE"):
            environment[f"XDG_{name}_HOME"] = str(home / name.lower())

        def attach():
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen([str(binary), *arguments], cwd=home,
                                        env=environment, stdin=self.slave,
                                        stdout=self.slave, stderr=self.slave,
                                        start_new_session=True, preexec_fn=attach)

    def resize(self, rows, columns):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))

    def read(self, timeout=0.05):
        if select.select([self.master], [], [], timeout)[0]:
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            self.output.extend(chunk)
            assert len(self.output) < 10_000_000, "PTY output bound exceeded"
            return bool(chunk)
        return True

    def wait(self, text, start=0, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.read()
            plain = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", self.output[start:])
            # Ratatui ends each completed frame by hiding the cursor. A body
            # substring can arrive before the frame footer and input cycle finish.
            if (b"".join(text.encode().split()) in b"".join(plain.split())
                    and self.output.endswith(b"\x1b[?25l")):
                return
            assert self.process.poll() is None, f"exited before displaying {text!r}"
        raise AssertionError(f"timed out displaying {text!r}")

    def send(self, keys):
        start = len(self.output)
        os.write(self.master, keys)
        return start

    def hold(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.read(min(0.05, max(0, deadline - time.monotonic())))
            assert self.process.poll() is None, "view exited during idle sample"

    def quit(self, expected=0):
        started = time.monotonic()
        self.send(b"q")
        while self.process.poll() is None and time.monotonic() - started < 2:
            self.read()
        assert self.process.poll() == expected, (
            f"quit expected status {expected}, got {self.process.poll()} after "
            f"{time.monotonic() - started:.3f}s; terminal tail={bytes(self.output[-300:])!r}"
        )
        while select.select([self.master], [], [], 0)[0] and self.read(0):
            pass
        assert termios.tcgetattr(self.master) == self.original, "terminal mode was not restored"
        assert b"\x1b[?1049l" in self.output, "alternate screen was not restored"
        assert b"\x1b[?25h" in self.output, "cursor was not restored"
        return time.monotonic() - started

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        os.close(self.master)
        os.close(self.slave)


def cpu_seconds(process):
    result = subprocess.run(["/bin/ps", "-p", str(process.pid), "-o", "time="],
                            check=True, capture_output=True, text=True)
    fields = result.stdout.strip().split(":")
    assert 2 <= len(fields) <= 3, f"unexpected process CPU time: {result.stdout!r}"
    total = 0.0
    for field in fields:
        total = total * 60 + float(field)
    return total


def exercise(binary, home):
    corpus = home / "navigation"
    corpus.mkdir()
    for index in range(60):
        child = corpus / f"row-{index:03}"
        child.mkdir()
        (child / "selected-child").write_bytes(b"x" * (100 - index))
    view = View(binary, home, ["analyze", str(corpus)])
    try:
        view.wait("Contents")
        start = view.send(b"k" * 60 + b"j" * 45 + b"\r")
        view.wait("row-045", start)
        view.wait("selected-child", start)
        start = view.send(b"?")
        view.wait("open or close this reference", start)
        start = len(view.output)
        view.resize(24, 90)
        view.wait("open or close this reference", start)
        start = view.send(b"\x1b")
        view.wait("selected-child", start)
        latency = view.quit()
    finally:
        view.close()
    wide = home / "progress"
    wide.mkdir()
    for index in range(2048):
        (wide / f"entry-{index:04}").touch()
    view = View(binary, home, ["analyze", str(wide)])
    try:
        view.wait("Measuring")
        progress_latency = view.quit()
    finally:
        view.close()
    view = View(binary, home, ["status", "--watch"])
    try:
        view.wait("health")
        start = len(view.output)
        view.resize(24, 90)
        view.wait("health", start)
        view.quit(expected=1)  # The empty PATH deliberately reports unavailable metrics.
    finally:
        view.close()
    stub = home / "bin/sysctl"
    stub.write_text('#!/bin/sh\nprintf "%s\\n" "$$" > "$HOME/probe-pid"\nexec /bin/sleep 60\n')
    stub.chmod(0o755)
    view = View(binary, home, ["status", "--watch"])
    try:
        view.wait("sampling")
        deadline = time.monotonic() + 5
        while not (home / "probe-pid").exists():
            assert time.monotonic() < deadline, "blocking probe never started"
            view.hold(0.05)
        stalled_latency = view.quit()
    finally:
        view.close()
        if (home / "probe-pid").exists():
            try:
                os.kill(int((home / "probe-pid").read_text()), signal.SIGTERM)
            except ProcessLookupError:
                pass
        stub.unlink()
    return {"analyze_quit_seconds": latency, "analyze_progress_quit_seconds": progress_latency,
            "stalled_status_quit_seconds": stalled_latency}


def benchmark(binary, home, seconds, count):
    corpus = home / "wide"
    corpus.mkdir(exist_ok=True)
    if not any(corpus.iterdir()):
        for index in range(count):
            (corpus / f"entry-{index:05}").write_bytes(b"x")
    measurements = []
    for arguments, ready, exit_code in [(["analyze", str(corpus)], "Contents", 0),
                                         (["status", "--watch"], "health", 1)]:
        view = View(binary, home, arguments)
        try:
            view.wait(ready, timeout=60)
            view.hold(0.3)
            before = cpu_seconds(view.process)
            load = os.getloadavg()
            started = time.monotonic()
            view.hold(seconds)
            elapsed = time.monotonic() - started
            used = cpu_seconds(view.process) - before
            view.quit(expected=exit_code)
            measurements.append({"view": arguments[0], "seconds": elapsed,
                                 "process_cpu_seconds": used, "load_start": load,
                                 "load_end": os.getloadavg(), "children": count})
        finally:
            view.close()
    return {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "terminal": "120x40", "measurements": measurements}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--benchmark", action="store_true")
    parser.add_argument("--seconds", type=float, default=6)
    parser.add_argument("--children", type=int, default=6000)
    args = parser.parse_args()
    if args.seconds <= 0 or args.children <= 0:
        parser.error("seconds and children must be positive")
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="devtrim-readonly-") as directory:
        home = Path(directory)
        (home / "bin").mkdir()
        result = benchmark(binary, home, args.seconds, args.children) if args.benchmark else exercise(binary, home)
        print(json.dumps(result))


if __name__ == "__main__":
    main()
