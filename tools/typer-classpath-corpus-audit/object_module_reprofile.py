#!/usr/bin/env python3
"""Join frozen ObjectModuleClassUnavailable rows to current module traces.

The audit report is a stable, normalized text format. This module deliberately
uses source paths and tree indexes rather than compiler arena IDs.
"""

import collections
import csv
import io
import re
import argparse
from pathlib import Path


METHOD_ROW = re.compile(
    r"^  method=(?P<method>.+)#tree=(?P<tree>\d+)::site=(?P<site>.*?)"
    r"::attempt=(?P<attempt>.*?)::blocker=(?P<blocker>.*?)::typed=(?P<typed>true|false)$"
)
BASELINE_BLOCKER = "ObjectModuleClassUnavailable"
METHOD_HEADER = (
    "method",
    "tree",
    "site",
    "attempt",
    "blocker",
    "typed",
)
MODULE_PACKAGE_HEADER = (
    "request_key",
    "source_file",
    "enclosing_attempt",
    "resolution",
    "object_origin",
    "object_identity",
    "module_class_identity",
)
PRE_PARITY_MODULE_PACKAGE_HEADER = (
    "request_key",
    "source_file",
    "enclosing_attempt",
    "resolution",
    "object_identity",
    "module_class_identity",
)
MODULE_OPERATION_HEADER = (
    "operation",
    "source_file",
    "enclosing_attempt",
    "result",
    "result_state",
    "module_state",
    "transaction_state",
    "error_category",
    "object_origin",
    "object_identity",
    "module_class_identity",
)
MODULE_MEMBER_HEADER = (
    "source_file",
    "enclosing_attempt",
    "module_class_identity",
    "request_key",
    "result",
    "result_symbol",
    "module_state_before",
    "module_state_after",
    "transaction_state",
)
OUTPUT_HEADER = (
    "baseline_key",
    "source_file",
    "method_tree",
    "enclosing_attempt",
    "attribution",
    "baseline_blocker",
    "current_blocker",
    "baseline_object_origin",
    "baseline_object_identities",
    "baseline_module_class_identities",
    "baseline_resolution_requests",
    "baseline_request_observations",
    "baseline_distinct_resolution_targets",
    "current_object_origin",
    "current_object_identities",
    "current_module_class_identities",
    "current_resolution_requests",
    "current_request_observations",
    "current_distinct_resolution_targets",
    "module_lookup",
    "completion",
    "member_lookup",
    "member_request_keys",
    "module_member_attempts",
    "distinct_module_member_requests",
    "classification",
    "fully_typed",
)
FROZEN_HEADER = (
    "baseline_key",
    "source_file",
    "method_tree",
    "site",
    "enclosing_attempt",
    "attribution",
    "baseline_blocker",
    "object_origin",
    "object_identities",
    "module_class_identities",
    "resolution_requests",
    "request_observations",
    "distinct_resolution_targets",
)


def report_section(report, name):
    lines = report.splitlines()
    marker = f"{name}:"
    try:
        start = lines.index(marker) + 1
    except ValueError as error:
        raise ValueError(f"missing audit report section: {name}") from error
    result = []
    for line in lines[start:]:
        if line and not line.startswith(" ") and line.endswith(":"):
            break
        result.append(line)
    return result


def tsv_section(report, name, header):
    lines = report_section(report, name)
    expected = "  " + "\t".join(header)
    try:
        start = lines.index(expected) + 1
    except ValueError as error:
        raise ValueError(f"missing {name} table header") from error
    rows = []
    for line in lines[start:]:
        if not line.startswith("  "):
            break
        cells = line[2:].split("\t")
        if len(cells) != len(header):
            raise ValueError(f"invalid {name} row: expected {len(header)} cells")
        rows.append(dict(zip(header, cells)))
    return rows


def method_attempts(report):
    rows = {}
    for line in report_section(report, "local_method_attempts"):
        match = METHOD_ROW.match(line)
        if match is None:
            continue
        row = match.groupdict()
        key = f"{row['method']}#tree={row['tree']}"
        if key in rows and rows[key] != row:
            raise ValueError(f"conflicting method attempt row: {key}")
        rows[key] = row
    return rows


def stable_attempt_key(value):
    """Normalize `path#tree=N::...` and preserve unknown attribution."""
    match = re.match(r"^(.*?#tree=\d+)(?:::.+)?$", value)
    return match.group(1) if match else ""


def identity_origin(observations, operations):
    """Return only origins actually represented by the report, never guessed."""
    origins = {row.get("object_origin", "unknown") for row in observations + operations}
    origins.difference_update(("", "-"))
    if len(origins) > 1:
        origins.discard("unknown")
    return ",".join(sorted(origins)) if origins else "unknown"


