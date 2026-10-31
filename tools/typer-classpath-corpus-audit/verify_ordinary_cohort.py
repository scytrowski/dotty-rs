#!/usr/bin/env python3
"""Validate every frozen cohort attempt and require protected roots to stay typed."""

from __future__ import annotations

import argparse
import csv
import re
import sys
from pathlib import Path


PINNED_SCALA_REVISION = "777528f19a58e794c9954a42f433373472ec57f8"
REPORT_COLUMNS = (
    "method",
    "owner_path",
    "method_name",
    "line",
    "range",
    "modifiers",
    "expected_result",
    "actual_result",
    "complexity",
    "status",
    "source_tree",
    "typed_tree",
)
SUMMARY_KEYS = {
    "schema",
    "pinned_scala_revision",
    "selected",
    "attempted",
    "typed",
    "blocked",
    "excluded",
    "not_attempted",
}


def read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def _identity_tree(identity: str) -> str | None:
    source_path, separator, tree = identity.rpartition("#tree=")
    if not separator or not source_path or not re.fullmatch(r"(?:0|[1-9][0-9]*)", tree):
        return None
    return tree


def _parse_summary(lines: list[str], header_index: int, path: Path) -> dict[str, int | str]:
    values: dict[str, int | str] = {}
    for line in lines[:header_index]:
        if not line:
            continue
        key, separator, value = line.partition("=")
        if not separator or not key or not value:
            raise ValueError(f"{path}: malformed ordinary audit summary row: {line!r}")
        if key not in SUMMARY_KEYS:
            raise ValueError(f"{path}: unexpected ordinary audit summary key: {key!r}")
        if key in values:
            raise ValueError(f"{path}: duplicate ordinary audit summary key: {key!r}")
        if key in {"schema", "pinned_scala_revision"}:
            values[key] = value
        elif not re.fullmatch(r"(?:0|[1-9][0-9]*)", value):
            raise ValueError(f"{path}: malformed ordinary audit counter {key}={value!r}")
        else:
            values[key] = int(value)
    missing = SUMMARY_KEYS - values.keys()
    if missing:
        raise ValueError(f"{path}: ordinary audit summary is missing {sorted(missing)}")
    if values["schema"] != "1":
        raise ValueError(f"{path}: unsupported ordinary audit schema {values['schema']!r}")
    if values["pinned_scala_revision"] != PINNED_SCALA_REVISION:
        raise ValueError(f"{path}: ordinary audit used an unexpected Scala source revision")
    return values


def parse_report(path: Path) -> tuple[dict[str, dict[str, str]], dict[str, int | str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    try:
        header_index = next(
            index
            for index, line in enumerate(lines)
            if line.split("\t") == list(REPORT_COLUMNS)
        )
    except (StopIteration, ValueError) as error:
        raise ValueError(f"{path}: ordinary audit header is missing") from error
    summary = _parse_summary(lines, header_index, path)
    rows = list(csv.DictReader(lines[header_index:], delimiter="\t"))
    for index, row in enumerate(rows, start=header_index + 2):
        malformed = (
            None in row
            or set(row) != set(REPORT_COLUMNS)
            or any(value is None for value in row.values())
        )
        if malformed:
            raise ValueError(f"{path}:{index}: malformed ordinary audit row width")
    by_identity = {row["method"]: row for row in rows}
    if len(by_identity) != len(rows):
        raise ValueError(f"{path}: duplicate ordinary-method identity")
    return by_identity, summary


def _validate_attempted_rows(
    report: dict[str, dict[str, str]], summary: dict[str, int | str], path: Path
) -> list[str]:
    failures: list[str] = []
    counts = {"typed": 0, "blocked": 0, "excluded": 0, "not_attempted": 0}
    for identity, row in report.items():
        identity_tree = _identity_tree(identity)
        if identity_tree is None:
            failures.append(f"{identity}: malformed method identity")
            continue
        if not row["owner_path"] or not row["method_name"]:
            failures.append(f"{identity}: attempted outcome is missing its method owner or name")
        status = row["status"]
        if status == "typed":
            counts["typed"] += 1
            if not re.fullmatch(r"(?:0|[1-9][0-9]*)", row["typed_tree"]):
                failures.append(f"{identity}: typed row has no valid typed-tree mapping")
            if not row["actual_result"] or row["actual_result"] == "not_available":
                failures.append(f"{identity}: typed row has no result shape")
        elif status.startswith("blocked::") and status.removeprefix("blocked::").strip():
            counts["blocked"] += 1
            if row["typed_tree"] not in ("", "-"):
                failures.append(f"{identity}: blocked row unexpectedly has a typed-tree mapping")
            if row["actual_result"] != "not_available":
                failures.append(f"{identity}: blocked row has an inconsistent result shape")
        else:
            counts["not_attempted"] += 1
            failures.append(
                f"{identity}: expected typed or blocked::* attempted outcome, got {status!r}"
            )
        if row["source_tree"] != identity_tree:
            failures.append(
                f"{identity}: invalid source-tree mapping; identity requires {identity_tree}, "
                f"got {row['source_tree']!r}"
            )

    selected = summary["selected"]
    attempted = summary["attempted"]
    for key, actual in counts.items():
        if summary[key] != actual:
            failures.append(
                f"{path}: {key} counter is {summary[key]}, but report rows contain {actual}"
            )
    if selected != len(report):
        failures.append(
            f"{path}: selected counter is {selected}, but report has {len(report)} rows"
        )
    if attempted != counts["typed"] + counts["blocked"]:
        failures.append(
            f"{path}: attempted counter {attempted} does not equal typed + blocked "
            f"({counts['typed']} + {counts['blocked']})"
        )
    if selected != attempted:
        failures.append(
            f"{path}: selected={selected} but attempted={attempted}; every root must be attempted"
        )
    if summary["excluded"] != 0:
        failures.append(f"{path}: excluded counter must be zero")
    if summary["not_attempted"] != 0:
        failures.append(f"{path}: not_attempted counter must be zero")
    return failures


def verify(
    cohort_path: Path,
    report_path: Path,
    manifest_path: Path | None = None,
) -> list[str]:
    cohort = read_tsv(cohort_path)
    report, summary = parse_report(report_path)
    failures = []
    failures.extend(_validate_attempted_rows(report, summary, report_path))
    required_rows = [row for row in cohort if row.get("gate") == "required_typed"]
    required_ids = [row["identity"] for row in required_rows]
    if len(required_ids) != len(set(required_ids)):
        failures.append(f"{cohort_path}: duplicate required_typed identity")

    if manifest_path is not None:
        manifest = read_tsv(manifest_path)
        manifest_ids = [row["identity"] for row in manifest]
        if len(manifest_ids) != len(set(manifest_ids)):
            failures.append(f"{manifest_path}: duplicate frozen manifest identity")
        for row in manifest:
            identity_tree = _identity_tree(row.get("identity", ""))
            manifest_tree = row.get("source_tree", "")
            if identity_tree is None or (manifest_tree and manifest_tree != identity_tree):
                failures.append(
                    f"{row.get('identity', '<missing identity>')}: "
                    "invalid frozen source-tree mapping"
                )
        expected = set(manifest_ids)
        actual = set(report)
        for identity in sorted(expected - actual):
            failures.append(f"{identity}: missing from ordinary audit report")
        for identity in sorted(actual - expected):
            failures.append(f"{identity}: unexpected in ordinary audit report")
        if summary["selected"] != len(manifest):
            failures.append(
                f"{manifest_path}: selected counter is {summary['selected']}, "
                f"expected {len(manifest)} frozen roots"
            )

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
