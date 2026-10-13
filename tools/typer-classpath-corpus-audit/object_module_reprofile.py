#!/usr/bin/env python3
"""Join frozen ObjectModuleClassUnavailable rows to current module traces.

The audit report is a stable, normalized text format. This module deliberately
uses source paths and tree indexes rather than compiler arena IDs.
"""

import collections
import csv
import io
import re


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
    "object_identity",
    "module_class_identity",
)
MODULE_OPERATION_HEADER = (
    "operation",
    "source_file",
    "enclosing_attempt",
    "result",
    "result_state",
    "error_category",
    "object_identity",
    "module_class_identity",
)
OUTPUT_HEADER = (
    "baseline_key",
    "source_file",
    "method_tree",
    "enclosing_attempt",
    "attribution",
    "baseline_blocker",
    "current_blocker",
    "object_origin",
    "object_identities",
    "module_class_identities",
    "module_lookup",
    "completion",
    "member_lookup",
    "classification",
    "fully_typed",
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
    origins.discard("")
    return ",".join(sorted(origins)) if origins else "unknown"


def classify(module_lookups, completions, member_lookups, current_blocker):
    """Classify the furthest evidenced module stage for one baseline row."""
    if current_blocker == "method_not_observed":
        return "method_not_observed"
    if current_blocker == "typed":
        return "fully_typed"
    if current_blocker != BASELINE_BLOCKER:
        if current_blocker.startswith("ImportQualifierNotFound"):
            return "earlier_import_package_blocker"
        return "blocker_moved"
    if any(item == "error" for item in module_lookups):
        return "module_lookup_error"
    if any(item in ("unresolved", "resolved_wrong_kind", "resolved_unknown_symbol") for item in module_lookups):
        return "object_missing_expected_module_class"
    if not module_lookups:
        return "module_lookup_unobserved"
    completion_results = [item.split("|", 1)[0] for item in completions]
    completion_states = [item.split("|", 1)[1] for item in completions if "|" in item]
    if any(item in ("error", "unsupported", "invalid_success") for item in completion_results):
        return "module_completion_failed"
    if "completed" not in completion_results:
        return "module_shell_not_completed"
    if "Complete(ClassInfo)" not in completion_states:
        return "module_completion_invalid_class_info"
    if any(item == "success" for item in member_lookups):
        return "module_member_resolved"
    if member_lookups:
        return "module_member_lookup_failed"
    return "module_completed_member_unattempted"


def preserve_rows(frozen_report, current_report):
    frozen = method_attempts(frozen_report)
    current = method_attempts(current_report)
    frozen_rows = [
        (key, row)
        for key, row in frozen.items()
        if row["blocker"] == BASELINE_BLOCKER
    ]
    if len(frozen_rows) != 576:
        raise ValueError(f"expected 576 frozen observations, got {len(frozen_rows)}")
    if len({row["method"].rsplit("/", 1)[0] for _, row in frozen_rows}) != 77:
        raise ValueError("the frozen cohort must span exactly 77 source files")

    old_packages = tsv_section(
        frozen_report, "module_package_term_observations", MODULE_PACKAGE_HEADER
    )
    new_packages = tsv_section(
        current_report, "module_package_term_observations", MODULE_PACKAGE_HEADER
    )
    new_operations = tsv_section(
        current_report, "module_operation_observations", MODULE_OPERATION_HEADER
    )
    old_by_attempt = collections.defaultdict(list)
    new_by_attempt = collections.defaultdict(list)
    ops_by_attempt = collections.defaultdict(list)
    for source in old_packages:
        old_by_attempt[stable_attempt_key(source["enclosing_attempt"])].append(source)
    for source in new_packages:
        new_by_attempt[stable_attempt_key(source["enclosing_attempt"])].append(source)
    for source in new_operations:
        ops_by_attempt[stable_attempt_key(source["enclosing_attempt"])].append(source)

    output = []
    for key, old in sorted(frozen_rows):
        old_attempt = stable_attempt_key(old["attempt"])
        package_rows = old_by_attempt[old_attempt] + new_by_attempt[old_attempt]
        operation_rows = ops_by_attempt[old_attempt]
        current_row = current.get(key)
        current_blocker = current_row["blocker"] if current_row else "method_not_observed"
        method_tree = int(old["tree"])
        attempted_tree = int(old_attempt.rsplit("=", 1)[1]) if "#tree=" in old_attempt else -1
        lookups = [
            row["result"] for row in operation_rows
            if row["operation"] == "module_class_of_object"
        ]
        completions = [
            row["result"] + "|" + row["result_state"] for row in operation_rows
            if row["operation"] == "complete_module_class"
        ]
        member_lookups = [
            row["result"] for row in operation_rows
            if row["operation"] == "resolve_module_member"
        ]
        objects = sorted({row["object_identity"] for row in package_rows + operation_rows if row["object_identity"] not in ("", "-")})
        module_classes = sorted({row["module_class_identity"] for row in package_rows + operation_rows if row["module_class_identity"] not in ("", "-")})
        output.append({
            "baseline_key": key + f"::attempt={old_attempt}",
            "source_file": old["method"],
            "method_tree": old["tree"],
            "enclosing_attempt": old_attempt,
            "attribution": "direct" if method_tree == attempted_tree else "inherited",
            "baseline_blocker": old["blocker"],
            "current_blocker": current_blocker,
            "object_origin": identity_origin(package_rows, operation_rows),
            "object_identities": ";".join(objects) or "unknown",
            "module_class_identities": ";".join(module_classes) or "unknown",
            "module_lookup": ";".join(sorted(set(lookups))) or "unobserved",
            "completion": ";".join(sorted(set(completions))) or "unobserved",
            "member_lookup": ";".join(sorted(set(member_lookups))) or "unobserved",
            "classification": classify(lookups, completions, member_lookups, current_blocker),
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
