from __future__ import annotations

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


if __name__ == "__main__":
    unittest.main()
