import tempfile
import unittest
from pathlib import Path

from verify_focused_gate import GateError, verify_baseline, write_timing_tsv


class VerifyFocusedGateTests(unittest.TestCase):
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
