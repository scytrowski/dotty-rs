#!/usr/bin/env python3
"""Fail closed when the inherited-member acceptance receipt is incomplete."""

from __future__ import annotations

import argparse
import csv
import re
import sys
from pathlib import Path


FIXTURE = Path("crates/dotty-typer/tests/fixtures/inherited-members-3.9.0")
SOURCE_SHA256 = "f93da74272996204a4c29d07385d3988c811c8084e7fb3352fdb2be00eee26be"
METHODS = ("direct", "withBias", "lifted", "mapped")
TESTS = (
    "explicit_member_selection_controls_are_separate_from_the_four_root_denominator",
    "failed_field_lookup_rolls_back_before_a_valid_same_session_retry",
    "imported_name_ambiguous_with_inherited_immutable_field_rejects_root",
    "inherited_field_rejections_and_mutability_controls_keep_the_root_atomic",
    "inherited_member_baseline_types_four_roots_in_two_independent_sessions",
    "inherited_method_rejections_do_not_create_typed_roots",
    "inherited_single_clause_accepts_multiple_ordinary_parameters",
    "invalid_parent_class_info_does_not_fall_through_or_publish_a_root",
    "local_and_child_fields_keep_precedence_over_an_inherited_parent_val",
    "rejected_lookup_rolls_back_before_a_valid_same_session_retry",
    "renamed_reordered_parent_and_unrelated_owner_keep_canonical_method_identity",
    "wrong_inherited_method_result_does_not_publish_a_typed_root",
)
EXPECTED_HEADER = (
    "owner",
    "method",
    "source_tree",
    "source_span",
    "classinfo_parent",
    "parent_method_owner",
    "parent_field_owner",
    "selected_inherited_target",
    "source_signature",
    "rust_status",
    "rust_result",
    "first_error_chain",
)
POSITIVE_REPORT_HEADER = (
    "source_sha256",
    "method",
    "source_root",
    "source_span",
    "declaring_parent",
    "canonical_member",
    "source_signature",
    "typed_result",
    "closure_env",
    "sessions",
    "normalized_sessions_equal",
)
NEGATIVE_REPORT_HEADER = (
    "case",
    "rust_gate",
    "rust_outcome",
    "scala_fixture",
    "scala_3_9_0_classification",
)
COHORT_REPORT_HEADER = (
    "cohort",
    "identity",
    "baseline_status",
    "integrated_status",
    "protected_success",
    "baseline_two_pass_equal",
    "integrated_two_pass_equal",
)
NEGATIVE_CASES = {
    "missing_inherited_method",
    "wrong_inherited_argument",
    "private_inherited_method",
    "wrong_inherited_method_result",
    "wrong_inherited_field_result",
    "local_and_child_shadowing",
    "same_named_unrelated_class",
    "overloaded_parent_valid_source",
    "generic_parent_valid_source",
    "outer_and_overloaded_member_ambiguity",
    "outer_and_generic_member_ambiguity",
    "broken_or_stale_parent_hierarchy",
    "capture_failure_rollback_retry",
}
NEGATIVE_RECEIPTS = {
    "missing_inherited_method": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "rejected_without_typed_root",
        "NegativeMissingMember.scala",
        "rejected",
    ),
    "wrong_inherited_argument": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "rejected_without_typed_root",
        "NegativeWrongArgument.scala",
        "rejected",
    ),
    "private_inherited_method": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "rejected_without_typed_root",
        "NegativePrivateAccess.scala",
        "rejected",
    ),
    "wrong_inherited_method_result": (
        "wrong_inherited_method_result_does_not_publish_a_typed_root",
        "no_typed_root",
        "NegativeWrongResult.scala",
        "rejected",
    ),
    "wrong_inherited_field_result": (
        "inherited_field_rejections_and_mutability_controls_keep_the_root_atomic",
        "no_typed_root",
        "NegativeWrongFieldResult.scala",
        "rejected",
    ),
    "local_and_child_shadowing": (
        "local_and_child_fields_keep_precedence_over_an_inherited_parent_val",
        "local_or_child_symbol_selected",
        "ShadowedNames.scala",
        "accepted",
    ),
    "same_named_unrelated_class": (
        "inherited_field_rejections_and_mutability_controls_keep_the_root_atomic",
        "no_inherited_owner_fallback",
        "UnrelatedSameSpelling.scala",
        "accepted_control",
    ),
    "overloaded_parent_valid_source": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "OverloadedSelectionDeferred",
        "UnsupportedParentMembers.scala",
        "accepted_but_rust_unsupported",
    ),
    "generic_parent_valid_source": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "OverloadedSelectionDeferred",
        "UnsupportedParentMembers.scala",
        "accepted_but_rust_unsupported",
    ),
    "outer_and_overloaded_member_ambiguity": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "OverloadedSelectionDeferred",
        "NegativeOverloadedOuter.scala",
        "rejected_as_ambiguous",
    ),
    "outer_and_generic_member_ambiguity": (
        "inherited_method_rejections_do_not_create_typed_roots",
        "OverloadedSelectionDeferred",
        "NegativeGenericOuter.scala",
        "rejected_as_ambiguous",
    ),
    "broken_or_stale_parent_hierarchy": (
        "invalid_parent_class_info_does_not_fall_through_or_publish_a_root",
        "rejected_without_typed_root",
        "-",
        "rust_internal_state_no_source_equivalent",
    ),
    "capture_failure_rollback_retry": (
        "lexical_closure_rollback::*",
        "rejected_then_same_session_retry_succeeds",
        "lexical-closure-contract/run",
        "pinned_valid_and_invalid_capture_controls",
    ),
}