def classify(module_lookups, completions, member_lookups, current_blocker, object_origin="unknown"):
    """Classify the furthest evidenced module stage for one baseline row."""
    if current_blocker == "method_not_observed":
        return "method_not_observed"
    if current_blocker == "typed":
        return "fully_typed"
    if current_blocker != BASELINE_BLOCKER and not (module_lookups or completions or member_lookups):
        if current_blocker.startswith("ImportQualifierNotFound"):
            return "earlier_import_package_blocker"
        return "blocker_moved"
    lookup_results = [item.split("|", 1)[0] for item in module_lookups]
    if any(item == "error" for item in lookup_results):
        return "classpath_module_lookup_error" if object_origin.startswith("classpath_") else "module_lookup_error"
    if "unresolved" in lookup_results:
        if object_origin == "source_local":
            return "source_object_missing_module_class_mapping"
        if object_origin == "cross_source":
            return "cross_source_object_identity_unavailable"
        if object_origin.startswith("classpath_"):
            return "classpath_object_module_lookup_returned_none"
        return "module_lookup_returned_none_origin_unknown"
    if any(item in ("resolved_wrong_kind", "resolved_unknown_symbol") for item in lookup_results):
        return "malformed_module_class_identity"
    if not module_lookups:
        return "module_lookup_unobserved"
    completion_parts = [item.split("|", 2) for item in completions]
    committed_completions = [item for item in completion_parts if len(item) < 3 or item[2] != "rolled_back"]
    if not committed_completions and completion_parts:
        return "module_completion_rolled_back"
    completion_results = [
        "completed" if item[0] == "completed_during_member_lookup" else item[0]
        for item in committed_completions
    ]
    completion_states = [item[1] for item in committed_completions if len(item) >= 2]
    if any(item in ("error", "unsupported", "invalid_success") for item in completion_results):
        return "module_completion_failed"
    if "completed" not in completion_results:
        return "module_shell_not_completed"
    if "Complete(ClassInfo)" not in completion_states:
        return "module_completion_invalid_class_info"
    if any(item in ("success", "success|observed") for item in member_lookups):
        return "module_member_resolved"
    if any(item.endswith("|rolled_back") for item in member_lookups):
        return "module_member_lookup_rolled_back"
    if member_lookups:
        return "module_member_lookup_failed"
    return "module_completed_member_unattempted"


def frozen_rows(frozen_report):
    frozen = method_attempts(frozen_report)
    try:
        packages = tsv_section(
            frozen_report, "module_package_term_observations", MODULE_PACKAGE_HEADER
        )
    except ValueError:
        packages = tsv_section(
            frozen_report,
            "module_package_term_observations",
            PRE_PARITY_MODULE_PACKAGE_HEADER,
        )
        for row in packages:
            row["object_origin"] = "unknown"
    packages_by_attempt = collections.defaultdict(list)
    for package in packages:
        packages_by_attempt[stable_attempt_key(package["enclosing_attempt"])].append(package)
    rows = []
    for key, row in sorted(frozen.items()):
        if row["blocker"] != BASELINE_BLOCKER:
            continue
        attempt = stable_attempt_key(row["attempt"])
        matching = packages_by_attempt[attempt]
        tree = int(row["tree"])
        attempt_tree = int(attempt.rsplit("=", 1)[1]) if "#tree=" in attempt else -1
        rows.append({
            "baseline_key": key + f"::attempt={attempt}",
            "source_file": row["method"],
            "method_tree": row["tree"],
            "site": row["site"],
            "enclosing_attempt": attempt,
            "attribution": "direct" if tree == attempt_tree else "inherited",
            "baseline_blocker": row["blocker"],
            "object_origin": identity_origin(matching, []),
            "object_identities": ";".join(sorted({item["object_identity"] for item in matching if item["object_identity"] not in ("", "-")})) or "unknown",
            "module_class_identities": ";".join(sorted({item["module_class_identity"] for item in matching if item["module_class_identity"] not in ("", "-")})) or "unknown",
            "resolution_requests": ";".join(sorted({item["request_key"] for item in matching})) or "unknown",
            "request_observations": str(len(matching)),
            "distinct_resolution_targets": str(len({item["request_key"] for item in matching})),
        })
    if len(rows) != 576:
        raise ValueError(f"expected 576 frozen observations, got {len(rows)}")
    if len({row["source_file"] for row in rows}) != 77:
        raise ValueError("the frozen cohort must span exactly 77 source files")
    return rows


def render_frozen(rows):
    output = io.StringIO(newline="")
    writer = csv.DictWriter(output, FROZEN_HEADER, delimiter="\t", lineterminator="\n")
    writer.writeheader()
    writer.writerows(rows)
    return output.getvalue()


