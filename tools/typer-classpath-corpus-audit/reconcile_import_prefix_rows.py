#!/usr/bin/env python3
"""Join frozen #1128 import rows to current observations without losing cohorts."""

import argparse
import csv
import json
from pathlib import Path
import sys

from import_prefix_profile import (
    extract_observations,
    ordered_source_package_candidates,
    read_classpath_evidence,
    source_package_candidates,
)


def tree_index(key: str) -> str:
    return key.rsplit("#tree=", 1)[1]


def same_import_first_segment_failure(baseline_row, current_observation):
    return (
        current_observation is not None
        and current_observation["first_failing_segment"]
        == baseline_row["first_failing_segment"]
    )


def candidate_evidence(row, evidence):
    result = []
    for candidate in ordered_source_package_candidates(row):
        item = dict(candidate)
        if item["path"] == "<root>":
            item["classpath"] = "not_applicable"
            item["classpath_resources"] = []
        else:
            frozen = evidence.get((row["target_key"], item["path"]))
            item["classpath"] = (
                "direct"
                if frozen and frozen["contains_package_direct"] == "True"
                else "descendant"
                if frozen and frozen["descendant_only"] == "True"
                else "absent"
            )
            resources = []
            if frozen:
                resources.extend(frozen["direct_origins"].split(";"))
                resources.extend(frozen["descendant_resources"].split(";"))
            item["classpath_resources"] = sorted(
                set(resource for resource in resources if resource not in ("", "-"))
            )
        result.append(item)
    return result


