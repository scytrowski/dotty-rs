import unittest

from reconcile_import_prefix_rows import same_import_first_segment_failure


class ReconcileImportPrefixRowsTests(unittest.TestCase):
    def test_only_counts_the_original_failed_segment(self):
        baseline = {"first_failing_segment": "core"}
        current_same = {"first_failing_segment": "core"}
        current_advanced = {"first_failing_segment": "Contexts"}

        self.assertTrue(same_import_first_segment_failure(baseline, current_same))
        self.assertFalse(same_import_first_segment_failure(baseline, current_advanced))
        self.assertFalse(same_import_first_segment_failure(baseline, None))


if __name__ == "__main__":
    unittest.main()
