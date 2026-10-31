import tempfile
import unittest
import importlib.util
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("verify_lexical_closure_acceptance.py")
SPEC = importlib.util.spec_from_file_location("verify_lexical_closure_acceptance", MODULE_PATH)
acceptance = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(acceptance)


class LexicalClosureAcceptanceTests(unittest.TestCase):
    def test_current_frozen_inputs_are_complete_and_unchanged(self) -> None:
        repo_root = Path(__file__).resolve().parents[2]
        acceptance.validate_evidence_inputs(repo_root)

    def test_changed_frozen_input_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.tsv"
            path.write_text("changed\n", encoding="utf-8")
            with self.assertRaisesRegex(acceptance.AcceptanceInputError, "SHA-256 changed"):
                acceptance.check_hash(path, "0" * 64)

    def test_positive_test_log_requires_the_exact_non_skipped_test(self) -> None:
        test_name = acceptance.TESTS["positive"][0]
        log = (
            f"test {test_name} ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 962 filtered out\n"
        )
        acceptance.validate_test_log(log, "positive")

    def test_zero_test_success_is_rejected_as_a_skip(self) -> None:
        log = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 963 filtered out\n"
        with self.assertRaisesRegex(acceptance.AcceptanceInputError, r"executed \[\]"):
            acceptance.validate_test_log(log, "positive")

    def test_negative_test_log_requires_all_three_matrix_tests(self) -> None:
        test_names = acceptance.TESTS["negative"]
        lines = [*(f"test {name} ... ok" for name in test_names)]
        lines.append("test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 960 filtered out")
        acceptance.validate_test_log("\n".join(lines) + "\n", "negative")

    def test_incomplete_negative_matrix_run_is_rejected(self) -> None:
        name = acceptance.TESTS["negative"][0]
        log = (
            f"test {name} ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 962 filtered out\n"
        )
        with self.assertRaisesRegex(acceptance.AcceptanceInputError, "executed"):
            acceptance.validate_test_log(log, "negative")

    def test_ignored_or_failed_matrix_cases_are_rejected(self) -> None:
        test_names = acceptance.TESTS["negative"]
        lines = [*(f"test {name} ... ok" for name in test_names)]
        lines.append("test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 960 filtered out")
        with self.assertRaisesRegex(acceptance.AcceptanceInputError, "zero failed and zero ignored"):
            acceptance.validate_test_log("\n".join(lines) + "\n", "negative")


if __name__ == "__main__":
    unittest.main()
