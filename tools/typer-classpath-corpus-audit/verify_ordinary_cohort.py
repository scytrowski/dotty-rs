#!/usr/bin/env python3
"""Require frozen baseline-typed ordinary methods to remain fully typed."""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path


def read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def parse_report(path: Path) -> dict[str, dict[str, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    try:
        header_index = lines.index(
            "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
            "expected_result\tcomplexity\tstatus\tattempted_tree"
        )
    except ValueError as error:
        raise ValueError(f"{path}: ordinary audit header is missing") from error
    rows = csv.DictReader(lines[header_index:], delimiter="\t")
    return {row["method"]: row for row in rows}


def verify(cohort_path: Path, report_path: Path) -> list[str]:
    cohort = read_tsv(cohort_path)
    report = parse_report(report_path)
    failures = []
    for row in cohort:
        if row.get("gate") != "required_typed":
            continue
        identity = row["identity"]
        audit = report.get(identity)
        if audit is None:
            failures.append(f"{identity}: missing from ordinary audit report")
            continue
        status = audit["status"]
        if not status.startswith("typed::result="):
            failures.append(f"{identity}: expected fully typed, got {status}")
            continue
        actual_result = status.removeprefix("typed::result=")
        if actual_result != row["oracle_result"]:
            failures.append(
                f"{identity}: result shape changed; expected {row['oracle_result']}, "
                f"got {actual_result}"
            )
    if not any(row.get("gate") == "required_typed" for row in cohort):
        failures.append(f"{cohort_path}: no required_typed rows")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("cohort", type=Path)
    parser.add_argument("report", type=Path)
    arguments = parser.parse_args()
    try:
        failures = verify(arguments.cohort, arguments.report)
    except (OSError, ValueError, KeyError) as error:
        print(error, file=sys.stderr)
        return 2
    if failures:
        print("ordinary cohort gate failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("ordinary cohort gate passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
