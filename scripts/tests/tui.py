#!/usr/bin/env python3
"""Exercise the real TUI in an isolated, sized PTY.

Five flows: menu/help/quit with terminal restoration; the type-ahead
boundary — keys typed before a plan is displayed must never approve it;
selection narrowing a plan; the project purge view applying through the
real node-modules and artifacts owners; and the detail pane at the minimum
size, where a long path cannot fit and the pane must say what it hides.
"""

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


ESCAPE = re.compile(
    r"\x1b\[([0-?]*)[ -/]*([@-~])"  # CSI: parameters and final byte
    r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC
    r"|\x1b[ -/]*[0-~]"  # any other escape
)


def render_screen(output, rows, columns):
    """The visible grid rebuilt from the cursor moves, clears and text the
    crossterm backend emits, one string per row. Ratatui repaints only the
    cells that changed, so the byte stream alone cannot say what is on screen.
    Every character takes one cell; the fixtures are ASCII and box drawing."""
    grid = [[" "] * columns for _ in range(rows)]
    row = column = 0

    def put(text):
        nonlocal row, column
        for character in text:
            if character == "\r":
                column = 0
            elif character == "\n":
                row = min(row + 1, rows - 1)
            elif character >= " ":
                if row < rows and column < columns:
                    grid[row][column] = character
                column += 1

    text = bytes(output).decode("utf-8", errors="replace")
    position = 0
    for match in ESCAPE.finditer(text):
        put(text[position:match.start()])
        position = match.end()
        parameters, final = match.group(1), match.group(2)
        if final in ("H", "f"):
            numbers = [int(value) if value else 1 for value in parameters.split(";")]
            row = numbers[0] - 1
            column = numbers[1] - 1 if len(numbers) > 1 else 0
        elif final == "J" and parameters in ("2", "3"):
            grid = [[" "] * columns for _ in range(rows)]
        elif final in ("J", "K") and parameters in ("", "0") and 0 <= row < rows:
            grid[row][column:] = [" "] * max(columns - column, 0)
            if final == "J":
                grid[row + 1:] = [[" "] * columns for _ in range(rows - row - 1)]
    put(text[position:])
    return ["".join(cells) for cells in grid]


def screen_text(lines):
    """Rows joined without their border cells and padding, so a path wrapped
    across rows of a bordered pane reads back whole."""
    return "".join(line[1:-1].rstrip() for line in lines)


