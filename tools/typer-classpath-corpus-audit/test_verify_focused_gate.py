import tempfile
import unittest
from pathlib import Path

from verify_focused_gate import (
    NESTED_ROWS,
    NESTED_CLASSIFIED_CASES,
    NESTED_CONTROL_TESTS,
    NESTED_ROLLBACK_TEST,
    NESTED_SOURCE_SHA256,
    NESTED_TEST,
    NESTED_FIXTURE,
    GateError,
    expected_nested_proof,
    validate_nested_control_log,
    validate_nested_negative_sidecars,
    validate_nested_positive_log,
    verify_baseline,
    write_timing_tsv,
)


class VerifyFocusedGateTests(unittest.TestCase):
    @property
    def repository_root(self) -> Path:
        return Path(__file__).resolve().parents[2]

    def nested_positive_log(self) -> tuple[str, str]:
        import subprocess

        head = subprocess.run(
            ["git", "-C", str(self.repository_root), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        lines = [
            f"test {NESTED_TEST} ... ok",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 0.12s",
            f"ISSUE2137_TESTED_SHA\t{head}",
            "ISSUE2137_EXECUTED_TESTS\t1",
        ]
        sessions = {
            "session-a": ("100-aaa", "21321", "aaa1", "aaa2", "21321-1"),
            "session-b": ("100-bbb", "21322", "bbb1", "bbb2", "21322-2"),
        }
        for (session, root), (path_tree, span, mapping_count, result) in NESTED_ROWS.items():
            nonce, source_id, store, arena, resolver = sessions[session]
            proof = expected_nested_proof(self.repository_root, root, mapping_count, result)
            lines.append(
                "ISSUE2137_POSITIVE\t"
                + "\t".join((session, nonce, source_id, store, arena, resolver,
                              root, path_tree, span, "Evaluator$", NESTED_SOURCE_SHA256, proof))
            )
        return head, "\n".join(lines) + "\n"

    def nested_control_log(self, head: str) -> str:
        lines = [f"test {name} ... ok" for name in sorted(NESTED_CONTROL_TESTS)]
        lines.extend((f"test result: ok. {len(NESTED_CONTROL_TESTS)} passed; 0 failed; 0 ignored; 0 measured; finished in 0.12s",
                      f"ISSUE2137_CONTROLS_TESTED_SHA\t{head}"))
        for case, expected in NESTED_CLASSIFIED_CASES.items():
            filename, source_id, method, path_tree, span, owner, classification, outcome = expected
            lines.append("ISSUE2137_CLASSIFIED_CASE\t" + "\t".join((case, filename, source_id, method,
                path_tree, span, owner, classification, outcome)))
        lines.append("ISSUE2137_CLASSIFIED_CASE_COUNT\t10")
        return "\n".join(lines) + "\n"

    def nested_rollback_log(self, head: str) -> str:
        rollback = self.repository_root / NESTED_FIXTURE / "rust-late-failure.tsv"
        marker = "ISSUE2136_NESTED_ROLLBACK\t" + "\n".join(rollback.read_text(encoding="utf-8").splitlines())
        return (f"test {NESTED_ROLLBACK_TEST} ... ok\n"
                f"test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; finished in 0.12s\n"
                f"ISSUE2137_ROLLBACK_TESTED_SHA\t{head}\n{marker}\n")

    def test_accepts_exact_four_nested_positive_receipts(self) -> None:
        head, log = self.nested_positive_log()
        rows = validate_nested_positive_log(self.repository_root, log, head, head)
        self.assertEqual(len(rows), 4)

    def test_rejects_missing_duplicate_substituted_or_partial_positive_rows(self) -> None:
        head, log = self.nested_positive_log()
        mutations = {
            "missing row": "\n".join(log.splitlines()[:-1]),
            "duplicate row": log + next(line for line in log.splitlines() if line.startswith("ISSUE2137_POSITIVE\t")) + "\n",
            "wrong method": log.replace("\tpeelLeft\tExpr.scala#tree40", "\teval\tExpr.scala#tree40", 1),
            "wrong owner": log.replace("\tEvaluator$\t", "\tOther$\t", 1),
            "partial DefDef": log.replace("complete_source_to_typed", "partial_source_to_typed", 1),
            "missing source mapping": log.replace("source_mapping=complete", "source_mapping=missing", 1),
            "wrong source pin": log.replace(NESTED_SOURCE_SHA256, "0" * 64, 1),
            "same corruption in both sessions": log.replace("sum(right)", "sum(left)"),
        }
        for label, mutated in mutations.items():
            with self.subTest(label=label), self.assertRaises(GateError):
                validate_nested_positive_log(self.repository_root, mutated, head, head)

    def test_rejects_fake_sessions_stale_sha_and_zero_execution(self) -> None:
        head, log = self.nested_positive_log()
        session_b_nonce = next(line.split("\t")[2] for line in log.splitlines() if line.startswith("ISSUE2137_POSITIVE\tsession-b"))
        session_a_nonce = next(line.split("\t")[2] for line in log.splitlines() if line.startswith("ISSUE2137_POSITIVE\tsession-a"))
        mutations = {
            "aliased session witness": log.replace(session_b_nonce, session_a_nonce),
            "stale tested revision": log.replace(head, "0" * 40),
            "skipped execution": log.replace("1 passed; 0 failed", "0 passed; 0 failed"),
            "missing test execution": log.replace(f"test {NESTED_TEST} ... ok\n", ""),
            "malformed receipt": log.replace("\tEvaluator$\t", "\tEvaluator$", 1),
        }
        for label, mutated in mutations.items():
            with self.subTest(label=label), self.assertRaises(GateError):
                validate_nested_positive_log(self.repository_root, mutated, head, head)
        with self.assertRaisesRegex(GateError, "PR head"):
            validate_nested_positive_log(self.repository_root, log, head, "0" * 40)

    def test_local_report_requires_clean_committed_evidence(self) -> None:
        from unittest.mock import patch

        head, log = self.nested_positive_log()
        with patch("verify_focused_gate._git_is_clean", return_value=False):
            with self.assertRaisesRegex(GateError, "clean tested revision"):
                validate_nested_positive_log(self.repository_root, log, head, head, require_clean=True)

    def test_requires_executed_negative_controls_and_actual_rollback_receipt(self) -> None:
        head, _ = self.nested_positive_log()
        control_log = self.nested_control_log(head)
        validate_nested_control_log(self.repository_root, control_log, head)
        with self.assertRaises(GateError):
            validate_nested_control_log(self.repository_root, control_log.replace("nested_sum_bounded_invalid_roots_fail_without_partial_mappings", "skipped"), head)
        rollback_log = self.nested_rollback_log(head)
        validate_nested_negative_sidecars(self.repository_root, rollback_log, head)
        with self.assertRaises(GateError):
            validate_nested_negative_sidecars(self.repository_root, rollback_log.replace("full_rollback_retry_same_session_and_fresh_parity", "partial"), head)

    def test_control_and_rollback_receipts_bind_to_tested_sha(self) -> None:
        head, _ = self.nested_positive_log()
        controls = self.nested_control_log(head)
        rollback = self.nested_rollback_log(head)
        for marker, log, validator in (
            ("ISSUE2137_CONTROLS_TESTED_SHA", controls, lambda value: validate_nested_control_log(self.repository_root, value, head)),
            ("ISSUE2137_ROLLBACK_TESTED_SHA", rollback, lambda value: validate_nested_negative_sidecars(self.repository_root, value, head)),
        ):
            for mutated in (log.replace(f"{marker}\t{head}", ""),
                            log.replace(head, "0" * 40),
                            log + f"{marker}\t{'f' * 40}\n"):
                with self.subTest(marker=marker), self.assertRaisesRegex(GateError, "exact tested revision"):
                    validator(mutated)

    def test_classifications_require_exact_executed_pinned_case_roots(self) -> None:
        head, _ = self.nested_positive_log()
        valid = self.nested_control_log(head)
        validate_nested_control_log(self.repository_root, valid, head)
        mutations = (
            valid.replace("ISSUE2137_CLASSIFIED_CASE\tWrongNestedArity", "ISSUE2137_CLASSIFIED_CASE\tMissingCase", 1),
            valid.replace("WrongNestedArity.scala#tree55@358:375", "WrongNestedArity.scala#tree54@358:375"),
            valid.replace("270:475", "270:474", 1),
            valid.replace("error=PatternTypeMismatch", "error=OtherError"),
            valid.replace("ISSUE2137_CLASSIFIED_CASE_COUNT\t10", "ISSUE2137_CLASSIFIED_CASE_COUNT\t9"),
            "\n".join(line for line in valid.splitlines() if "ISSUE2137_CLASSIFIED_CASE\tWrongNestedArity" not in line) + "\n",
        )
        for mutated in mutations:
            with self.subTest(mutated=mutated[-80:]), self.assertRaises(GateError):
                validate_nested_control_log(self.repository_root, mutated, head)

    def test_test_summaries_reconcile_observed_pass_ignored_and_mandatory_events(self) -> None:
        head, log = self.nested_positive_log()
        bad_positive = (
            log.replace("1 passed; 0 failed; 0 ignored", "0 passed; 0 failed; 1 ignored"),
            log.replace("1 passed; 0 failed; 0 ignored", "1 passed; 0 failed; 1 ignored"),
            log.replace("1 passed; 0 failed; 0 ignored", "2 passed; 0 failed; 0 ignored"),
            log.replace("test result: ok.", "test result: FAILED.", 1),
        )
        for mutated in bad_positive:
            with self.assertRaises(GateError):
                validate_nested_positive_log(self.repository_root, mutated, head, head)
        controls = self.nested_control_log(head)
        for mutated in (
            controls.replace("4 passed; 0 failed; 0 ignored", "0 passed; 0 failed; 4 ignored"),
            controls.replace("4 passed; 0 failed; 0 ignored", "4 passed; 0 failed; 1 ignored"),
            controls.replace("4 passed; 0 failed; 0 ignored", "3 passed; 0 failed; 0 ignored"),
            controls.replace("test nested_sum_bounded_invalid_roots_fail_without_partial_mappings ... ok\n", ""),
        ):
            with self.assertRaises(GateError):
                validate_nested_control_log(self.repository_root, mutated, head)
        rollback = self.nested_rollback_log(head)
        with self.assertRaises(GateError):
            validate_nested_negative_sidecars(self.repository_root, rollback.replace("1 passed; 0 failed; 0 ignored", "0 passed; 0 failed; 1 ignored"), head)

    def test_rejects_malformed_classification_sidecar(self) -> None:
        import shutil

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / NESTED_FIXTURE
            fixture.mkdir(parents=True)
            source_fixture = self.repository_root / NESTED_FIXTURE
            for name in ("negative-cases.tsv", "rust-late-failure.tsv"):
                shutil.copy2(source_fixture / name, fixture / name)
            head, _ = self.nested_positive_log()
            valid_log = self.nested_rollback_log(head)
            validate_nested_negative_sidecars(root, valid_log, head)
            (fixture / "negative-cases.tsv").write_text("case\tclassification\treason\nWrongNestedArity\trejected\tpartial\n", encoding="utf-8")
            with self.assertRaises(GateError):
                validate_nested_negative_sidecars(root, valid_log, head)

    def test_rejects_altered_normalized_semantic_fixture(self) -> None:
        import shutil

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / NESTED_FIXTURE
            fixture.mkdir(parents=True)
            source = self.repository_root / NESTED_FIXTURE / "rust-sum-complete.tsv"
            target = fixture / source.name
            shutil.copy2(source, target)
            expected_nested_proof(root, "sum", 38, "Int")
            target.write_text(target.read_text(encoding="utf-8").replace("sum(right)", "sum(left)"), encoding="utf-8")
            with self.assertRaisesRegex(GateError, "receipt pin changed"):
                expected_nested_proof(root, "sum", 38, "Int")

    def test_extracts_and_compares_normalized_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = root / "baseline.txt"
            report = root / "report.md"
            expected = "status=FOCUSED / PARTIAL\nselected_files=1\n"
            baseline.write_text(expected, encoding="utf-8")
            report.write_text(
                "# Focused audit\n\n## Selected audit output\n\n```text\n"
                + expected
                + "```\n\nTwo normalized focused reports matched byte-for-byte.\n",
                encoding="utf-8",
            )

            verify_baseline(report, baseline)

    def test_rejects_semantic_output_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = root / "baseline.txt"
            report = root / "report.md"
            baseline.write_text("methods_typed=0\n", encoding="utf-8")
            report.write_text(
                "## Selected audit output\n\n```text\nmethods_typed=1\n```\n\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(GateError, "first differing line: 1"):
                verify_baseline(report, baseline)

    def test_writes_timing_and_rss_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "first.log"
            rss = root / "first.rss"
            output = root / "timings.tsv"
            log.write_text(
                "FOCUSED_TIMING files=3 wall_ms=20 classpath_setup_ms=2 "
                "file_discovery_ms=1 source_read_ms=3 source_processing_and_merge_ms=14\n",
                encoding="utf-8",
            )
            rss.write_text("MAX_RSS_KB=12345\n", encoding="utf-8")

            write_timing_tsv(output, [log], [rss])

            self.assertEqual(
                output.read_text(encoding="utf-8"),
                "sample\tfiles\twall_ms\tclasspath_setup_ms\tfile_discovery_ms\t"
                "source_read_ms\tsource_processing_and_merge_ms\tpeak_rss_kb\n"
                "1\t3\t20\t2\t1\t3\t14\t12345\n",
            )

    def test_rejects_missing_rss_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "first.log"
            rss = root / "first.rss"
            log.write_text(
                "FOCUSED_TIMING files=3 wall_ms=20 classpath_setup_ms=2 "
                "file_discovery_ms=1 source_read_ms=3 source_processing_and_merge_ms=14\n",
                encoding="utf-8",
            )
            rss.write_text("", encoding="utf-8")

            with self.assertRaisesRegex(GateError, "RSS is missing"):
                write_timing_tsv(root / "timings.tsv", [log], [rss])


if __name__ == "__main__":
    unittest.main()