def read_frozen(source):
    if "local_method_attempts:" in source:
        return frozen_rows(source)
    reader = csv.DictReader(io.StringIO(source), delimiter="\t")
    if tuple(reader.fieldnames or ()) != FROZEN_HEADER:
        raise ValueError("frozen row file has an unexpected TSV header")
    rows = list(reader)
    if len(rows) != 576:
        raise ValueError(f"expected 576 frozen observations, got {len(rows)}")
    return rows


def preserve_rows(frozen_report, current_report):
    frozen_rows_ = read_frozen(frozen_report)
    current = method_attempts(current_report)
    new_packages = tsv_section(
        current_report, "module_package_term_observations", MODULE_PACKAGE_HEADER
    )
    new_operations = tsv_section(
        current_report, "module_operation_observations", MODULE_OPERATION_HEADER
    )
    new_members = tsv_section(
        current_report, "module_member_observations", MODULE_MEMBER_HEADER
    )
    new_by_attempt = collections.defaultdict(list)
    ops_by_attempt = collections.defaultdict(list)
    members_by_attempt = collections.defaultdict(list)
    for source in new_packages:
        new_by_attempt[stable_attempt_key(source["enclosing_attempt"])].append(source)
    for source in new_operations:
        ops_by_attempt[stable_attempt_key(source["enclosing_attempt"])].append(source)
    for source in new_members:
        members_by_attempt[stable_attempt_key(source["enclosing_attempt"])].append(source)

    output = []
    for old in frozen_rows_:
        key = old["source_file"] + "#tree=" + old["method_tree"]
        old_attempt = old["enclosing_attempt"]
        package_rows = new_by_attempt[old_attempt]
        operation_rows = ops_by_attempt[old_attempt]
        member_rows = members_by_attempt[old_attempt]
        current_row = current.get(key)
        current_blocker = current_row["blocker"] if current_row else "method_not_observed"
        method_tree = int(old["method_tree"])
        lookups = [
            row["result"] + "|" + row["transaction_state"] for row in operation_rows
            if row["operation"] == "module_class_of_object"
        ]
        completions = [
            row["result"] + "|" + row["module_state"] + "|" + row["transaction_state"] for row in operation_rows
            if row["operation"] == "complete_module_class"
        ]
        completions.extend(
            "completed_during_member_lookup|Complete(ClassInfo)|" + row["transaction_state"]
            for row in member_rows
            if row["module_state_before"] != "Complete(ClassInfo)"
            and row["module_state_after"] == "Complete(ClassInfo)"
        )
        member_lookups = [row["result"] + "|" + row["transaction_state"] for row in member_rows]
        objects = sorted({row["object_identity"] for row in package_rows + operation_rows if row["object_identity"] not in ("", "-")})
        module_classes = sorted({row["module_class_identity"] for row in package_rows + operation_rows if row["module_class_identity"] not in ("", "-")})
        request_observations, distinct_requests = target_counts([
            {"request_keys": [row["request_key"] for row in package_rows]}
        ])
        current_origin = identity_origin(package_rows, operation_rows)
        output.append({
            "baseline_key": old["baseline_key"],
            "source_file": old["source_file"],
            "method_tree": old["method_tree"],
            "enclosing_attempt": old_attempt,
            "attribution": old["attribution"],
            "baseline_blocker": old["baseline_blocker"],
            "current_blocker": current_blocker,
            "baseline_object_origin": old["object_origin"],
            "baseline_object_identities": old["object_identities"],
            "baseline_module_class_identities": old["module_class_identities"],
            "baseline_resolution_requests": old["resolution_requests"],
            "baseline_request_observations": old["request_observations"],
            "baseline_distinct_resolution_targets": old["distinct_resolution_targets"],
            "current_object_origin": current_origin,
            "current_object_identities": ";".join(objects) or "unobserved",
            "current_module_class_identities": ";".join(module_classes) or "unobserved",
            "current_resolution_requests": ";".join(sorted({row["request_key"] for row in package_rows})) or "unobserved",
            "current_request_observations": str(request_observations),
            "current_distinct_resolution_targets": str(distinct_requests),
            "module_lookup": ";".join(sorted(set(lookups))) or "unobserved",
            "completion": ";".join(sorted(set(completions))) or "unobserved",
            "member_lookup": ";".join(sorted(set(member_lookups))) or "unobserved",
            "member_request_keys": ";".join(sorted({row["request_key"] for row in member_rows})) or "unobserved",
            "module_member_attempts": str(len(member_rows)),
            "distinct_module_member_requests": str(len({row["request_key"] for row in member_rows})),
            "classification": classify(
                lookups,
                completions,
                member_lookups,
                current_blocker,
                current_origin,
            ),
            "fully_typed": str(current_blocker == "typed").lower(),
        })
    return output


