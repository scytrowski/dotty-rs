#!/usr/bin/env python3
"""Join the frozen #1342 TASTy roots to current parent and source outcomes."""

from __future__ import annotations

import argparse
import csv
import re
from collections import defaultdict
from pathlib import Path


ROOT_PATTERN = re.compile(r"(?:^|::)requested=([^:]+)")


def read_tsv(path: Path) -> tuple[list[str], list[dict[str, str]]]:
    with path.open(newline="", encoding="utf-8") as source:
        reader = csv.DictReader(source, delimiter="\t")
        if reader.fieldnames is None:
            raise ValueError(f"missing TSV header: {path}")
        rows = list(reader)
        if any(None in row or any(value is None for value in row.values()) for row in rows):
            raise ValueError(f"row field count does not match header: {path}")
        return reader.fieldnames, rows


def reconcile(baseline_path: Path, historical_parent_path: Path, parent_path: Path, provenance_path: Path):
    baseline_headers, baseline_rows = read_tsv(baseline_path)
    historical_headers, historical_rows = read_tsv(historical_parent_path)
    parent_headers, parent_rows = read_tsv(parent_path)
    provenance_headers, provenance_rows = read_tsv(provenance_path)
    required = {
        baseline_path: {"requested_class_root", "weighted_observations", "resource_path", "artifact", "artifact_version", "artifact_sha256"},
        historical_parent_path: {"requested_class_root", "parent_index", "resolution_stage", "resolved_binary_name", "standalone_decode_parent_index", "standalone_decode_cause"},
        parent_path: {"requested_class_root", "parent_index", "shape", "resolution_stage", "resolved_binary_name", "deepest_typed_cause", "failing_binary_name"},
        provenance_path: {"root_cause_key", "source_file", "observations"},
    }
    for path, columns in required.items():
        headers = (baseline_headers if path == baseline_path else historical_headers if path == historical_parent_path
                   else parent_headers if path == parent_path else provenance_headers)
        missing = columns - set(headers)
        if missing:
            raise ValueError(f"{path} missing columns: {', '.join(sorted(missing))}")

    baseline: dict[str, dict[str, str]] = {}
    for row in baseline_rows:
        root = row["requested_class_root"]
        if root in baseline:
            raise ValueError(f"duplicate frozen root: {root}")
        int(row["weighted_observations"])
        baseline[root] = row
    if len(baseline) != 50:
        raise ValueError(f"expected 50 frozen roots, found {len(baseline)}")

    historical_parents: dict[str, dict[int, dict[str, str]]] = defaultdict(dict)
    for row in historical_rows:
        root = row["requested_class_root"]
        if root not in baseline:
            raise ValueError(f"historical parent profile contains non-frozen root: {root}")
        index = int(row["parent_index"])
        if index in historical_parents[root]:
            raise ValueError(f"duplicate historical parent index for {root}: {index}")
        historical_parents[root][index] = row
    if set(historical_parents) != set(baseline):
        missing = sorted(set(baseline) - set(historical_parents))
        raise ValueError(f"historical parent profile dropped frozen roots: {', '.join(missing)}")

    parents: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in parent_rows:
        root = row["requested_class_root"]
        if root not in baseline:
            raise ValueError(f"current parent profile contains non-frozen root: {root}")
        parents[root].append(row)
    if set(parents) != set(baseline):
        missing = sorted(set(baseline) - set(parents))
        raise ValueError(f"current parent profile dropped frozen roots: {', '.join(missing)}")
    for root, rows in parents.items():
        indexes = [int(row["parent_index"]) for row in rows]
        if len(indexes) != len(set(indexes)):
            raise ValueError(f"duplicate parent index for {root}")
        rows.sort(key=lambda row: int(row["parent_index"]))

    origins: dict[str, dict[str, int]] = defaultdict(dict)
    files: dict[str, set[str]] = defaultdict(set)
    current_weights: dict[str, int] = defaultdict(int)
    new_roots: set[str] = set()
    for row in provenance_rows:
        match = ROOT_PATTERN.search(row["root_cause_key"])
        if match is None:
            raise ValueError(f"cannot extract requested root from key: {row['root_cause_key']}")
        root = match.group(1)
        observations = int(row["observations"])
        current_weights[root] += observations
        files[root].add(row["source_file"])
        origins[root][row["root_cause_key"]] = origins[root].get(row["root_cause_key"], 0) + observations
        if root not in baseline:
            new_roots.add(root)

    header = [
        "requested_class_root", "requested_identity", "historical_weighted_observations",
        "historical_resource_path", "historical_artifact", "historical_artifact_version",
        "historical_artifact_sha256", "current_source_audit_observations",
        "current_source_audit_files", "current_error_root_origins", "parent_index",
        "original_parent_shape", "original_resolution_stage", "original_resolved_binary_name",
        "original_unresolved_parent_index", "original_unresolved_cause", "parent_shape",
        "resolution_stage", "resolved_binary_name", "hierarchy_load_cause",
        "hierarchy_failing_binary_name", "current_parent_reference_tag", "current_parent_reference_name",
        "current_parent_reference_prefix", "current_parent_raw_tree",
    ]
    output: list[list[str]] = [header]
    for root in sorted(baseline):
        old = baseline[root]
        root_parents = parents[root]
        for index, parent in enumerate(root_parents):
            original = historical_parents.get(root, {}).get(int(parent["parent_index"]), {})
            output.append([
                root,
                "ModuleClass" if root.endswith("$") else "Class",
                old["weighted_observations"], old["resource_path"], old["artifact"],
                old["artifact_version"], old["artifact_sha256"],
                str(current_weights.get(root, 0)),
                ";".join(sorted(files.get(root, set()))),
                ";".join(f"{key}={value}" for key, value in sorted(origins.get(root, {}).items())),
                parent["parent_index"], original.get("shape", ""),
                original.get("resolution_stage", ""), original.get("resolved_binary_name", ""),
                original.get("standalone_decode_parent_index", ""),
                original.get("standalone_decode_cause", ""), parent["shape"], parent["resolution_stage"],
                parent["resolved_binary_name"], parent["deepest_typed_cause"],
                parent["failing_binary_name"], parent.get("reference_tag", ""),
                parent.get("reference_name", ""), parent.get("reference_prefix", ""),
                parent.get("raw_tree", ""),
            ])
    for root in sorted(new_roots):
        for origin, weight in sorted(origins[root].items()):
            output.append([
                root, "new-current-root", "", "", "", "", "", str(weight),
                ";".join(sorted(files[root])), f"{origin}={weight}",
                "", "", "", "", "", "", "not-profiled-frozen-root", "", "", "", "", "", "", "",
            ])

    baseline_weight = sum(int(row["weighted_observations"]) for row in baseline.values())
    provenance_weight = sum(current_weights.values())
    frozen_provenance_weight = sum(current_weights.get(root, 0) for root in baseline)
    resolved_original_parents = 0
    still_unresolved_original_parents = 0
    for root, old_rows in historical_parents.items():
        failed_indices = {
            int(row["standalone_decode_parent_index"])
            for row in old_rows.values()
            if row["standalone_decode_parent_index"] not in ("", "-")
        }
        for index in failed_indices:
            current = next((row for row in parents[root] if int(row["parent_index"]) == index), None)
            if current is None:
                raise ValueError(f"current profile is missing original failed parent {root}[{index}]")
            if current["resolved_binary_name"]:
                resolved_original_parents += 1
            else:
                still_unresolved_original_parents += 1
    summary = [
        f"frozen_roots={len(baseline)}",
        f"frozen_weighted_observations={baseline_weight}",
        f"current_frozen_root_source_observations={frozen_provenance_weight}",
        f"historical_to_current_weight_delta={baseline_weight - frozen_provenance_weight}",
        f"exact_original_parent_references_resolved={resolved_original_parents}",
        f"exact_original_parent_references_still_unresolved={still_unresolved_original_parents}",
        f"current_source_observations_all_roots={provenance_weight}",
        f"current_distinct_source_roots={len(current_weights)}",
        f"new_current_roots={len(new_roots)}",
        f"new_current_root_names={';'.join(sorted(new_roots))}",
        f"frozen_roots_with_zero_current_source_observations={sum(1 for root in baseline if not current_weights.get(root, 0))}",
        f"historical_roots_fully_resolved={sum(1 for root, rows in parents.items() if all(row['deepest_typed_cause'] == 'none' for row in rows))}",
        f"historical_roots_where_original_failed_parent_resolved={resolved_original_parents}",
        f"provenance_files={len(set().union(*files.values())) if files else 0}",
    ]
    return output, summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--historical-parent-profile", type=Path, required=True)
    parser.add_argument("--parent-profile", type=Path, required=True)
    parser.add_argument("--source-provenance", type=Path, required=True)
    parser.add_argument("--rows", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    args = parser.parse_args()
    rows, summary = reconcile(args.baseline, args.historical_parent_profile, args.parent_profile, args.source_provenance)
    args.rows.parent.mkdir(parents=True, exist_ok=True)
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    with args.rows.open("w", newline="", encoding="utf-8") as output_file:
        csv.writer(output_file, delimiter="\t", lineterminator="\n").writerows(rows)
    args.summary.write_text("\n".join(summary) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
