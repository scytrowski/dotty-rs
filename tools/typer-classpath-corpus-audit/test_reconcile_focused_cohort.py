import tempfile
import unittest
from pathlib import Path

from reconcile_focused_cohort import ReconciliationError, reconcile


class FocusedCohortReconciliationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.frozen = self.root / "frozen.tsv"
        self.report = self.root / "focused.txt"

    def tearDown(self):
        self.temporary.cleanup()

    def test_preserves_each_frozen_row_and_marks_current_status(self):
        self.frozen.write_text(
            "blocker\tfile\tmethod\trequest\n"
            "Missing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\talpha\n"
            "Missing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\tbeta\n",
            encoding="utf-8",
        )
        self.report.write_text(
            "method_outcomes:\n"
            "  method=compiler/src/A.scala#tree=12::site=A::attempt=A::blocker=TypeNameNotFound::typed=false\n"
            "blocker_counts:\n",
            encoding="utf-8",
        )
        rows = reconcile(self.frozen, self.report, "package-term")
        self.assertEqual(len(rows), 2)
        self.assertEqual([row["current_first_blocker"] for row in rows], ["TypeNameNotFound"] * 2)
        self.assertEqual([row["request"] for row in rows], ["alpha", "beta"])

    def test_reports_missing_method_rows_instead_of_skipping_them(self):
        self.frozen.write_text(
            "source_file\tmethod_tree\tbaseline_blocker\n"
            "compiler/src/A.scala\t12\tObjectModuleClassUnavailable\n",
            encoding="utf-8",
        )
        self.report.write_text("method_outcomes:\nblocker_counts:\n", encoding="utf-8")
        rows = reconcile(self.frozen, self.report, "object-module")
        self.assertEqual(rows[0]["reconciliation"], "missing-current-method")
        self.assertEqual(rows[0]["current_first_blocker"], "MISSING")

    def test_rejects_duplicate_current_method_rows(self):
        self.frozen.write_text(
            "blocker\tfile\tmethod\nMissing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\n",
            encoding="utf-8",
        )
        self.report.write_text(
            "method_outcomes:\n"
            "  method=compiler/src/A.scala#tree=12::blocker=One::typed=false\n"
            "  method=compiler/src/A.scala#tree=12::blocker=Two::typed=false\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ReconciliationError, "duplicate"):
            reconcile(self.frozen, self.report, "package-term")


if __name__ == "__main__":
    unittest.main()
