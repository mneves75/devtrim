#!/usr/bin/env python3
"""Check paint-control validation with complete and stale synthetic reports."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CONTROLS = ROOT / "scripts/tests/pageload-controls.py"
NODE = """#!/usr/bin/env python3
import json
import os
import sys

if sys.argv[2].endswith('missing-chrome') or sys.argv[2] == '/usr/bin/true':
    sys.exit(2)
page = sys.argv[-1]
if page in ('broken.html', 'csp.html', 'blank.html'):
    sys.exit(2)
candidate = {'latepaint.html': 'late-target', 'largerpaint.html': 'larger-target'}.get(page)
if page == os.environ.get('DEVTRIM_STALE_PAINT_CONTROL'):
    candidate = 'initial-target'
lcp = {'slow.html': 450, 'latepaint.html': 650, 'largerpaint.html': 250}.get(page, 50)
sample = {'load': 25, 'lcp': lcp, 'lcp_element_id': candidate}
print(json.dumps([{'samples': [sample] * 3}] * 2))
sys.exit(0 if page == 'good.html' else 1)
"""


class PaintControlValidation(unittest.TestCase):
    def run_controls(self, stale_page=None):
        (ROOT / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="paint-control-", dir=ROOT / "target") as folder:
            fixture = Path(folder)
            node = fixture / "node"
            node.write_text(NODE)
            node.chmod(0o755)
            environment = dict(
                os.environ,
                PATH=str(fixture) + os.pathsep + os.environ.get("PATH", ""),
                TMPDIR=str(fixture),
            )
            if stale_page:
                environment["DEVTRIM_STALE_PAINT_CONTROL"] = stale_page
            else:
                environment.pop("DEVTRIM_STALE_PAINT_CONTROL", None)
            return subprocess.run(
                [sys.executable, "-B", str(CONTROLS), "/fixture/chrome"],
                env=environment, capture_output=True, text=True, timeout=30,
            )

    def test_complete_paint_reports_pass(self):
        result = self.run_controls()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_late_initial_candidate_never_proves_delayed_paint(self):
        for page in ("latepaint.html", "largerpaint.html"):
            with self.subTest(page=page):
                result = self.run_controls(page)
                self.assertEqual(
                    result.returncode, 1,
                    "PV pageload/control-candidate: a stale candidate satisfied the paint control\n"
                    + result.stdout + result.stderr,
                )
                self.assertIn("FAIL " + page, result.stdout)


if __name__ == "__main__":
    unittest.main()
