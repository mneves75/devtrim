#!/usr/bin/env python3
"""Render real devtrim screens into the landing page.

The pictures on index.html are not mock-ups: this script runs the binary in a
sized PTY against a disposable home holding two stale demo projects (sparse
files, so their logical sizes cost no disk), rebuilds each screen with its
colors from the bytes the TUI writes, and replaces the HTML between the
`<!-- tui:NAME -->` / `<!-- cli:NAME -->` markers. Rerun it after any change
to the interface:

    python3 scripts/site/frames.py target/debug/devtrim [index.html]

The demo home is the fixed `/private/tmp/devtrim-demo`, so the paths the frames
show are the same on every run; it must not exist beforehand and is removed on
exit. The real HOME is never read or changed.
"""

import html
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("tui", ROOT / "scripts/tests/tui.py")
tui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tui)

# Named ANSI foreground colors (30-37, 90-97) mapped to the page's classes.
FOREGROUND = {1: "r", 2: "g", 3: "y", 4: "u", 5: "m", 6: "c"}
GIB = 1024**3


def colored_screen(output, rows, columns):
    """The visible grid with each cell's style, rebuilt the way
    `tui.render_screen` rebuilds the text, plus SGR attributes."""
    blank = ("", " ")
    grid = [[blank] * columns for _ in range(rows)]
    row = column = 0
    style = {"fg": None, "bold": False, "reverse": False}

    def css():
        classes = []
        if style["reverse"]:
            classes.append("v")
        elif style["fg"]:
            classes.append(style["fg"])
        if style["bold"]:
            classes.append("b")
        return " ".join(classes)

    def put(text):
        nonlocal row, column
        for character in text:
            if character == "\r":
                column = 0
            elif character == "\n":
                row = min(row + 1, rows - 1)
            elif character >= " ":
                if row < rows and column < columns:
                    grid[row][column] = (css(), character)
                column += 1

    def sgr(parameters):
        values = [int(value) if value else 0 for value in parameters.split(";")] or [0]
        index = 0
        while index < len(values):
            value = values[index]
            if value == 0:
                style.update(fg=None, bold=False, reverse=False)
            elif value == 1:
                style["bold"] = True
            elif value == 22:
                style["bold"] = False
            elif value == 7:
                style["reverse"] = True
            elif value == 27:
                style["reverse"] = False
            elif 30 <= value <= 37 or 90 <= value <= 97:
                base = value % 10
                style["fg"] = "k" if value == 90 else FOREGROUND.get(base)
            elif value == 39:
                style["fg"] = None
            elif value in (38, 48):
                index += 2 if values[index + 1:index + 2] == [5] else 4
            index += 1

    text = bytes(output).decode("utf-8", errors="replace")
    position = 0
    for match in tui.ESCAPE.finditer(text):
        put(text[position:match.start()])
        position = match.end()
        parameters, final = match.group(1), match.group(2)
        if parameters is None:
            continue
        if final == "m":
            sgr(parameters)
        elif final in ("H", "f"):
            numbers = [int(value) if value else 1 for value in parameters.split(";")]
            row = numbers[0] - 1
            column = numbers[1] - 1 if len(numbers) > 1 else 0
        elif final == "J" and parameters in ("2", "3"):
            grid = [[blank] * columns for _ in range(rows)]
        elif final in ("J", "K") and parameters in ("", "0") and 0 <= row < rows:
            grid[row][column:] = [blank] * max(columns - column, 0)
            if final == "J":
                grid[row + 1:] = [[blank] * columns for _ in range(rows - row - 1)]
    put(text[position:])
    return grid


def to_html(grid, first_row=0, last_row=None):
    lines = []
    for cells in grid[first_row:last_row]:
        spans, current, run = [], None, ""
        for classes, character in cells:
            if classes != current and run:
                spans.append((current, run))
                run = ""
            current = classes
            run += character
        if run:
            spans.append((current, run))
        line = "".join(
            f'<span class="{classes}">{html.escape(text)}</span>' if classes else html.escape(text)
            for classes, text in spans
        )
        lines.append(line.rstrip())
    return "\n".join(lines).rstrip()


def sparse(path, size):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as handle:
        handle.truncate(size)


def demo_home(home):
    """Two stale repositories, each offering what `purge` corroborates."""
    tui.stale_project(home)  # PATH stubs: no build process, history from 2020
    for leftover in (home / "dev").iterdir():
        os.rename(leftover, home / f"unused-{leftover.name}")
    api = home / "dev" / "payments-api"
    (api / ".git").mkdir(parents=True)
    (api / "Cargo.toml").write_text('[package]\nname = "payments-api"\n')
    sparse(api / "target" / "debug" / "deps" / "libpayments.rlib", int(4.6 * GIB))
    sparse(api / "target" / "release" / "payments-api", int(1.3 * GIB))
    shop = home / "dev" / "storefront"
    (shop / ".git").mkdir(parents=True)
    (shop / "package.json").write_text("{}\n")
    sparse(shop / "node_modules" / "next" / "dist" / "bundle.js", int(1.1 * GIB))
    sparse(shop / ".next" / "cache" / "webpack" / "pack", int(0.6 * GIB))
    sparse(shop / ".turbo" / "cache" / "run.tar", int(0.2 * GIB))


