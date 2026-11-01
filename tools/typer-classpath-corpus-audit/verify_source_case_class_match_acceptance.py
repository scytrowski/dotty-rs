#!/usr/bin/env python3
"""Fail closed on the pinned whole-method Box Match acceptance receipt."""

from __future__ import annotations

import argparse
import csv
import hashlib
import re
import sys
from pathlib import Path


FIXTURE = Path("crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0")
SOURCE_SHA256 = "df57333192eb934169f06a07873c2e1ba88c9159404f60e5bfe33b3d016e215c"
SCALA_REVISION = "777528f19a58e794c9954a42f433373472ec57f8"
RUST_TESTS = {
    "flagged_product2_late_case_failure_rolls_back_before_valid_sibling",
    "flagged_product2_negative_matrix_rejects_without_publishing_roots",
    "flagged_product2_requires_canonical_product_identity",
    "flagged_product2_whole_method_baseline",
    "frozen_extractor_negative_matrix_rejects_or_defers_without_typed_roots",
    "frozen_source_case_class_match_methods_type_as_whole_defdefs",
    "late_body_error_rolls_back_extractor_and_case_binder_state_before_retry",
    "ordinary_classes_wrong_method_results_and_case_binder_escape_are_rejected",
    "wrong_component_type_and_wrong_extractor_arities_are_rejected",
}
NAMER_TESTS = {
    "class_pattern_bindings_follow_nested_bind_typed_and_unapply_patterns",
    "explicit_companion_unapply_is_preserved_without_generated_identity_collisions",
    "generated_unapply_is_independent_of_case_class_companion_declaration_order",
    "ordinary_class_companion_does_not_receive_generated_unapply",
    "unknown_explicit_unapply_parameter_identity_defers_generation_for_alias_contexts",
}
TYPER_TESTS = {
    "typer::expression::patterns::tests::binary_product_extractors_reject_wrong_selector_and_source_arities",
    "typer::expression::patterns::tests::boolean_extractor_types_zero_patterns_and_preserves_unapply_identity",
    "typer::expression::patterns::tests::erased_unapply_parameters_are_rejected_even_when_source_parsing_lacks_erased_params",
    "typer::expression::patterns::tests::extractor_resolution_reports_focused_qualifier_and_unapply_errors",
    "typer::expression::patterns::tests::extractor_unapply_callable_shapes_are_rejected_explicitly",
    "typer::expression::patterns::tests::simple_extractor_plan_retains_unapply_identity_types_and_source_arguments",
    "typer::expression::patterns::tests::unary_extractor_falls_back_to_get_after_product_arity_mismatch",
    "typer::tests::generated_case_class_unapply_has_source_canonical_plain_signature_and_member_lookup",
    "typer::tests::generated_case_class_unapply_validates_and_rolls_back_source_class_info",
    "typer::tests::generated_product2_unapply_completes_the_canonical_flagged_signature",
    "typer::tests::generated_product2_unapply_rejects_forged_provenance_and_field_identities",
    "typer::tests::local_patdef_alias_over_extractor_binds_the_original_selector_type",
    "typer::tests::non_case_classes_and_explicit_unapply_collisions_have_no_generated_role",
    "typer::tests::unsupported_generated_case_class_unapply_shapes_defer_without_partial_signature",
}
POSITIVE = {
    ("first", "unwrap", "Positive.scala#tree18", "64:136", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Bind(value:Int)", "Int", "complete", "true"),
    ("first", "classify", "Positive.scala#tree36", "140:237", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Literal(0:Int),Bind(value:Int)", "Int", "complete", "true"),
    ("second", "unwrap", "Positive.scala#tree18", "64:136", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Bind(value:Int)", "Int", "complete", "true"),
    ("second", "classify", "Positive.scala#tree36", "140:237", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Literal(0:Int),Bind(value:Int)", "Int", "complete", "true"),
}
NEGATIVE = {
    "wrong_declared_result": ("result_type_mismatch", "scala_invalid", "WrongResult.scala#tree18", "66:138"),
    "wrong_arity_zero": ("extractor_arity_unsupported", "scala_invalid", "WrongArityZero.scala#tree17", "69:128"),
    "wrong_arity_two": ("product_selector_count_mismatch", "scala_invalid", "WrongArityTwo.scala#tree21", "68:135"),
    "wrong_constructor_type": ("extractor_constraint_deferred", "scala_invalid", "WrongConstructorType.scala#tree22", "90:163"),
    "wrong_pattern_type": ("extractor_constraint_deferred", "scala_invalid", "WrongPatternType.scala#tree24", "115:187"),
    "non_case_missing_extractor": ("extractor_qualifier_not_found", "scala_invalid", "NonCaseMissingExtractor.scala#tree18", "77:144"),
    "case_binder_escape": ("term_name_not_found", "scala_invalid", "CaseBinderEscape.scala#tree22", "71:154"),
    "wrong_selector_type": ("typed_pattern_type_mismatch", "scala_valid_rust_deferred", "WrongSelectorType.scala#tree20", "72:144"),
    "explicit_unapply_wrong_result": ("extractor_result_member_not_found", "scala_invalid", "ExplicitUnapplyWrongResult.scala#tree28", "138:211"),
}
SCALA_CASES = {
    "wrong_declared_result": "WrongResult",
    "wrong_arity_zero": "WrongArityZero",
    "wrong_arity_two": "WrongArityTwo",
    "wrong_constructor_type": "WrongConstructorType",
    "wrong_pattern_type": "WrongPatternType",
    "non_case_missing_extractor": "NonCaseMissingExtractor",
    "case_binder_escape": "CaseBinderEscape",
    "wrong_selector_type": "WrongSelectorType",
    "explicit_unapply_wrong_result": "ExplicitUnapplyWrongResult",
}
NEGATIVE_STATIC = {
    **NEGATIVE,
    "explicit_unapply_reuse": ("explicit_source_method_preserved_no_duplicate", "scala_valid_with_warning", "not_typed", "-"),
    "explicit_unapply_alias_collision": ("deferred_before_typed_root", "scala_valid_with_warning", "not_typed", "-"),
}


class ReceiptError(ValueError):
    pass


def read_tsv(path: Path) -> tuple[list[str], list[dict[str, str]]]:
    try:
        with path.open(encoding="utf-8", newline="") as stream:
            reader = csv.DictReader(stream, delimiter="\t")
            if reader.fieldnames is None or len(reader.fieldnames) != len(set(reader.fieldnames)):
                raise ReceiptError(f"{path}: missing or duplicate columns")
            rows = list(reader)
    except OSError as error:
        raise ReceiptError(f"cannot read {path}: {error}") from error
    if any(None in row or any(value is None for value in row.values()) for row in rows):
        raise ReceiptError(f"{path}: malformed row")
    return reader.fieldnames, rows


def validate_inputs(root: Path) -> None:
    fixture = root / FIXTURE
    source = fixture / "Positive.scala"
    if hashlib.sha256(source.read_bytes()).hexdigest() != SOURCE_SHA256:
        raise ReceiptError("frozen two-root Positive.scala hash changed")
    hashes = read_tsv(fixture / "source-hashes.tsv")[1]
    if not hashes or len({row["path"] for row in hashes}) != len(hashes):
        raise ReceiptError("negative/oracle source hash manifest is empty or duplicated")
    for row in hashes:
        path = fixture / row["path"]
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
            raise ReceiptError(f"frozen Scala source hash mismatch: {row['path']}")
    matrix_lines = (fixture / "scala-matrix.tsv").read_text(encoding="utf-8").splitlines()
    # The Scala runner intentionally emits a headerless two-column receipt.
    matrix_rows = [line.split("\t") for line in matrix_lines]
    if any(len(row) != 2 for row in matrix_rows):
        raise ReceiptError("Scala matrix has malformed columns")
    matrix_map = dict(matrix_rows)
    if len(matrix_map) != len(matrix_rows):
        raise ReceiptError("Scala matrix contains duplicated cases")
    for name in SCALA_CASES.values():
        expected = "accepted_with_warning" if name == "WrongSelectorType" else "rejected"
        if matrix_map.get(name) != expected:
            raise ReceiptError(f"Scala 3.9 classification changed for {name}: {matrix_map.get(name)}")
    positive_header, positive_rows = read_tsv(root / "docs/typer-source-case-class-match-positive-3.9.0.tsv")
    if len(positive_header) != 14 or len(positive_rows) != 4:
        raise ReceiptError("committed root receipts do not use the exact expected schemas")
    negative_rows = read_tsv(root / "docs/typer-source-case-class-match-negative-3.9.0.tsv")[1]
    if len(negative_rows) != len(NEGATIVE_STATIC) or len(negative_rows[0]) != 9:
        raise ReceiptError("committed negative receipt has missing or duplicate controls")
    for row in negative_rows:
        name = row["case"]
        if name not in NEGATIVE_STATIC:
            raise ReceiptError(f"committed negative receipt added an unreviewed case: {name}")
        category, scala_classification, source_root, source_span = NEGATIVE_STATIC[name]
        filename = source_root.split("#", 1)[0] if source_root != "not_typed" else ""
        expected_source = str(FIXTURE / filename) if filename else "crates/dotty-namer/tests/parsed_unit.rs"
        expected_test = RUST_TESTS if name in NEGATIVE else NAMER_TESTS
        if (
            row["rust_outcome"] != category
            or row["scala_classification"] != scala_classification
            or row["source_root"] != source_root
            or row["source_span"] != source_span
            or row["source_path"] != expected_source
            or row["first_blocker"] != ("-" if name == "explicit_unapply_reuse" else category if name in NEGATIVE else "unsupported_explicit_unapply_identity")
            or row["rust_test"] not in expected_test
        ):
            raise ReceiptError(f"{name}: negative source/root/outcome receipt changed")
    cohorts = read_tsv(root / "docs/typer-source-case-class-match-cohorts-3.9.0.tsv")[1]
    if len(cohorts) != 50:
        raise ReceiptError("cohort receipt must retain all 25 + 25 frozen roots")
    for cohort, required_file, expected_count in (
        ("main", "required-main.tsv", 16),
        ("analysis", "required-analysis.tsv", 15),
    ):
        contract_rows = [row for row in cohorts if row["cohort"] == cohort]
        required = read_tsv(root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0" / required_file)[1]
        required_map = {row["identity"]: row["oracle_result"] for row in required}
        if len(contract_rows) != 25 or len(required_map) != expected_count:
            raise ReceiptError(f"{cohort}: frozen 25-root denominator or protected count changed")
        expected_rows = {
            (row["identity"], row.get("source_tree") or row["identity"].rsplit("=", 1)[-1],
             str(row["identity"] in required_map).lower(), required_map.get(row["identity"], "-"))
            for row in read_tsv(root / ("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv" if cohort == "main" else "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv"))[1]
        }
        actual_rows = {
            (row["identity"], row["source_tree"], row["protected_success"], row["required_result"])
            for row in contract_rows
        }
        if actual_rows != expected_rows:
            raise ReceiptError(f"{cohort}: static source-tree/protected-set receipt changed")


def passed_tests(text: str) -> set[str]:
    passed = set()
    pending = None
    for line in text.splitlines():
        completed = re.match(r"^test (.+?) \.\.\. (ok|FAILED)$", line)
        if completed:
            pending = None
            if completed.group(2) == "ok":
                passed.add(completed.group(1))
            continue
        started = re.match(r"^test (.+?) \.\.\.(?: .*)?$", line)
        if started:
            pending = started.group(1)
        elif pending and line == "ok":
            passed.add(pending)
            pending = None
        elif pending and line == "FAILED":
            pending = None
    return passed


def marker_lines(text: str, marker: str) -> list[str]:
    return [line[line.index(marker):] for line in text.splitlines() if marker in line]


def require_test_log(path: Path, expected: set[str]) -> str:
    text = path.read_text(encoding="utf-8")
    actual = passed_tests(text)
    if actual != expected:
        raise ReceiptError(f"{path}: executed tests {sorted(actual)!r}, expected {sorted(expected)!r}")
    summary = re.findall(
        r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"\d+ measured; (\d+) filtered out(?:; finished in [0-9.]+s)?$",
        text,
        re.MULTILINE,
    )
    if len(summary) != 1 or summary[0][0] != str(len(expected)) or summary[0][1:3] != ("0", "0"):
        raise ReceiptError(f"{path}: not all {len(expected)} required tests passed without skips")
    return text


def require_test_logs(paths: list[Path], expected: set[str]) -> None:
    actual_all: set[str] = set()
    for path in paths:
        text = path.read_text(encoding="utf-8")
        actual = passed_tests(text)
        if not actual or actual_all.intersection(actual):
            raise ReceiptError(f"{path}: test log is empty or duplicates a required regression")
        summaries = re.findall(
            r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; "
            r"\d+ measured; (\d+) filtered out(?:; finished in [0-9.]+s)?$",
            text,
            re.MULTILINE,
        )
        if len(summaries) != 1 or summaries[0][0] != str(len(actual)) or summaries[0][1:3] != ("0", "0"):
            raise ReceiptError(f"{path}: required regression did not pass without skips")
        actual_all.update(actual)
    if actual_all != expected:
        raise ReceiptError(f"Typer regressions executed {sorted(actual_all)!r}, expected {sorted(expected)!r}")


def validate_rust_log(path: Path) -> None:
    text = require_test_log(path, RUST_TESTS)
    positive_lines = marker_lines(text, "ISSUE1938_POSITIVE\t")
    actual_positive = {tuple(line.split("\t")[1:]) for line in positive_lines}
    if len(positive_lines) != 4 or actual_positive != POSITIVE:
        raise ReceiptError("positive receipt must contain the exact two roots in two sessions")
    committed = read_tsv(Path("docs/typer-source-case-class-match-positive-3.9.0.tsv"))[1]
    committed_rows = {
        (
            row["session"], row["root"], row["source_path"], row["source_span"],
            row["class_owner"], row["method_owner"], row["signature"], row["parent"],
            row["product_selector"], row["selected_unapply"], row["ordered_cases"],
            row["match_result"], row["typed_root"],
            row["normalized_sessions_equal"],
        )
        for row in committed
    }
    if committed_rows != actual_positive or len(committed) != 4:
        raise ReceiptError("committed positive receipt disagrees with actual typed-root log")
    negative_lines = marker_lines(text, "ISSUE1938_NEGATIVE\t")
    actual_negative = {}
    for line in negative_lines:
        fields = line.split("\t")
        if len(fields) != 7 or fields[1] in actual_negative:
            raise ReceiptError("negative receipt row is malformed or duplicated")
        actual_negative[fields[1]] = (
            fields[4].removeprefix("rejected:"), fields[5], fields[2], fields[3], fields[6]
        )
    expected_negative = {name: (*value, value[0]) for name, value in NEGATIVE.items()}
    if actual_negative != expected_negative:
        raise ReceiptError(f"negative classifications mismatch: {actual_negative!r}")
    negative_rows = read_tsv(Path("docs/typer-source-case-class-match-negative-3.9.0.tsv"))[1]
    committed_negative = {
        row["case"]: (
            row["rust_outcome"], row["scala_classification"], row["source_root"],
            row["source_span"], row["first_blocker"],
        )
        for row in negative_rows
    }
    expected_static = {
        name: (value[0], value[1], value[2], value[3], value[0]) for name, value in NEGATIVE.items()
    }
    expected_static.update({
        "explicit_unapply_reuse": (*NEGATIVE_STATIC["explicit_unapply_reuse"], "-"),
        "explicit_unapply_alias_collision": (*NEGATIVE_STATIC["explicit_unapply_alias_collision"], "unsupported_explicit_unapply_identity"),
    })
    if committed_negative != expected_static:
        raise ReceiptError("committed negative receipt disagrees with executed Rust matrix")


def validate_oracle_log(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    required_lines = {
        "verified eight pinned Scala 3.9.0 artifacts",
        f"Scala 3.9.0 source revision: {SCALA_REVISION}",
        'JDK: JAVA_VERSION="21.0.8"',
        "Positive whole-source typer snapshot matches.",
        "Distinct same-spelled extractor owner snapshot matches.",
        "Bounded extractor acceptance matrix matches.",
    }
    missing = required_lines - set(text.splitlines())
    if missing:
        raise ReceiptError(f"pinned Scala oracle log is missing required evidence: {sorted(missing)!r}")
    matrix_lines = (Path(__file__).resolve().parents[2] / FIXTURE / "scala-matrix.tsv").read_text(encoding="utf-8").splitlines()
    if len(matrix_lines) != 18 or len({line.split("\t", 1)[0] for line in matrix_lines}) != 18:
        raise ReceiptError("Scala oracle matrix did not execute all 18 unique frozen controls")


def parse_ordinary_report(path: Path) -> dict[str, dict[str, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    marker = "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
    try:
        start = next(index for index, line in enumerate(lines) if line.startswith(marker))
    except StopIteration as error:
        raise ReceiptError(f"{path}: ordinary report header missing") from error
    rows = csv.DictReader(lines[start:], delimiter="\t")
    return {row["method"]: row for row in rows}


def validate_cohorts(root: Path, reports: list[Path], output: Path) -> None:
    if len(reports) != 4:
        raise ReceiptError("cohort receipt requires first/second main and analysis reports")
    manifests = {
        "main": root / "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv",
        "analysis": root / "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv",
    }
    protected_paths = {
        "main": root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv",
        "analysis": root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv",
    }
    parsed = [parse_ordinary_report(path) for path in reports]
    rows_out = []
    for cohort, base in (("main", 0), ("analysis", 2)):
        roots = read_tsv(manifests[cohort])[1]
        protected_rows = read_tsv(protected_paths[cohort])[1]
        protected = {row["identity"] for row in protected_rows}
        protected_results = {row["identity"]: row["oracle_result"] for row in protected_rows}
        expected_protected = 16 if cohort == "main" else 15
        if len(roots) != 25 or len(protected) != expected_protected:
            raise ReceiptError(f"{cohort}: frozen denominator/protected-root count changed")
        root_identities = {row["identity"] for row in roots}
        if set(parsed[base]) != root_identities or set(parsed[base + 1]) != root_identities:
            raise ReceiptError(f"{cohort}: ordinary report omitted a frozen root")
        for frozen in roots:
            identity = frozen["identity"]
            first, second = parsed[base][identity], parsed[base + 1][identity]
            source_tree = frozen.get("source_tree") or identity.rsplit("=", 1)[-1]
            baseline_status = frozen.get("baseline_status") or ("typed" if identity in protected else "not_protected")
            gate = frozen.get("gate") or ("required_typed" if identity in protected else "observe")
            stable = (
                first["status"] == second["status"]
                and first["actual_result"] == second["actual_result"]
                and first["source_tree"] == second["source_tree"] == source_tree
            )
            is_protected = identity in protected
            if not stable:
                raise ReceiptError(f"{identity}: two deterministic passes disagree")
            if baseline_status == "typed" and not is_protected:
                raise ReceiptError(f"{identity}: frozen typed root omitted from protected set")
            if is_protected and (first["status"] != "typed" or second["status"] != "typed"):
                raise ReceiptError(f"{identity}: protected {cohort} root lost typed acceptance")
            if is_protected and (first["actual_result"] != protected_results[identity] or second["actual_result"] != protected_results[identity]):
                raise ReceiptError(f"{identity}: protected {cohort} result changed")
            rows_out.append((
                cohort, identity, source_tree, baseline_status,
                gate, first["status"], second["status"], first["actual_result"],
                str(is_protected).lower(), str(stable).lower(),
            ))
    header = (
        "cohort", "identity", "source_tree", "baseline_status", "gate", "first_status",
        "second_status", "actual_result", "protected_success", "two_pass_equal",
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
        writer.writerow(header)
        writer.writerows(rows_out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    inputs = sub.add_parser("inputs")
    inputs.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    for command, tests in (("rust-log", RUST_TESTS), ("namer-log", NAMER_TESTS), ("typer-log", TYPER_TESTS)):
        item = sub.add_parser(command)
        item.add_argument("log", type=Path, nargs="+" if command == "typer-log" else None)
    oracle = sub.add_parser("oracle-log")
    oracle.add_argument("log", type=Path)
    cohorts = sub.add_parser("cohorts")
    cohorts.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    cohorts.add_argument("--report", action="append", type=Path, required=True)
    cohorts.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        root = getattr(args, "repo_root", Path(__file__).resolve().parents[2])
        validate_inputs(root)
        if args.command == "inputs":
            print("frozen Box sources, Scala classifications, and compact receipts verified")
        elif args.command == "rust-log":
            validate_rust_log(args.log)
            print("exact two-root/two-session and nine-case negative Typer receipt verified")
        elif args.command == "oracle-log":
            validate_oracle_log(args.log)
            print("pinned Scala 3.9.0 oracle, snapshots, hashes, and 18-case matrix verified")
        elif args.command in {"namer-log", "typer-log"}:
            expected = NAMER_TESTS if args.command == "namer-log" else TYPER_TESTS
            if args.command == "namer-log":
                require_test_log(args.log, expected)
            else:
                require_test_logs(args.log, expected)
            print(f"all {len(expected)} exact {args.command[:-4]} extractor regressions passed")
        else:
            validate_cohorts(root, args.report, args.output)
            print("all 25 + 25 deterministic ordinary roots and protected 16 + 15 receipt verified")
    except (OSError, KeyError, ValueError, ReceiptError, csv.Error) as error:
        print(f"source case-class Match gate failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
