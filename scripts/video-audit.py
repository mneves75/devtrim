#!/usr/bin/env python3
"""Fail the video's npm audit on any advisory not waived, in date, in video/audit-waivers.json.

usage: scripts/video-audit.py [--report <npm-audit.json>] [--waivers <file>] [--today YYYY-MM-DD]

Without --report it runs `npm audit --package-lock-only --json` in video/. Every
advisory the report names, at any severity, must match a waiver whose expiry
(a UTC date, inclusive) has not passed. An expired, malformed, repeated or
stale waiver (one whose advisory the report no longer names) fails too, so a
waiver cannot outlive its reason. --report, --waivers and --today exist for
scripts/tests/video-audit-controls.py.
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

VIDEO = Path(__file__).resolve().parents[1] / "video"
GHSA = re.compile(r"GHSA(?:-[23456789cfghjmpqrvwx]{4}){3}")
ADVISORY_URL = re.compile(r"https://github\.com/advisories/(" + GHSA.pattern + r")")
DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
WAIVER_KEYS = {"id", "expires", "reason"}
SEVERITIES = ("info", "low", "moderate", "high", "critical")


class Refused(Exception):
    pass


def strict_json(text: str, what: str) -> object:
    def no_repeats(pairs: list) -> dict:
        keys = [key for key, _ in pairs]
        if len(keys) != len(set(keys)):
            raise Refused(f"{what} repeats a key")
        return dict(pairs)

    try:
        return json.loads(text, object_pairs_hook=no_repeats)
    except json.JSONDecodeError as error:
        raise Refused(f"{what} is not JSON: {error}") from None


def parse_date(text: object, what: str) -> datetime.date:
    if not isinstance(text, str) or not DATE.fullmatch(text):
        raise Refused(f"{what} must be a YYYY-MM-DD date")
    try:
        return datetime.date.fromisoformat(text)
    except ValueError:
        raise Refused(f"{what} {text} is not a calendar date") from None


def advisories(report: object) -> set[str]:
    if not isinstance(report, dict):
        raise Refused("the audit report is not a JSON object")
    if "error" in report:
        raise Refused(f"npm audit failed: {json.dumps(report['error'])}")
    if report.get("auditReportVersion") != 2 or not isinstance(report.get("vulnerabilities"), dict):
        raise Refused("the audit report lacks auditReportVersion 2 and its vulnerabilities")
    vulnerabilities = report["vulnerabilities"]
    # npm counts every entry by severity; a report missing entries it counted is partial.
    tally = {severity: 0 for severity in SEVERITIES}
    for name, vulnerability in vulnerabilities.items():
        severity = vulnerability.get("severity") if isinstance(vulnerability, dict) else None
        if severity not in tally:
            raise Refused(f"{name}: the report gives no known severity")
        tally[severity] += 1
    counted = report.get("metadata", {}).get("vulnerabilities") if isinstance(report.get("metadata"), dict) else None
    if counted != {**tally, "total": len(vulnerabilities)}:
        raise Refused(f"the report's metadata counts {counted}, not the {len(vulnerabilities)} entries it lists")
    found = set()
    resolved = set()
    paths = {}
    for name, vulnerability in vulnerabilities.items():
        via = vulnerability.get("via")
        if not isinstance(via, list) or not via:
            raise Refused(f"{name}: the report gives no advisory or dependency path")
        paths[name] = []
        for entry in via:
            if isinstance(entry, str):
                # A dependency path: the package it names must carry an advisory.
                if entry not in vulnerabilities:
                    raise Refused(f"{name}: depends on {entry}, which is not in the report")
                paths[name].append(entry)
                continue
            match = ADVISORY_URL.fullmatch(entry.get("url", "")) if isinstance(entry, dict) else None
            if match is None:
                raise Refused(f"{name}: an advisory without a GitHub advisory URL cannot be waived by ID")
            found.add(match.group(1))
            resolved.add(name)
    # Every entry must reach a concrete advisory; a cycle of paths reaches none.
    changed = True
    while changed:
        changed = False
        for name, targets in paths.items():
            if name not in resolved and any(target in resolved for target in targets):
                resolved.add(name)
                changed = True
    unresolved = sorted(set(vulnerabilities) - resolved)
    if unresolved:
        raise Refused(f"{', '.join(unresolved)}: reaches no advisory through its dependency paths")
    return found


def waivers(text: str, today: datetime.date) -> dict[str, datetime.date]:
    listed = strict_json(text, "the waiver file")
    if not isinstance(listed, list):
        raise Refused("the waiver file must be a JSON array")
    by_id: dict[str, datetime.date] = {}
    for waiver in listed:
        if not isinstance(waiver, dict) or set(waiver) != WAIVER_KEYS:
            raise Refused(f"a waiver must have exactly the keys {sorted(WAIVER_KEYS)}")
        advisory = waiver["id"]
        if not isinstance(advisory, str) or not GHSA.fullmatch(advisory):
            raise Refused(f"waiver id {advisory!r} is not a GHSA identifier")
        if not isinstance(waiver["reason"], str) or not waiver["reason"].strip():
            raise Refused(f"{advisory}: the waiver needs a reason")
        expires = parse_date(waiver["expires"], f"{advisory}: expires")
        if advisory in by_id:
            raise Refused(f"{advisory} is waived twice")
        if today > expires:
            raise Refused(f"{advisory}: the waiver expired on {expires}")
        by_id[advisory] = expires
    return by_id


def npm_audit() -> str:
    result = subprocess.run(
        ["npm", "audit", "--package-lock-only", "--json"],
        cwd=VIDEO,
        capture_output=True,
        text=True,
        timeout=300,
    )
    # npm exits 1 when it finds advisories; anything else is a failed audit.
    if result.returncode not in (0, 1):
        raise Refused(f"npm audit exited {result.returncode}: {result.stderr.strip()}")
    return result.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.strip().splitlines()[0])
    parser.add_argument("--report")
    parser.add_argument("--waivers", default=str(VIDEO / "audit-waivers.json"))
    parser.add_argument("--today")
    options = parser.parse_args()
    try:
        today = (
            parse_date(options.today, "--today")
            if options.today
            else datetime.datetime.now(datetime.timezone.utc).date()
        )
        text = Path(options.report).read_text(encoding="utf-8") if options.report else npm_audit()
        found = advisories(strict_json(text, "the audit report"))
        waived = waivers(Path(options.waivers).read_text(encoding="utf-8"), today)
        problems = [f"{advisory} is reported and not waived" for advisory in sorted(found - set(waived))]
        problems += [
            f"{advisory} is waived but no longer reported; remove its waiver"
            for advisory in sorted(set(waived) - found)
        ]
        if problems:
            raise Refused("; ".join(problems))
    except Refused as refusal:
        print(f"video-audit: {refusal}", file=sys.stderr)
        return 1
    if not found:
        print("video-audit: no advisories")
    else:
        print("video-audit: " + ", ".join(f"{advisory} waived until {waived[advisory]}" for advisory in sorted(found)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