def render(rows):
    output = io.StringIO(newline="")
    writer = csv.DictWriter(output, OUTPUT_HEADER, delimiter="\t", lineterminator="\n")
    writer.writeheader()
    writer.writerows(rows)
    return output.getvalue()


def target_counts(rows):
    """Count repeated request rows independently from distinct target keys."""
    requests = sum(len(row["request_keys"]) for row in rows)
    targets = {key for row in rows for key in row["request_keys"]}
    return requests, len(targets)


def format_summary(rows):
    groups = collections.defaultdict(list)
    for row in rows:
        groups[row["classification"]].append(row)
    next_slice = {
        "source_object_missing_module_class_mapping": "derive the expected ModuleClass for source-owned Objects and validate ownership",
        "cross_source_object_identity_unavailable": "expose source-owned module identity through the shared source semantic index",
        "classpath_object_module_lookup_returned_none": "fix the audited resolver's missing ModuleClass lookup for validated classpath Objects",
        "classpath_module_lookup_error": "normalize and repair the resolver error at the exact classpath lookup stage",
        "module_lookup_error": "inspect the observed resolver error before selecting an implementation slice",
        "malformed_module_class_identity": "validate the Object/ModuleClass kind, name, and owner pair",
        "module_shell_not_completed": "request completion of the validated ModuleClass on the narrow lookup path",
        "module_completion_failed": "repair only the evidenced TASTy parent/member decode failure",
        "module_completion_invalid_class_info": "repair ClassInfo construction/ownership before member lookup",
        "module_completion_rolled_back": "trace the enclosing transaction and retain attempted versus committed state separately",
        "module_member_lookup_failed": "fix the exact completed-module member lookup failure",
        "module_member_lookup_rolled_back": "inspect the caller rollback before claiming the member result as committed",
        "module_completed_member_unattempted": "add a focused caller path that requests a member from the completed module",
        "module_lookup_unobserved": "instrument the missing resolution edge before proposing semantic work",
        "module_lookup_returned_none_origin_unknown": "capture authoritative Object provenance and module lookup ownership",
        "earlier_import_package_blocker": "keep first-segment/package resolution as a separate, bounded issue",
        "blocker_moved": "inspect the new first blocker as a separate candidate",
        "fully_typed": "no follow-up required for this row",
        "method_not_observed": "restore exact source method/tree attribution before drawing conclusions",
    }
    output = [
        "| Root cause | Rows / files / distinct targets | Representative coordinates | Smallest evidence-backed slice |",
        "| --- | ---: | --- | --- |",
    ]
    for category, items in sorted(groups.items()):
        files = {item["source_file"] for item in items}
        targets = {
            target
            for item in items
            for target in item["current_resolution_requests"].split(";")
            if target and target not in ("unknown", "unobserved")
        }
        examples = []
        for item in sorted(items, key=lambda row: row["baseline_key"]):
            request = item["current_resolution_requests"].split(";", 1)[0]
            if request == "unobserved":
                request = item["baseline_resolution_requests"].split(";", 1)[0]
            examples.append(f"`{item['baseline_key']}` ({request})")
            if len(examples) == 3:
                break
        candidate = next_slice.get(category, "classify the concrete evidence first; do not guess at semantics")
        output.append(
            f"| `{category}` | {len(items)} / {len(files)} / {len(targets)} | {'; '.join(examples)} | {candidate} |"
        )
    blocker_counts = collections.Counter(row["current_blocker"] for row in rows)
    output.extend((
        "",
        "Current first blockers in the frozen cohort: "
        + ", ".join(f"`{key}` {value}" for key, value in sorted(blocker_counts.items()))
        + ".",
    ))
    return "\n".join(output)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("freeze", "compare"))
    parser.add_argument("source", help="frozen baseline report or baseline TSV")
    parser.add_argument("output", help="baseline TSV for freeze; current report for compare")
    parser.add_argument("--rows", help="comparison TSV output path")
    args = parser.parse_args()
    source = Path(args.source).read_text()
    if args.mode == "freeze":
        Path(args.output).write_text(render_frozen(frozen_rows(source)))
        return
    if not args.rows:
        parser.error("compare mode requires --rows OUTPUT.tsv")
    rows = preserve_rows(source, Path(args.output).read_text())
    Path(args.rows).write_text(render(rows))
    print(f"preserved_rows={len(rows)} source_files={len({row['source_file'] for row in rows})}")
    print(format_summary(rows))


if __name__ == "__main__":
    main()
