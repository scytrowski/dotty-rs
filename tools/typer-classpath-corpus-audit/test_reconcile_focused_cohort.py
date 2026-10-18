import json
import tempfile
import unittest
from pathlib import Path

from reconcile_focused_cohort import ReconciliationError, reconcile


OBSERVATION_HEADER = (
    "resolver_observations:\n"
    "  attempt\ttarget\toutcome\tcause\tmaterialized_prefix\tselected_identity\n"
)


class FocusedCohortReconciliationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.frozen = self.root / "frozen.tsv"
        self.report = self.root / "focused.txt"

    def tearDown(self):
        self.temporary.cleanup()

    def test_joins_each_frozen_row_to_its_current_request_target_stage(self):
        attempt = "compiler/src/A.scala#tree=20"
        self.frozen.write_text(
            "blocker\tfile\tmethod\trequest_attempts\trequest_targets\n"
            f"Missing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\t{attempt}::alpha\ttarget=alpha\n"
            f"Missing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\t{attempt}::beta\ttarget=beta\n",
            encoding="utf-8",
        )
        self.report.write_text(
            "method_outcomes:\n"
            "  method=compiler/src/A.scala#tree=12::site=A::attempt=compiler/src/A.scala#tree=20::blocker=TypeNameNotFound::typed=false\n"
            "blocker_counts:\n"
            + OBSERVATION_HEADER
            + f"  {attempt}\ttarget=alpha\tunresolved\tmissing member\tpackage/A\t-\n"
            + f"  {attempt}\ttarget=beta\tresolved\t-\tpackage/B\tpackage/B::B\n",
            encoding="utf-8",
        )
        rows = reconcile(self.frozen, self.report, "package-term")
        self.assertEqual(len(rows), 2)
        self.assertEqual([row["current_first_blocker"] for row in rows], ["TypeNameNotFound"] * 2)
        self.assertEqual([row["current_request_stage"] for row in rows], ["target-observed"] * 2)
        first, second = (json.loads(row["current_request_observations"])[0] for row in rows)
        self.assertEqual(first["outcome"], "unresolved")
        self.assertEqual(first["materialized_prefix"], "package/A")
        self.assertEqual(second["outcome"], "resolved")
        self.assertEqual(second["selected_identity"], "package/B::B")

    def test_marks_missing_method_and_attempt_explicitly(self):
        self.frozen.write_text(
            "source_file\tmethod_tree\tbaseline_blocker\tenclosing_attempt\tdistinct_resolution_targets\n"
            "compiler/src/A.scala\t12\tObjectModuleClassUnavailable\tcompiler/src/A.scala#tree=20\ttarget=module\n",
            encoding="utf-8",
        )
        self.report.write_text("method_outcomes:\nblocker_counts:\n" + OBSERVATION_HEADER, encoding="utf-8")
        rows = reconcile(self.frozen, self.report, "object-module")
        self.assertEqual(rows[0]["method_reconciliation"], "missing-current-method")
        self.assertEqual(rows[0]["current_first_blocker"], "MISSING")
        self.assertEqual(rows[0]["current_request_stage"], "missing-current-attempt")

    def test_reports_target_mismatch_without_reusing_method_outcome(self):
        attempt = "compiler/src/A.scala#tree=20"
        self.frozen.write_text(
            "blocker\tfile\tmethod\trequest_attempts\trequest_targets\n"
            f"Missing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\t{attempt}::alpha\ttarget=expected\n",
            encoding="utf-8",
        )
        self.report.write_text(
            "method_outcomes:\n"
            "  method=compiler/src/A.scala#tree=12::blocker=One::typed=false\n"
            "blocker_counts:\n"
            + OBSERVATION_HEADER
            + f"  {attempt}\ttarget=other\tunresolved\t-\t-\t-\n",
            encoding="utf-8",
        )
        rows = reconcile(self.frozen, self.report, "package-term")
        self.assertEqual(rows[0]["method_reconciliation"], "blocker-observed")
        self.assertEqual(rows[0]["current_request_stage"], "missing-current-target")
        self.assertEqual(json.loads(rows[0]["current_request_observations"]), [])

    def test_rejects_duplicate_current_method_rows(self):
        self.frozen.write_text(
            "blocker\tfile\tmethod\nMissing\tcompiler/src/A.scala\tcompiler/src/A.scala#tree=12\n",
            encoding="utf-8",
        )
        self.report.write_text(
            "method_outcomes:\n"
            "  method=compiler/src/A.scala#tree=12::blocker=One::typed=false\n"
            "  method=compiler/src/A.scala#tree=12::blocker=Two::typed=false\n"
            "blocker_counts:\n"
            + OBSERVATION_HEADER,
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ReconciliationError, "duplicate"):
            reconcile(self.frozen, self.report, "package-term")


if __name__ == "__main__":
    unittest.main()