class ReceiptError(ValueError):
    """The acceptance evidence is missing, malformed, duplicated, or changed."""


def read_tsv(path: Path) -> tuple[list[str], list[dict[str, str]]]:
    try:
        with path.open(encoding="utf-8", newline="") as source:
            reader = csv.DictReader(source, delimiter="\t")
            if reader.fieldnames is None or len(reader.fieldnames) != len(set(reader.fieldnames)):
                raise ReceiptError(f"{path}: missing or duplicate TSV columns")
            rows = list(reader)
    except OSError as error:
        raise ReceiptError(f"cannot read {path}: {error}") from error
    if any(None in row or any(value is None for value in row.values()) for row in rows):
        raise ReceiptError(f"{path}: malformed TSV row width")
    return list(reader.fieldnames), rows


def validate_positive_receipt(
    header: list[str], rows: list[dict[str, str]], expected: list[dict[str, str]]
) -> None:
    if tuple(header) != EXPECTED_HEADER:
        raise ReceiptError("four-root receipt has a missing, extra, or reordered column")
    methods = [row["method"] for row in rows]
    if methods != list(METHODS):
        raise ReceiptError(f"four-root receipt identities are {methods!r}, expected {list(METHODS)!r}")
    if rows != expected:
        raise ReceiptError("four-root receipt differs from the frozen complete-method identities")


def validate_negative_receipt(header: list[str], rows: list[dict[str, str]]) -> None:
    cases = [row.get("case", "") for row in rows]
    if tuple(header) != NEGATIVE_REPORT_HEADER or set(cases) != NEGATIVE_CASES or len(rows) != len(NEGATIVE_CASES):
        raise ReceiptError("negative receipt must contain the exact unique scoped case matrix")
    for row in rows:
        expected = (
            row["rust_gate"],
            row["rust_outcome"],
            row["scala_fixture"],
            row["scala_3_9_0_classification"],
        )
        if expected != NEGATIVE_RECEIPTS[row["case"]]:
            raise ReceiptError(f"{row['case']}: Rust/Scala negative classification changed")


