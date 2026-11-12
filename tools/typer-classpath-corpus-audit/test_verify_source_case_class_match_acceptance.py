from __future__ import annotations

import csv
import shutil
import tempfile
import unittest
from pathlib import Path

import verify_source_case_class_match_acceptance as gate


class SourceCaseClassMatchAcceptanceTests(unittest.TestCase):
    def write_log(self, path: Path, body: str) -> Path:
        path.write_text(body, encoding="utf-8")
        return path

    def test_exact_test_inventory_rejects_zero_executed_tests(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(
                Path(directory) / "zero.log",
                "running 0 tests\n\n"
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out\n",
            )
            with self.assertRaises(gate.ReceiptError):
                gate.require_test_log(log, {"required_case"})

    def test_namer_inventory_includes_bounded_generic_product1_identity_cases(self) -> None:
        self.assertEqual(
            gate.NAMER_TESTS,
            {
                "class_pattern_bindings_follow_nested_bind_typed_and_unapply_patterns",
                "explicit_companion_unapply_is_preserved_without_generated_identity_collisions",
                "generated_unapply_is_independent_of_case_class_companion_declaration_order",
                "generic_product1_case_classes_publish_only_canonical_unapply_identity",
                "generic_unapply_identity_rejects_shapes_outside_the_bounded_product1_rule",
                "incomparable_generic_companion_unapply_fails_without_publishing_a_candidate",
                "ordinary_class_companion_does_not_receive_generated_unapply",
                "unknown_explicit_unapply_parameter_identity_defers_generation_for_alias_contexts",
            },
        )

    def test_single_thread_nocapture_logs_keep_tests_with_interleaved_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(
                Path(directory) / "nocapture.log",
                "running 1 test\n"
                "test receipt_case ... ISSUE1938_POSITIVE\tfirst\tunwrap\n"
                "ISSUE1938_POSITIVE\tsecond\tunwrap\n"
                "ok\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out\n",
            )
            self.assertEqual(gate.passed_tests(log.read_text(encoding="utf-8")), {"receipt_case"})
            gate.require_test_log(log, {"receipt_case"})

    def test_single_thread_nocapture_failed_test_is_not_counted_as_passed(self) -> None:
        self.assertEqual(
            gate.passed_tests("test failed_case ... marker output\nFAILED\n"),
            set(),
        )

    def test_receipt_markers_survive_the_rust_test_prefix(self) -> None:
        log = (
            "test receipt_case ... ISSUE1938_POSITIVE\tfirst\tunwrap\n"
            "ISSUE1938_POSITIVE\tsecond\tunwrap\n"
        )
        self.assertEqual(
            gate.marker_lines(log, "ISSUE1938_POSITIVE\t"),
            ["ISSUE1938_POSITIVE\tfirst\tunwrap", "ISSUE1938_POSITIVE\tsecond\tunwrap"],
        )

    def test_combined_logs_require_the_exact_nonduplicated_test_set(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            first = self.write_log(
                Path(directory) / "first.log",
                "test first_case ... ok\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out\n",
            )
            second = self.write_log(
                Path(directory) / "second.log",
                "test second_case ... ok\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out\n",
            )
            gate.require_test_logs([first, second], {"first_case", "second_case"})
            with self.assertRaises(gate.ReceiptError):
                gate.require_test_logs([first, second], {"first_case", "missing_case"})

    def test_combined_logs_reject_duplicate_regressions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            body = (
                "test same_case ... ok\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n"
            )
            first = self.write_log(Path(directory) / "first.log", body)
            second = self.write_log(Path(directory) / "second.log", body)
            with self.assertRaises(gate.ReceiptError):
                gate.require_test_logs([first, second], {"same_case"})

    def test_oracle_receipt_rejects_missing_snapshot_or_artifact_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(Path(directory) / "oracle.log", "Bounded extractor acceptance matrix matches.\n")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_oracle_log(log)

    def test_scala_matrix_requires_all_19_unique_controls_and_exact_classifications(self) -> None:
        rows = [f"{name}\t{classification}" for name, classification in gate.SCALA_MATRIX_CASES.items()]
        self.assertEqual(len(rows), 19)
        gate.validate_scala_matrix(rows)
        with self.assertRaises(gate.ReceiptError):
            gate.validate_scala_matrix(rows[:-1])
        with self.assertRaises(gate.ReceiptError):
            gate.validate_scala_matrix([*rows, rows[0]])
        with self.assertRaises(gate.ReceiptError):
            gate.validate_scala_matrix([*rows, "Unreviewed\taccepted"])
        changed_classification = [
            "InputSupertype\trejected" if row.startswith("InputSupertype\t") else row
            for row in rows
        ]
        with self.assertRaises(gate.ReceiptError):
            gate.validate_scala_matrix(changed_classification)

    def sealed_adt_log(self, *, duplicate: bool = False, changed: bool = False) -> str:
        rows = sorted(gate.SEALED_ADT_POSITIVE)
        if changed:
            session, root, receipt = rows[-1]
            rows[-1] = (session, root, receipt.replace("selector=Event", "selector=Created", 1))
        lines = ["ISSUE2000_POSITIVE\t" + "\t".join(row) for row in rows]
        if duplicate:
            lines.append(lines[0])
        return "\n".join(lines) + "\n"

    def test_sealed_event_receipts_require_exact_two_roots_in_two_sessions(self) -> None:
        gate.validate_sealed_adt_receipts(self.sealed_adt_log())

    def test_sealed_event_receipts_reject_missing_duplicate_and_substituted_rows(self) -> None:
        with self.assertRaises(gate.ReceiptError):
            gate.validate_sealed_adt_receipts(self.sealed_adt_log().replace("first\textract\t", "first\tother\t", 1))
        with self.assertRaises(gate.ReceiptError):
            gate.validate_sealed_adt_receipts(self.sealed_adt_log(duplicate=True))
        with self.assertRaises(gate.ReceiptError):
            gate.validate_sealed_adt_receipts(self.sealed_adt_log(changed=True))

    def test_sealed_rollback_and_cohort_sidecars_match_frozen_inputs(self) -> None:
        root = Path(__file__).resolve().parents[2]
        gate.validate_sealed_rollback_sidecar(root)
        gate.validate_sealed_cohort_contract(root)
        gate.validate_recorded_cohort_receipt(root)

    def test_recursive_expr_frozen_inputs_and_positive_sidecar_are_exact(self) -> None:
        root = Path(__file__).resolve().parents[2]
        gate.validate_recursive_adt_inputs(root)
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(Path(directory) / "recursive-positive.log", self.recursive_positive_log())
            gate.validate_recursive_adt_positive_log(log, "a" * 40, "a" * 40)

    def recursive_positive_log(self) -> str:
        lines = ["running 1 test"]
        for session in ("first", "second"):
            for root, tree, span in (("unwrap", "tree36", "147:255"), ("eval", "tree58", "259:375")):
                proof = gate.RECURSIVE_ADT_POSITIVE_PROOFS[root]
                lines.append(
                    f"ISSUE2040_POSITIVE\t{session}\t{root}\tExpr.scala#{tree}\t{span}\t{proof}"
                )
        lines += ["test evaluator_recursive_adt_roots_type_after_generated_unapply_completion ... ok",
                  "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out"]
        lines.insert(1, f"ISSUE2040_TESTED_SHA\t{'a' * 40}")
        return "\n".join(lines) + "\n"

    def test_recursive_expr_positive_receipt_rejects_missing_duplicate_and_changed_rows(self) -> None:
        good = self.recursive_positive_log()
        cases = [
            good.replace("ISSUE2040_POSITIVE\tsecond\teval", "ISSUE2040_POSITIVE\tsecond\tother"),
            good + next(line for line in good.splitlines() if line.startswith("ISSUE2040_POSITIVE\tfirst\tunwrap")) + "\n",
            good.replace("same Method SymbolId", "same-named method"),
        ]
        for index, body in enumerate(cases):
            with tempfile.TemporaryDirectory() as directory:
                log = self.write_log(Path(directory) / f"receipt-{index}.log", body)
                with self.assertRaises(gate.ReceiptError):
                    gate.validate_recursive_adt_positive_log(log, "a" * 40, "a" * 40)

    def test_recursive_expr_negative_sidecar_and_rollback_inventory_are_exact(self) -> None:
        root = Path(__file__).resolve().parents[2]
        sidecar = root / "docs/typer-recursive-adt-e2e-negative-rollback-3.9.0.tsv"
        self.assertTrue(sidecar.is_file())
        _, rows = gate.read_tsv(sidecar)
        self.assertEqual(len(rows), 17)
        self.assertEqual({row["case"] for row in rows if row["case"].startswith("foreign_")},
                         {"foreign_succ_owner", "foreign_eval_wrong_argument", "foreign_eval_control", "foreign_expr_succ_owner"})

    def test_recursive_expr_negative_gate_requires_all_cases_and_all_six_audit_tests(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(Path(directory) / "negative.log", self.recursive_negative_log())
            gate.validate_recursive_adt_negative_log(log)
            missing = self.write_log(Path(directory) / "missing.log", self.recursive_negative_log().replace(
                "ISSUE2039_NEGATIVE\twrong_reference_literal", "OMITTED\twrong_reference_literal"))
            with self.assertRaises(gate.ReceiptError):
                gate.validate_recursive_adt_negative_log(missing)
            skipped = self.write_log(Path(directory) / "skipped.log", "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out\n")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_recursive_adt_negative_log(skipped)

    def recursive_negative_log(self) -> str:
        root = Path(__file__).resolve().parents[2]
        fixture = root / gate.RECURSIVE_ADT_SOURCE / "rust-negative-matrix.tsv"
        lines = ["running 6 tests"]
        for row in gate.read_tsv(fixture)[1]:
            lines.append(f"ISSUE2039_NEGATIVE\t{row['case']}\t{row['root']}\t{row['scala_result']}\t{row['rust_result']}")
        for case, evidence in (
            ("late_result_mismatch", "observed=Succ.inner:Expr,Evaluator.eval(inner):Int,Match,CaseDef[0..1];restored=store,resolver,typed_arena,SourceTypedIndex,scope_projection,binder_frames"),
            ("same_session_retry", "repeated_diagnostic=stable;recovered=Evaluator.unwrap(expr:Expr):Expr,Evaluator.eval(expr:Expr):Int;fresh_session=normalized_equal"),
        ):
            lines.append(f"ISSUE2040_ROLLBACK\t{case}\t{evidence}")
        lines.extend([
            "test typer::tests::method_parameter_capture::recursive_adt_rollback::recursive_eval_late_result_failure_observes_binder_call_and_rolls_back_for_retry ... ok",
            "test typer::tests::method_parameter_capture::recursive_adt_rollback::recursive_adt_negative_matrix_is_stable_across_fresh_sessions ... ok",
            "test typer::tests::method_parameter_capture::recursive_adt_rollback::binary_recursive_second_argument_failure_and_late_failure_restore_then_retry_eval ... ok",
            "test typer::tests::method_parameter_capture::recursive_adt_rollback::binary_late_failure_restores_complete_state_retries_then_types_both_original_roots ... ok",
            "test typer::tests::method_parameter_capture::recursive_adt_rollback::binary_classified_source_controls_have_stable_retry_and_session_receipts ... ok",
            "test typer::tests::method_parameter_capture::recursive_adt_rollback::nested_late_failures_restore_and_retry_before_original_roots_recover ... ok",
            "test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1011 filtered out",
        ])
        return "\n".join(lines) + "\n"

    def test_recursive_expr_oracle_log_rejects_missing_pinned_evidence(self) -> None:
        required = [
            "verified all 8 pinned Scala 3.9.0 artifacts",
            f"Scala source revision: {gate.SCALA_REVISION}",
            'JDK: JAVA_VERSION="21.0.8"; IMPLEMENTOR_VERSION="Temurin-21.0.8+9"',
            "Two-root typed snapshot and 13-case classification matrix match.",
            "verified frozen source SHA-256 manifest",
        ]
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(Path(directory) / "oracle.log", "\n".join(required) + "\n")
            gate.validate_recursive_adt_oracle_log(log)
            log.write_text("\n".join(required[:-1]) + "\n", encoding="utf-8")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_recursive_adt_oracle_log(log)

    def test_recorded_per_root_cohort_receipt_rejects_missing_and_changed_roots(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        paths = (
            "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv",
            "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv",
            "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv",
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv",
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv",
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative in paths:
                destination = root / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(repository / relative, destination)
            receipt = root / paths[0]
            original = receipt.read_text(encoding="utf-8")
            receipt.write_text("\n".join(original.splitlines()[:-1]) + "\n", encoding="utf-8")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_recorded_cohort_receipt(root)
            receipt.write_text(original.replace("library/src/scala/Option.scala#tree=118", "substituted#tree=118", 1), encoding="utf-8")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_recorded_cohort_receipt(root)

    def cohort_reports(self, receipt_rows, directory: Path, overrides=None):
        overrides = overrides or {}
        header = "method\towner_path\tmethod_name\tline\trange\tmodifiers\tstatus\tactual_result\tsource_tree"
        reports = []
        for cohort in ("main", "analysis"):
            cohort_rows = [row for row in receipt_rows if row["cohort"] == cohort]
            for pass_name in ("first", "second"):
                lines = [header]
                for row in cohort_rows:
                    identity = row["identity"]
                    status, actual = overrides.get(identity, (row[f"{pass_name}_status"], row["actual_result"]))
                    lines.append("\t".join((identity, "", "", "0", "", "", status, actual, row["source_tree"])))
                path = directory / f"{cohort}-{pass_name}.tsv"
                path.write_text("\n".join(lines) + "\n", encoding="utf-8")
                reports.append(path)
        return reports

    def set_report_cell(self, reports, report_index: int, identity: str, field: str, value: str) -> None:
        path = reports[report_index]
        lines = path.read_text(encoding="utf-8").splitlines()
        header = lines[0].split("\t")
        column = header.index(field)
        for index, line in enumerate(lines[1:], start=1):
            row = line.split("\t")
            if row[0] == identity:
                row[column] = value
                lines[index] = "\t".join(row)
                path.write_text("\n".join(lines) + "\n", encoding="utf-8")
                return
        self.fail(f"report omitted {identity}")

    def test_live_cohort_receipt_rejects_wrong_schema_and_root_set(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        receipt_rows = gate.read_tsv(baseline)[1]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = self.cohort_reports(receipt_rows, root)
            output = root / "live.tsv"
            bad_schema = root / "bad-schema.tsv"
            lines = baseline.read_text(encoding="utf-8").splitlines()
            lines[0] = lines[0].replace("cohort", "kind", 1)
            bad_schema.write_text("\n".join(lines) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(gate.ReceiptError, "wrong schema"):
                gate.validate_cohorts(repository, reports, output, bad_schema)

            bad_roots = root / "bad-roots.tsv"
            lines = baseline.read_text(encoding="utf-8").splitlines()
            lines[1] = lines[1].replace("Option.scala#tree=118", "Option.scala#tree=999", 1)
            bad_roots.write_text("\n".join(lines) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(gate.ReceiptError, "root set/order"):
                gate.validate_cohorts(repository, reports, output, bad_roots)

    def test_live_cohort_receipt_accepts_exact_per_root_evidence(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        receipt_rows = gate.read_tsv(baseline)[1]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = self.cohort_reports(receipt_rows, root)
            output = root / "live.tsv"
            gate.validate_cohorts(repository, reports, output, baseline)
            self.assertEqual(gate.read_tsv(output), gate.read_tsv(baseline))

    def test_live_cohort_receipt_accepts_only_measured_either_swap_blocker_movement(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        baseline_bytes = baseline.read_bytes()
        receipt_rows = gate.read_tsv(baseline)[1]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = self.cohort_reports(
                receipt_rows,
                root,
                {gate.EITHER_SWAP_IDENTITY: (gate.EITHER_SWAP_LIVE_BLOCKER.replace("491", "630"), "not_available")},
            )
            output = root / "live.tsv"
            gate.validate_cohorts(repository, reports, output, baseline)
            live_rows = gate.read_tsv(output)[1]
            live = next(row for row in live_rows if row["identity"] == gate.EITHER_SWAP_IDENTITY)
            self.assertEqual(live["first_status"], gate.EITHER_SWAP_LIVE_BLOCKER.replace("491", "630"))
            self.assertEqual(live["second_status"], gate.EITHER_SWAP_LIVE_BLOCKER.replace("491", "630"))
            self.assertEqual(live["actual_result"], "not_available")
            self.assertEqual(baseline.read_bytes(), baseline_bytes)

    def test_live_cohort_receipt_rejects_other_either_swap_blocker_drift(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        receipt_rows = gate.read_tsv(baseline)[1]
        invalid_overrides = (
            ("blocked::DifferentFailure", "not_available"),
            (gate.EITHER_SWAP_PREVIOUS_LIVE_BLOCKER, "not_available"),
            (gate.EITHER_SWAP_POLYMORPHIC_BLOCKER, "not_available"),
            (gate.EITHER_SWAP_LIVE_BLOCKER, "scala.util.Either[typeparam#0,typeparam#1]"),
            (gate.EITHER_SWAP_LIVE_BLOCKER.replace("SourceId(0)", "SourceId(1)"), "not_available"),
            (gate.EITHER_SWAP_LIVE_BLOCKER.replace("tree_index: 72", "tree_index: 73"), "not_available"),
            (gate.EITHER_SWAP_LIVE_BLOCKER.replace("parameter_index: 0", "parameter_index: 1"), "not_available"),
            (gate.EITHER_SWAP_LIVE_BLOCKER.replace("UnconstrainedTypeParameter", "OtherError"), "not_available"),
            (gate.EITHER_SWAP_LIVE_BLOCKER.replace("TypeId(491)", "TypeId(binder)"), "not_available"),
            (gate.EITHER_SWAP_LIVE_BLOCKER.replace("parameter_index: 0", "parameter_index: 0, extra: true"), "not_available"),
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, override in enumerate(invalid_overrides):
                reports = self.cohort_reports(
                    receipt_rows,
                    root,
                    {gate.EITHER_SWAP_IDENTITY: override},
                )
                with self.subTest(override=override), self.assertRaisesRegex(
                    gate.ReceiptError, gate.EITHER_SWAP_IDENTITY
                ):
                    gate.validate_cohorts(
                        repository, reports, root / f"live-{index}.tsv", baseline
                    )

    def test_live_either_match_receipt_rejects_pass_source_root_and_frozen_field_drift(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        receipt_rows = gate.read_tsv(baseline)[1]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = self.cohort_reports(
                receipt_rows,
                root,
                {gate.EITHER_SWAP_IDENTITY: (gate.EITHER_SWAP_LIVE_BLOCKER, "not_available")},
            )
            self.set_report_cell(
                reports, 1, gate.EITHER_SWAP_IDENTITY, "status",
                gate.EITHER_SWAP_LIVE_BLOCKER.replace("TypeId(491)", "TypeId(630)"),
            )
            with self.assertRaisesRegex(gate.ReceiptError, "two deterministic passes disagree"):
                gate.validate_cohorts(repository, reports, root / "pass-drift.tsv", baseline)

            reports = self.cohort_reports(
                receipt_rows,
                root,
                {gate.EITHER_SWAP_IDENTITY: (gate.EITHER_SWAP_LIVE_BLOCKER, "not_available")},
            )
            self.set_report_cell(reports, 0, gate.EITHER_SWAP_IDENTITY, "source_tree", "83")
            with self.assertRaisesRegex(gate.ReceiptError, "two deterministic passes disagree"):
                gate.validate_cohorts(repository, reports, root / "tree-drift.tsv", baseline)

            reports = self.cohort_reports(
                receipt_rows,
                root,
                {"library/src/scala/Option.scala#tree=118": (gate.EITHER_SWAP_LIVE_BLOCKER, "not_available")},
            )
            with self.assertRaisesRegex(gate.ReceiptError, "protected main root lost typed acceptance"):
                gate.validate_cohorts(repository, reports, root / "wrong-root.tsv", baseline)

    def test_live_either_match_exception_keeps_historical_receipt_fields_exact(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        receipt_rows = gate.read_tsv(baseline)[1]
        changes = {
            "actual_result": "scala.util.Either[typeparam#0,typeparam#1]",
            "protected_success": "true",
            "two_pass_equal": "false",
            "baseline_status": "typed",
            "gate": "required_typed",
            "first_status": gate.EITHER_SWAP_LIVE_BLOCKER,
            "source_tree": "83",
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = self.cohort_reports(
                receipt_rows,
                root,
                {gate.EITHER_SWAP_IDENTITY: (gate.EITHER_SWAP_LIVE_BLOCKER.replace("491", "630"), "not_available")},
            )
            for field, value in changes.items():
                rows = [dict(row) for row in receipt_rows]
                row = next(row for row in rows if row["identity"] == gate.EITHER_SWAP_IDENTITY)
                row[field] = value
                changed_receipt = root / f"receipt-{field}.tsv"
                header = gate.read_tsv(baseline)[0]
                with changed_receipt.open("w", encoding="utf-8", newline="") as stream:
                    writer = csv.DictWriter(stream, fieldnames=header, delimiter="\t", lineterminator="\n")
                    writer.writeheader()
                    writer.writerows(rows)
                with self.subTest(field=field), self.assertRaisesRegex(gate.ReceiptError, gate.EITHER_SWAP_IDENTITY):
                    gate.validate_cohorts(repository, reports, root / f"out-{field}.tsv", changed_receipt)

    def test_live_cohort_receipt_reports_unexpected_per_root_drift(self) -> None:
        repository = Path(__file__).resolve().parents[2]
        baseline = repository / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv"
        receipt_rows = gate.read_tsv(baseline)[1]
        changed_root = "library/src/scala/util/Either.scala#tree=84"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = self.cohort_reports(
                receipt_rows,
                root,
                {changed_root: ("typed", "scala.util.Either[typeparam#0,typeparam#1]")},
            )
            with self.assertRaisesRegex(gate.ReceiptError, changed_root):
                gate.validate_cohorts(
                    repository, reports, root / "live.tsv", baseline
                )

    def test_rust_receipt_rejects_mismatched_tested_revision(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(Path(directory) / "rust.log", "")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_rust_log(log, "a" * 40, "b" * 40)

    def test_issue1999_rollback_log_requires_both_named_non_skipped_tests(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(
                Path(directory) / "rollback.log",
                "test typer::tests::issue_1999_late_event_result_failure_observes_both_cases_before_atomic_rollback ... ok\n"
                "test typer::tests::issue_1999_structural_corruption_restores_then_recovers_in_same_typer ... ok\n"
                "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
            )
            gate.validate_named_test_log(log, gate.ISSUE1999_TESTS)
            empty = self.write_log(
                Path(directory) / "empty.log",
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out\n",
            )
            with self.assertRaises(gate.ReceiptError):
                gate.validate_named_test_log(empty, gate.ISSUE1999_TESTS)

    def product2_log(self, *, duplicate: bool = False, changed: bool = False) -> str:
        tests = sorted(gate.RUST_TESTS)
        lines = [f"test {name} ... ok" for name in tests]
        for session in ("first", "second"):
            for name, row in gate.PRODUCT2_RUST_ROOTS.items():
                fields = row.split("\t")
                if changed and session == "second" and name == "classify":
                    fields[-3] = "Boolean"
                lines.append("ISSUE1972_POSITIVE\t" + session + "\t" + "\t".join(fields))
        if duplicate:
            lines.append(lines[-1])
        lines.append(
            f"test result: ok. {len(tests)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        )
        return "\n".join(lines) + "\n"

    def test_product2_receipt_accepts_exact_fresh_session_roots(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(Path(directory) / "product2.log", self.product2_log())
            gate.validate_product2_log(log)

    def test_product2_receipt_rejects_duplicate_or_changed_session_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            duplicate = self.write_log(Path(directory) / "duplicate.log", self.product2_log(duplicate=True))
            changed = self.write_log(Path(directory) / "changed.log", self.product2_log(changed=True))
            with self.assertRaises(gate.ReceiptError):
                gate.validate_product2_log(duplicate)
            with self.assertRaises(gate.ReceiptError):
                gate.validate_product2_log(changed)

    def test_product2_oracle_receipt_requires_pinned_runtime_and_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = self.write_log(
                Path(directory) / "product2-oracle.log",
                "verified eight pinned Scala 3.9.0 artifacts\n"
                f"Scala 3.9.0 source revision: {gate.SCALA_REVISION}\n"
                'JDK: JAVA_VERSION="21.0.8"\n'
                "Positive typed snapshot and negative matrix match.\n"
                "verified frozen source SHA-256 manifest\n",
            )
            gate.validate_product2_oracle_log(log)
            log.write_text("Positive typed snapshot and negative matrix match.\n", encoding="utf-8")
            with self.assertRaises(gate.ReceiptError):
                gate.validate_product2_oracle_log(log)


    def binary_positive_rows(self):
        graph = (
            ";Expr=source-sealed-trait;Num/Add=source-case-class;Add.parents=Expr+scala.Product;"
            "Add.fields=left/right:Expr,distinct,class-owned;Add.selectors=_1/_2:Expr,distinct,class-owned;"
        )
        select = (
            "root=selectLeft@158:271;signature=(Expr):Expr;selector=expr:Expr;"
            "cases=Num(_)=>expr,Add(left,_2)=>left;Add.unapply=companion-owned(Add):Add;"
            "selectors=_1(left):Expr,_2(right):Expr;binder=left@Add._1:case-local;"
            "wildcard=Add._2:unbound;mapped_source_trees=19" + graph + "no=_3"
        )
        evaluate = (
            "root=eval@275:410;signature=(Expr):Int;Add.binders=left@_1:Expr,right@_2:Expr,distinct;"
            "calls=eval(left):Int+eval(right):Int;callee=Evaluator.eval:same_source_symbol;"
            "addition=Int+Int:Int;match=Int;mappings=25" + graph
            + "Add.unapply=companion-owned(Add):Add;no=_3"
        )
        return [
            *((session, "selectLeft", "Expr.scala#tree39", "158:271", select)
              for session in ("session-a", "session-b")),
            *((session, "eval", "Expr.scala#tree64", "275:410", evaluate)
              for session in ("session-a", "session-b")),
        ]

    def binary_positive_log(self, rows) -> str:
        lines = [
            "running 1 test",
            "ISSUE2080_SESSION\tsession-a\towner=0x101\tstore=0x201\tast=0x301\tindex=0x401\tsource=SourceId(1)",
            "ISSUE2080_SESSION\tsession-b\towner=0x102\tstore=0x202\tast=0x302\tindex=0x402\tsource=SourceId(2)",
            f"ISSUE2080_TESTED_SHA\t{'a' * 40}",
        ]
        lines.extend("ISSUE2080_POSITIVE\t" + "\t".join(row) for row in rows)
        lines.extend([
            "test binary_recursive_adt_original_roots_share_two_real_sessions ... ok",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out",
        ])
        return "\n".join(lines) + "\n"

    def test_binary_recursive_adt_positive_receipt_writes_exact_four_run_bound_rows(self) -> None:
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "positive.log"
            log.write_text(self.binary_positive_log(self.binary_positive_rows()), encoding="utf-8")
            output = Path(directory) / "positive.tsv"
            gate.validate_binary_recursive_adt_positive_logs(
                root, [log], "a" * 40, "a" * 40, "a" * 40, "b" * 40, output
            )
            self.assertEqual(len(gate.read_tsv(output)[1]), 4)

    def test_binary_recursive_adt_positive_receipt_rejects_incomplete_contradictory_and_prefix_counts(self) -> None:
        root = Path(__file__).resolve().parents[2]
        lines = self.binary_positive_log(self.binary_positive_rows())
        mutations = (
            # Remove a required selector proof from both sessions; parity alone must not accept it.
            lines.replace(";selectors=_1(left):Expr,_2(right):Expr", ""),
            # Duplicate a semantic key with a contradictory value.
            lines.replace(";mappings=25", ";mappings=250;mappings=25"),
            lines.replace("mappings=25", "mappings=24"),
            # Prefix counts are invalid even when duplicated identically across sessions.
            lines.replace("mapped_source_trees=19", "mapped_source_trees=190"),
            lines.replace("mappings=25", "mappings=250"),
            # Duplicate and contradictory session ownership evidence is not a fresh-session proof.
            lines.replace("store=0x202", "store=0x201"),
        )
        for index, mutated in enumerate(mutations):
            with tempfile.TemporaryDirectory() as directory:
                log = Path(directory) / "bad.log"
                log.write_text(mutated, encoding="utf-8")
                with self.assertRaises(gate.ReceiptError, msg=f"mutation {index} was accepted"):
                    gate.validate_binary_recursive_adt_positive_logs(
                        root, [log], "a" * 40, "a" * 40, "a" * 40, "b" * 40,
                        Path(directory) / "bad.tsv",
                    )

    def test_binary_recursive_adt_positive_receipt_rejects_duplicate_rows_and_stale_head(self) -> None:
        root = Path(__file__).resolve().parents[2]
        text = self.binary_positive_log(self.binary_positive_rows())
        first = next(line for line in text.splitlines() if line.startswith("ISSUE2080_POSITIVE\t"))
        mutations = (text + first + "\n", text)
        for index, mutated in enumerate(mutations):
            with tempfile.TemporaryDirectory() as directory:
                log = Path(directory) / "bad.log"
                log.write_text(mutated, encoding="utf-8")
                expected_sha = "c" * 40 if index else "a" * 40
                with self.assertRaises(gate.ReceiptError):
                    gate.validate_binary_recursive_adt_positive_logs(
                        root, [log], "a" * 40, expected_sha, "a" * 40, "b" * 40,
                        Path(directory) / "bad.tsv",
                    )

    def test_binary_recursive_adt_cohort_sidecar_preserves_exact_roots_and_provenance(self) -> None:
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "cohorts.tsv"
            gate.validate_binary_recursive_adt_cohort_receipt(
                root, root / "docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv",
                "a" * 40, "b" * 40, "b" * 40, output,
            )
            self.assertEqual(len(gate.read_tsv(output)[1]), 50)

    def binary_negative_logs(self, root: Path, directory: Path, *, omit_control: bool = False):
        fixture = root / gate.BINARY_ADT_SOURCE
        rust_rows = gate.read_tsv(fixture / "rust-control-matrix.tsv")[1]
        control_lines = ["ISSUE2080_TESTED_SHA\t" + "a" * 40]
        control_lines.extend(
            "ISSUE2079_BINARY_CONTROL\t" + "\t".join(row[column] for column in gate.read_tsv(fixture / "rust-control-matrix.tsv")[0])
            for row in (rust_rows[:-1] if omit_control else rust_rows)
        )
        required = (
            "ISSUE2078_BINARY_EVAL_FAILURES\tsecond_argument\tApplicationArgumentTypeMismatch(Boolean,Expr);first_call=observed;second_call=unpublished;rollback=both_calls,arguments,binders;retry=original_eval",
            "late_result\tExpectedExpressionTypeMismatch;observed=eval(left):canonical,eval(right):canonical;rollback=both_calls,arguments,binders;retry=original_eval",
            "ISSUE2080_ROLLBACK\tlate_after_two_calls\tbefore=both_Add_Expr_binders_and_both_eval_Applies_same_canonical_method_distinct_argument_binders;restored=complete_state_projection+resolver+typed_ast+types+SourceTypedIndex+scope_buckets;retry=stable_failure_then_selectLeft_and_eval_recovered_in_session;fresh_session=normalized_equal",
        )
        control_lines.extend(required)
        control_lines.extend(f"test {name} ... ok" for name in sorted(gate.RECURSIVE_ADT_NEGATIVE_TESTS))
        control_lines.append("test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")
        control = self.write_log(directory / "negative.log", "\n".join(control_lines) + "\n")

        structural_file = fixture / "rust-structural-controls.tsv"
        structural_rows = gate.read_tsv(structural_file)[1]
        structural_header = gate.read_tsv(structural_file)[0]
        structural_lines = ["ISSUE2080_TESTED_SHA\t" + "a" * 40]
        structural_lines.extend(
            "ISSUE2079_STRUCTURAL_CONTROL\t" + "\t".join(row[column] for column in structural_header)
            for row in structural_rows
        )
        structural_lines.extend([
            "test corrupted_reference_product2_identities_reject_then_retry_original_select_left ... ok",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out",
        ])
        structural = self.write_log(directory / "structural.log", "\n".join(structural_lines) + "\n")
        return control, structural

    def test_binary_recursive_adt_negative_receipt_requires_controls_and_late_rollback(self) -> None:
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            control, structural = self.binary_negative_logs(root, Path(directory))
            output = Path(directory) / "negative.tsv"
            gate.validate_binary_recursive_adt_negative_logs(
                root, control, structural, control, "b" * 40, "a" * 40, "a" * 40, output,
            )
            self.assertEqual(len(gate.read_tsv(output)[1]), 33)

    def test_binary_recursive_adt_negative_receipt_rejects_omitted_control(self) -> None:
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            control, structural = self.binary_negative_logs(root, Path(directory), omit_control=True)
            with self.assertRaises(gate.ReceiptError):
                gate.validate_binary_recursive_adt_negative_logs(
                    root, control, structural, control, "b" * 40, "a" * 40, "a" * 40,
                    Path(directory) / "negative.tsv",
                )


if __name__ == "__main__":
    unittest.main()
