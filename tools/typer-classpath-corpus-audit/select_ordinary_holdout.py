#!/usr/bin/env python3
"""Select a deterministic, diverse A0 sample from an ordinary-method inventory."""

from __future__ import annotations

import argparse
import csv
import hashlib
from pathlib import Path
import sys


BUCKETS = ((6, 8, 5), (9, 12, 5), (13, 16, 5), (17, 20, 5), (21, 24, 5))
OUTPUT_COLUMNS = (
    "identity",
    "owner_path",
    "method_name",
    "line",
    "rank_score",
    "tier",
    "score_bucket",
    "feature_bucket",
    "tie_break_sha256",
)
SEEDED_OUTPUT_COLUMNS = (*OUTPUT_COLUMNS, "selection_seed")


class HoldoutError(ValueError):
    pass


def inventory_rows(path: Path) -> list[dict[str, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    header_index = next(
        (index for index, line in enumerate(lines) if line.startswith("method\towner_path\t")),
        None,
    )
    if header_index is None:
        raise HoldoutError(f"inventory has no method header: {path}")
    try:
        return list(csv.DictReader(lines[header_index:], delimiter="\t"))
    except csv.Error as error:
        raise HoldoutError(f"cannot parse candidate inventory {path}: {error}") from error


def identities(path: Path) -> set[str]:
    try:
        with path.open(encoding="utf-8", newline="") as stream:
            rows = csv.DictReader(stream, delimiter="\t")
            if not rows.fieldnames or "identity" not in rows.fieldnames:
                raise HoldoutError(f"manifest has no identity column: {path}")
            return {row["identity"] for row in rows if row["identity"]}
    except OSError as error:
        raise HoldoutError(f"cannot read manifest {path}: {error}") from error


def select(
    inventory: Path,
    main_manifest: Path,
    *,
    excluded_manifests: tuple[Path, ...] = (),
    seed: str | None = None,
) -> list[dict[str, str]]:
    excluded = identities(main_manifest)
    for manifest in excluded_manifests:
        excluded.update(identities(manifest))
    rows = inventory_rows(inventory)
    candidates = []
    for row in rows:
        try:
            score = int(row["rank_score"])
        except (KeyError, ValueError) as error:
            raise HoldoutError("candidate row has an invalid rank_score") from error
        identity = row.get("method", "")
        if (
            not identity
            or row.get("tier") != "A0"
            or row.get("parameter_kind") != "ordinary"
            or identity in excluded
            or not 6 <= score <= 24
        ):
            continue
        row = dict(row)
        row["identity"] = identity
        row["_score"] = str(score)
        tie_input = identity if seed is None else f"{seed}\0{identity}"
        row["_tie"] = hashlib.sha256(tie_input.encode("utf-8")).hexdigest()
        candidates.append(row)

    candidates.sort(key=lambda row: (int(row["_score"]), row["_tie"], row["identity"]))
    selected: list[dict[str, str]] = []
    selected_files: set[str] = set()
    selected_owners: set[str] = set()
    for lower, upper, quota in BUCKETS:
        bucket_name = f"A0-{lower:02d}-{upper:02d}"
        count = 0
        for row in candidates:
            score = int(row["_score"])
            source_file = row["identity"].split("#tree=", 1)[0]
            owner = row["owner_path"]
            if not lower <= score <= upper or source_file in selected_files or owner in selected_owners:
                continue
            features = row.get("features", "none")
            row.update(
                method_name=row["method_name"],
                line=row["line"],
                score_bucket=bucket_name,
                feature_bucket=features,
                tie_break_sha256=row["_tie"],
            )
            if seed is not None:
                row["selection_seed"] = seed
            selected.append(row)
            selected_files.add(source_file)
            selected_owners.add(owner)
            count += 1
            if count == quota:
                break
        if count != quota:
            raise HoldoutError(f"bucket {bucket_name} yielded {count} of {quota} unique file/owner rows")
    if len(selected) != sum(quota for _, _, quota in BUCKETS):
        raise HoldoutError("selection did not produce the fixed holdout size")
    return selected


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("main_manifest", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--exclude-manifest", type=Path, action="append", default=[])
    parser.add_argument("--seed", help="seed the static SHA-256 tie-break for a new sample")
    args = parser.parse_args()
    try:
        rows = select(
            args.inventory,
            args.main_manifest,
            excluded_manifests=tuple(args.exclude_manifest),
            seed=args.seed,
        )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("w", encoding="utf-8", newline="") as stream:
            columns = SEEDED_OUTPUT_COLUMNS if args.seed is not None else OUTPUT_COLUMNS
            writer = csv.DictWriter(stream, fieldnames=columns, delimiter="\t", lineterminator="\n")
            writer.writeheader()
            for row in rows:
                writer.writerow({column: row.get(column, "") for column in columns})
    except (OSError, HoldoutError, KeyError) as error:
        print(f"ordinary holdout selection: {error}", file=sys.stderr)
        return 2
    print(f"selected={len(rows)} distinct_files={len({r['identity'].split('#tree=', 1)[0] for r in rows})} distinct_owners={len({r['owner_path'] for r in rows})}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