def validate_inputs(repo_root: Path) -> list[dict[str, str]]:
    source = FIXTURE / "InheritedMembers.scala"
    import hashlib

    if hashlib.sha256((repo_root / source).read_bytes()).hexdigest() != SOURCE_SHA256:
        raise ReceiptError("authentic four-root Scala source changed")
    baseline_path = repo_root / FIXTURE / "four-root-baseline.tsv"
    header, rows = read_tsv(baseline_path)
    if tuple(header) != EXPECTED_HEADER:
        raise ReceiptError("four-root baseline columns changed")
    if len(rows) != 4 or [row["method"] for row in rows] != list(METHODS):
        raise ReceiptError("four-root baseline must contain each frozen Child method once and in order")
    for row in rows:
        if (
            row["owner"] != "inheritancetyper.Child"
            or row["classinfo_parent"] != "inheritancetyper.Parent"
            or row["parent_method_owner"] != "inheritancetyper.Parent.add"
            or row["parent_field_owner"] != "inheritancetyper.Parent.bias"
            or row["rust_status"] != "fully_typed"
            or row["first_error_chain"] != "-"
        ):
            raise ReceiptError(f"{row['method']}: baseline is not a complete canonical inherited root")
    selected = {"direct": "Parent.add", "withBias": "Parent.bias", "lifted": "Parent.add", "mapped": "Parent.bias"}
    results = {"direct": "Int", "withBias": "Int", "lifted": "Function1[Int,Int]", "mapped": "Function1[Int,Int]"}
    signatures = {"direct": "(Int):Int", "withBias": "(Int):Int", "lifted": "():Function1[Int,Int]", "mapped": "():Function1[Int,Int]"}
    for row in rows:
        method = row["method"]
        if row["selected_inherited_target"] != selected[method] or row["rust_result"] != results[method] or row["source_signature"] != signatures[method]:
            raise ReceiptError(f"{method}: canonical target, source signature, or result changed")

    positive_header, positive = read_tsv(repo_root / "docs/typer-inherited-members-e2e-3.9.0.tsv")
    if tuple(positive_header) != POSITIVE_REPORT_HEADER or len(positive) != 4:
        raise ReceiptError("positive report must be a four-row receipt with the exact schema")
    if [row["method"] for row in positive] != list(METHODS):
        raise ReceiptError("positive report has missing, duplicate, or reordered source roots")
    for source_row, report_row in zip(rows, positive, strict=True):
        method = source_row["method"]
        capture = "[Child.this]" if method in {"lifted", "mapped"} else "-"
        if (
            report_row["source_sha256"] != SOURCE_SHA256
            or report_row["source_root"] != source_row["source_tree"]
            or report_row["source_span"] != source_row["source_span"]
            or report_row["declaring_parent"] != "Parent"
            or report_row["canonical_member"] != source_row["selected_inherited_target"]
            or report_row["source_signature"] != source_row["source_signature"]
            or report_row["typed_result"] != source_row["rust_result"]
            or report_row["closure_env"] != capture
            or report_row["sessions"] != "2"
            or report_row["normalized_sessions_equal"] != "true"
        ):
            raise ReceiptError(f"{method}: positive report receipt is incomplete or mismatched")

    negative_header, negative = read_tsv(
        repo_root / "docs/typer-inherited-members-negative-receipt-3.9.0.tsv"
    )
    validate_negative_receipt(negative_header, negative)

    cohort_header, cohorts = read_tsv(
        repo_root / "docs/typer-inherited-members-cohort-receipt-3.9.0.tsv"
    )
    if tuple(cohort_header) != COHORT_REPORT_HEADER or len(cohorts) != 50:
        raise ReceiptError("cohort receipt must contain the exact 50 frozen roots and schema")
    for cohort_name, expected_count in (("main", 16), ("analysis", 15)):
        cohort_rows = [row for row in cohorts if row["cohort"] == cohort_name]
        identities = [row["identity"] for row in cohort_rows]
        protection_rows = read_tsv(
            repo_root
            / f"docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-{cohort_name}.tsv"
        )[1]
        protections = {row["identity"] for row in protection_rows}
        if len(cohort_rows) != 25 or len(set(identities)) != 25:
            raise ReceiptError(f"{cohort_name} cohort receipt must retain 25 unique roots")
        if set(identities) != {row["identity"] for row in read_tsv(
            repo_root
            / ("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv" if cohort_name == "main" else "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv")
        )[1]}:
            raise ReceiptError(f"{cohort_name} cohort receipt identities changed")
        if len(protections) != expected_count:
            raise ReceiptError(f"{cohort_name} protected cohort size changed")
        for row in cohort_rows:
            protected = row["identity"] in protections
            if (
                row["protected_success"] != str(protected).lower()
                or row["baseline_two_pass_equal"] != "true"
                or row["integrated_two_pass_equal"] != "true"
                or (protected and (row["baseline_status"] != "typed" or row["integrated_status"] != "typed"))
            ):
                raise ReceiptError(f"{cohort_name} protected/deterministic root receipt changed")
    return rows


def validate_test_log(text: str, expected_rows: list[dict[str, str]]) -> None:
    passed = re.findall(r"^test (?:inherited_members_baseline::)?(.+?) \.\.\.", text, re.MULTILINE)
    if passed != list(TESTS):
        raise ReceiptError(f"Rust matrix executed {passed!r}; expected exactly {list(TESTS)!r}")
    summaries = re.findall(
        r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"\d+ measured; \d+ filtered out(?:; finished in [0-9.]+s)?$",
        text,
        re.MULTILINE,
    )
    if summaries != [(str(len(TESTS)), "0", "0")]:
        raise ReceiptError(
            f"Rust matrix did not report all {len(TESTS)} tests passed with zero failures or skips"
        )
    header, actual_rows = [], []
    expected_lines = ["\t".join(expected_rows[0].keys())]
    expected_lines.extend("\t".join(row[column] for column in expected_rows[0]) for row in expected_rows)
    for line in text.splitlines():
        marker = line.find("inheritancetyper.Child\t")
        if marker >= 0:
            actual_rows.append(line[marker:])
    expected_receipts = expected_lines[1:]
    if actual_rows != expected_receipts:
        raise ReceiptError("Rust log is missing, duplicating, or mismatching a four-root receipt row")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    inputs = subparsers.add_parser("inputs", help="validate the frozen source and four-root receipt")
    inputs.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    test_log = subparsers.add_parser("test-log", help="reject skipped or incomplete inherited-member tests")
    test_log.add_argument("log", type=Path)
    test_log.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()
    try:
        rows = validate_inputs(args.repo_root)
        if args.command == "test-log":
            validate_test_log(args.log.read_text(encoding="utf-8"), rows)
            print(
                f"inherited-member Rust gate executed all {len(TESTS)} fixed tests "
                "and four exact receipts"
            )
        else:
            print("frozen inherited-member source and four complete Child method receipts verified")
    except (OSError, ReceiptError) as error:
        print(f"Inherited-member gate failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
