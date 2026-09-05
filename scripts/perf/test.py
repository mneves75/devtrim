#!/usr/bin/env python3
"""Exercise benchmark refusal paths and real Hyperfine argument parsing."""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile

SCRIPTS = Path(__file__).resolve().parent


def run(script, *args, success=True, **environment):
    result = subprocess.run(
        ["bash", str(SCRIPTS / script), *map(str, args)],
        env={**os.environ, **environment}, capture_output=True, text=True,
        check=False,
    )
    assert (result.returncode == 0) == success, result.stdout + result.stderr
    return result


with tempfile.TemporaryDirectory(prefix="devtrim-perf ") as temporary:
    root = Path(temporary)
    corpus = root / "home ' quoted $ literal"
    for count in ["-1", "01", "1+2", "1000000", "nope"]:
        run("corpus.sh", corpus, count, success=False)
        assert not corpus.exists()
    run("corpus.sh", root / "missing" / "home", success=False)
    dangling = root / "dangling"
    dangling.symlink_to(root / "absent")
    run("corpus.sh", dangling, success=False)
    run("corpus.sh", corpus, 0, 0, 1, 2)
    run("corpus.sh", corpus, 0, 0, 1, 2, success=False)
    git_corpus = root / "git corpus"
    run("corpus.sh", git_corpus, 1, 1, 1, 0, GIT_DIR=str(root / "wrong-git-dir"))
    assert (git_corpus / "dev/stale-0/.git").is_dir()
    assert (git_corpus / "dev/recent-0/.git").is_dir()
    assert not (root / "wrong-git-dir").exists()
    assert len(list((corpus / "dev/noise/b0").iterdir())) == 2
    assert (corpus / ".cache/huggingface/hub/f0").is_file()
    sentinel = corpus / ".cache/huggingface/token"
    sentinel_before = sentinel.read_bytes()
    for program, suffix in [("npm", ".npm"), ("brew", "Library/Caches/Homebrew")]:
        output = subprocess.check_output([str(corpus / "bin" / program)], env={"HOME": str(corpus)}, text=True)
        assert output.rstrip("\n") == str(corpus / suffix)
    binaries = []
    for name in ["before ' quoted", 'after " quoted']:
        directory = root / name
        directory.mkdir()
        binary = directory / "devtrim"
        binary.write_text('#!/bin/sh\nif [ "$1" = --version ]; then echo fixture; exit 0; fi\nprintf \'%s\\n\' \'{"operation":"scan","findings":[],"errors":[]}\'\n')
        binary.chmod(0o755)
        binaries.append(binary)
    result_dir = root / "results ' quoted"
    run("ab.sh", *binaries, corpus, 2, PERF_OUT=str(result_dir), PERF_FORCE="1")
    for order in ["baseline-first", "candidate-first"]:
        result = json.loads((result_dir / f"{order}.json").read_text())
        assert len(result["results"]) == 2
        assert (result_dir / f"{order}-load-before.txt").is_file()
        assert (result_dir / f"{order}-load-after.txt").is_file()
    assert sentinel.read_bytes() == sentinel_before
    run("ab.sh", *binaries, corpus, "0", success=False)
    run("ab.sh", *binaries, corpus, 2, success=False, PERF_OUT=str(result_dir))
    # Raise the observed load only after the final real Hyperfine ordering.
    tools = root / "load-tools"
    tools.mkdir()
    counter = root / "completed-orderings"
    counter_path = shlex.quote(str(counter))
    hyperfine = shutil.which("hyperfine")
    assert hyperfine, "Hyperfine is required"
    wrapper = tools / "hyperfine"
    wrapper.write_text(
        f'#!/bin/sh\n{shlex.quote(hyperfine)} "$@" || exit "$?"\n'
        '[ "${1:-}" = --version ] && exit 0\n'
        f'count=0\nif [ -f {counter_path} ]; then read -r count < {counter_path}; fi\n'
        f'printf "%s\\n" "$((count + 1))" > {counter_path}\n'
    )
    load = tools / "sysctl"
    load.write_text(
        '#!/bin/sh\nif [ "$2" = hw.ncpu ]; then echo 18; exit 0; fi\n'
        f'count=0\nif [ -f {counter_path} ]; then read -r count < {counter_path}; fi\n'
        'if [ "$count" -ge 2 ]; then echo "{ 999 999 999 }"; else echo "{ 0 0 0 }"; fi\n'
    )
    wrapper.chmod(0o755)
    load.chmod(0o755)
    for force in ["0", "1"]:
        counter.write_text("0\n")
        output = root / f"late-load-{force}"
        result = run("ab.sh", *binaries, corpus, 2, success=force == "1",
                     PERF_OUT=str(output), PERF_FORCE=force,
                     PATH=str(tools) + os.pathsep + os.environ["PATH"])
        assert "999" in (output / "candidate-first-load-after.txt").read_text()
        if force == "0":
            assert result.returncode == 2 and "refusing timing" in result.stderr
        else:
            assert "not a verified speedup" in (output / "warnings.txt").read_text()
    cases = {
        "unequal": 'printf \'%s\\n\' \'{"operation":"scan","findings":[{}],"errors":[]}\'\n',
        "failure": "exit 7\n",
        "invalid": "echo invalid\n",
    }
    for label, body in cases.items():
        binaries[1].write_text("#!/bin/sh\n" + body)
        output = root / label
        run("ab.sh", *binaries, corpus, 2, success=False, PERF_OUT=str(output), PERF_FORCE="1")
        assert not (output / "baseline-first.json").exists()
print("PASS: real Hyperfine quoted same-name binaries, distinct ordering evidence, corpus/stubs/sentinel, count/path refusals, unequal/failed/invalid scans, post-run overload refusal and override warning")
