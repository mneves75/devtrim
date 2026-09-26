#!/usr/bin/env python3
"""Drive the real devtrim TUI in a PTY against a disposable HOME and dump
each screen (chars + colors) as JSON for the video. Nothing is applied."""
import fcntl, json, os, pty, select, struct, subprocess, sys, tempfile, termios, time
from pathlib import Path
import pyte

BIN = Path(sys.argv[1]).resolve()
WORK = Path(__file__).resolve().parent
# Disposable HOME; the video shows its prefix as /Users/you.
HOME = Path(tempfile.mkdtemp(prefix="brag-home-")).resolve()
COLS, ROWS = 104, 30
GB = 1_000_000_000

for name in ("bin", "config", "state", "cache"):
    (HOME / name).mkdir(parents=True)

def script(path, body):
    path.write_text(f"#!/bin/sh\nset -eu\n{body}\n")
    path.chmod(0o755)

# No build running; every repository was last touched in 2020.
script(HOME / "bin" / "pgrep", "exit 1")
script(HOME / "bin" / "git", "case \"$*\" in\n  *' -g '*) printf 'HEAD@{2020-01-01}\\n' ;;\n  *) printf '2020-01-01\\n' ;;\nesac")

def sparse(path, size):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        f.truncate(size)

def repo(name, files, blobs):
    root = HOME / "dev" / name
    (root / ".git").mkdir(parents=True)
    for rel, text in files.items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(text)
    for rel, size in blobs.items():
        sparse(root / rel, size)

repo("checkout-api", {"Cargo.toml": '[package]\nname = "checkout-api"\n', "package.json": "{}"},
     {"target/debug/deps/libcheckout.rlib": int(8.7 * GB), "node_modules/esbuild/bin/esbuild": int(1.3 * GB)})
repo("marketing-site", {"package.json": "{}"},
     {"node_modules/next/dist/index.js": int(2.6 * GB), ".next/cache/webpack/pack": int(1.9 * GB)})
repo("ml-notebooks", {".venv/pyvenv.cfg": "home = /usr/bin\n"},
     {".venv/lib/python3.12/site-packages/torch/lib/libtorch.dylib": int(4.4 * GB)})
repo("ios-client", {"Podfile": "platform :ios, '17.0'\n"},
     {"Pods/Firebase/Firebase.framework/Firebase": int(1.1 * GB)})

env = {"HOME": str(HOME), "PATH": str(HOME / "bin"), "XDG_CONFIG_HOME": str(HOME / "config"),
       "XDG_STATE_HOME": str(HOME / "state"), "XDG_CACHE_HOME": str(HOME / "cache"),
       "TERM": "xterm-256color", "LANG": "en_US.UTF-8"}

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
def attach():
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
proc = subprocess.Popen([str(BIN)], stdin=slave, stdout=slave, stderr=slave, cwd=HOME, env=env,
                        start_new_session=True, preexec_fn=attach)
screen = pyte.Screen(COLS, ROWS)
stream = pyte.ByteStream(screen)

def pump(seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if select.select([master], [], [], 0.05)[0]:
            try:
                stream.feed(os.read(master, 65536))
            except OSError:
                return

def text():
    return "\n".join(screen.display)

def wait_for(needle, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        pump(0.1)
        if needle in text():
            pump(0.4)
            return
    raise SystemExit(f"timeout waiting for {needle!r}\n{text()}")

def dump(name):
    rows = []
    for y in range(ROWS):
        line = screen.buffer[y]
        rows.append([[line[x].data, line[x].fg, line[x].bg, line[x].bold, line[x].reverse] for x in range(COLS)])
    (WORK / "screens").mkdir(exist_ok=True)
    (WORK / "screens" / f"{name}.json").write_text(json.dumps(rows))
    (WORK / "screens" / f"{name}.txt").write_text(text())
    print(f"== {name}\n{text()}\n")

wait_for("Scan everything"); dump("menu")
os.write(master, b"p"); wait_for("Review every finding"); dump("purge")
os.write(master, b"j"); pump(0.6); dump("purge-j")
os.write(master, b" "); wait_for("5/6 selected"); dump("purge-left-out")
os.write(master, b"j"); pump(0.6); dump("purge-left-out-j")
os.write(master, b"a"); pump(1.2); dump("confirm")
os.write(master, b"\x1b"); pump(0.6)
proc.kill(); proc.wait()
