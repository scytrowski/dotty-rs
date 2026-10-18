#!/usr/bin/env python3
"""Join focused method outcomes to every frozen row in a historical cohort."""

from __future__ import annotations

import argparse
import csv
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
        if blocker is None or typed not in {"true", "false"}:
            raise ReconciliationError(f"focused method outcome lacks blocker/typed status: {line}")
        if identity in outcomes:
            raise ReconciliationError(f"duplicate focused method outcome: {identity}")
        outcomes[identity] = (blocker, typed)
    return outcomes


def reconcile(cohort: Path, report: Path, cohort_name: str) -> list[dict[str, str]]:
    outcomes = current_outcomes(report)
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
            else:
                identity = f"{frozen[source_column]}#tree={frozen[tree_column]}"
            current = outcomes.get(identity)
            if current is None:
                blocker, typed, status = "MISSING", "unknown", "missing-current-method"
            else:
                blocker, typed = current
                status = "typed" if typed == "true" else "blocker-observed"
            rows.append(
                {
                    **frozen,
                    "selected_method": identity,
                    "focused_cohort": cohort_name,
                    "current_first_blocker": blocker,
                    "current_typed": typed,
                    "reconciliation": status,
                    "frozen_row_number": str(row_number),
                }
            )
    if not rows:
        raise ReconciliationError(f"frozen cohort has no data rows: {cohort}")
    rows.sort(key=lambda row: (row["selected_method"], int(row["frozen_row_number"])))
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
    missing = sum(row["reconciliation"] == "missing-current-method" for row in rows)
    typed = sum(row["current_typed"] == "true" for row in rows)
    print(f"rows={len(rows)} typed={typed} missing={missing}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
