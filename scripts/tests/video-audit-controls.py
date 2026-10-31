#!/usr/bin/env python3
"""Prove scripts/video-audit.py passes only a report whose every advisory is waived in date.

Each case feeds a planted `npm audit --json` report and waiver file through
--report, --waivers and --today, so no network or npm is involved. The clean
report and the in-date waiver are the positive controls; every other case must
fail with its own reason.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "video-audit.py"
GHSA = "GHSA-g3hv-75wg-m249"
OTHER = "GHSA-c2v8-3x9q-wp4h"


def advisory(name: str, ghsa: str | None) -> dict:
    entry = {"source": 1, "name": name, "title": "planted", "severity": "moderate", "range": "<11.0.1"}
    if ghsa:
        entry["url"] = f"https://github.com/advisories/{ghsa}"
    return entry


SEVERITIES = ("info", "low", "moderate", "high", "critical")


def counts(vulnerabilities: dict) -> dict:
    tally = {severity: 0 for severity in SEVERITIES}
    for vulnerability in vulnerabilities.values():
        tally[vulnerability["severity"]] += 1
    return {**tally, "total": len(vulnerabilities)}


def report(vulnerabilities: dict, metadata: dict | None = None) -> dict:
    metadata = {"vulnerabilities": counts(vulnerabilities)} if metadata is None else metadata
    return {"auditReportVersion": 2, "vulnerabilities": vulnerabilities, "metadata": metadata}


MINIMATCH = {
    "minimatch": {"name": "minimatch", "severity": "moderate", "via": [advisory("minimatch", GHSA)]},
    "eslint": {"name": "eslint", "severity": "moderate", "via": ["minimatch"]},
}
WAIVER = {"id": GHSA, "expires": "2026-11-30", "reason": "lint-only; no fix in eslint's range"}

# name, report (dict, or raw text), waivers (list, or raw text), today, expected exit, expected text
CASES = [
    ("clean", report({}), [], "2026-10-31", 0, "no advisories"),
    ("waived", report(MINIMATCH), [WAIVER], "2026-10-31", 0, GHSA),
    ("waived-last-day", report(MINIMATCH), [WAIVER], "2026-11-30", 0, GHSA),
    ("unwaived", report(MINIMATCH), [], "2026-10-31", 1, "not waived"),
    (
        "second-advisory",
        report({**MINIMATCH, "zod": {"name": "zod", "severity": "low", "via": [advisory("zod", OTHER)]}}),
        [WAIVER],
        "2026-10-31",
        1,
        OTHER,
    ),
    ("expired", report(MINIMATCH), [WAIVER], "2026-12-01", 1, "expired"),
    ("stale", report({}), [WAIVER], "2026-10-31", 1, "no longer reported"),
    ("no-ghsa-url", report({"x": {"name": "x", "severity": "low", "via": [advisory("x", None)]}}), [], "2026-10-31", 1, "GitHub advisory"),
    (
        "dangling-via",
        report({"eslint": {"name": "eslint", "severity": "moderate", "via": ["minimatch"]}}),
        [],
        "2026-10-31",
        1,
        "not in the report",
    ),
    ("bad-date", report(MINIMATCH), [{**WAIVER, "expires": "2026-02-30"}], "2026-10-31", 1, "expires"),
    ("extra-key", report(MINIMATCH), [{**WAIVER, "owner": "x"}], "2026-10-31", 1, "keys"),
    ("blank-reason", report(MINIMATCH), [{**WAIVER, "reason": " "}], "2026-10-31", 1, "reason"),
    ("duplicate-waiver", report(MINIMATCH), [WAIVER, WAIVER], "2026-10-31", 1, "twice"),
    ("duplicate-key", report(MINIMATCH), '[{"id": "%s", "id": "%s", "expires": "2026-11-30", "reason": "r"}]' % (GHSA, GHSA), "2026-10-31", 1, "repeats"),
    (
        "unresolved-cycle",
        report({**MINIMATCH, "x": {"name": "x", "severity": "high", "via": ["y"]}, "y": {"name": "y", "severity": "high", "via": ["x"]}}),
        [WAIVER],
        "2026-10-31",
        1,
        "reaches no advisory",
    ),
    (
        "self-reference",
        report({**MINIMATCH, "x": {"name": "x", "severity": "high", "via": ["x"]}}),
        [WAIVER],
        "2026-10-31",
        1,
        "reaches no advisory",
    ),
    (
        "partial-report",
        report(MINIMATCH, {"vulnerabilities": {**counts(MINIMATCH), "high": 1, "total": 3}}),
        [WAIVER],
        "2026-10-31",
        1,
        "metadata",
    ),
    (
        "severity-miscount",
        report(MINIMATCH, {"vulnerabilities": {**counts(MINIMATCH), "moderate": 1, "high": 1}}),
        [WAIVER],
        "2026-10-31",
        1,
        "metadata",
    ),
    ("no-metadata", report(MINIMATCH, {}), [WAIVER], "2026-10-31", 1, "metadata"),
    ("old-format", {"vulnerabilities": {}}, [], "2026-10-31", 1, "auditReportVersion"),
    ("npm-error", {"error": {"code": "ENOLOCK", "summary": "no lockfile"}}, [], "2026-10-31", 1, "ENOLOCK"),
    ("not-json", report({}), "[", "2026-10-31", 1, "JSON"),
]


def main() -> int:
    failures = []
    with tempfile.TemporaryDirectory() as scratch:
        for name, planted_report, waivers, today, expected, text in CASES:
            report_path = Path(scratch, f"{name}-report.json")
            waivers_path = Path(scratch, f"{name}-waivers.json")
            report_path.write_text(json.dumps(planted_report), encoding="utf-8")
            waivers_path.write_text(waivers if isinstance(waivers, str) else json.dumps(waivers), encoding="utf-8")
            result = subprocess.run(
                [sys.executable, "-B", str(SCRIPT), "--report", str(report_path), "--waivers", str(waivers_path), "--today", today],
                capture_output=True,
                text=True,
            )
            output = result.stdout + result.stderr
            if result.returncode != expected or text not in output:
                failures.append(f"{name}: exit {result.returncode} (want {expected}), output lacks {text!r}:\n{output}")
    for failure in failures:
        print(f"video-audit-controls: FAIL {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"video-audit-controls: {len(CASES)} cases behave as planted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