def reconcile(baseline, evidence, current_evidence, current_rows, reconciliation_rows):
    current_by_import = {
        (row["source_file"], row["method"], row["import_tree"]): row
        for row in current_rows
    }
    current_by_method = {}
    for row in current_rows:
        current_by_method.setdefault((row["source_file"], row["method"]), []).append(row)

    reconciliation_by_key = {}
    for row in reconciliation_rows:
        key = (row["source_file"], row["baseline_method"], row["baseline_import_tree"])
        if key in reconciliation_by_key:
            raise ValueError(f"duplicate current reconciliation key: {key}")
        reconciliation_by_key[key] = row

    header = [
        "stable_key",
        "source_file",
        "source_method_tree",
        "enclosing_attempt_tree",
        "baseline_import_tree",
        "baseline_target_key",
        "baseline_attempted_method",
        "baseline_attribution",
        "original_failed_qualifier",
        "original_first_failing_segment",
        "original_failure_category",
        "original_source_package_registry_evidence",
        "historical_ordered_lexical_candidates",
        "historical_qualified_candidate",
        "current_ordered_lexical_candidates",
        "current_candidate_observation_status",
        "current_import_selectors",
        "current_same_import_first_segment_failure",
        "current_other_failed_imports_in_method",
        "current_import_blocker_relation",
        "current_method_blocker",
        "current_blocker_origin_method",
        "current_selected_package_identity",
        "final_imported_member_outcome",
        "current_method_typed",
        "current_resolver_requests",
    ]
    rows = []
    cohort = []
    for baseline_row in baseline:
        stable_key = "|".join(
            (
                baseline_row["source_file"],
                tree_index(baseline_row["method"]),
                tree_index(baseline_row["blocker_origin"]),
                tree_index(baseline_row["import_tree"]),
            )
        )
        current_key = (
            baseline_row["source_file"],
            baseline_row["method"],
            baseline_row["import_tree"],
        )
        joined = reconciliation_by_key.get(current_key)
        if joined is None:
            raise ValueError(f"frozen row did not join current reconciliation: {stable_key}")

        historical_candidates = candidate_evidence(baseline_row, evidence)
        eligible_paths = source_package_candidates(baseline_row)
        qualified = any(
            item["path"] in eligible_paths and item["classpath"] == "direct"
            for item in historical_candidates
        )
        current_observation = current_by_import.get(current_key)
        current_candidates = (
            candidate_evidence(current_observation, current_evidence)
            if current_observation
            else []
        )
        same_first_segment_failure = same_import_first_segment_failure(
            baseline_row, current_observation
        )
        other_imports = [
            {
                "import_tree": row["import_tree"],
                "first_failing_segment": row["first_failing_segment"],
                "resolver_outcome": row["resolver_outcome"],
            }
            for row in current_by_method.get(current_key[:2], [])
            if row["import_tree"] != baseline_row["import_tree"]
        ]
        if same_first_segment_failure:
            import_relation = "same_original_import_first_segment_failure"
        elif current_observation:
            import_relation = "same_import_different_first_failing_segment"
        elif joined["current_method_blocker"] != "ImportQualifierNotFound":
            import_relation = "current_first_blocker_is_not_import_qualifier_not_found"
        elif other_imports:
            import_relation = "different_import_failure_observed_in_same_method"
        else:
            import_relation = "import_failure_site_not_observed"
        values = {
            "stable_key": stable_key,
            "source_file": baseline_row["source_file"],
            "source_method_tree": tree_index(baseline_row["method"]),
            "enclosing_attempt_tree": tree_index(baseline_row["blocker_origin"]),
            "baseline_import_tree": tree_index(baseline_row["import_tree"]),
            "baseline_target_key": baseline_row["target_key"],
            "baseline_attempted_method": joined["baseline_attempted_method"],
            "baseline_attribution": joined["baseline_attribution"],
            "original_failed_qualifier": baseline_row["qualifier_segments"],
            "original_first_failing_segment": baseline_row["first_failing_segment"],
            "original_failure_category": joined["baseline_evidence_category"],
            "original_source_package_registry_evidence": baseline_row[
                "source_package_registry_lookup"
            ],
            "historical_ordered_lexical_candidates": json.dumps(
                historical_candidates, sort_keys=True, separators=(",", ":")
            ),
            "historical_qualified_candidate": str(qualified).lower(),
            "current_ordered_lexical_candidates": json.dumps(
                current_candidates, sort_keys=True, separators=(",", ":")
            ),
            "current_candidate_observation_status": (
                "observed_at_original_import"
                if same_first_segment_failure
                else "observed_at_original_import_with_different_first_failing_segment"
                if current_observation
                else "original_first_segment_failure_not_observed"
            ),
            "current_import_selectors": joined["current_import_selectors"],
            "current_same_import_first_segment_failure": str(
                same_first_segment_failure
            ).lower(),
            "current_other_failed_imports_in_method": json.dumps(
                other_imports, sort_keys=True, separators=(",", ":")
            ),
            "current_import_blocker_relation": import_relation,
            "current_method_blocker": joined["current_method_blocker"],
            "current_blocker_origin_method": joined["current_blocker_origin"],
            "current_selected_package_identity": joined["current_selected_identity"],
            "final_imported_member_outcome": joined["final_import_identity"],
            "current_method_typed": joined["current_method_typed"],
            "current_resolver_requests": joined["current_resolver_requests"],
        }
        rows.append(values)
        if qualified:
            cohort.append(values)

    if len(rows) != 2394:
        raise ValueError(f"expected 2,394 frozen rows, got {len(rows)}")
    if len(cohort) != 1615:
        raise ValueError(f"expected 1,615 historically qualified rows, got {len(cohort)}")
    return header, rows, cohort