class Session:
    """One devtrim TUI process attached to its own PTY and disposable home."""

    def __init__(self, binary, home, rows=40, columns=120):
        for name in ("bin", "config", "state", "cache"):
            (home / name).mkdir(exist_ok=True)
        self.environment = {
            "HOME": str(home),
            "PATH": str(home / "bin"),
            "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_STATE_HOME": str(home / "state"),
            "XDG_CACHE_HOME": str(home / "cache"),
            "TERM": "xterm-256color",
            "LANG": "en_US.UTF-8",
        }
        self.binary = binary
        self.home = home
        self.rows = rows
        self.columns = columns
        self.output = bytearray()
        self.process = None
        self.deadline = time.monotonic() + 15

    def __enter__(self):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(
            self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", self.rows, self.columns, 0, 0)
        )
        self.original = termios.tcgetattr(self.slave)
        slave = self.slave

        def attach_terminal():
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            [str(self.binary)], stdin=self.slave, stdout=self.slave, stderr=self.slave,
            cwd=self.home, env=self.environment, start_new_session=True,
            preexec_fn=attach_terminal,
        )
        return self

    def __exit__(self, *_):
        if self.process is not None and self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        os.close(self.master)
        os.close(self.slave)

    def read_output(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise AssertionError("timed out waiting for TUI")
        if select.select([self.master], [], [], min(remaining, 0.1))[0]:
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            self.output.extend(chunk)
            if not chunk:
                return False
        if len(self.output) > 2_000_000:
            raise AssertionError("TUI exceeded output bound")
        return True

    def send(self, keys):
        os.write(self.master, keys)

    def wait_for(self, text):
        start = len(self.output)
        while True:
            try:
                self.read_output()
            except AssertionError as error:
                screen = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b" ", self.output[-1200:])
                tail = " ".join(screen.decode(errors="replace").split())
                raise AssertionError(f"waiting for {text!r}: {error}; last output: {tail}") from error
            rendered = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", self.output[start:])
            if b"".join(text.encode().split()) in b"".join(rendered.split()):
                return
            if self.process.poll() is not None:
                raise AssertionError(f"TUI exited before rendering {text!r}")

    def wait_for_screen(self, *texts):
        """Waits until every text is visible at once on the rebuilt screen,
        unlike `wait_for`, which matches text drawn after the call."""
        while True:
            try:
                self.read_output()
            except AssertionError as error:
                screen = "\n".join(render_screen(self.output, self.rows, self.columns))
                raise AssertionError(f"waiting for {texts!r} on screen: {error}\n{screen}") from error
            visible = screen_text(render_screen(self.output, self.rows, self.columns))
            if all(text in visible for text in texts):
                return
            if self.process.poll() is not None:
                raise AssertionError(f"TUI exited before showing {texts!r}")

    def settle(self, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.read_output()

    def quit(self):
        self.send(b"q")
        while self.process.poll() is None:
            self.read_output()
        # Drain the final terminal-restoration sequence after process exit.
        while select.select([self.master], [], [], 0)[0]:
            if not self.read_output():
                break
        if self.process.returncode != 0:
            raise AssertionError(f"TUI exited with status {self.process.returncode}")


def verify_menu(binary):
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-") as directory:
        with Session(binary, Path(directory).resolve()) as session:
            session.wait_for("Scan everything")
            if b"\x1b[?1049h" not in session.output:
                raise AssertionError("TUI did not enter alternate screen")
            session.send(b"?")
            session.wait_for("open or close this reference")
            session.send(b"\x1b")
            session.wait_for("Enter opens")
            session.quit()
            if termios.tcgetattr(session.master) != session.original:
                raise AssertionError("TUI did not restore terminal attributes")
            if b"\x1b[?1049l" not in session.output or b"\x1b[?25h" not in session.output:
                raise AssertionError("TUI did not restore screen and cursor")


def verify_type_ahead(binary):
    """`2sa0⏎` selects caches, toggles SHRED, opens the critical confirmation
    and types its 0 GB answer. Written in one burst, everything after `2` is
    queued while the scan blocks the loop and must be discarded. The same keys
    typed after the results render are the positive control that the burst
    would otherwise have permanently deleted the cache."""
    # The system temporary directory resolves under the protected /private/var,
    # so an apply there is refused; the build directory is a writable user path.
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        cache = home / ".cache" / "uv"
        cache.mkdir(parents=True)
        (cache / "blob").write_bytes(b"x")
        with Session(binary, home) as session:
            session.wait_for("Scan everything")
            session.send(b"2sa0\r")
            session.wait_for("Review every finding")
            session.settle(1.0)
            if not cache.exists():
                raise AssertionError("type-ahead approved a plan before it was displayed")
            if b"permanentlydeleted" in b"".join(session.output.split()):
                raise AssertionError("type-ahead reached the apply summary")
            session.send(b"sa0\r")
            session.wait_for("permanently deleted uv package cache")
            if cache.exists():
                raise AssertionError("positive control: the same keys after preview did not apply")
            session.quit()


def verify_selection(binary):
    """Space leaves the highlighted cache out of the plan, so the approval covers
    only the other one. The cache left out surviving is the proof that selection
    narrowed the plan; the other cache disappearing is the positive control that
    the same approval did apply."""
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        kept = home / ".cache" / "uv"
        removed = home / ".cache" / "node"
        for cache in (kept, removed):
            cache.mkdir(parents=True)
            (cache / "blob").write_bytes(b"x")
        with Session(binary, home) as session:
            session.wait_for("Scan everything")
            session.send(b"2")
            session.wait_for("Review every finding")
            session.send(b" ")
            # Ratatui redraws only changed cells, so the title's "2 of 2" turning
            # into "1 of 2" emits one character; the detail line is drawn whole.
            session.wait_for("Left out of this plan")
            session.send(b"sa0\r")
            session.wait_for("permanently deleted node core cache")
            if not kept.exists():
                raise AssertionError("a cache left out of the plan was deleted")
            if removed.exists():
                raise AssertionError("positive control: the selected cache was not deleted")
            session.quit()


def write_script(path, body):
    path.write_text(f"#!/bin/sh\nset -eu\n{body}\n")
    path.chmod(0o755)


def stale_project(home):
    """A stale repository offering a `target` and a `node_modules`, under a
    directory name long enough that its paths wrap in the detail pane."""
    (home / "bin").mkdir()
    # No build process is running, and the repository's history is old.
    write_script(home / "bin" / "pgrep", "exit 1")
    write_script(
        home / "bin" / "git",
        "case \"$*\" in\n  *' -g '*) printf 'HEAD@{2020-01-01}\\n' ;;\n"
        "  *) printf '2020-01-01\\n' ;;\nesac",
    )
    project = home / "dev" / "-".join(["a-project-whose-directory-name-wraps"] * 4)
    (project / ".git").mkdir(parents=True)
    (project / "Cargo.toml").write_text('[package]\nname = "fixture"\n')
    (project / "target" / "debug").mkdir(parents=True)
    (project / "target" / "debug" / "out").write_bytes(b"x" * 4096)
    (project / "node_modules" / "pkg").mkdir(parents=True)
    (project / "node_modules" / "pkg" / "index.js").write_bytes(b"x")
    return project


def verify_purge(binary):
    """The project purge view runs the real node-modules and artifacts owners.
    A stale repository offers its `target` and its `node_modules`; leaving the
    second out with Space must remove only the first. The surviving
    `node_modules` proves the selection held through both categories' apply;
    the removed `target` is the positive control that the approval applied.
    The detail pane grows to show the highlighted path whole, which the row,
    keeping only its end, cannot."""
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        project = stale_project(home)
        with Session(binary, home) as session:
            session.wait_for("Scan everything")
            session.send(b"p")
            session.wait_for("Review every finding")
            # The larger `target` is first; move to `node_modules` and leave it out.
            session.send(b"j ")
            session.wait_for_screen("Left out of this plan", str(project / "node_modules"))
            session.send(b"sa0\r")
            # The note names the full path, which can wrap; the headline cannot.
            session.wait_for("purge: 1 item(s)")
            if (project / "target").exists():
                raise AssertionError("positive control: the selected target was not removed")
            if not (project / "node_modules" / "pkg" / "index.js").exists():
                raise AssertionError("a node_modules left out of the plan was removed")
            session.quit()


def verify_minimum_size(binary):
    """At the minimum 64×18 the same long path cannot fit in the detail pane.
    The pane must still show the selection state and say how many lines it
    hides, never cut them silently."""
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        stale_project(home)
        with Session(binary, home, rows=18, columns=64) as session:
            session.wait_for("Scan everything")
            session.send(b"p")
            session.wait_for("Review every finding")
            session.send(b"j ")
            session.wait_for_screen("Left out of this plan", "more line(s); enlarge the terminal")
            session.quit()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    arguments = parser.parse_args()
    binary = arguments.binary.resolve(strict=True)
    try:
        verify_menu(binary)
        verify_type_ahead(binary)
        verify_selection(binary)
        verify_purge(binary)
        verify_minimum_size(binary)
    except (AssertionError, OSError, termios.error) as error:
        print(f"tui: {error}", file=sys.stderr)
        return 1
    print(
        "tui: menu, help, cancel, quit, terminal restoration, type-ahead discard,"
        " selection, project purge, and the minimum-size detail pane passed"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
