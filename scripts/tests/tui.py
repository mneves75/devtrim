#!/usr/bin/env python3
"""Exercise the real menu/help/quit flow in an isolated, sized PTY."""

import argparse
import errno
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time


def verify(binary):
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-") as directory:
        home = Path(directory)
        for name in ("bin", "config", "state", "cache"):
            (home / name).mkdir()
        environment = {
            "HOME": str(home),
            "PATH": str(home / "bin"),
            "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_STATE_HOME": str(home / "state"),
            "XDG_CACHE_HOME": str(home / "cache"),
            "TERM": "xterm-256color",
            "LANG": "en_US.UTF-8",
        }
        master, slave = pty.openpty()
        process = None
        output = bytearray()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
            original = termios.tcgetattr(slave)
            def attach_terminal():
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

            process = subprocess.Popen(
                [str(binary)], stdin=slave, stdout=slave, stderr=slave,
                cwd=home, env=environment, start_new_session=True,
                preexec_fn=attach_terminal,
            )
            deadline = time.monotonic() + 15

            def read_output():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError("timed out waiting for TUI")
                if select.select([master], [], [], min(remaining, 0.1))[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        chunk = b""
                    output.extend(chunk)
                    if not chunk:
                        return False
                if len(output) > 2_000_000:
                    raise AssertionError("TUI exceeded output bound")
                return True

            def wait_for(text):
                start = len(output)
                while True:
                    try:
                        read_output()
                    except AssertionError as error:
                        raise AssertionError(f"waiting for {text!r}: {error}") from error
                    rendered = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", output[start:])
                    if b"".join(text.encode().split()) in b"".join(rendered.split()):
                        return
                    if process.poll() is not None:
                        raise AssertionError(f"TUI exited before rendering {text!r}")

            wait_for("Scan everything")
            if b"\x1b[?1049h" not in output:
                raise AssertionError("TUI did not enter alternate screen")
            os.write(master, b"?")
            wait_for("open or close this reference")
            os.write(master, b"\x1b")
            wait_for("Enter opens")
            os.write(master, b"q")
            while process.poll() is None:
                read_output()
            # Drain the final terminal-restoration sequence after process exit.
            while select.select([master], [], [], 0)[0]:
                if not read_output():
                    break
            if process.returncode != 0:
                raise AssertionError(f"TUI exited with status {process.returncode}")
            if termios.tcgetattr(master) != original:
                raise AssertionError("TUI did not restore terminal attributes")
            if b"\x1b[?1049l" not in output or b"\x1b[?25h" not in output:
                raise AssertionError("TUI did not restore screen and cursor")
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    arguments = parser.parse_args()
    try:
        verify(arguments.binary.resolve(strict=True))
    except (AssertionError, OSError, termios.error) as error:
        print(f"tui: {error}", file=sys.stderr)
        return 1
    print("tui: menu, help, cancel, quit, and terminal restoration passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
