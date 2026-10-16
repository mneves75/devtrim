#!/usr/bin/env python3
"""Exercise the real TUI in an isolated, sized PTY.

The flows: menu/help/quit with terminal restoration; the type-ahead
boundary — keys typed before a plan is displayed must never approve it;
selection narrowing a plan; the project purge view applying through the
real node-modules and artifacts owners; the detail pane at the minimum
size, where a long path cannot fit and the pane must say what it hides;
a scan blocked on a probe that does not answer, which must keep the screen
alive and stop promptly on a key or a signal; and an apply blocked the same
way, which must refuse a quit key yet stop on SIGTERM.
"""

import argparse
import errno
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
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

    def wait_for_screen_without(self, text):
        """Waits until `text` is gone from the rebuilt screen, as an overlay's
        text is once the overlay closes."""
        while True:
            try:
                self.read_output()
            except AssertionError as error:
                screen = "\n".join(render_screen(self.output, self.rows, self.columns))
                raise AssertionError(f"waiting for {text!r} to leave the screen: {error}\n{screen}") from error
            if text not in screen_text(render_screen(self.output, self.rows, self.columns)):
                return
            if self.process.poll() is not None:
                raise AssertionError(f"TUI exited while still showing {text!r}")

    def settle(self, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.read_output()

    def quit(self, expected_status=0):
        self.send(b"q")
        while self.process.poll() is None:
            self.read_output()
        # Drain the final terminal-restoration sequence after process exit.
        while select.select([self.master], [], [], 0)[0]:
            if not self.read_output():
                break
        if self.process.returncode != expected_status:
            raise AssertionError(
                f"PV tui/scan-error: TUI exited with status {self.process.returncode},"
                f" expected {expected_status}"
            )


def verify_scan_error_exit(binary):
    """A visible per-repository error must survive into the process status;
    the same category with a valid Git answer is the exit-zero control."""
    for failed in (False, True):
        with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
            home = Path(directory).resolve()
            project = home / "dev" / "fixture"
            (project / ".git").mkdir(parents=True)
            (project / "node_modules").mkdir()
            (project / "node_modules" / "index.js").write_text("fixture")
            (project / "package.json").write_text("{}")
            (home / "bin").mkdir()
            write_script(home / "bin" / "pgrep", "exit 1")
            write_script(
                home / "bin" / "git",
                "printf 'synthetic-git-failure\\n' >&2; exit 1" if failed else
                "case \"$*\" in *ls-files*) ;; *' -g '*) printf 'HEAD@{2020-01-01}\\n' ;;"
                " *) printf '2020-01-01\\n' ;; esac",
            )
            with Session(binary, home) as session:
                session.wait_for("Scan everything")
                session.send(b"3")
                session.wait_for_screen("synthetic-git-failure" if failed else "Review every finding")
                session.settle(0.25)
                if failed:
                    session.send(b"a")
                    session.wait_for_screen("This result has no actionable findings.")
                session.quit(expected_status=1 if failed else 0)
                if termios.tcgetattr(session.master) != session.original:
                    raise AssertionError("PV tui/scan-error: terminal was not restored")


def verify_menu(binary):
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-") as directory:
        with Session(binary, Path(directory).resolve()) as session:
            session.wait_for("Scan everything")
            if b"\x1b[?1049h" not in session.output:
                raise AssertionError("TUI did not enter alternate screen")
            session.send(b"?")
            session.wait_for("open or close this reference")
            session.send(b"\x1b")
            # The overlay is gone and the menu beneath it intact. Only the cells
            # the overlay covered are redrawn, so this reads the screen rather
            # than waiting for new output.
            session.wait_for_screen_without("open or close this reference")
            session.wait_for_screen("Enter opens")
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
            session.wait_for_screen("Review every finding")
            session.settle(1.0)
            if not cache.exists():
                raise AssertionError("type-ahead approved a plan before it was displayed")
            if b"permanentlydeleted" in b"".join(session.output.split()):
                raise AssertionError("type-ahead reached the apply summary")
            session.send(b"sa0\r")
            session.wait_for_screen("permanently deleted uv package cache")
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
        removed = home / ".cache" / "node" / "corepack"
        for cache in (kept, removed):
            cache.mkdir(parents=True)
            (cache / "blob").write_bytes(b"x")
        with Session(binary, home) as session:
            session.wait_for("Scan everything")
            session.send(b"2")
            session.wait_for_screen("Review every finding")
            session.send(b" ")
            # Ratatui redraws only changed cells, so the title's "2 of 2" turning
            # into "1 of 2" emits one character; the detail line is drawn whole.
            session.wait_for("Left out of this plan")
            session.send(b"sa0\r")
            session.wait_for_screen("permanently deleted Corepack package-manager cache")
            if not kept.exists():
                raise AssertionError("a cache left out of the plan was deleted")
            if removed.exists():
                raise AssertionError("positive control: the selected cache was not deleted")
            session.quit()


def verify_action_keys_explain_themselves(binary):
    """No action key passes in silence. In the read-only scan, `A` names the
    menu entry that acts on the highlighted cache; the starting status, which
    says only that the result is read-only, is the control the new line
    replaces. In the caches view the footer names what `A` does next: "A none"
    with everything selected, then "A all" once `A` has left everything out."""
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        # No build or Xcode process runs, so every category scans cleanly and
        # the session can end with status 0.
        (home / "bin").mkdir()
        write_script(home / "bin" / "pgrep", "exit 1")
        for name in ("uv", "node/corepack"):
            cache = home / ".cache" / name
            cache.mkdir(parents=True)
            (cache / "blob").write_bytes(b"x")
        with Session(binary, home) as session:
            session.wait_for("Scan everything")
            session.send(b"1")
            session.wait_for_screen("Read-only result", "uv package cache")
            session.send(b"A")
            session.wait_for_screen("Read-only scan. To act on it: b, then 2 (caches).")
            session.send(b"b2")
            session.wait_for_screen("2/2 selected", "A none")
            session.send(b"A")
            session.wait_for_screen(
                "0/2 selected", "A all", "Every item is left out; A selects them all again."
            )
            session.quit()


def write_script(path, body):
    path.write_text(f"#!/bin/sh\nset -eu\n{body}\n")
    path.chmod(0o755)


def stale_project(home, pgrep="exit 1"):
    """A stale repository offering a `target` and a `node_modules`, under a
    directory name long enough that its paths wrap in the detail pane."""
    (home / "bin").mkdir()
    # By default no build process is running; the repository's history is
    # old, and it tracks nothing under either directory.
    write_script(home / "bin" / "pgrep", pgrep)
    write_script(
        home / "bin" / "git",
        "case \"$*\" in\n  *ls-files*) ;;\n  *' -g '*) printf 'HEAD@{2020-01-01}\\n' ;;\n"
        "  *) printf '2020-01-01\\n' ;;\nesac",
    )
    project = home / "dev" / "-".join(["a-project-whose-directory-name-wraps"] * 4)
    (project / ".git").mkdir(parents=True)
    (project / "Cargo.toml").write_text('[package]\nname = "fixture"\n')
    (project / "target" / "debug").mkdir(parents=True)
    (project / "target" / "debug" / "out").write_bytes(b"x" * 4096)
    (project / "package.json").write_text("{}\n")
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
            session.wait_for_screen("Review every finding")
            # The larger `target` is first; move to `node_modules` and leave it out.
            session.send(b"j ")
            session.wait_for_screen("Left out of this plan", str(project / "node_modules"))
            session.send(b"sa0\r")
            # The note names the full path, which can wrap; the headline cannot.
            session.wait_for_screen("purge: 1 item(s)")
            if (project / "target").exists():
                raise AssertionError("positive control: the selected target was not removed")
            if not (project / "node_modules" / "pkg" / "index.js").exists():
                raise AssertionError("a node_modules left out of the plan was removed")
            session.quit()


def verify_minimum_size(binary):
    """At the minimum 64×18 the same long path cannot fit in the detail pane.
    The pane must still show the selection state and say how many lines it
    hides, never cut them silently, and Enter must show them: the view scrolls
    to the finding's last line, the project, and Esc returns to the results."""
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        project = stale_project(home)
        with Session(binary, home, rows=18, columns=64) as session:
            session.wait_for("Scan everything")
            session.send(b"p")
            session.wait_for_screen("Review every finding")
            session.send(b"j ")
            session.wait_for_screen("Left out of this plan", "more line(s); Enter shows all")
            session.send(b"\r")
            session.wait_for_screen(
                "Esc back to the results", "Details · lines 1-", str(project / "node_modules")
            )
            visible = screen_text(render_screen(session.output, session.rows, session.columns))
            if f"project: {project}" in visible:
                raise AssertionError("the project line must need scrolling to prove the view scrolls")
            session.send(b"G")
            session.wait_for_screen(f"project: {project}")
            session.send(b"\x1b")
            session.wait_for_screen("Left out of this plan", "more line(s); Enter shows all")
            session.quit()


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def elapsed_on_screen(session):
    """The working screen's clock, in whole seconds, once it is drawn."""
    while True:
        visible = screen_text(render_screen(session.output, session.rows, session.columns))
        found = re.search(r"(\d+) s elapsed", visible)
        if found:
            return int(found.group(1))
        session.read_output()


def verify_slow_scan_stops(binary):
    """A liveness probe that does not answer must not freeze the TUI. The
    stubbed `pgrep` records its pid and sleeps for two minutes. With no key
    pressed the working screen's clock must move; then each way of stopping
    must end the run promptly with the terminal restored and the probe killed:
    q and Ctrl+C typed during the scan exit 130, as SIGINT does, and SIGTERM
    exits 143. The probe still running just before the stop is the positive
    control that the scan was blocked on it. Returns the seconds each stop
    took, measured from the key or signal to the exit."""
    stops = (
        ("q", b"q", 130),
        ("ctrl-c", b"\x03", 130),
        ("sigint", signal.SIGINT, 130),
        ("sigterm", signal.SIGTERM, 143),
    )
    timings = {}
    for name, stop, expected in stops:
        with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
            home = Path(directory).resolve()
            marker = home / "pgrep.pid"
            stale_project(home, pgrep=f"echo $$ > '{marker}'\nexec /bin/sleep 120")
            probe = None
            try:
                with Session(binary, home, rows=18, columns=64) as session:
                    # A loaded machine starts devtrim and the stub slowly.
                    session.deadline = time.monotonic() + 90
                    session.wait_for("Scan everything")
                    session.send(b"3")
                    session.wait_for_screen("Scanning node-modules")
                    while not marker.exists() or not marker.read_text().endswith("\n"):
                        session.read_output()
                    probe = int(marker.read_text())
                    first = elapsed_on_screen(session)
                    while elapsed_on_screen(session) == first:
                        session.read_output()
                    if not alive(probe):
                        raise AssertionError("positive control: the stubbed probe is not running")
                    mark = len(session.output)
                    started = time.monotonic()
                    if isinstance(stop, bytes):
                        session.send(stop)
                    else:
                        session.process.send_signal(stop)
                    while session.process.poll() is None:
                        session.read_output()
                    timings[name] = round(time.monotonic() - started, 2)
                    while select.select([session.master], [], [], 0)[0]:
                        if not session.read_output():
                            break
                    if session.process.returncode != expected:
                        raise AssertionError(
                            f"{name} during a scan exited {session.process.returncode},"
                            f" expected {expected}"
                        )
                    # Generous for a machine at load 100; the report states the measured time.
                    if timings[name] > 10:
                        raise AssertionError(f"{name} took {timings[name]} s to stop the scan")
                    restored = session.output[mark:]
                    if termios.tcgetattr(session.master) != session.original:
                        raise AssertionError(f"{name}: terminal attributes were not restored")
                    if b"\x1b[?1049l" not in restored or b"\x1b[?25h" not in restored:
                        raise AssertionError(f"{name}: screen and cursor were not restored")
                # The killed probe is reaped once devtrim is gone; a loaded
                # machine can take a while, so the bound is wall-clock.
                gone_by = time.monotonic() + 60
                while alive(probe) and time.monotonic() < gone_by:
                    time.sleep(0.1)
                if alive(probe):
                    raise AssertionError(f"{name}: the probe the scan started still runs")
            finally:
                if probe is not None and alive(probe):
                    os.kill(probe, signal.SIGKILL)
    return timings


def verify_blocked_apply_refuses_quit(binary):
    """Quitting mid-apply could leave a target half removed, so q and Ctrl+C
    are refused until an apply returns, while SIGTERM still stops it. The
    stubbed `pgrep` answers the scan at once, then blocks the apply's own
    liveness preflight, which runs before anything is removed. Both keys must
    leave devtrim running with the refusal on screen; SIGTERM must then exit
    143 with the terminal restored and the probe killed. The probe running is
    the positive control that the apply was blocked on it, and the
    `node_modules` surviving shows the stop came before any removal."""
    with tempfile.TemporaryDirectory(prefix="devtrim-tui-", dir=binary.parent) as directory:
        home = Path(directory).resolve()
        scanned = home / "pgrep.scanned"
        marker = home / "pgrep.pid"
        project = stale_project(
            home,
            pgrep=(
                f"if [ ! -e '{scanned}' ]; then : > '{scanned}'; exit 1; fi\n"
                f"echo $$ > '{marker}'\nexec /bin/sleep 120"
            ),
        )
        probe = None
        try:
            with Session(binary, home, rows=18, columns=64) as session:
                session.deadline = time.monotonic() + 90
                session.wait_for("Scan everything")
                session.send(b"3")
                session.wait_for_screen("Review every finding")
                session.send(b"a")
                session.wait_for_screen("Press y to apply this exact plan")
                session.send(b"y")
                session.wait_for_screen("Applying only the exact previewed findings")
                while not marker.exists() or not marker.read_text().endswith("\n"):
                    session.read_output()
                probe = int(marker.read_text())
                if not alive(probe):
                    raise AssertionError("positive control: the stubbed apply probe is not running")
                session.send(b"q\x03")
                session.settle(1.5)
                if session.process.poll() is not None:
                    raise AssertionError("q or Ctrl+C quit in the middle of an apply")
                session.wait_for_screen("Quitting is refused until this apply returns.")
                mark = len(session.output)
                session.process.send_signal(signal.SIGTERM)
                while session.process.poll() is None:
                    session.read_output()
                while select.select([session.master], [], [], 0)[0]:
                    if not session.read_output():
                        break
                if session.process.returncode != 143:
                    raise AssertionError(
                        f"SIGTERM during an apply exited {session.process.returncode}, expected 143"
                    )
                restored = session.output[mark:]
                if termios.tcgetattr(session.master) != session.original:
                    raise AssertionError("SIGTERM during an apply left the terminal attributes")
                if b"\x1b[?1049l" not in restored or b"\x1b[?25h" not in restored:
                    raise AssertionError("SIGTERM during an apply left the screen and cursor")
            gone_by = time.monotonic() + 60
            while alive(probe) and time.monotonic() < gone_by:
                time.sleep(0.1)
            if alive(probe):
                raise AssertionError("the probe the apply started still runs")
            if not (project / "node_modules" / "pkg" / "index.js").exists():
                raise AssertionError("an apply stopped in its preflight removed a target")
        finally:
            if probe is not None and alive(probe):
                os.kill(probe, signal.SIGKILL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    arguments = parser.parse_args()
    binary = arguments.binary.resolve(strict=True)
    try:
        verify_menu(binary)
        verify_type_ahead(binary)
        verify_selection(binary)
        verify_action_keys_explain_themselves(binary)
        verify_purge(binary)
        verify_minimum_size(binary)
        verify_scan_error_exit(binary)
        stop_seconds = verify_slow_scan_stops(binary)
        verify_blocked_apply_refuses_quit(binary)
    except (AssertionError, OSError, termios.error) as error:
        print(f"tui: {error}", file=sys.stderr)
        return 1
    print(
        "tui: menu, help, cancel, quit, terminal restoration, type-ahead discard,"
        " selection, action-key explanations, project purge, the minimum-size"
        " detail pane and view, stopping a scan blocked on its probe, and refusing to"
        " quit an apply blocked on its probe passed"
    )
    print(f"tui: seconds from stop to exit during a blocked scan: {stop_seconds}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
