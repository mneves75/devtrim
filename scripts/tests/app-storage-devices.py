#!/usr/bin/env python3
"""Keep native siblings after a real, read-only foreign-device boundary."""

from __future__ import annotations

import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile


def checked(command: list[str], *, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
    result = subprocess.run(command, env=env, capture_output=True, timeout=60)
    if result.returncode:
        raise RuntimeError(
            f"{command[0]} exited {result.returncode}: "
            f"{result.stderr.decode(errors='replace')}"
        )
    return result


def measure(binary: Path, home: Path, roots: list[Path], expected: int) -> None:
    environment = os.environ.copy()
    environment.update(HOME=str(home), PATH="/usr/bin:/bin", NO_COLOR="1")
    command = [str(binary), "app-storage", "--json"]
    for root in roots:
        command.extend(["--root", str(root)])
    result = checked(command, env=environment)
    document = json.loads(result.stdout)
    assert document["operation"] == "app-storage" and document["applied"] is False
    assert document["errors"] == [], document
    assert all(finding["action"]["type"] == "info" for finding in document["findings"])
    actual = sum(finding["size_bytes"] for finding in document["findings"])
    assert actual == expected, f"expected {expected} native bytes, reported {actual}: {document}"


def scenario(binary: Path, base: Path, name: str) -> None:
    directory = base / name
    home = directory / "home"
    home.mkdir(parents=True)
    if name == "whatsapp":
        root = home / "Library/Group Containers/group.net.whatsapp.WhatsApp.shared/Message/Media"
        roots = []
    elif name == "bundle":
        root = directory / "projects/edit.screenstudio"
        roots = [root.parent]
    else:
        root = directory / "projects"
        roots = [root]
    root.mkdir(parents=True)
    native = root / ("z-native.screenstudio/raw" if name == "discovery" else "z-native.raw")
    native.parent.mkdir(parents=True, exist_ok=True)
    native.write_bytes(b"native data" * 1024)
    expected = native.stat().st_blocks * 512
    assert expected > 0

    foreign_source = directory / "foreign-source/hidden.screenstudio"
    foreign_source.mkdir(parents=True)
    (foreign_source / "raw").write_bytes(b"foreign data must stay excluded" * 1024)
    image = directory / "foreign.dmg"
    checked([
        "/usr/bin/hdiutil", "create", "-size", "16m", "-fs", "HFS+",
        "-volname", "devtrim-test", "-format", "UDRW", "-srcfolder",
        str(foreign_source.parent), str(image),
    ])
    mount = root / "a-device"
    mount.mkdir()
    try:
        attached = checked([
            "/usr/bin/hdiutil", "attach", "-readonly", "-noautoopen", "-mountpoint",
            str(mount), "-plist", str(image),
        ])
        entities = plistlib.loads(attached.stdout)["system-entities"]
        assert any(entity.get("mount-point") == str(mount) for entity in entities)
        assert mount.stat().st_dev != root.stat().st_dev, "fixture did not cross devices"
        measure(binary, home, roots, expected)
        if name == "discovery":
            foreign_root = mount / "hidden.screenstudio"
            foreign_bytes = (foreign_root / "raw").stat().st_blocks * 512
            measure(binary, home, [root, foreign_root], expected + foreign_bytes)
    finally:
        # Never recursively clean a fixture while its mounted filesystem remains.
        if os.path.ismount(mount):
            checked(["/usr/bin/hdiutil", "detach", str(mount)])
        assert not os.path.ismount(mount), f"test mount remains: {mount}"


def main() -> int:
    if sys.platform != "darwin" or len(sys.argv) != 2:
        print("usage (macOS): app-storage-devices.py <devtrim-binary>", file=sys.stderr)
        return 2
    binary = Path(sys.argv[1]).resolve(strict=True)
    scratch = Path(__file__).resolve().parents[2] / ".scratch"
    scratch.mkdir(exist_ok=True)
    base = Path(tempfile.mkdtemp(prefix="app-storage-devices-", dir=scratch))
    failures = []
    try:
        for name in ("discovery", "whatsapp", "bundle"):
            try:
                scenario(binary, base, name)
                print(f"PASS {name}: foreign volume excluded, native siblings retained")
            except Exception as error:
                failures.append(name)
                print(f"FAIL {name}: {error}", file=sys.stderr)
    finally:
        mounts = [
            base / "discovery/projects/a-device",
            base / "whatsapp/home/Library/Group Containers/group.net.whatsapp.WhatsApp.shared/Message/Media/a-device",
            base / "bundle/projects/edit.screenstudio/a-device",
        ]
        if any(os.path.ismount(mount) for mount in mounts):
            print(f"retaining fixture with attached test volume: {base}", file=sys.stderr)
            failures.append("cleanup")
        else:
            shutil.rmtree(base)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