def capture(binary, home, rows, columns, steps):
    with tui.Session(binary, home, rows=rows, columns=columns) as session:
        for keys, waits in steps:
            if keys:
                session.send(keys)
            session.wait_for_screen(*waits)
        session.settle(0.3)
        grid = colored_screen(session.output, rows, columns)
        session.send(b"\x1b")
        session.process.kill()
        return grid


def refusals(binary, home):
    environment = {"HOME": str(home), "PATH": str(home / "bin"), "LANG": "en_US.UTF-8"}
    out = []
    for argv in (["purge", "--apply"], ["largest", "--apply"]):
        result = subprocess.run(
            [str(binary), *argv], stdin=subprocess.DEVNULL, capture_output=True,
            env=environment, cwd=home, text=True, timeout=60,
        )
        lines = (result.stdout + result.stderr).strip().splitlines()
        if result.returncode == 0:
            raise AssertionError(f"`devtrim {' '.join(argv)}` was expected to refuse")
        kept = [line for line in lines if "error" in line.lower() or "refus" in line.lower()]
        out.append(f'<span class="k">$</span> devtrim {html.escape(" ".join(argv))} &lt; /dev/null')
        out += [f'<span class="r">{html.escape(line)}</span>' for line in (kept or lines[-2:])]
        out.append(f'<span class="k">exit {result.returncode}</span>')
        out.append("")
    return "\n".join(out).rstrip()


def scan_summary(binary, home):
    """`devtrim scan` as a reader runs it: the human report, uncolored since
    stdout is a pipe, with the demo home spelled `~` as a shell shows it."""
    environment = {"HOME": str(home), "PATH": str(home / "bin"), "LANG": "en_US.UTF-8"}
    result = subprocess.run(
        [str(binary), "scan"], stdin=subprocess.DEVNULL, capture_output=True,
        env=environment, cwd=home, text=True, timeout=120,
    )
    if result.returncode != 0:
        raise AssertionError(f"`devtrim scan` failed: {result.stderr}")
    lines = result.stdout.rstrip().replace(str(home), "~").splitlines()
    body = "\n".join(
        [f'<span class="k">$</span> devtrim scan'] + [html.escape(line) for line in lines]
    )
    return body, max(len(line) for line in lines)


def block(name, columns, body):
    return (
        f'<div class="term-frame" style="--cols: {columns}" data-ui>'
        f'<pre class="term" tabindex="0" aria-label="devtrim screen: {name}"><code>{body}</code></pre></div>'
    )


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit("usage: scripts/site/frames.py <devtrim binary> [index.html]")
    binary = Path(sys.argv[1]).resolve()
    page = Path(sys.argv[2] if len(sys.argv) == 3 else ROOT / "index.html")
    frames = {}
    home = Path("/private/tmp/devtrim-demo")
    try:
        home.mkdir()
    except FileExistsError:
        sys.exit(f"{home} already exists; remove it after checking it is an old demo home")
    try:
        home = home.resolve()
        demo_home(home)
        rows, columns = 24, 100
        results = capture(binary, home, rows, columns, [
            (None, ("Scan everything",)),
            (b"p", ("Review every finding", "payments-api")),
        ])
        frames["tui:results"] = block("purge preview", columns, to_html(results, 3, rows))
        confirm = capture(binary, home, rows, 84, [
            (None, ("Scan everything",)),
            (b"p", ("Review every finding", "payments-api")),
            (b"jj ", ("Left out of this plan",)),
            (b"a", ("This plan:",)),
        ])
        frames["tui:confirm"] = block("confirm exact plan", 84, to_html(confirm, 3, rows))
        scan, scan_columns = scan_summary(binary, home)
        frames["cli:scan"] = block("devtrim scan", scan_columns, scan)
        frames["cli:refusals"] = (
            f'<pre class="term" tabindex="0" aria-label="devtrim refusals"><code>{refusals(binary, home)}</code></pre>'
        )
    finally:
        shutil.rmtree(home)
    source = page.read_text()
    for name, body in frames.items():
        pattern = re.compile(rf"(<!-- {re.escape(name)} -->\n).*?(<!-- /{re.escape(name)} -->)", re.S)
        source, count = pattern.subn(lambda match: match.group(1) + body + "\n" + match.group(2), source)
        if count != 1:
            raise AssertionError(f"{page} must hold exactly one {name} marker pair, found {count}")
    page.write_text(source)
    print(f"wrote {len(frames)} frames into {page}")


if __name__ == "__main__":
    main()
