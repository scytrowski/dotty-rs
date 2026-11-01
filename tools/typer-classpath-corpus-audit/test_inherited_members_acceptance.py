from __future__ import annotations

import unittest
from pathlib import Path

import verify_inherited_members_acceptance as gate


class InheritedMembersAcceptanceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.rows = gate.validate_inputs(Path(__file__).resolve().parents[2])

    def test_frozen_inputs_have_four_complete_canonical_roots(self) -> None:
        self.assertEqual([row["method"] for row in self.rows], list(gate.METHODS))
        self.assertEqual([row["rust_status"] for row in self.rows], ["fully_typed"] * 4)

    def test_accepts_exact_receipt(self) -> None:
        gate.validate_positive_receipt(list(self.rows[0]), self.rows, self.rows)

    def test_rejects_missing_root(self) -> None:
        with self.assertRaises(gate.ReceiptError):
            gate.validate_positive_receipt(list(self.rows[0]), self.rows[:-1], self.rows)

    def test_rejects_duplicate_root(self) -> None:
        rows = [*self.rows[:3], self.rows[2]]
        with self.assertRaises(gate.ReceiptError):
            gate.validate_positive_receipt(list(self.rows[0]), rows, self.rows)

    def test_rejects_mismatched_root(self) -> None:
        rows = [dict(row) for row in self.rows]
        rows[2]["selected_inherited_target"] = "Child.add"
        with self.assertRaises(gate.ReceiptError):
            gate.validate_positive_receipt(list(self.rows[0]), rows, self.rows)

    def test_rejects_malformed_columns(self) -> None:
        with self.assertRaises(gate.ReceiptError):
            gate.validate_positive_receipt([*list(self.rows[0]), "unexpected"], self.rows, self.rows)

    def test_rejects_missing_duplicate_and_mismatched_negative_receipts(self) -> None:
        path = Path(__file__).resolve().parents[2] / "docs/typer-inherited-members-negative-receipt-3.9.0.tsv"
        header, rows = gate.read_tsv(path)
        missing = rows[:-1]
        duplicate = [*rows[:-1], rows[-2]]
        mismatched = [dict(row) for row in rows]
        mismatched[0]["scala_3_9_0_classification"] = "unexpectedly_accepted"
        for candidate in (missing, duplicate, mismatched):
            with self.subTest(candidate=candidate), self.assertRaises(gate.ReceiptError):
                gate.validate_negative_receipt(header, candidate)

    def _log(self, tests: list[str] | None = None, rows: list[dict[str, str]] | None = None) -> str:
        tests = list(gate.TESTS) if tests is None else tests
        rows = self.rows if rows is None else rows
        lines = [*(f"test {test} ... ok" for test in tests)]
        lines.extend("\t".join(row.values()) for row in rows)
        lines.append(
            f"test result: ok. {len(tests)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        )
        return "\n".join(lines)

    def test_accepts_complete_rust_matrix_and_receipts(self) -> None:
        gate.validate_test_log(self._log(), self.rows)

    def test_rejects_skipped_or_duplicate_test(self) -> None:
        for tests in (list(gate.TESTS[:-1]), [*gate.TESTS[:-1], gate.TESTS[-2]]):
            with self.subTest(tests=tests), self.assertRaises(gate.ReceiptError):
                gate.validate_test_log(self._log(tests=tests), self.rows)

    def test_rejects_missing_duplicate_and_mismatched_log_receipts(self) -> None:
        cases = (
            list(self.rows[:-1]),
            [*self.rows[:3], self.rows[2]],
            [*self.rows[:2], {**self.rows[2], "selected_inherited_target": "Child.add"}, self.rows[3]],
        )
        for rows in cases:
            with self.subTest(rows=rows), self.assertRaises(gate.ReceiptError):
                gate.validate_test_log(self._log(rows=rows), self.rows)


if __name__ == "__main__":
    unittest.main()
