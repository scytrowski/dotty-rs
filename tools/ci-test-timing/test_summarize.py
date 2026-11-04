import unittest
from pathlib import Path

from summarize import parse_output, render


FIXTURES = Path(__file__).parent / "fixtures"


class TimingParserTests(unittest.TestCase):
    def fixture(self, name):
        return (FIXTURES / name).read_text(encoding="utf-8")

    def test_success_reports_multiple_binaries_and_build_time(self):
        targets, build_seconds = parse_output(self.fixture("success.log"))

        self.assertEqual([target.name for target in targets], ["first_target", "second_target"])
        self.assertEqual([target.seconds for target in targets], [12.5, 3.25])
        self.assertEqual(build_seconds, 71.0)

    def test_failure_result_is_measured(self):
        targets, _ = parse_output(self.fixture("failure.log"))

        self.assertEqual(len(targets), 1)
        self.assertEqual((targets[0].name, targets[0].result, targets[0].seconds), ("failing_target", "FAILED", 0.75))

    def test_truncated_binary_is_reported_as_missing(self):
        output = self.fixture("truncated.log")

        report = render(output, ["complete_target", "truncated_target"], 4, 101)

        self.assertIn("`complete_target` | 0.10s | ok", report)
        self.assertIn("`truncated_target` | missing | unparsed or no result", report)
        self.assertIn("measured binaries do not exactly match", report)
        self.assertIn("Cargo exit code: 101", report)


if __name__ == "__main__":
    unittest.main()
