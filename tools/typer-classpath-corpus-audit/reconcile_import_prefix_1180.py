#!/usr/bin/env python3
"""Join the frozen 1,063-row #1180 view to a current Rust reconciliation."""

import argparse
import csv
import json
import re
from collections import Counter
from pathlib import Path


FROZEN_COUNTS = {"rows": 1063, "files": 81, "targets": 31, "origins": 393}
PROFILE_COLUMNS = [
    "source_file", "method", "blocker_origin", "import_tree", "line",
    "qualifier_source", "qualifier_segments", "context_owner", "context_package",
    "namespace", "source_lookup", "source_package_registry_lookup", "resolved_prefix",
    "first_failing_segment", "request_kind", "resolver_outcome", "target_key",
]
CURRENT_COLUMNS = [
    "current_import_selectors", "current_outcome", "current_method_blocker",
    "current_enclosing_attempt_blocker",
    "current_method_typed", "current_blocker_origin",
]


def read_tsv(path):
    with path.open(encoding="utf-8", newline="") as stream:
        return list(csv.DictReader(stream, delimiter="\t"))


def tree(key):
    return key.rsplit("#tree=", 1)[1]


def stable_key(source_file, method_tree, origin_tree, import_tree):
    return f"{source_file}|{method_tree}|{origin_tree}|{import_tree}"


def frozen_rows(profile_text):
    marker = "import_prefix_observations:\n"
    if marker not in profile_text:
        raise ValueError("frozen #1180 profile has no import_prefix_observations section")
    lines = profile_text.split(marker, 1)[1].splitlines()
    count = int(lines[0].strip().removeprefix("observations="))
    header = lines[1].strip().split("\t")
    if header != PROFILE_COLUMNS:
        raise ValueError("frozen #1180 observation columns changed")
    rows = []
    for line in lines[2:]:
        if not line.startswith("  "):
            break
        values = line[2:].split("\t")
        if len(values) != len(PROFILE_COLUMNS):
            raise ValueError("malformed frozen #1180 observation row")
        rows.append(dict(zip(PROFILE_COLUMNS, values)))
    if len(rows) != count:
        raise ValueError(f"frozen profile declares {count} rows but contains {len(rows)}")
    return rows


def import_identity(row):
    return (
        row["source_file"],
        tree(row["method"]),
        tree(row["blocker_origin"].split(":", 1)[1]),
        tree(row["import_tree"]),
    )


def current_identity(row):
    if "source_method_tree" in row:
        return (
            row["source_file"],
            row["source_method_tree"],
            row["enclosing_attempt_tree"],
            row["baseline_import_tree"],
        )
    if "method_tree" not in row and "method" in row:
        origin = row["blocker_origin"].split(":", 1)[-1]
        return (
            row["source_file"],
            tree(row["method"]),
            tree(origin),
            tree(row["import_tree"]),
        )
    return (
        row["source_file"], row["method_tree"], row["blocker_origin_tree"], row["import_tree"]
    )


def object_module_index(rows):
    evidence = {}
    for row in rows:
        identities = {
            identity.replace("/", ".")
            for field in ("current_object_identities", "current_module_class_identities")
            for identity in row[field].split(";")
            if identity and identity != "-"
        }
        for identity in identities:
            item = evidence.setdefault(
                identity,
                {
                    "canonical_identity": identity,
                    "profile_rows": 0,
                    "module_class_identities": set(),
                    "classifications": set(),
                    "module_lookups": set(),
                    "completion_outcomes": set(),
                    "member_outcomes": set(),
                    "module_member_attempts": 0,
                    "distinct_module_member_requests": set(),
                },
            )
            item["profile_rows"] += 1
            for column, field in (
                ("current_module_class_identities", "module_class_identities"),
                ("classification", "classifications"),
                ("module_lookup", "module_lookups"),
                ("completion", "completion_outcomes"),
                ("member_lookup", "member_outcomes"),
                ("distinct_module_member_requests", "distinct_module_member_requests"),
            ):
                item[field].update(
                    value.replace("/", ".")
                    for value in row[column].split(";")
                    if value and value != "-"
                )
            try:
                item["module_member_attempts"] += int(row["module_member_attempts"])
            except (KeyError, ValueError):
                pass
    return {
        identity: [
            {
                key: sorted(value) if isinstance(value, set) else value
                for key, value in record.items()
            }
        ]
        for identity, record in sorted(evidence.items())
    }


