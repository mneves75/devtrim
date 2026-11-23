"""Prove the TruffleHog scan step keeps its failure and logs no raw match (DIST-LOG-1).

Builds a throwaway repository holding a synthetic GitHub-token-shaped string,
then runs the workflows' command with stdout discarded, as both workflows
do: TruffleHog's printer writes each raw match on stdout. Verification is off
so the synthetic match reports offline. The positive control runs the same
scan with stdout kept and must show the raw value, proving the fixture is
detected at all.
"""
import subprocess
import sys
import tempfile
from pathlib import Path

binary = sys.argv[1] if len(sys.argv) > 1 else "trufflehog"
# The value is split so this file itself never matches a detector.
token = "ghp_" + "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"
with tempfile.TemporaryDirectory(prefix="devtrim-secret-scan-") as directory:
    repo = Path(directory)
    git = ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    (repo / "config.txt").write_text(f"token = {token}\n")
    subprocess.run(git + ["add", "config.txt"], check=True)
    subprocess.run(git + ["commit", "-q", "-m", "fixture"], check=True)
    scan = [binary, "git", f"file://{repo}", "--results=verified,unknown,unverified",
            "--no-verification", "--fail", "--fail-on-scan-errors", "--no-update", "--no-color"]

    kept = subprocess.run(scan, capture_output=True, text=True)
    assert kept.returncode == 183, f"control: fixture not detected (exit {kept.returncode})"
    assert token in kept.stdout, "control: raw value not on stdout, so discarding it proves nothing"

    quiet = subprocess.run(scan, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    assert quiet.returncode == 183, f"discarded stdout lost the failure (exit {quiet.returncode})"
    assert token not in quiet.stderr, "the raw value reached stderr"
    assert token[4:12] not in quiet.stderr, "part of the raw value reached stderr"

print("trufflehog control: detected with stdout kept (positive control), exit 183 kept with stdout discarded, no raw bytes on stderr")
