#!/usr/bin/env python3
"""Reconcile frozen ordinary-method cohorts and preserve complete audit causes."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path
import re
import sys


OUTPUT_COLUMNS = (
    "cohort",
    "identity",
    "owner_path",
    "method_name",
    "tier",
    "score_bucket",
    "baseline_status",
    "baseline_cause",
    "baseline_blocker",
    "final_status",
    "final_cause",
    "final_blocker",
    "baseline_result",
    "final_result",
    "disposition",
    "source_tree",
    "baseline_typed_tree_id",
    "final_typed_tree_id",
    "typed_tree_id_changed",
)


class ReconcileError(ValueError):
    pass


def tsv_rows(path: Path) -> list[dict[str, str]]:
    try:
        with path.open(encoding="utf-8", newline="") as stream:
            return list(csv.DictReader(stream, delimiter="\t"))
    except OSError as error:
        raise ReconcileError(f"cannot read {path}: {error}") from error


def audit_rows(path: Path) -> dict[str, dict[str, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    try:
        header = next(
            index for index, line in enumerate(lines)
            if line.startswith("method\towner_path\tmethod_name\tline\trange\tmodifiers\t")
        )
    except StopIteration as error:
        raise ReconcileError(f"ordinary-method header missing from {path}") from error
    rows = list(csv.DictReader(lines[header:], delimiter="\t"))
    by_identity = {row["method"]: row for row in rows}
    if len(by_identity) != len(rows):
        raise ReconcileError(f"duplicate method identity in {path}")
    return by_identity


def blocker_kind(status: str) -> str:
    if status == "typed":
        return "-"
    if not status.startswith("blocked::"):
        return status
    detail = status.removeprefix("blocked::")
    return re.split(r"[\s({:]", detail, maxsplit=1)[0]


def reconcile(
    cohort_name: str,
    manifest: list[dict[str, str]],
    before: dict[str, dict[str, str]],
    after: dict[str, dict[str, str]],
) -> list[dict[str, str]]:
    identities = [row["identity"] for row in manifest]
    if len(identities) != len(set(identities)):
        raise ReconcileError(f"duplicate {cohort_name} manifest identity")
    expected = set(identities)
    if set(before) != expected or set(after) != expected:
        raise ReconcileError(
            f"{cohort_name} report identity mismatch: "
            f"before_missing={len(expected - set(before))}, before_extra={len(set(before) - expected)}, "
            f"after_missing={len(expected - set(after))}, after_extra={len(set(after) - expected)}"
        )

    output = []
    for row in manifest:
        identity = row["identity"]
        old = before[identity]
        new = after[identity]
        old_typed = old["status"] == "typed"
        new_typed = new["status"] == "typed"
        if old_typed and not new_typed:
            disposition = "regressed"
        elif not old_typed and new_typed:
            disposition = "newly_typed"
        elif new_typed:
            disposition = "retained_typed"
        elif blocker_kind(old["status"]) != blocker_kind(new["status"]):
            disposition = "blocker_changed"
        else:
            disposition = "unchanged_blocked"
        expected_source_tree = row.get("source_tree")
        if expected_source_tree and new["source_tree"] != expected_source_tree:
            raise ReconcileError(f"{identity}: source tree changed from frozen {expected_source_tree}")
        if row.get("gate") == "required_typed":
            expected_result = row.get("oracle_result", "")
            if not new_typed or new["actual_result"] != expected_result:
                raise ReconcileError(
                    f"{identity}: required typed method/result changed; "
                    f"expected {expected_result}, got {new['status']} / {new['actual_result']}"
                )
        if disposition == "regressed":
            raise ReconcileError(f"{identity}: previously typed method is blocked in the final report")
        output.append({
            "cohort": cohort_name,
            "identity": identity,
            "owner_path": row.get("owner_path", new.get("owner_path", "")),
            "method_name": row.get("method_name", new.get("method_name", "")),
            "tier": row.get("tier", ""),
            "score_bucket": row.get("score_bucket", ""),
            "baseline_status": "typed" if old_typed else "blocked",
            "baseline_cause": old["status"],
            "baseline_blocker": blocker_kind(old["status"]),
            "final_status": "typed" if new_typed else "blocked",
            "final_cause": new["status"],
            "final_blocker": blocker_kind(new["status"]),
            "baseline_result": old["actual_result"],
            "final_result": new["actual_result"],
            "disposition": disposition,
            "source_tree": new["source_tree"],
            "baseline_typed_tree_id": old["typed_tree"] or "-",
            "final_typed_tree_id": new["typed_tree"] or "-",
            "typed_tree_id_changed": str(old["typed_tree"] != new["typed_tree"]).lower(),
        })
    return output


def write_rows(path: Path, rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=OUTPUT_COLUMNS, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--main-manifest", required=True, type=Path)
    parser.add_argument("--holdout-manifest", required=True, type=Path)
    parser.add_argument("--main-before", required=True, type=Path)
    parser.add_argument("--main-after", required=True, type=Path)
    parser.add_argument("--holdout-before", required=True, type=Path)
    parser.add_argument("--holdout-after", required=True, type=Path)
    parser.add_argument("--third-manifest", type=Path)
    parser.add_argument("--third-before", type=Path)
    parser.add_argument("--third-after", type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        main_manifest = tsv_rows(args.main_manifest)
        holdout_manifest = tsv_rows(args.holdout_manifest)
        if len(main_manifest) != 25 or len(holdout_manifest) != 25:
            raise ReconcileError("both frozen cohorts must contain exactly 25 rows")
        overlap = {row["identity"] for row in main_manifest} & {row["identity"] for row in holdout_manifest}
        if overlap:
            raise ReconcileError(f"main and holdout overlap at {sorted(overlap)[0]}")
        third_arguments = (args.third_manifest, args.third_before, args.third_after)
        if any(value is not None for value in third_arguments) and not all(
            value is not None for value in third_arguments
        ):
            raise ReconcileError("third manifest, before report, and after report must be supplied together")
        third_manifest = tsv_rows(args.third_manifest) if args.third_manifest else None
        if third_manifest is not None:
            if len(third_manifest) != 25:
                raise ReconcileError("third frozen cohort must contain exactly 25 rows")
            identities = {row["identity"] for row in main_manifest + holdout_manifest}
            third_ids = {row["identity"] for row in third_manifest}
            if len(third_ids) != 25 or identities & third_ids:
                raise ReconcileError("third cohort must contain 25 unique identities disjoint from main and holdout")
        main = reconcile("main", main_manifest, audit_rows(args.main_before), audit_rows(args.main_after))
        holdout = reconcile("holdout", holdout_manifest, audit_rows(args.holdout_before), audit_rows(args.holdout_after))
        write_rows(args.output_dir / "main.tsv", main)
        write_rows(args.output_dir / "holdout.tsv", holdout)
        if third_manifest is not None:
            third = reconcile(
                "third",
                third_manifest,
                audit_rows(args.third_before),
                audit_rows(args.third_after),
            )
            write_rows(args.output_dir / "third.tsv", third)
    except (OSError, ReconcileError, KeyError) as error:
        print(f"acceptance cohort reconciliation: {error}", file=sys.stderr)
        return 2
    for name, rows in (("main", main), ("holdout", holdout)):
        counts = {value: sum(row["disposition"] == value for row in rows) for value in ("newly_typed", "retained_typed", "unchanged_blocked", "blocker_changed")}
        typed_before = sum(row["baseline_status"] == "typed" for row in rows)
        typed_after = sum(row["final_status"] == "typed" for row in rows)
        print(f"{name}: {typed_before}/25 -> {typed_after}/25; {counts}")
    if third_manifest is not None:
        counts = {
            value: sum(row["disposition"] == value for row in third)
            for value in ("newly_typed", "retained_typed", "unchanged_blocked", "blocker_changed")
        }
        typed_before = sum(row["baseline_status"] == "typed" for row in third)
        typed_after = sum(row["final_status"] == "typed" for row in third)
        print(f"third: {typed_before}/25 -> {typed_after}/25; {counts}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