def write_tsv(path: Path, header, rows):
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=header, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--baseline-evidence", type=Path, required=True)
    parser.add_argument("--current-evidence", type=Path, required=True)
    parser.add_argument("--current-profile", type=Path, required=True)
    parser.add_argument("--current-reconciliation", type=Path, required=True)
    parser.add_argument("--rows", type=Path, required=True)
    parser.add_argument("--qualified-cohort", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    args = parser.parse_args()

    baseline = extract_observations(args.baseline.read_text(encoding="utf-8"))
    evidence = read_classpath_evidence(
        args.baseline_evidence.read_text(encoding="utf-8")
    )
    current_evidence = read_classpath_evidence(
        args.current_evidence.read_text(encoding="utf-8")
    )
    current = extract_observations(args.current_profile.read_text(encoding="utf-8"))
    with args.current_reconciliation.open(encoding="utf-8", newline="") as stream:
        reconciliation = list(csv.DictReader(stream, delimiter="\t"))
    header, rows, cohort = reconcile(
        baseline, evidence, current_evidence, current, reconciliation
    )
    write_tsv(args.rows, header, rows)
    write_tsv(args.qualified_cohort, header, cohort)

    family_counts = {}
    family_files = {}
    family_origins = {}
    family_rows = {}
    for row in cohort:
        family = row["baseline_target_key"].split("::", 1)[0]
        family_counts[family] = family_counts.get(family, 0) + 1
        family_files.setdefault(family, set()).add(row["source_file"])
        family_origins.setdefault(family, set()).add(
            f"{row['source_file']}#tree={row['enclosing_attempt_tree']}"
        )
        family_rows.setdefault(family, []).append(row)
    method_blockers = {}
    for row in cohort:
        blocker = row["current_method_blocker"]
        method_blockers[blocker] = method_blockers.get(blocker, 0) + 1
    import_relations = {}
    for row in cohort:
        relation = row["current_import_blocker_relation"]
        import_relations[relation] = import_relations.get(relation, 0) + 1
    other_import_rows = sum(bool(json.loads(row["current_other_failed_imports_in_method"])) for row in cohort)
    unresolved_original = sum(row["current_same_import_first_segment_failure"] == "true" for row in cohort)
    selected_package = sum(row["current_selected_package_identity"] != "-" for row in cohort)
    method_typed = sum(row["current_method_typed"] == "true" for row in cohort)
    family_summary = {
        family: {
            "observations": count,
            "files": len(family_files[family]),
            "blocker_origins": len(family_origins[family]),
        }
        for family, count in sorted(family_counts.items())
    }
    table = [
        "| Target | Frozen rows | Files | Blocker origins | Same original prefix still failing | Other failed import observed in method | Package identity recorded | Fully typed |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for family, family_observations in sorted(family_rows.items()):
        same_prefix_failures = sum(
            row["current_same_import_first_segment_failure"] == "true"
            for row in family_observations
        )
        other_imports = sum(
            bool(json.loads(row["current_other_failed_imports_in_method"]))
            for row in family_observations
        )
        identities = sum(
            row["current_selected_package_identity"] != "-"
            for row in family_observations
        )
        typed = sum(row["current_method_typed"] == "true" for row in family_observations)
        table.append(
            f"| `{family}` | {len(family_observations)} | {len(family_files[family])} | "
            f"{len(family_origins[family])} | {same_prefix_failures} | {other_imports} | "
            f"{identities} | {typed} |"
        )
    summary = [
        "historical_qualified_rows=1615",
        "historical_qualified_keys=5",
        "historical_qualified_files=130",
        "historical_qualified_blocker_origins=584",
        "current_same_original_import_first_segment_failures=" + str(unresolved_original),
        "current_rows_with_other_failed_import_observations_in_method=" + str(other_import_rows),
        "current_import_blocker_relations=" + json.dumps(import_relations, sort_keys=True),
        "current_selected_package_identity_observed=" + str(selected_package),
        "current_fully_typed_methods=" + str(method_typed),
        "current_method_blockers=" + json.dumps(method_blockers, sort_keys=True),
        "historical_qualified_target_list=" + ",".join(sorted(family_counts)),
        "historical_qualified_by_target=" + json.dumps(family_summary, sort_keys=True),
        "",
        *table,
    ]
    args.summary.write_text("\n".join(summary) + "\n", encoding="utf-8")
    print("\n".join(summary), file=sys.stderr)


if __name__ == "__main__":
    main()
