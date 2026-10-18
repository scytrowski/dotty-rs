#!/usr/bin/env python3
"""Reconcile frozen cohort rows with current method and resolver observations."""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
import sys


class ReconciliationError(ValueError):
    pass


def current_outcomes(report: Path) -> dict[str, tuple[str, str]]:
    outcomes: dict[str, tuple[str, str]] = {}
    in_methods = False
    for line in report.read_text(encoding="utf-8").splitlines():
        if line == "method_outcomes:":
            in_methods = True
            continue
        if in_methods and not line.startswith("  "):
            in_methods = False
        if not in_methods or not line.startswith("  method="):
            continue
        record = line.strip()[len("method=") :]
        identity, separator, detail = record.partition("::")
        if not separator or "#tree=" not in identity:
            raise ReconciliationError(f"malformed focused method outcome: {line}")
        blocker_marker = "blocker="
        typed_marker = "::typed="
        if blocker_marker not in detail or typed_marker not in detail:
            raise ReconciliationError(f"focused method outcome lacks blocker/typed status: {line}")
        blocker = detail.rsplit(blocker_marker, 1)[1].split(typed_marker, 1)[0]
        typed = detail.rsplit(typed_marker, 1)[1]
        if typed not in {"true", "false"}:
            raise ReconciliationError(f"focused method outcome lacks blocker/typed status: {line}")
        if identity in outcomes:
            raise ReconciliationError(f"duplicate focused method outcome: {identity}")
        outcomes[identity] = (blocker, typed)
    return outcomes


def current_resolver_observations(report: Path) -> list[dict[str, str]]:
    observations: list[dict[str, str]] = []
    in_observations = False
    columns: list[str] | None = None
    for line in report.read_text(encoding="utf-8").splitlines():
        if line == "resolver_observations:":
            in_observations = True
            continue
        if in_observations and not line.startswith("  "):
            in_observations = False
        if not in_observations or not line.startswith("  "):
            continue
        fields = line[2:].split("\t")
        if columns is None:
            columns = fields
            expected = ["attempt", "target", "outcome", "cause", "materialized_prefix", "selected_identity"]
            if columns != expected:
                raise ReconciliationError("resolver observation columns are missing or malformed")
            continue
        if len(fields) != len(columns):
            raise ReconciliationError(f"malformed resolver observation row: {line}")
        observations.append(dict(zip(columns, fields, strict=True)))
    if columns is None:
        raise ReconciliationError("focused report is missing resolver_observations")
    return observations


def target_keys(value: str) -> set[str]:
    return {part.strip() for part in value.split(";") if part.strip()}


def reconcile(cohort: Path, report: Path, cohort_name: str) -> list[dict[str, str]]:
    outcomes = current_outcomes(report)
    observations = current_resolver_observations(report)
    with cohort.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        if reader.fieldnames is None:
            raise ReconciliationError(f"frozen cohort has no header: {cohort}")
        source_column = "file" if cohort_name == "package-term" else "source_file"
        tree_column = "method" if cohort_name == "package-term" else "method_tree"
        blocker_column = "blocker" if cohort_name == "package-term" else "baseline_blocker"
        required = {source_column, tree_column, blocker_column}
        if not required.issubset(reader.fieldnames):
            raise ReconciliationError(f"frozen cohort is missing columns: {sorted(required - set(reader.fieldnames))}")
        rows = []
        for row_number, frozen in enumerate(reader, 2):
            if cohort_name == "package-term":
                identity = frozen[tree_column]
                attempts = {
                    value.split("::", 1)[0]
                    for value in frozen.get("request_attempts", "").split(";")
                    if value
                }
                expected_targets = target_keys(frozen.get("request_targets", ""))
                row_key = "|".join((identity, frozen.get("request_attempts", ""), frozen.get("request_targets", "")))
            else:
                identity = f"{frozen[source_column]}#tree={frozen[tree_column]}"
                attempts = {frozen.get("enclosing_attempt", "").split("::", 1)[0]}
                expected_targets = target_keys(frozen.get("distinct_resolution_targets", ""))
                row_key = frozen.get("baseline_key", "") or "|".join((identity, frozen.get("enclosing_attempt", "")))
            current = outcomes.get(identity)
            if current is None:
                blocker, typed, method_status = "MISSING", "unknown", "missing-current-method"
            else:
                blocker, typed = current
                method_status = "typed" if typed == "true" else "blocker-observed"
            attempt_observations = [observation for observation in observations if observation["attempt"] in attempts]
            matched = [observation for observation in attempt_observations if observation["target"] in expected_targets]
            if matched:
                observation_status = "target-observed"
            elif not attempt_observations:
                observation_status = "missing-current-attempt"
            elif not expected_targets:
                observation_status = "no-frozen-target-key"
            else:
                observation_status = "missing-current-target"
            rows.append(
                {
                    **frozen,
                    "frozen_row_key": row_key,
                    "selected_method": identity,
                    "focused_cohort": cohort_name,
                    "current_first_blocker": blocker,
                    "current_typed": typed,
                    "method_reconciliation": method_status,
                    "current_attempt": ";".join(sorted(attempts)),
                    "current_request_stage": observation_status,
                    "current_request_observations": json.dumps(matched, sort_keys=True, separators=(",", ":")),
                    "frozen_row_number": str(row_number),
                }
            )
    if not rows:
        raise ReconciliationError(f"frozen cohort has no data rows: {cohort}")
    rows.sort(key=lambda row: (row["selected_method"], row["frozen_row_key"], int(row["frozen_row_number"])))
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cohort", choices=("package-term", "object-module"))
    parser.add_argument("frozen", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--rows", type=Path, required=True)
    args = parser.parse_args()
    try:
        rows = reconcile(args.frozen, args.report, args.cohort)
        fields = list(rows[0])
        with args.rows.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=fields, delimiter="\t", lineterminator="\n")
            writer.writeheader()
            writer.writerows(rows)
    except (OSError, ReconciliationError) as error:
        print(f"focused cohort reconciliation: {error}", file=sys.stderr)
        return 2
    missing = sum(row["method_reconciliation"] == "missing-current-method" for row in rows)
    observed = sum(row["current_request_stage"] == "target-observed" for row in rows)
    print(f"rows={len(rows)} target_observed={observed} missing_methods={missing}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
