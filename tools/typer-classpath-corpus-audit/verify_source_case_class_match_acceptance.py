#!/usr/bin/env python3
"""Fail closed on the pinned Box Match and extractor acceptance receipts."""

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
EITHER_SWAP_IDENTITY = "library/src/scala/util/Either.scala#tree=84"
EITHER_SWAP_HISTORICAL_BLOCKER = (
    "blocked::ExtractorUnapplyNotFound { source: SourceId(0), tree_index: 69, "
    "extractor: SymbolId(106) }"
)
EITHER_SWAP_PREVIOUS_LIVE_BLOCKER = (
    "blocked::MalformedMethodClause { method: SymbolId(112), "
    "method_tree_index: 521, clause_index: 0 }"
)
EITHER_SWAP_LIVE_BLOCKER = (
    "blocked::ExtractorUnapplyPolymorphic { source: SourceId(0), tree_index: 69, "
    "unapply: SymbolId(112), callable: TypeId(552) }"
)
PRODUCT2_FIXTURE = Path("crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0")
PRODUCT2_SOURCE_SHA256 = "4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31"
SCALA_MATRIX_CASES = {
    "Wildcard": "accepted",
    "WrongResult": "rejected",
    "WrongArityZero": "rejected",
    "WrongArityTwo": "rejected",
    "WrongSelectorType": "accepted_with_warning",
    "WrongConstructorType": "rejected",
    "SameSpelledExtractor": "accepted_with_warning",
    "ExplicitCompanion": "accepted",
    "ExplicitUnapplyOverload": "accepted",
    "ExplicitUnapplyReuse": "accepted_with_warning",
    "ExplicitUnapplyWrongResult": "rejected",
    "PackageAliasUnapply": "accepted_with_warning",
    "ImportedAliasUnapply": "accepted_with_warning",
    "CompanionAliasUnapply": "accepted_with_warning",
    "NonCaseControl": "accepted",
    "InputSupertype": "accepted_with_warning",
    "WrongPatternType": "rejected",
    "NonCaseMissingExtractor": "rejected",
    "CaseBinderEscape": "rejected",
}
SEALED_ADT_SCALA_CASES = {
    "Event": "accepted",
    "NonSealedEvent": "accepted",
    "WrongConstructorLiteral": "rejected",
    "WrongResult": "rejected",
    "UnrelatedExtractorType": "rejected",
    "MissingParent": "rejected",
    "WrongParent": "rejected",
    "InvalidFieldShape": "rejected",
    "BinderEscape": "rejected",
    "DuplicateVariant": "accepted_with_warning",
    "NonExhaustive": "accepted_with_warning",
    "WrongAliasOwner": "accepted_with_warning",
}
PRODUCT2_CASES = {
    "WrongArityOne": "rejected",
    "WrongArityThree": "rejected",
    "WrongLiteralTypes": "rejected",
    "WrongResult": "rejected",
    "ReversedFields": "accepted",
    "ExplicitSelectorResultMismatch": "rejected",
    "ExplicitSelectors": "accepted",
    "ExplicitUnapplyCollision": "accepted_with_warning",
    "OrdinaryNonCase": "rejected",
    "UnrelatedSameNameExtractor": "accepted_with_warning",
}
PRODUCT2_RUST_ROOTS = {
    "select": "select\tPositive.scala#tree24\t86:208\tmatching.Flagged\tmatching.Matcher\t(Flagged):Int\tscala.Product\t_1:Int@Flagged,_2:Boolean@Flagged\tFlagged$.unapply\tBind(value:Int)@153:158;Bind(enabled:Boolean)@160:167\tcomplete\tInt\ttrue\t" + PRODUCT2_SOURCE_SHA256,
    "classify": "classify\tPositive.scala#tree44\t212:332\tmatching.Flagged\tmatching.Matcher\t(Flagged):Int\tscala.Product\t_1:Int@Flagged,_2:Boolean@Flagged\tFlagged$.unapply\tLiteral(0:Int),Literal(true:Boolean);Bind(value:Int),Wildcard(Boolean)\tcomplete\tInt\ttrue\t" + PRODUCT2_SOURCE_SHA256,
}
PRODUCT2_NEGATIVE_EVIDENCE = {
    "WrongArityOne": ("rejected", "rust_rejected", "flagged_product2_negative_matrix_rejects_without_publishing_roots"),
    "WrongArityThree": ("rejected", "rust_rejected", "flagged_product2_negative_matrix_rejects_without_publishing_roots"),
    "WrongLiteralTypes": ("rejected", "rust_rejected", "flagged_product2_negative_matrix_rejects_without_publishing_roots"),
    "WrongResult": ("rejected", "rust_rejected", "flagged_product2_negative_matrix_rejects_without_publishing_roots"),
    "ReversedFields": ("accepted", "deferred", "no exact Rust fixture"),
    "ExplicitSelectorResultMismatch": ("rejected", "rust_deferred", "frozen_extractor_negative_matrix_rejects_or_defers_without_typed_roots"),
    "ExplicitSelectors": ("accepted", "deferred", "no exact Rust fixture"),
    "ExplicitUnapplyCollision": ("accepted_with_warning", "identity_control", "explicit_companion_unapply_is_preserved_without_generated_identity_collisions"),
    "OrdinaryNonCase": ("rejected", "rust_rejected", "flagged_product2_negative_matrix_rejects_without_publishing_roots"),
    "UnrelatedSameNameExtractor": ("accepted_with_warning", "identity_control", "unknown_explicit_unapply_parameter_identity_defers_generation_for_alias_contexts"),
}
RUST_TESTS = {
    "flagged_product2_negative_matrix_rejects_without_publishing_roots",
    "flagged_product2_requires_canonical_product_identity",
    "flagged_product2_whole_method_baseline",
    "frozen_extractor_negative_matrix_rejects_or_defers_without_typed_roots",
    "frozen_source_case_class_match_methods_type_as_whole_defdefs",
    "late_body_error_rolls_back_extractor_and_case_binder_state_before_retry",
    "ordinary_classes_wrong_method_results_and_case_binder_escape_are_rejected",
    "precompleted_source_parent_graph_failures_roll_back_and_retry_in_both_routes",
    "sealed_event_extract_and_classify_are_complete_owned_methods",
    "sealed_event_rejects_variant_extractor_with_a_foreign_owner",
    "source_recursive_product1_reference_is_canonical_in_both_completion_routes",
    "source_recursive_product1_rejects_unrelated_trait_field_and_keeps_sibling_retryable",
    "completed_source_product1_rejects_broken_selector_authentication",
    "generic_product1_selector_identity_remains_explicitly_deferred",
    "source_trait_parent_does_not_enable_product2_completion",
    "source_trait_parent_product1_defers_multi_parent_selector_shape",
    "source_trait_parent_product1_preserves_identity_in_both_completion_routes",
    "source_trait_parent_product1_records_frozen_match_first_error_movement",
    "source_trait_parent_product1_rejects_generic_and_class_parent_shapes",
    "source_trait_parent_product1_rolls_back_then_retries_sibling_in_same_session",
    "source_trait_product1_parent_is_authenticated_by_symbol_not_spelling",
    "wrong_component_type_and_wrong_extractor_arities_are_rejected",
}
NAMER_TESTS = {
    "class_pattern_bindings_follow_nested_bind_typed_and_unapply_patterns",
    "explicit_companion_unapply_is_preserved_without_generated_identity_collisions",
    "generated_unapply_is_independent_of_case_class_companion_declaration_order",
    "generic_product1_case_classes_publish_only_canonical_unapply_identity",
    "generic_unapply_identity_rejects_shapes_outside_the_bounded_product1_rule",
    "incomparable_generic_companion_unapply_fails_without_publishing_a_candidate",
    "ordinary_class_companion_does_not_receive_generated_unapply",
    "unknown_explicit_unapply_parameter_identity_defers_generation_for_alias_contexts",
}
TYPER_TESTS = {
    "typer::tests::issue_1972_failed_root_retries_with_same_typer_and_matches_clean_run",
    "typer::expression::patterns::tests::binary_product_extractors_reject_wrong_selector_and_source_arities",
    "typer::expression::patterns::tests::boolean_extractor_types_zero_patterns_and_preserves_unapply_identity",
    "typer::expression::patterns::tests::erased_unapply_parameters_are_rejected_even_when_source_parsing_lacks_erased_params",
    "typer::expression::patterns::tests::extractor_resolution_reports_focused_qualifier_and_unapply_errors",
    "typer::expression::patterns::tests::extractor_unapply_callable_shapes_are_rejected_explicitly",
    "typer::expression::patterns::tests::recursive_source_product2_generated_unapply_projects_two_distinct_expr_selectors",
    "typer::expression::patterns::tests::simple_extractor_plan_retains_unapply_identity_types_and_source_arguments",
    "typer::expression::patterns::tests::unary_extractor_falls_back_to_get_after_product_arity_mismatch",
    "typer::tests::generated_case_class_unapply_has_source_canonical_plain_signature_and_member_lookup",
    "typer::tests::generated_case_class_unapply_validates_and_rolls_back_source_class_info",
    "typer::tests::generated_generic_unapply_uses_unit_scala_2_13_mode_and_preserves_parameter_order",
    "typer::tests::generated_product2_unapply_completes_the_canonical_flagged_signature",
    "typer::tests::generated_product2_unapply_rejects_forged_provenance_and_field_identities",
    "typer::tests::generated_two_reference_field_unapply_accepts_consistent_complete_field_info",
    "typer::tests::generated_two_reference_field_unapply_rejects_second_field_and_rolls_back",
    "typer::tests::generated_two_reference_field_unapply_uses_canonical_add_identity",
    "typer::tests::generated_two_reference_field_unapply_validates_existing_info_and_retries",
    "typer::tests::local_patdef_alias_over_extractor_binds_the_original_selector_type",
    "typer::tests::non_case_classes_and_explicit_unapply_collisions_have_no_generated_role",
    "typer::tests::reference_unapply_rejects_unrelated_field_transactionally_and_retries_sibling",
    "typer::tests::reference_unapply_revalidates_completed_product1_parent_and_retries",
    "typer::tests::reference_unapply_rejects_malformed_formal_after_reference_validation",
    "typer::tests::reference_unapply_publication_rolls_back_with_enclosing_transaction_and_retries",
    "typer::tests::unsupported_generated_case_class_unapply_shapes_defer_without_partial_signature",
}
ISSUE1999_TESTS = {
    "typer::tests::issue_1999_late_event_result_failure_observes_both_cases_before_atomic_rollback",
    "typer::tests::issue_1999_structural_corruption_restores_then_recovers_in_same_typer",
}
RECURSIVE_ADT_POSITIVE_TESTS = {
    "evaluator_recursive_adt_roots_type_after_generated_unapply_completion",
}
RECURSIVE_ADT_NEGATIVE_TESTS = {
    "typer::tests::method_parameter_capture::recursive_adt_rollback::recursive_adt_negative_matrix_is_stable_across_fresh_sessions",
    "typer::tests::method_parameter_capture::recursive_adt_rollback::recursive_eval_late_result_failure_observes_binder_call_and_rolls_back_for_retry",
    "typer::tests::method_parameter_capture::recursive_adt_rollback::binary_recursive_second_argument_failure_and_late_failure_restore_then_retry_eval",
    "typer::tests::method_parameter_capture::recursive_adt_rollback::binary_late_failure_restores_complete_state_retries_then_types_both_original_roots",
    "typer::tests::method_parameter_capture::recursive_adt_rollback::binary_classified_source_controls_have_stable_retry_and_session_receipts",
    "typer::tests::method_parameter_capture::recursive_adt_rollback::nested_late_failures_restore_and_retry_before_original_roots_recover",
}
BINARY_ADT_POSITIVE_TESTS = {
    "binary_recursive_adt_original_roots_share_two_real_sessions",
}
BINARY_ADT_STRUCTURAL_TEST = {
    "corrupted_reference_product2_identities_reject_then_retry_original_select_left",
}
BINARY_ADT_CONTROL_TEST = {
    "typer::tests::method_parameter_capture::recursive_adt_rollback::binary_classified_source_controls_have_stable_retry_and_session_receipts",
}
BINARY_ADT_SOURCE = Path("crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0")
BINARY_SOURCE_MANIFEST_SHA256 = "5e79d33b024b27a348dc5f72e3e599871cc3cd792f75973734326045478b5691"
BINARY_OUTPUT_MANIFEST_SHA256 = "d09a74b2900e9fc51a4ee8855561fba42cc38ac8ac0ff6c542bbd0f500f707b9"
BINARY_ARTIFACT_MANIFEST_SHA256 = "0b69527d4b2d2bb9d6cebba92b481f0a0f2d9aed7b2a8a2b15dbda3233128573"
BINARY_ADT_POSITIVE_ROWS = {
    ("session-a", "selectLeft", "Expr.scala#tree39", "158:271"),
    ("session-b", "selectLeft", "Expr.scala#tree39", "158:271"),
    ("session-a", "eval", "Expr.scala#tree64", "275:410"),
    ("session-b", "eval", "Expr.scala#tree64", "275:410"),
}
RECURSIVE_ADT_POSITIVE_PROOFS = {
    "unwrap": (
        "owner=adt.Evaluator$|signature=(Expr):Expr|selector=Expr|"
        "cases=Num(_)=>expr:Expr,Succ(inner:Expr)=>inner:Expr|"
        "extractors=Num$.unapply:Num,Succ$.unapply:Succ|"
        "parents=Num:Expr+Product,Succ:Expr+Product|"
        "fields=Num._1:Int,Succ._1:Expr|"
        "mappings=DefDef,formal,Match,selector,CaseDef[0..1],UnApply[0..1],"
        "qualifier[0..1],wildcard,binder,body[0..1]|result=Expr"
    ),
    "eval": (
        "owner=adt.Evaluator$|signature=(Expr):Int|selector=Expr|"
        "cases=Num(value:Int)=>value:Int,Succ(inner:Expr)=>eval(inner):Int+1:Int|"
        "extractors=Num$.unapply:Num,Succ$.unapply:Succ|"
        "parents=Num:Expr+Product,Succ:Expr+Product|"
        "fields=Num._1:Int,Succ._1:Expr|"
        "recursive=Evaluator.eval same Method SymbolId;argument=Succ.inner:Expr;result=Int|"
        "mappings=DefDef,formal,Match,selector,CaseDef[0..1],UnApply[0..1],"
        "qualifier[0..1],binder,body[0..1],recursive-function,argument,call,"
        "addition,literal|result=Int"
    ),
}
RECURSIVE_ADT_SOURCE_SHA256 = "073be43ad13b7518902a341d3e06b00254d990a325855c70738469c70c8e74ab"
RECURSIVE_ADT_ARTIFACT_MANIFEST_SHA256 = "0b69527d4b2d2bb9d6cebba92b481f0a0f2d9aed7b2a8a2b15dbda3233128573"
RECURSIVE_ADT_SOURCE = Path("crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0")
SCOPE_ROLLBACK_TESTS = {
    "symbols::scope::tests::remove_drops_the_name_bucket_after_its_last_symbol",
}
POSITIVE = {
    ("first", "unwrap", "Positive.scala#tree18", "64:136", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Bind(value:Int)", "Int", "complete", "true"),
    ("first", "classify", "Positive.scala#tree36", "140:237", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Literal(0:Int),Bind(value:Int)", "Int", "complete", "true"),
    ("second", "unwrap", "Positive.scala#tree18", "64:136", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Bind(value:Int)", "Int", "complete", "true"),
    ("second", "classify", "Positive.scala#tree36", "140:237", "matching.Box", "matching.Matcher", "(Box):Int", "scala.Product", "_1:Int@Box", "Box$.unapply", "Literal(0:Int),Bind(value:Int)", "Int", "complete", "true"),
}
SEALED_ADT_POSITIVE = {
    (session, "extract", "Event.scala#tree36|150:262|signature=(Event):Int|owner=Matcher$|selector=Event|cases=Created(id),Deleted(code)|extractors=Created$.unapply,Deleted$.unapply|binders=Created:id:Int,Deleted:code:Int|typed=complete DefDef+Match|mappings=all source RHS trees")
    for session in ("first", "second")
} | {
    (session, "classify", "Event.scala#tree60|266:400|signature=(Event):Int|owner=Matcher$|selector=Event|cases=Created(0),Created(id),Deleted(_)|extractors=Created$.unapply,Created$.unapply,Deleted$.unapply|binders=Created:id:Int|typed=complete DefDef+Match|mappings=all source RHS trees")
    for session in ("first", "second")
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


def validate_scala_matrix(lines: list[str]) -> None:
    observed: dict[str, str] = {}
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 2:
            raise ReceiptError("Scala matrix has malformed columns")
        name, classification = fields
        if name in observed:
            raise ReceiptError(f"Scala matrix contains duplicated case: {name}")
        observed[name] = classification
    if observed != SCALA_MATRIX_CASES:
        missing = sorted(SCALA_MATRIX_CASES.keys() - observed.keys())
        unexpected = sorted(observed.keys() - SCALA_MATRIX_CASES.keys())
        changed = sorted(
            name
            for name in SCALA_MATRIX_CASES.keys() & observed.keys()
            if observed[name] != SCALA_MATRIX_CASES[name]
        )
        raise ReceiptError(
            "Scala matrix inventory/classification changed "
            f"(missing={missing!r}, unexpected={unexpected!r}, changed={changed!r})"
        )


def validate_inputs(root: Path) -> None:
    fixture = root / FIXTURE
    source = fixture / "Positive.scala"
    if hashlib.sha256(source.read_bytes()).hexdigest() != SOURCE_SHA256:
        raise ReceiptError("frozen two-root Positive.scala hash changed")
    sealed_fixture = root / "crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0"
    sealed_hashes = read_tsv(sealed_fixture / "source-hashes.tsv")[1]
    if not sealed_hashes or len({row["path"] for row in sealed_hashes}) != len(sealed_hashes):
        raise ReceiptError("sealed Event source hash manifest is empty or duplicated")
    for row in sealed_hashes:
        path = sealed_fixture / row["path"]
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
            raise ReceiptError(f"frozen sealed Event Scala source hash mismatch: {row['path']}")
    sealed_matrix = [line.split("\t") for line in (sealed_fixture / "scala-matrix.tsv").read_text(encoding="utf-8").splitlines()]
    if len(sealed_matrix) != len(SEALED_ADT_SCALA_CASES) or any(len(row) != 2 for row in sealed_matrix):
        raise ReceiptError("sealed Event Scala matrix is malformed or incomplete")
    if dict(sealed_matrix) != SEALED_ADT_SCALA_CASES or len({row[0] for row in sealed_matrix}) != len(SEALED_ADT_SCALA_CASES):
        raise ReceiptError("sealed Event Scala classifications changed or contain duplicate controls")
    product2_source = root / PRODUCT2_FIXTURE / "Positive.scala"
    if hashlib.sha256(product2_source.read_bytes()).hexdigest() != PRODUCT2_SOURCE_SHA256:
        raise ReceiptError("frozen Product2 Positive.scala hash changed")
    product2_hashes = read_tsv(root / PRODUCT2_FIXTURE / "source-hashes.tsv")[1]
    if not product2_hashes or len({row["path"] for row in product2_hashes}) != len(product2_hashes):
        raise ReceiptError("Product2 source hash manifest is empty or duplicated")
    for row in product2_hashes:
        path = root / PRODUCT2_FIXTURE / row["path"]
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
            raise ReceiptError(f"frozen Product2 Scala source hash mismatch: {row['path']}")
    product2_matrix = [line.split("\t") for line in (root / PRODUCT2_FIXTURE / "scala-matrix.tsv").read_text(encoding="utf-8").splitlines()]
    if len(product2_matrix) != len(PRODUCT2_CASES) or any(len(row) != 2 for row in product2_matrix):
        raise ReceiptError("Product2 Scala matrix is malformed or incomplete")
    if dict(product2_matrix) != PRODUCT2_CASES:
        raise ReceiptError("Product2 Scala classifications changed")
    product2_negative = read_tsv(root / "docs/typer-heterogeneous-product2-match-negative-3.9.0.tsv")[1]
    if len(product2_negative) != len(PRODUCT2_NEGATIVE_EVIDENCE):
        raise ReceiptError("Product2 negative sidecar must retain all ten frozen controls")
    seen_negative = set()
    for row in product2_negative:
        name = row["case"]
        if name not in PRODUCT2_NEGATIVE_EVIDENCE or name in seen_negative:
            raise ReceiptError("Product2 negative sidecar contains an unknown or duplicate control")
        seen_negative.add(name)
        classification, disposition, evidence = PRODUCT2_NEGATIVE_EVIDENCE[name]
        source_path = root / PRODUCT2_FIXTURE / f"{name}.scala"
        expected_source_hash = hashlib.sha256(source_path.read_bytes()).hexdigest()
        if (
            row["source_path"] != str(PRODUCT2_FIXTURE / f"{name}.scala")
            or row["source_sha256"] != expected_source_hash
            or row["scala_classification"] != classification
            or row["rust_disposition"] != disposition
            or row["rust_evidence"] != evidence
            or row["integrated_source_sha"] != "3a854831b9c074261e31e82d325c0350f606b95a"
        ):
            raise ReceiptError(f"Product2 negative sidecar changed for {name}")
    product2_cohorts = read_tsv(root / "docs/typer-heterogeneous-product2-match-cohorts-3.9.0.tsv")[1]
    match_cohorts = read_tsv(root / "docs/typer-source-case-class-match-cohorts-3.9.0.tsv")[1]
    if len(product2_cohorts) != 50 or len(match_cohorts) != 50:
        raise ReceiptError("Product2 cohort sidecar must retain all 25 + 25 frozen roots")
    baseline_manifests = {
        "main": read_tsv(root / "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv")[1],
        "analysis": read_tsv(root / "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv")[1],
    }
    baseline_maps = {
        cohort: {row["identity"]: row for row in rows}
        for cohort, rows in baseline_manifests.items()
    }
    for sidecar, baseline in zip(product2_cohorts, match_cohorts):
        frozen = baseline_maps[sidecar["cohort"]].get(sidecar["identity"])
        expected_status = None if frozen is None else (
            frozen.get("baseline_status")
            or ("typed" if baseline["protected_success"] == "true" else "not_protected")
        )
        expected_gate = None if frozen is None else (
            frozen.get("gate")
            or ("required_typed" if baseline["protected_success"] == "true" else "observe")
        )
        if (
            frozen is None
            or
            sidecar["cohort"] != baseline["cohort"]
            or sidecar["identity"] != baseline["identity"]
            or sidecar["source_tree"] != baseline["source_tree"]
            or sidecar["baseline_status"] != expected_status
            or sidecar["gate"] != expected_gate
            or sidecar["protected_root"] != baseline["protected_success"]
            or sidecar["required_result"] != baseline["required_result"]
            or sidecar["integrated_source_sha"] != "3a854831b9c074261e31e82d325c0350f606b95a"
            or sidecar["first_status"] != sidecar["second_status"]
            or sidecar["two_pass_equal"] != "true"
            or sidecar["github_actions_run"] != "https://github.com/scytrowski/dotty-rs/actions/runs/41366399491"
            or sidecar["pull_request_url"] != "https://github.com/scytrowski/dotty-rs/pull/1987"
            or (
                sidecar["protected_root"] == "true"
                and (
                    sidecar["first_status"] != "typed"
                    or sidecar["actual_result"] != sidecar["required_result"]
                )
            )
        ):
            raise ReceiptError("Product2 cohort sidecar changed a protected root or provenance")
    hashes = read_tsv(fixture / "source-hashes.tsv")[1]
    if not hashes or len({row["path"] for row in hashes}) != len(hashes):
        raise ReceiptError("negative/oracle source hash manifest is empty or duplicated")
    for row in hashes:
        path = fixture / row["path"]
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
            raise ReceiptError(f"frozen Scala source hash mismatch: {row['path']}")
    matrix_lines = (fixture / "scala-matrix.tsv").read_text(encoding="utf-8").splitlines()
    # The Scala runner intentionally emits a headerless two-column receipt.
    validate_scala_matrix(matrix_lines)
    matrix_map = dict(line.split("\t") for line in matrix_lines)
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
    validate_sealed_rollback_sidecar(root)
    validate_sealed_cohort_contract(root)
    validate_recorded_cohort_receipt(root)


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


def validate_recursive_adt_inputs(root: Path) -> None:
    fixture = root / RECURSIVE_ADT_SOURCE
    source = fixture / "Expr.scala"
    if not source.is_file() or hashlib.sha256(source.read_bytes()).hexdigest() != RECURSIVE_ADT_SOURCE_SHA256:
        raise ReceiptError("frozen recursive Expr source SHA-256 changed")
    artifact_manifest = fixture / "scala-artifacts.sha256.tsv"
    if hashlib.sha256(artifact_manifest.read_bytes()).hexdigest() != RECURSIVE_ADT_ARTIFACT_MANIFEST_SHA256:
        raise ReceiptError("pinned recursive Expr Scala artifact coordinates/hashes changed")
    for manifest in ("source-hashes.tsv", "output-sha256.tsv"):
        _, rows = read_tsv(fixture / manifest)
        if not rows or len({row["path"] for row in rows}) != len(rows):
            raise ReceiptError(f"recursive Expr {manifest} is empty or duplicated")
        for row in rows:
            path = fixture / row["path"]
            if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
                raise ReceiptError(f"recursive Expr pinned input hash mismatch: {row['path']}")
    artifact_rows = [line.split("\t") for line in (fixture / "scala-artifacts.sha256.tsv").read_text(encoding="utf-8").splitlines()]
    if len(artifact_rows) != 8 or any(len(row) != 2 for row in artifact_rows):
        raise ReceiptError("recursive Expr Scala oracle must pin exactly eight artifacts")
    for digest, relative in artifact_rows:
        path = root / "target/coursier/v1/https/repo1.maven.org/maven2" / relative
        # Artifact bytes are verified by the pinned runner, while the manifest itself
        # is bound here so a changed coordinate/hash cannot silently alter CI inputs.
        if not re.fullmatch(r"[0-9a-f]{64}", digest) or not relative.startswith("org/"):
            raise ReceiptError(f"malformed recursive Expr Scala artifact pin: {relative}")
    matrix = read_tsv(fixture / "scala-negative-matrix.tsv")[1]
    if len(matrix) != 15 or len({row["case"] for row in matrix}) != 15:
        raise ReceiptError("recursive Expr Scala negative matrix must have 15 unique cases")
    negative_cases = read_tsv(fixture / "negative-cases.tsv")[1]
    scala_classifications = {row["case"]: row["scala_result"] for row in negative_cases}
    if len(scala_classifications) != 15 or {row["case"]: row["scala_result"] for row in matrix} != scala_classifications:
        raise ReceiptError("recursive Expr Scala negative outcomes differ from the frozen mutation inventory")
    required = {"wrong_reference_literal", "extractor_arity_zero", "unwrap_result_int", "eval_result_boolean",
                "cross_case_binder", "eval_wrong_type", "eval_wrong_arity", "foreign_succ_owner",
                "foreign_eval_control", "foreign_expr_succ_owner"}
    if not required.issubset({row["case"] for row in matrix}):
        raise ReceiptError("recursive Expr Scala matrix is missing required source controls")


def validate_recursive_adt_oracle_log(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    required_lines = {
        "verified all 8 pinned Scala 3.9.0 artifacts",
        f"Scala source revision: {SCALA_REVISION}",
        'JDK: JAVA_VERSION="21.0.8"; IMPLEMENTOR_VERSION="Temurin-21.0.8+9"',
        "Two-root typed snapshot and 13-case classification matrix match.",
        "verified frozen source SHA-256 manifest",
    }
    if required_lines - set(text.splitlines()):
        raise ReceiptError(f"recursive Expr pinned oracle log is missing evidence: {sorted(required_lines - set(text.splitlines()))!r}")


def validate_recursive_adt_positive_log(path: Path, tested_sha: str, expected_sha: str) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", tested_sha) or tested_sha != expected_sha:
        raise ReceiptError("recursive Expr Rust evidence must match the exact 40-character audited SHA")
    text = require_test_log(path, RECURSIVE_ADT_POSITIVE_TESTS)
    if marker_lines(text, "ISSUE2040_TESTED_SHA\t") != [f"ISSUE2040_TESTED_SHA\t{tested_sha}"]:
        raise ReceiptError("recursive Expr Rust log is missing the exact tested checkout SHA")
    lines = marker_lines(text, "ISSUE2040_POSITIVE\t")
    actual: dict[tuple[str, str], tuple[str, ...]] = {}
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 6:
            raise ReceiptError("recursive Expr positive receipt has malformed columns")
        _, session, root, source_path_tree, span, *proof = fields
        key = session, root
        if session not in {"first", "second"} or root not in RECURSIVE_ADT_POSITIVE_PROOFS or key in actual:
            raise ReceiptError("recursive Expr positive receipt has an unexpected or duplicate session/root")
        if source_path_tree not in {"Expr.scala#tree36", "Expr.scala#tree58"}:
            raise ReceiptError("recursive Expr positive receipt substituted a source tree")
        expected_tree = "Expr.scala#tree36" if root == "unwrap" else "Expr.scala#tree58"
        expected_span = "147:255" if root == "unwrap" else "259:375"
        if source_path_tree != expected_tree or span != expected_span:
            raise ReceiptError("recursive Expr positive receipt changed its frozen path/tree/span")
        if "\t".join(proof) != RECURSIVE_ADT_POSITIVE_PROOFS[root]:
            raise ReceiptError(f"recursive Expr {root} semantic receipt differs from the asserted contract")
        actual[key] = (source_path_tree, span, *proof)
    expected_keys = {(session, root) for session in ("first", "second") for root in ("unwrap", "eval")}
    if len(lines) != 4 or set(actual) != expected_keys:
        raise ReceiptError("recursive Expr requires exactly unwrap/eval in two fresh sessions")
    if actual[("first", "unwrap")] != actual[("second", "unwrap")] or actual[("first", "eval")] != actual[("second", "eval")]:
        raise ReceiptError("recursive Expr normalized semantic receipts differ across fresh sessions")
    sidecar_path = Path(__file__).resolve().parents[2] / "docs/typer-recursive-adt-e2e-positive-3.9.0.tsv"
    header, rows = read_tsv(sidecar_path)
    expected_header = ["session", "root", "source_path_tree", "source_span", "normalized_semantic_sha256", "proof"]
    if header != expected_header or len(rows) != 4:
        raise ReceiptError("recursive Expr positive sidecar has the wrong schema or denominator")
    expected_rows = set()
    for (session, root), receipt in actual.items():
        normalized = "\t".join(receipt)
        expected_rows.add((session, root, receipt[0], receipt[1], hashlib.sha256(normalized.encode()).hexdigest(), "\t".join(receipt[2:])))
    if {tuple(row[column] for column in header) for row in rows} != expected_rows:
        raise ReceiptError("committed recursive Expr positive sidecar differs from the four executed receipts")


def validate_recursive_adt_negative_oracle_log(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    if "verified 15 pinned Scala negative/control cases" not in text:
        raise ReceiptError("recursive Expr pinned Scala negative runner did not execute all 15 controls")


def validate_recursive_adt_negative_log(path: Path) -> None:
    text = require_test_log(path, RECURSIVE_ADT_NEGATIVE_TESTS)
    root = Path(__file__).resolve().parents[2]
    expected = read_tsv(root / RECURSIVE_ADT_SOURCE / "rust-negative-matrix.tsv")[1]
    actual_lines = marker_lines(text, "ISSUE2039_NEGATIVE\t")
    actual: dict[str, tuple[str, str, str]] = {}
    for line in actual_lines:
        fields = line.split("\t")
        if len(fields) != 5 or fields[1] in actual:
            raise ReceiptError("recursive Expr negative receipt is malformed or duplicated")
        actual[fields[1]] = (fields[2], fields[3], fields[4])
    if len(expected) != 15 or len(actual_lines) != 15:
        raise ReceiptError("recursive Expr negative matrix did not execute all 15 named cases")
    for row in expected:
        observed = actual.get(row["case"])
        if observed != (row["root"], row["scala_result"], row["rust_result"]):
            raise ReceiptError(f"recursive Expr negative receipt changed for {row['case']}")
    rollback_lines = marker_lines(text, "ISSUE2040_ROLLBACK\t")
    expected_rollback = {
        "late_result_mismatch": "observed=Succ.inner:Expr,Evaluator.eval(inner):Int,Match,CaseDef[0..1];restored=store,resolver,typed_arena,SourceTypedIndex,scope_projection,binder_frames",
        "same_session_retry": "repeated_diagnostic=stable;recovered=Evaluator.unwrap(expr:Expr):Expr,Evaluator.eval(expr:Expr):Int;fresh_session=normalized_equal",
    }
    if len(rollback_lines) != 2:
        raise ReceiptError("recursive Expr rollback proof must contain the late failure and same-session retry")
    actual_rollback = {}
    for line in rollback_lines:
        fields = line.split("\t")
        if len(fields) != 3 or fields[1] in actual_rollback:
            raise ReceiptError("recursive Expr rollback receipt is malformed or duplicated")
        actual_rollback[fields[1]] = fields[2]
    if actual_rollback != expected_rollback:
        raise ReceiptError("recursive Expr rollback/retry receipt differs from the observed contract")
    sidecar_path = root / "docs/typer-recursive-adt-e2e-negative-rollback-3.9.0.tsv"
    header, rows = read_tsv(sidecar_path)
    if len(rows) != 17 or header != ["case", "scala_classification", "rust_outcome", "evidence"]:
        raise ReceiptError("recursive Expr negative/rollback sidecar has the wrong schema or inventory")
    expected_rows = {
        (row["case"], row["scala_result"], row["rust_result"], row["rust_result"])
        for row in expected
    } | {
        (case, "rejected", "rolled_back_then_retry_succeeded", evidence)
        for case, evidence in expected_rollback.items()
    }
    if {tuple(row[column] for column in header) for row in rows} != expected_rows:
        raise ReceiptError("committed recursive Expr negative/rollback sidecar differs from executed evidence")


def _write_tsv(path: Path, header: list[str], rows: list[tuple[str, ...]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
        writer.writerow(header)
        writer.writerows(rows)


def validate_binary_recursive_adt_inputs(root: Path) -> None:
    fixture = root / BINARY_ADT_SOURCE
    for name, expected_hash in (
        ("source-hashes.tsv", BINARY_SOURCE_MANIFEST_SHA256),
        ("output-sha256.tsv", BINARY_OUTPUT_MANIFEST_SHA256),
        ("scala-artifacts.sha256.tsv", BINARY_ARTIFACT_MANIFEST_SHA256),
    ):
        if hashlib.sha256((fixture / name).read_bytes()).hexdigest() != expected_hash:
            raise ReceiptError(f"binary recursive Expr pinned manifest changed: {name}")
    source_rows = read_tsv(fixture / "source-hashes.tsv")[1]
    source_hashes = {row["file"]: row["sha256"] for row in source_rows}
    scala_files = {path.name for path in fixture.glob("*.scala")}
    if len(source_hashes) != len(source_rows) or set(source_hashes) != scala_files:
        raise ReceiptError("binary recursive Expr source hash inventory changed")
    for name, expected in source_hashes.items():
        if hashlib.sha256((fixture / name).read_bytes()).hexdigest() != expected:
            raise ReceiptError(f"binary recursive Expr source hash changed: {name}")
    output_rows = read_tsv(fixture / "output-sha256.tsv")[1]
    for row in output_rows:
        path = fixture / row["file"]
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
            raise ReceiptError(f"binary recursive Expr output hash changed: {row['file']}")
    artifacts = [line.split("\t") for line in (fixture / "scala-artifacts.sha256.tsv").read_text(encoding="utf-8").splitlines()]
    if len(artifacts) != 8 or any(len(row) != 2 or not re.fullmatch(r"[0-9a-f]{64}", row[0]) for row in artifacts):
        raise ReceiptError("binary recursive Expr oracle must pin exactly eight valid artifact hashes")
    control_roots = read_tsv(fixture / "rust-control-roots.tsv")[1]
    scala = read_tsv(fixture / "scala-matrix.tsv")[1]
    rust = read_tsv(fixture / "rust-control-matrix.tsv")[1]
    structural = read_tsv(fixture / "rust-structural-controls.tsv")[1]
    if (
        len(control_roots) != 22
        or len(scala) != 22
        or len(rust) != 22
        or len(structural) != 8
        or {row["case"] for row in control_roots} != {row["source"] for row in scala}
        or {row["case"] for row in control_roots} != {row["case"] for row in rust}
        or len({row["case"] for row in structural}) != 8
    ):
        raise ReceiptError("binary recursive Expr source/Scala/Rust/structural control inventory changed")


def validate_binary_recursive_adt_oracle_log(path: Path, root: Path) -> None:
    fixture = root / BINARY_ADT_SOURCE
    source_sha = hashlib.sha256((fixture / "Expr.scala").read_bytes()).hexdigest()
    required = {
        "verified all 8 pinned Scala 3.9.0 artifacts",
        f"verified immutable positive source SHA-256: {source_sha}",
        f"Scala source revision: {SCALA_REVISION}",
        'JDK: JAVA_VERSION="21.0.8"; IMPLEMENTOR_VERSION="Temurin-21.0.8+9"',
        "verified 22 classified controls and rationales",
        "verified pinned typed snapshot and diagnostic output hashes",
        "Two original source roots and 22 Scala control classifications match.",
    }
    actual = set(path.read_text(encoding="utf-8").splitlines())
    if required - actual:
        raise ReceiptError(f"binary recursive Expr pinned Scala runner is missing evidence: {sorted(required - actual)!r}")


def parse_binary_semantic_proof(method: str, proof: str) -> dict[str, str]:
    actual: dict[str, str] = {}
    for part in proof.split(";"):
        key, separator, value = part.partition("=")
        if not separator or not key or not value:
            raise ReceiptError(f"{method}: semantic proof has a malformed key/value field")
        if key in actual:
            raise ReceiptError(f"{method}: semantic proof duplicates field {key!r}")
        actual[key] = value
    common = {
        "Expr": "source-sealed-trait",
        "Num/Add": "source-case-class",
        "Add.parents": "Expr+scala.Product",
        "Add.fields": "left/right:Expr,distinct,class-owned",
        "Add.selectors": "_1/_2:Expr,distinct,class-owned",
        "no": "_3",
    }
    expected = {
        "selectLeft": {
            "root": "selectLeft@158:271",
            "signature": "(Expr):Expr",
            "selector": "expr:Expr",
            "cases": "Num(_)=>expr,Add(left,_2)=>left",
            "Add.unapply": "companion-owned(Add):Add",
            "selectors": "_1(left):Expr,_2(right):Expr",
            "binder": "left@Add._1:case-local",
            "wildcard": "Add._2:unbound",
            "mapped_source_trees": "19",
            **common,
        },
        "eval": {
            "root": "eval@275:410",
            "signature": "(Expr):Int",
            "Add.binders": "left@_1:Expr,right@_2:Expr,distinct",
            "calls": "eval(left):Int+eval(right):Int",
            "callee": "Evaluator.eval:same_source_symbol",
            "addition": "Int+Int:Int",
            "match": "Int",
            "mappings": "25",
            "Add.unapply": "companion-owned(Add):Add",
            **common,
        },
    }.get(method)
    if expected is None or actual != expected:
        missing = sorted(set(expected or ()) - set(actual))
        extra = sorted(set(actual) - set(expected or ()))
        wrong = sorted(key for key in set(actual) & set(expected or ()) if actual[key] != expected[key])
        raise ReceiptError(
            f"{method}: semantic proof does not equal the complete canonical proof "
            f"(missing={missing}, extra={extra}, wrong_values={wrong})"
        )
    return actual


def validate_binary_recursive_adt_positive_logs(
    root: Path,
    logs: list[Path],
    tested_sha: str,
    expected_sha: str,
    expected_head_sha: str,
    baseline_sha: str,
    output: Path,
) -> None:
    validate_binary_recursive_adt_inputs(root)
    if not re.fullmatch(r"[0-9a-f]{40}", tested_sha) or tested_sha != expected_sha or tested_sha != expected_head_sha:
        raise ReceiptError("binary recursive Expr evidence is not bound to the evaluated PR head")
    if not re.fullmatch(r"[0-9a-f]{40}", baseline_sha):
        raise ReceiptError("binary recursive Expr evidence contains a malformed baseline SHA")
    if len(logs) != 1:
        raise ReceiptError("binary recursive Expr proof must come from one integrated Rust test log")
    require_test_logs(logs, BINARY_ADT_POSITIVE_TESTS)
    actual: dict[tuple[str, str], tuple[str, str, str]] = {}
    text = logs[0].read_text(encoding="utf-8")
    tested_markers = marker_lines(text, "ISSUE2080_TESTED_SHA\t")
    if tested_markers != [f"ISSUE2080_TESTED_SHA\t{tested_sha}"]:
        raise ReceiptError(f"{logs[0]}: positive evidence does not carry the exact tested SHA")
    session_markers = marker_lines(text, "ISSUE2080_SESSION\t")
    session_witnesses: dict[str, tuple[str, ...]] = {}
    for line in session_markers:
        fields = line.split("\t")
        if len(fields) != 7 or fields[1] in session_witnesses:
            raise ReceiptError("binary recursive Expr session witness is malformed or duplicated")
        witness = tuple(fields[2:6])
        if any(not re.fullmatch(r"(?:owner|store|ast|index)=0x[0-9a-f]+", value) for value in witness):
            raise ReceiptError("binary recursive Expr session witness contains a malformed owner address")
        if not re.fullmatch(r"source=SourceId\([0-9]+\)", fields[6]):
            raise ReceiptError("binary recursive Expr session witness contains a malformed source identity")
        witness += (fields[6],)
        session_witnesses[fields[1]] = witness
    if set(session_witnesses) != {"session-a", "session-b"}:
        raise ReceiptError("binary recursive Expr requires two identified owned sessions")
    for slot in range(5):
        if session_witnesses["session-a"][slot] == session_witnesses["session-b"][slot]:
            raise ReceiptError("binary recursive Expr sessions share an ownership/store/AST/index witness")
    for line in marker_lines(text, "ISSUE2080_POSITIVE\t"):
        fields = line.split("\t")
        if len(fields) != 6:
            raise ReceiptError("binary recursive Expr positive row is malformed")
        _, session, method, source_path_tree, span, proof = fields
        key = session, method
        if key in actual or (session, method, source_path_tree, span) not in BINARY_ADT_POSITIVE_ROWS:
            raise ReceiptError("binary recursive Expr positive row is duplicated or substituted")
        parse_binary_semantic_proof(method, proof)
        actual[key] = (source_path_tree, span, proof)
    expected = {(session, method) for session in ("session-a", "session-b") for method in ("selectLeft", "eval")}
    if len(actual) != 4 or set(actual) != expected:
        raise ReceiptError("binary recursive Expr acceptance requires exactly four original roots in two sessions")
    for method in ("selectLeft", "eval"):
        if actual[("session-a", method)] != actual[("session-b", method)]:
            raise ReceiptError(f"{method}: normalized semantic proof differs across fresh sessions")
    header = ["session", "root", "source_path_tree", "source_span", "normalized_semantic_sha256", "proof", "baseline_sha", "tested_sha", "pr_head_sha"]
    rows = []
    for session, method in ((s, m) for s in ("session-a", "session-b") for m in ("selectLeft", "eval")):
        source_path_tree, span, proof = actual[(session, method)]
        rows.append((session, method, source_path_tree, span, hashlib.sha256(proof.encode()).hexdigest(), proof, baseline_sha, tested_sha, expected_head_sha))
    _write_tsv(output, header, rows)


def validate_binary_recursive_adt_negative_logs(
    root: Path,
    control_log: Path,
    structural_log: Path,
    rollback_log: Path,
    baseline_sha: str,
    tested_sha: str,
    expected_head_sha: str,
    output: Path,
) -> None:
    validate_binary_recursive_adt_inputs(root)
    for sha in (baseline_sha, tested_sha, expected_head_sha):
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            raise ReceiptError("binary recursive Expr negative receipt contains a malformed revision SHA")
    if tested_sha != expected_head_sha:
        raise ReceiptError("binary recursive Expr negative evidence does not match the PR head")
    for log in (control_log, structural_log, rollback_log):
        markers = marker_lines(log.read_text(encoding="utf-8"), "ISSUE2080_TESTED_SHA\t")
        if markers != [f"ISSUE2080_TESTED_SHA\t{tested_sha}"]:
            raise ReceiptError(f"{log}: negative evidence does not carry the exact tested SHA")
    control_text = control_log.read_text(encoding="utf-8")
    if BINARY_ADT_CONTROL_TEST - passed_tests(control_text):
        raise ReceiptError("binary recursive source controls did not execute in the required Rust negative suite")
    fixture = root / BINARY_ADT_SOURCE
    rust_rows = read_tsv(fixture / "rust-control-matrix.tsv")[1]
    source_rows = read_tsv(fixture / "rust-control-roots.tsv")[1]
    scala_rows = read_tsv(fixture / "scala-matrix.tsv")[1]
    expected_control_lines = ["ISSUE2079_BINARY_CONTROL\t" + "\t".join(row[column] for column in read_tsv(fixture / "rust-control-matrix.tsv")[0]) for row in rust_rows]
    actual_control_lines = marker_lines(control_text, "ISSUE2079_BINARY_CONTROL\t")
    if actual_control_lines != expected_control_lines:
        raise ReceiptError("binary recursive Expr controls were missing, reordered, duplicated, or changed")

    structural_text = require_test_log(structural_log, BINARY_ADT_STRUCTURAL_TEST)
    structural_rows = read_tsv(fixture / "rust-structural-controls.tsv")[1]
    expected_structural_lines = ["ISSUE2079_STRUCTURAL_CONTROL\t" + "\t".join(row[column] for column in read_tsv(fixture / "rust-structural-controls.tsv")[0]) for row in structural_rows]
    if marker_lines(structural_text, "ISSUE2079_STRUCTURAL_CONTROL\t") != expected_structural_lines:
        raise ReceiptError("binary recursive Expr structural rollback controls were missing or changed")

    rollback_text = require_test_log(rollback_log, RECURSIVE_ADT_NEGATIVE_TESTS)
    required_rollback = (
        "ISSUE2078_BINARY_EVAL_FAILURES\tsecond_argument\tApplicationArgumentTypeMismatch(Boolean,Expr);first_call=observed;second_call=unpublished;rollback=both_calls,arguments,binders;retry=original_eval",
        "late_result\tExpectedExpressionTypeMismatch;observed=eval(left):canonical,eval(right):canonical;rollback=both_calls,arguments,binders;retry=original_eval",
        "ISSUE2080_ROLLBACK\tlate_after_two_calls\tbefore=both_Add_Expr_binders_and_both_eval_Applies_same_canonical_method_distinct_argument_binders;restored=complete_state_projection+resolver+typed_ast+types+SourceTypedIndex+scope_buckets;retry=stable_failure_then_selectLeft_and_eval_recovered_in_session;fresh_session=normalized_equal",
    )
    if marker_lines(rollback_text, "ISSUE2078_BINARY_EVAL_FAILURES\t") != [required_rollback[0]]:
        raise ReceiptError("binary recursive Expr second-argument rollback receipt is missing, duplicated, or changed")
    if rollback_text.splitlines().count(required_rollback[1]) != 1:
        raise ReceiptError("binary recursive Expr late-result receipt is missing, duplicated, or changed")
    if marker_lines(rollback_text, "ISSUE2080_ROLLBACK\t") != [required_rollback[2]]:
        raise ReceiptError("binary recursive Expr full-state rollback receipt is missing, duplicated, or changed")
    root_by_case = {row["case"]: row["root"] for row in source_rows}
    scala_by_case = {row["source"]: row["classification"] for row in scala_rows}
    sidecar_rows: list[tuple[str, ...]] = []
    for row in rust_rows:
        if row["case"] not in root_by_case or root_by_case[row["case"]] != row["root"] or scala_by_case[row["case"]] != row["scala_classification"]:
            raise ReceiptError(f"{row['case']}: Rust and Scala control classifications diverged")
        evidence = (
            f"first_outcome={row['first_outcome']};first_span={row['first_span']};"
            f"first_rust_receipt={row['first_rust_receipt']};fresh_outcome={row['fresh_outcome']};"
            f"fresh_span={row['fresh_span']};fresh_rust_receipt={row['fresh_rust_receipt']}"
        )
        retry = f"first={row['first_retry']};fresh={row['fresh_retry']}"
        sidecar_rows.append((row["case"], row["scala_classification"], row["rust_outcome"], f"{row['root']}:{row['first_span']}", evidence, retry, baseline_sha, tested_sha, expected_head_sha))
    for row in structural_rows:
        marker = "ISSUE2079_STRUCTURAL_CONTROL\t" + "\t".join(row[column] for column in read_tsv(fixture / "rust-structural-controls.tsv")[0])
        if marker not in structural_text:
            raise ReceiptError(f"{row['case']}: structural corruption receipt did not execute")
        sidecar_rows.append(("structural:" + row["case"], "not applicable", "rejected", row["failure_root"], row["diagnostic"], row["rollback_retry"], baseline_sha, tested_sha, expected_head_sha))
    sidecar_rows.extend([
        ("second_argument_after_first_call", "rejected", "rolled_back_then_retry_succeeded", "Add(left,right)", required_rollback[0].split("\t", 2)[2], "first call/binders observed; invalid second call unpublished", baseline_sha, tested_sha, expected_head_sha),
        ("late_result_after_two_calls", "rejected", "rolled_back_then_retry_succeeded", "Add(left,right)", required_rollback[1].split("\t", 1)[1], "both canonical calls/binders observed before failure", baseline_sha, tested_sha, expected_head_sha),
        ("full_state_late_rollback", "rejected", "complete_state_restored_and_recovered", "Add(left,right)", required_rollback[2].split("\t", 2)[2], "same-session and independent-session recovery", baseline_sha, tested_sha, expected_head_sha),
    ])
    _write_tsv(output, ["case", "scala_classification", "rust_outcome", "source_root_or_span", "first_error_or_evidence", "retry_and_rollback", "baseline_sha", "tested_sha", "pr_head_sha"], sidecar_rows)


def validate_binary_recursive_adt_cohort_receipt(
    root: Path,
    source: Path,
    baseline_sha: str,
    tested_sha: str,
    expected_head_sha: str,
    output: Path,
) -> None:
    if any(not re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (baseline_sha, tested_sha, expected_head_sha)):
        raise ReceiptError("binary recursive Expr cohort contains a malformed revision SHA")
    if tested_sha != expected_head_sha:
        raise ReceiptError("binary recursive Expr cohort is not bound to the evaluated PR head")
    recorded = read_tsv(root / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv")[1]
    rows = read_tsv(source)[1]
    if len(rows) != 50 or len(recorded) != 50:
        raise ReceiptError("protected stdlib proof must contain the original exact 25 + 25 roots")
    if [(r["cohort"], r["identity"], r["source_tree"]) for r in rows] != [(r["cohort"], r["identity"], r["source_tree"]) for r in recorded]:
        raise ReceiptError("two-pass cohort receipt changed the original source identities or order")
    protected_paths = {
        "main": root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv",
        "analysis": root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv",
    }
    protected_ids = {
        cohort: {row["identity"] for row in read_tsv(path)[1]}
        for cohort, path in protected_paths.items()
    }
    actual_protected = {"main": set(), "analysis": set()}
    for row in rows:
        if row["first_status"] != row["second_status"] or row["two_pass_equal"] != "true":
            raise ReceiptError(f"{row['identity']}: deterministic cohort passes disagree")
        if row["protected_success"] not in {"true", "false"}:
            raise ReceiptError(f"{row['identity']}: protected-success flag is malformed")
        if row["protected_success"] == "true":
            if row["first_status"] != "typed" or row["second_status"] != "typed":
                raise ReceiptError(f"{row['identity']}: an accepted protected root regressed")
            actual_protected[row["cohort"]].add(row["identity"])
        elif row["gate"] == "required_typed":
            raise ReceiptError(f"{row['identity']}: required_typed root was not marked protected")
    if actual_protected != protected_ids or {name: len(values) for name, values in actual_protected.items()} != {"main": 16, "analysis": 15}:
        raise ReceiptError("two-pass cohort receipt changed the protected 16 + 15 root sets")
    _write_tsv(
        output,
        ["cohort", "identity", "source_tree", "baseline_status", "gate", "first_status", "second_status", "actual_result", "protected_success", "two_pass_equal", "baseline_sha", "tested_sha", "pr_head_sha"],
        [tuple(row[key] for key in ("cohort", "identity", "source_tree", "baseline_status", "gate", "first_status", "second_status", "actual_result", "protected_success", "two_pass_equal")) + (baseline_sha, tested_sha, expected_head_sha) for row in rows],
    )


def validate_sealed_rollback_sidecar(root: Path) -> None:
    fixture = root / "crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0"
    rust_failures = read_tsv(fixture / "rust-failure-matrix.tsv")[1]
    scala_failures = read_tsv(fixture / "scala-failure-matrix.tsv")[1]
    structural = read_tsv(fixture / "rust-structural-lifecycle.tsv")[1]
    scala_by_case = {row["case"]: row for row in scala_failures}
    if len(rust_failures) != 9 or len(scala_by_case) != 9 or len(structural) != 6:
        raise ReceiptError("sealed Event rollback fixture inventory changed")
    expected = set()
    for row in rust_failures:
        case = row["case"]
        scala = scala_by_case.get(case)
        if scala is None or scala["Scala result"] != "rejected":
            raise ReceiptError(f"{case}: Rust and Scala rollback controls do not align")
        transaction = (
            "partial Match and id/code binders observed; full rollback, stable retry, and both roots recovered"
            if case == "wrong_declared_result_after_cases"
            else "failed root retried; both canonical roots recovered in the same typer"
        )
        expected.add((
            case, row["method root"], row["Typer diagnostic"],
            scala["Scala result"], scala["severity"], transaction,
        ))
    for row in structural:
        expected.add((
            f"structural:{row['corruption']}", "Event.scala#tree36+tree60",
            row["first error"], "not applicable", "not applicable",
            f"recovered roots: {row['recovered roots']}",
        ))
    sidecar = read_tsv(root / "docs/typer-sealed-adt-e2e-negative-rollback-3.9.0.tsv")[1]
    actual = {
        (
            row["case"], row["source_root"], row["rust_first_error"],
            row["scala_result"], row["scala_first_severity"], row["rollback_and_retry"],
        )
        for row in sidecar
    }
    if len(sidecar) != 15 or actual != expected:
        raise ReceiptError("sealed Event negative/rollback sidecar differs from frozen Rust/Scala matrices")


def validate_sealed_cohort_contract(root: Path) -> None:
    expected = {
        "main": (
            16,
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv",
            "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv",
            "two deterministic passes retain all 16 exact protected roots and results",
        ),
        "analysis": (
            15,
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv",
            "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv",
            "two deterministic passes retain all 15 exact protected roots and results",
        ),
    }
    rows = read_tsv(root / "docs/typer-sealed-adt-e2e-cohorts-3.9.0.tsv")[1]
    if len(rows) != 2 or {row["cohort"] for row in rows} != set(expected):
        raise ReceiptError("sealed Event cohort contract must list main and analysis exactly once")
    for row in rows:
        count, protected_path, cohort_path, requirement = expected[row["cohort"]]
        protected = root / protected_path
        cohort = root / cohort_path
        protected_rows = read_tsv(protected)[1]
        cohort_rows = read_tsv(cohort)[1]
        if (
            row["denominator"] != "25"
            or row["protected_success_count"] != str(count)
            or len(protected_rows) != count
            or len(cohort_rows) != 25
            or row["protected_manifest_sha256"] != hashlib.sha256(protected.read_bytes()).hexdigest()
            or row["cohort_manifest_sha256"] != hashlib.sha256(cohort.read_bytes()).hexdigest()
            or row["pass_requirement"] != requirement
        ):
            raise ReceiptError(f"{row['cohort']}: frozen protected roots or cohort provenance changed")


def validate_recorded_cohort_receipt(root: Path) -> None:
    path = root / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
    header, rows = read_tsv(path)
    required_header = (
        "cohort", "identity", "source_tree", "baseline_status", "gate", "first_status",
        "second_status", "actual_result", "protected_success", "two_pass_equal",
    )
    if tuple(header) != required_header or len(rows) != 50:
        raise ReceiptError("recorded ordinary cohort receipt must contain the exact 25 + 25 schema")
    manifests = {
        "main": root / "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv",
        "analysis": root / "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv",
    }
    protected_paths = {
        "main": root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv",
        "analysis": root / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv",
    }
    expected_rows = []
    for cohort in ("main", "analysis"):
        roots = read_tsv(manifests[cohort])[1]
        protected_rows = read_tsv(protected_paths[cohort])[1]
        protected = {row["identity"] for row in protected_rows}
        if len(roots) != 25:
            raise ReceiptError(f"{cohort}: frozen cohort must contain exactly 25 roots")
        for frozen in roots:
            identity = frozen["identity"]
            baseline_status = frozen.get("baseline_status") or ("typed" if identity in protected else "not_protected")
            gate = frozen.get("gate") or ("required_typed" if identity in protected else "observe")
            expected_rows.append((cohort, identity, frozen.get("source_tree") or identity.rsplit("=", 1)[-1], baseline_status, gate))
    identity_gate_rows = [
        (row["cohort"], row["identity"], row["source_tree"], row["baseline_status"], row["gate"])
        for row in rows
    ]
    if identity_gate_rows != expected_rows:
        raise ReceiptError("recorded ordinary cohort receipt changed a frozen root identity, tree, or gate")
    for row in rows:
        if row["first_status"] != row["second_status"] or row["two_pass_equal"] != "true":
            raise ReceiptError(f"{row['identity']}: recorded deterministic passes disagree")
        if row["protected_success"] not in {"true", "false"}:
            raise ReceiptError(f"{row['identity']}: protected-success status is malformed")
        if row["gate"] == "required_typed" and (row["first_status"] != "typed" or row["protected_success"] != "true"):
            raise ReceiptError(f"{row['identity']}: recorded protected root lost typed acceptance")


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


def validate_named_test_log(path: Path, expected: set[str]) -> None:
    require_test_log(path, expected)


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


def validate_sealed_adt_receipts(text: str) -> None:
    sealed_lines = marker_lines(text, "ISSUE2000_POSITIVE\t")
    sealed_positive = {tuple(line.split("\t")[1:]) for line in sealed_lines}
    if len(sealed_lines) != 4 or sealed_positive != SEALED_ADT_POSITIVE:
        raise ReceiptError("sealed Event receipt must contain exactly extract/classify in two fresh sessions")
    sealed_rows = read_tsv(Path("docs/typer-sealed-adt-e2e-positive-3.9.0.tsv"))[1]
    expected_sealed_rows = set()
    for session, root, receipt in SEALED_ADT_POSITIVE:
        source_tree, span, *_ = receipt.split("|")
        expected_sealed_rows.add((
            session, root, source_tree, span,
            hashlib.sha256(f"{root}\t{receipt}".encode()).hexdigest(),
            "complete DefDef+Match; Event selector; sealed variants; Product._1; canonical unapply; typed binders; all source RHS mappings",
        ))
    actual_sealed_rows = {
        (
            row["session"], row["root"], row["source_path_tree"], row["source_span"],
            row["normalized_receipt_sha256"], row["proof"],
        )
        for row in sealed_rows
    }
    if len(sealed_rows) != 4 or actual_sealed_rows != expected_sealed_rows:
        raise ReceiptError("committed sealed Event sidecar differs from the four executed fresh-session receipts")


def validate_rust_log(path: Path, tested_sha: str, expected_sha: str) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", tested_sha) or not re.fullmatch(r"[0-9a-f]{40}", expected_sha):
        raise ReceiptError("tested and expected source revisions must be full lowercase Git SHAs")
    if tested_sha != expected_sha:
        raise ReceiptError(f"audited checkout {tested_sha} does not match expected PR head {expected_sha}")
    text = require_test_log(path, RUST_TESTS)
    revision_rows = marker_lines(text, "ISSUE2000_TESTED_SHA\t")
    if revision_rows != [f"ISSUE2000_TESTED_SHA\t{tested_sha}"]:
        raise ReceiptError("Rust evidence does not contain the exact tested source SHA")
    positive_lines = marker_lines(text, "ISSUE1938_POSITIVE\t")
    actual_positive = {tuple(line.split("\t")[1:]) for line in positive_lines}
    if len(positive_lines) != 4 or actual_positive != POSITIVE:
        raise ReceiptError("positive receipt must contain the exact two roots in two sessions")
    validate_sealed_adt_receipts(text)
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


def validate_product2_log(path: Path) -> None:
    text = require_test_log(path, RUST_TESTS)
    lines = marker_lines(text, "ISSUE1972_POSITIVE\t")
    actual: dict[str, dict[str, tuple[str, ...]]] = {"first": {}, "second": {}}
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 16 or fields[1] not in actual:
            raise ReceiptError("Product2 positive receipt has malformed columns or session")
        session, row = fields[1], tuple(fields[2:])
        name = row[0]
        if name not in PRODUCT2_RUST_ROOTS or name in actual[session]:
            raise ReceiptError("Product2 receipt has an unexpected or duplicated root")
        if "\t".join(row) != PRODUCT2_RUST_ROOTS[name]:
            raise ReceiptError(f"Product2 {name} receipt differs from the frozen semantic contract")
        actual[session][name] = row
    expected_names = set(PRODUCT2_RUST_ROOTS)
    if any(set(rows) != expected_names for rows in actual.values()) or len(lines) != 4:
        raise ReceiptError("Product2 gate requires exactly select/classify in two sessions")
    if actual["first"] != actual["second"]:
        raise ReceiptError("Product2 sessions differ in normalized semantic identity")
    header, committed = read_tsv(Path("docs/typer-heterogeneous-product2-match-positive-3.9.0.tsv"))
    if len(header) != 17 or len(committed) != 4:
        raise ReceiptError("Product2 positive sidecar has the wrong schema or row count")
    expected_rows = set()
    for session, roots in actual.items():
        for name, row in roots.items():
            canonical = "\t".join(row)
            expected_rows.add((session, *row, hashlib.sha256(canonical.encode()).hexdigest(), "3a854831b9c074261e31e82d325c0350f606b95a"))
    sidecar_rows = {tuple(row[column] for column in header) for row in committed}
    if sidecar_rows != expected_rows:
        raise ReceiptError("Product2 positive sidecar disagrees with fresh-session receipts")


def validate_product2_oracle_log(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    required = {
        "verified eight pinned Scala 3.9.0 artifacts",
        f"Scala 3.9.0 source revision: {SCALA_REVISION}",
        'JDK: JAVA_VERSION="21.0.8"',
        "Positive typed snapshot and negative matrix match.",
        "verified frozen source SHA-256 manifest",
    }
    if required - set(text.splitlines()):
        raise ReceiptError(f"Product2 Scala oracle log is missing pinned evidence: {sorted(required - set(text.splitlines()))!r}")
    matrix = (Path(__file__).resolve().parents[2] / PRODUCT2_FIXTURE / "scala-matrix.tsv").read_text(encoding="utf-8").splitlines()
    if len(matrix) != len(PRODUCT2_CASES) or len({line.split("\t", 1)[0] for line in matrix}) != len(PRODUCT2_CASES):
        raise ReceiptError("Product2 oracle did not run every unique frozen control")


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
    validate_scala_matrix(matrix_lines)


def parse_ordinary_report(path: Path) -> dict[str, dict[str, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    marker = "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
    try:
        start = next(index for index, line in enumerate(lines) if line.startswith(marker))
    except StopIteration as error:
        raise ReceiptError(f"{path}: ordinary report header missing") from error
    rows = csv.DictReader(lines[start:], delimiter="\t")
    return {row["method"]: row for row in rows}


def validate_cohorts(root: Path, reports: list[Path], output: Path, baseline_receipt: Path) -> None:
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
    if not baseline_receipt.is_absolute():
        baseline_receipt = root / baseline_receipt
    recorded_header, recorded_rows = read_tsv(baseline_receipt)
    expected = [dict(zip(header, row, strict=True)) for row in rows_out]
    if recorded_header != list(header):
        raise ReceiptError(
            "committed case-class Match receipt has the wrong schema: "
            f"expected {list(header)!r}, found {recorded_header!r}"
        )

    expected_keys = [(row["cohort"], row["identity"]) for row in expected]
    recorded_keys = [(row["cohort"], row["identity"]) for row in recorded_rows]
    if recorded_keys != expected_keys:
        expected_key_set, recorded_key_set = set(expected_keys), set(recorded_keys)
        changed = [
            f"{cohort}:{identity}"
            for cohort, identity in expected_keys
            if (cohort, identity) not in recorded_key_set
        ]
        changed.extend(
            f"unexpected {cohort}:{identity}"
            for cohort, identity in recorded_keys
            if (cohort, identity) not in expected_key_set
        )
        if not changed:
            changed = [f"{cohort}:{identity} (row order)" for cohort, identity in expected_keys]
        raise ReceiptError(
            "committed case-class Match receipt changed the exact 25 + 25 root set/order: "
            + ", ".join(changed)
        )

    differences = []
    for wanted, found in zip(expected, recorded_rows, strict=True):
        if wanted == found:
            continue
        if wanted["identity"] == EITHER_SWAP_IDENTITY:
            # Keep the checked-in 2026-09 receipt immutable. Permit only the
            # measured #2241 progression to the existing polymorphic extractor
            # boundary in live cohorts, while requiring every non-status field
            # to remain exactly historical.
            historical = dict(wanted)
            historical["first_status"] = EITHER_SWAP_HISTORICAL_BLOCKER
            historical["second_status"] = EITHER_SWAP_HISTORICAL_BLOCKER
            observed = (
                wanted["cohort"] == "main"
                and wanted["source_tree"] == "84"
                and wanted["baseline_status"] == "blocked"
                and wanted["gate"] == "observe"
                and wanted["first_status"] == EITHER_SWAP_LIVE_BLOCKER
                and wanted["second_status"] == EITHER_SWAP_LIVE_BLOCKER
                and wanted["actual_result"] == "not_available"
                and wanted["protected_success"] == "false"
                and wanted["two_pass_equal"] == "true"
            )
            if observed and historical == found:
                continue
        differences.append((wanted, found))
    if differences:
        roots = [f"{wanted['cohort']}:{wanted['identity']}" for wanted, _ in differences]
        raise ReceiptError(
            "live 25 + 25 two-pass receipt differs from committed evidence at: "
            + ", ".join(roots)
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    inputs = sub.add_parser("inputs")
    inputs.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    recursive_inputs = sub.add_parser("recursive-inputs")
    recursive_inputs.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    binary_inputs = sub.add_parser("binary-inputs")
    binary_inputs.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    for command, tests in (("rust-log", RUST_TESTS), ("namer-log", NAMER_TESTS), ("typer-log", TYPER_TESTS)):
        item = sub.add_parser(command)
        item.add_argument("log", type=Path, nargs="+" if command == "typer-log" else None)
        if command == "rust-log":
            item.add_argument("--tested-sha", required=True)
            item.add_argument("--expected-sha", required=True)
    issue1999 = sub.add_parser("issue1999-rust-log")
    issue1999.add_argument("log", type=Path)
    recursive_oracle = sub.add_parser("recursive-oracle-log")
    recursive_oracle.add_argument("log", type=Path)
    recursive_negative_oracle = sub.add_parser("recursive-negative-oracle-log")
    recursive_negative_oracle.add_argument("log", type=Path)
    recursive_positive = sub.add_parser("recursive-positive-log")
    recursive_positive.add_argument("log", type=Path)
    recursive_positive.add_argument("--tested-sha", required=True)
    recursive_positive.add_argument("--expected-sha", required=True)
    recursive_negative = sub.add_parser("recursive-negative-log")
    recursive_negative.add_argument("log", type=Path)
    binary_positive = sub.add_parser("binary-positive-log")
    binary_positive.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    binary_positive.add_argument("--log", type=Path, action="append", required=True)
    binary_positive.add_argument("--tested-sha", required=True)
    binary_positive.add_argument("--expected-sha", required=True)
    binary_positive.add_argument("--expected-head-sha", required=True)
    binary_positive.add_argument("--baseline-sha", required=True)
    binary_positive.add_argument("--output", type=Path, required=True)
    binary_oracle = sub.add_parser("binary-oracle-log")
    binary_oracle.add_argument("log", type=Path)
    binary_oracle.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    binary_negative = sub.add_parser("binary-negative-logs")
    binary_negative.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    binary_negative.add_argument("--control-log", type=Path, required=True)
    binary_negative.add_argument("--structural-log", type=Path, required=True)
    binary_negative.add_argument("--rollback-log", type=Path, required=True)
    binary_negative.add_argument("--baseline-sha", required=True)
    binary_negative.add_argument("--tested-sha", required=True)
    binary_negative.add_argument("--expected-head-sha", required=True)
    binary_negative.add_argument("--output", type=Path, required=True)
    binary_cohorts = sub.add_parser("binary-cohorts")
    binary_cohorts.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    binary_cohorts.add_argument("--source", type=Path, required=True)
    binary_cohorts.add_argument("--baseline-sha", required=True)
    binary_cohorts.add_argument("--tested-sha", required=True)
    binary_cohorts.add_argument("--expected-head-sha", required=True)
    binary_cohorts.add_argument("--output", type=Path, required=True)
    scope_rollback = sub.add_parser("scope-rollback-rust-log")
    scope_rollback.add_argument("log", type=Path)
    oracle = sub.add_parser("oracle-log")
    oracle.add_argument("log", type=Path)
    product2_oracle = sub.add_parser("product2-oracle-log")
    product2_oracle.add_argument("log", type=Path)
    product2 = sub.add_parser("product2-log")
    product2.add_argument("log", type=Path)
    cohorts = sub.add_parser("cohorts")
    cohorts.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    cohorts.add_argument("--report", action="append", type=Path, required=True)
    cohorts.add_argument("--output", type=Path, required=True)
    cohorts.add_argument("--baseline-receipt", type=Path, required=True)
    args = parser.parse_args()
    try:
        root = getattr(args, "repo_root", Path(__file__).resolve().parents[2])
        validate_inputs(root)
        if args.command == "inputs":
            print("frozen Box and sealed Event sources, classifications, and receipts verified")
        elif args.command == "recursive-inputs":
            validate_recursive_adt_inputs(root)
            print("frozen recursive Expr source, Scala artifacts, oracle outputs, and 15-case matrix verified")
        elif args.command == "binary-inputs":
            validate_binary_recursive_adt_inputs(root)
            print("frozen binary recursive Expr source, eight artifacts, typed tree, and control matrices verified")
        elif args.command == "recursive-oracle-log":
            validate_recursive_adt_oracle_log(args.log)
            print("pinned recursive Expr Scala/JDK positive oracle and artifact hashes verified")
        elif args.command == "recursive-negative-oracle-log":
            validate_recursive_adt_negative_oracle_log(args.log)
            print("all 15 pinned recursive Expr Scala negative/control cases executed")
        elif args.command == "recursive-positive-log":
            validate_recursive_adt_positive_log(args.log, args.tested_sha, args.expected_sha)
            print("exact four original recursive Expr DefDef receipts and tested SHA verified")
        elif args.command == "recursive-negative-log":
            validate_recursive_adt_negative_log(args.log)
            print("15 recursive Expr negative cases and late rollback/retry proof verified")
        elif args.command == "binary-positive-log":
            validate_binary_recursive_adt_positive_logs(
                root, args.log, args.tested_sha, args.expected_sha,
                args.expected_head_sha, args.baseline_sha, args.output,
            )
            print("exact four original binary recursive ADT DefDefs and revision-bound receipts verified")
        elif args.command == "binary-oracle-log":
            validate_binary_recursive_adt_inputs(root)
            validate_binary_recursive_adt_oracle_log(args.log, root)
            print("binary recursive ADT Scala 3.9.0/JDK and eight artifact hashes verified")
        elif args.command == "binary-negative-logs":
            validate_binary_recursive_adt_negative_logs(
                root, args.control_log, args.structural_log, args.rollback_log,
                args.baseline_sha, args.tested_sha, args.expected_head_sha, args.output,
            )
            print("all Scala-classified binary controls, structural negatives, and two-call rollback verified")
        elif args.command == "binary-cohorts":
            validate_binary_recursive_adt_cohort_receipt(
                root, args.source, args.baseline_sha, args.tested_sha,
                args.expected_head_sha, args.output,
            )
            print("exact 25 + 25 per-root two-pass receipt with 16 + 15 protected roots verified")
        elif args.command == "rust-log":
            validate_rust_log(args.log, args.tested_sha, args.expected_sha)
            print("exact Box and sealed Event roots, negative matrix, and tested SHA verified")
        elif args.command == "oracle-log":
            validate_oracle_log(args.log)
            print("pinned Scala 3.9.0 oracle, snapshots, hashes, and all 19 frozen matrix controls verified")
        elif args.command == "product2-oracle-log":
            validate_product2_oracle_log(args.log)
            print("pinned Scala 3.9.0 Product2 oracle, eight artifacts, and ten-case matrix verified")
        elif args.command == "product2-log":
            validate_product2_log(args.log)
            print("exact Matcher.select/classify Product2 receipts verified in two fresh sessions")
        elif args.command in {"namer-log", "typer-log"}:
            expected = NAMER_TESTS if args.command == "namer-log" else TYPER_TESTS
            if args.command == "namer-log":
                require_test_log(args.log, expected)
            else:
                require_test_logs(args.log, expected)
            print(f"all {len(expected)} exact {args.command[:-4]} extractor regressions passed")
        elif args.command == "issue1999-rust-log":
            validate_named_test_log(args.log, ISSUE1999_TESTS)
            print("both exact #1999 partial-Match rollback tests passed without skips")
        elif args.command == "scope-rollback-rust-log":
            validate_named_test_log(args.log, SCOPE_ROLLBACK_TESTS)
            print("scope rollback regression passed without skips")
        else:
            validate_cohorts(root, args.report, args.output, args.baseline_receipt)
            print("all 25 + 25 deterministic ordinary roots and protected 16 + 15 receipt verified")
    except (OSError, KeyError, ValueError, ReceiptError, csv.Error) as error:
        print(f"source case-class Match gate failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
