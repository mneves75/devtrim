#!/usr/bin/env python3
"""Render real devtrim screens into the landing page.

The pictures on index.html are not mock-ups: this script runs the binary in a
sized PTY against a disposable home holding two stale demo projects, rebuilds
each screen with its colors from the bytes the TUI writes, and replaces the HTML
between the
`<!-- tui:NAME -->` / `<!-- cli:NAME -->` markers. Rerun it after any change
to the interface:

    python3 scripts/site/frames.py target/debug/devtrim [index.html]

The demo home is the fixed `/private/tmp/devtrim-demo`, so the paths the frames
show are the same on every run; it must not exist beforehand and is removed on
exit. devtrim measures allocated blocks, so a sparse file would read 0 B: the
projects are built from APFS clones of one real 0.1 GiB file, each reporting
its own blocks as `du` counts them, so 7.8 GB of findings cost about 100 MiB.
The real HOME is never read or changed; the script never applies a plan, since
a Trash move goes to the developer's own Trash whatever HOME says. Each frame's
column count goes into `styles.css` between the `frames:cols` markers, since the
page's CSP admits no inline style attribute to carry it.
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

# Named ANSI foreground colors (30-37, 90-97, or 38;5;0-15) mapped to the page's
# classes. Black and bright black are the dim grey `k`; white stays the frame's ink.
FOREGROUND = {1: "r", 2: "g", 3: "y", 4: "u", 5: "m", 6: "c"}
GIB = 1024**3


def named(base):
    """The class for named color `base` (0-7); None is the default ink."""
    if base == 0:
        return "k"
    return FOREGROUND.get(base)


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
                style["fg"] = named(value % 10)
            elif value == 39:
                style["fg"] = None
            elif value == 38 and values[index + 1:index + 2] == [5]:
                # Crossterm writes Ratatui's named colors as 38;5;N, N in 0-15.
                number = values[index + 2] if index + 2 < len(values) else -1
                style["fg"] = named(number % 8) if 0 <= number <= 15 else None
                index += 2
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


def box_rows(grid):
    """The rows of the first bordered block: its `┌` title row through the
    first `└` below it. The frames start there, whatever the header's height."""
    text = ["".join(character for _, character in cells) for cells in grid]
    top = next((index for index, line in enumerate(text) if line.startswith("┌")), None)
    if top is None:
        raise AssertionError("no bordered block on the captured screen:\n" + "\n".join(text))
    # The confirmation clears the block beneath its title, leaving no bottom border.
    bottom = next((index for index in range(top, len(text)) if text[index].startswith("└")), len(text) - 1)
    return top, bottom + 1


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


CHUNK = GIB // 10


def chunks(home, folder, tenths):
    """`tenths` APFS clones of the home's one real chunk in `folder`: each clone
    reports the chunk's blocks, so the folder measures tenths/10 GiB."""
    folder.mkdir(parents=True, exist_ok=True)
    seed = home / ".chunk"
    if not seed.exists():
        with open(seed, "wb") as handle:
            for _ in range(CHUNK // (1 << 20)):
                handle.write(b"\x01" * (1 << 20))
            handle.write(b"\x01" * (CHUNK % (1 << 20)))
    for number in range(tenths):
        subprocess.run(["/bin/cp", "-c", str(seed), str(folder / f"part-{number:02}")], check=True)


def demo_home(home):
    """Two stale repositories, each offering what `purge` corroborates."""
    tui.stale_project(home)  # PATH stubs: no build process, history from 2020
    for leftover in (home / "dev").iterdir():
        os.rename(leftover, home / f"unused-{leftover.name}")
    api = home / "dev" / "payments-api"
    (api / ".git").mkdir(parents=True)
    (api / "Cargo.toml").write_text('[package]\nname = "payments-api"\n')
    chunks(home, api / "target" / "debug" / "deps", 46)
    chunks(home, api / "target" / "release", 13)
    shop = home / "dev" / "storefront"
    (shop / ".git").mkdir(parents=True)
    (shop / "package.json").write_text("{}\n")
    chunks(home, shop / "node_modules" / "next" / "dist", 11)
    chunks(home, shop / ".next" / "cache" / "webpack", 6)
    chunks(home, shop / ".turbo" / "cache", 2)


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
        f'<div class="term-frame" data-cols="{columns}" data-ui>'
        f'<pre class="term" role="region" tabindex="0" aria-label="devtrim screen: {name}"><code>{body}</code></pre></div>'
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
        frames["tui:results"] = block("purge preview", columns, to_html(results, box_rows(results)[0], rows))
        select = capture(binary, home, rows, 84, [
            (None, ("Scan everything",)),
            (b"p", ("Review every finding", "payments-api")),
            (b"jj ", ("Left out of this plan",)),
        ])
        # Only the preview block (title row to its bottom border), as wide as the confirmation
        # it stacks above, which carries the rest of the story.
        frames["tui:select"] = block("one finding left out", 84, to_html(select, *box_rows(select)))
        # At the minimum height the centred confirmation leaves the least empty space.
        confirm = capture(binary, home, 18, 84, [
            (None, ("Scan everything",)),
            (b"p", ("Review every finding", "payments-api")),
            (b"jj ", ("Left out of this plan",)),
            (b"a", ("This plan:",)),
        ])
        frames["tui:confirm"] = block("confirm exact plan", 84, to_html(confirm, box_rows(confirm)[0]))
        scan, scan_columns = scan_summary(binary, home)
        frames["cli:scan"] = block("devtrim scan", scan_columns, scan)
        frames["cli:refusals"] = (
            f'<pre class="term" role="region" tabindex="0" aria-label="devtrim refusals"><code>{refusals(binary, home)}</code></pre>'
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
    stylesheet = page.with_name("styles.css")
    widths = sorted({int(width) for width in re.findall(r'data-cols="(\d+)"', source)})
    rules = "".join(f'.term-frame[data-cols="{width}"] {{ --cols: {width}; }}\n' for width in widths)
    css, count = re.subn(
        r"(/\* frames:cols \(written by scripts/site/frames\.py\) \*/\n).*?(/\* /frames:cols \*/)",
        lambda match: match.group(1) + rules + match.group(2),
        stylesheet.read_text(),
        flags=re.S,
    )
    if count != 1:
        raise AssertionError(f"{stylesheet} must hold exactly one frames:cols marker pair, found {count}")
    stylesheet.write_text(css)
    print(f"wrote {len(frames)} frames into {page} and {len(widths)} widths into {stylesheet}")


if __name__ == "__main__":
    main()
