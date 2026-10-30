#!/usr/bin/env python3
"""Require prior typed roots and, when supplied, the full frozen manifest."""

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
        header_index = next(
            index
            for index, line in enumerate(lines)
            if line.startswith(
                "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
                "expected_result\tactual_result\tcomplexity\tstatus\tsource_tree\ttyped_tree"
            )
        )
    except (StopIteration, ValueError) as error:
        raise ValueError(f"{path}: ordinary audit header is missing") from error
    rows = list(csv.DictReader(lines[header_index:], delimiter="\t"))
    by_identity = {row["method"]: row for row in rows}
    if len(by_identity) != len(rows):
        raise ValueError(f"{path}: duplicate ordinary-method identity")
    return by_identity


def verify(
    cohort_path: Path,
    report_path: Path,
    manifest_path: Path | None = None,
) -> list[str]:
    cohort = read_tsv(cohort_path)
    report = parse_report(report_path)
    failures = []
    required_rows = [row for row in cohort if row.get("gate") == "required_typed"]
    required_ids = [row["identity"] for row in required_rows]
    if len(required_ids) != len(set(required_ids)):
        failures.append(f"{cohort_path}: duplicate required_typed identity")

    if manifest_path is not None:
        manifest = read_tsv(manifest_path)
        manifest_ids = [row["identity"] for row in manifest]
        if len(manifest_ids) != len(set(manifest_ids)):
            failures.append(f"{manifest_path}: duplicate frozen manifest identity")
        expected = set(manifest_ids)
        actual = set(report)
        for identity in sorted(expected - actual):
            failures.append(f"{identity}: missing from ordinary audit report")
        for identity in sorted(actual - expected):
            failures.append(f"{identity}: unexpected in ordinary audit report")

    for row in cohort:
        if row.get("gate") != "required_typed":
            continue
        identity = row["identity"]
        audit = report.get(identity)
        if audit is None:
            failures.append(f"{identity}: missing from ordinary audit report")
            continue
        status = audit["status"]
        if status != "typed" or audit["typed_tree"] in ("", "-"):
            failures.append(f"{identity}: expected fully typed, got {status}")
            continue
        if audit["source_tree"] != row["source_tree"]:
            failures.append(
                f"{identity}: source-tree mapping changed; expected {row['source_tree']}, "
                f"got {audit['source_tree']}"
            )
        # TreeId values are arena-local diagnostics. New successful roots can
        # change allocation order without changing the canonical method-to-tree
        # mapping, so the gate requires an owned typed root but does not compare
        # its numeric ID across sessions.
        actual_result = audit["actual_result"]
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
    parser.add_argument("--manifest", type=Path)
    arguments = parser.parse_args()
    try:
        failures = verify(arguments.cohort, arguments.report, arguments.manifest)
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
