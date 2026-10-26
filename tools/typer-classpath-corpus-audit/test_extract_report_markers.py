import unittest

import extract_report_markers


class ExtractReportMarkersTests(unittest.TestCase):
    def test_extracts_report_from_standalone_markers(self):
        log = "before\nBEGIN\nschema=1\nrow\nEND\nafter\n"
        self.assertEqual(
            extract_report_markers.extract_report(log, "BEGIN", "END"),
            "schema=1\nrow\n",
        )

    def test_extracts_when_test_harness_prefixes_start_marker(self):
        log = "test audit_name ... BEGIN\nschema=1\nrow\nEND\nok\n"
        self.assertEqual(
            extract_report_markers.extract_report(log, "BEGIN", "END"),
            "schema=1\nrow\n",
        )

    def test_ignores_harness_prefix_on_end_marker_line(self):
        log = "BEGIN\nschema=1\nrow\ntest audit_name ... END\n"
        self.assertEqual(
            extract_report_markers.extract_report(log, "BEGIN", "END"),
            "schema=1\nrow\n",
        )

    def test_rejects_missing_or_empty_marked_reports(self):
        for log in ("no markers\n", "BEGIN\nEND\n", "BEGIN\nschema=1\n"):
            with self.subTest(log=log), self.assertRaises(extract_report_markers.ReportMarkerError):
                extract_report_markers.extract_report(log, "BEGIN", "END")


if __name__ == "__main__":
    unittest.main()