def reconcile(profile, targets, classpath_evidence, current_rows, object_module_rows=()):
    frozen = frozen_rows(profile)
    if len(frozen) != FROZEN_COUNTS["rows"]:
        raise ValueError(f"expected 1,063 frozen #1180 rows, got {len(frozen)}")
    target_by_key = {row["target_key"]: row for row in targets}
    if len(target_by_key) != FROZEN_COUNTS["targets"]:
        raise ValueError("#1180 target table must contain exactly 31 stable keys")
    evidence_by_key = {}
    for row in classpath_evidence:
        evidence_by_key.setdefault(row["target_key"], []).append(row)
    module_by_object = object_module_index(object_module_rows)

    current_by_identity = {}
    for row in current_rows:
        identity = current_identity(row)
        if identity in current_by_identity:
            raise ValueError(f"duplicate current reconciliation identity: {identity}")
        current_by_identity[identity] = row

    result = []
    seen = set()
    for baseline in frozen:
        identity = import_identity(baseline)
        key = stable_key(*identity)
        if key in seen:
            raise ValueError(f"duplicate frozen #1180 stable key: {key}")
        seen.add(key)
        current = current_by_identity.get(identity)
        if current is None:
            raise ValueError(f"frozen #1180 row did not join current audit: {key}")
        target = target_by_key.get(baseline["target_key"])
        if target is None:
            raise ValueError(f"missing frozen #1180 target metadata: {baseline['target_key']}")
        qualifier_identity_match = re.search(
            r"(?:^|[:;])qualifier=resolved::([^;]+)", current["final_import_identity"]
        )
        qualifier_identity = qualifier_identity_match.group(1) if qualifier_identity_match else "-"
        qualifier_kind = re.search(r"kind=([^:]+)", qualifier_identity)
        qualifier_type = qualifier_kind.group(1) if qualifier_kind else "unresolved"
        qualifier_path = re.search(r"(?:^|::)path=([^:]+)", qualifier_identity)
        object_evidence = (
            module_by_object.get(qualifier_path.group(1), [])
            if qualifier_type == "Object" and qualifier_path
            else []
        )
        row = {
            "stable_key": key,
            "source_method_tree": identity[1],
            "enclosing_attempt_tree": identity[2],
            "baseline_import_tree": identity[3],
            **baseline,
            "target_classification": target["classification"],
            "target_weighted_observations": target["weighted_observations"],
            "target_unique_blocker_origins": str(
                len([origin for origin in target["unique_blocker_origins"].split(";") if origin])
            ),
            "classpath_evidence_target_key": baseline["target_key"],
            "classpath_evidence_rows": str(len(evidence_by_key.get(baseline["target_key"], []))),
            "current_qualifier_identity": qualifier_identity,
            "current_qualifier_kind": qualifier_type,
            "current_object_module_evidence_status": (
                "matched_object_module_evidence"
                if object_evidence
                else "object_not_in_module_profile_cohort"
                if qualifier_type == "Object"
                else "no_object_qualifier"
            ),
            **{column: current[column] for column in CURRENT_COLUMNS},
        }
        result.append(row)

    observed = {
        "rows": len(result),
        "files": len({row["source_file"] for row in result}),
        "targets": len({row["target_key"] for row in result}),
        "origins": len({(row["source_file"], tree(row["blocker_origin"])) for row in result}),
    }
    if observed != FROZEN_COUNTS:
        raise ValueError(f"#1180 frozen cohort identity changed: {observed}")
    return result


def summary(rows):
    blockers = Counter(row["current_method_blocker"] for row in rows)
    outcomes = Counter(row["current_outcome"] for row in rows)
    return [
        "frozen_observations=1063",
        "matched_observations=" + str(len(rows)),
        "distinct_files=81",
        "distinct_target_keys=31",
        "distinct_blocker_origins=393",
        "current_method_blockers=" + json.dumps(blockers, sort_keys=True),
        "current_import_outcomes=" + json.dumps(outcomes, sort_keys=True),
        "current_method_typed=" + str(sum(row["current_method_typed"] == "true" for row in rows)),
        "rows_with_selected_identity=" + str(sum(row["current_qualifier_identity"] != "-" for row in rows)),
        "current_qualifier_kinds=" + json.dumps(Counter(row["current_qualifier_kind"] for row in rows), sort_keys=True),
        "distinct_current_object_qualifiers=" + str(len({row["current_qualifier_identity"] for row in rows if row["current_qualifier_kind"] == "Object"})),
        "rows_with_object_module_evidence=" + str(sum(row["current_object_module_evidence_status"] == "matched_object_module_evidence" for row in rows)),
        "object_module_evidence_is_attempt_only=true",
    ]


def write_tsv(path, rows):
    if not rows:
        raise ValueError("refusing to write an empty reconciliation")
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(
            stream, fieldnames=list(rows[0]), delimiter="\t", lineterminator="\n"
        )
        writer.writeheader()
        writer.writerows(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument("--targets", type=Path, required=True)
    parser.add_argument("--classpath-evidence", type=Path, required=True)
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--object-modules", type=Path)
    parser.add_argument("--rows", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    args = parser.parse_args()
    rows = reconcile(
        args.profile.read_text(encoding="utf-8"),
        read_tsv(args.targets),
        read_tsv(args.classpath_evidence),
        read_tsv(args.current),
        read_tsv(args.object_modules) if args.object_modules else (),
    )
    write_tsv(args.rows, rows)
    args.summary.write_text("\n".join(summary(rows)) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
