import unittest
from pathlib import Path


class TastyParentWorkflowPathTests(unittest.TestCase):
    def test_provenance_consumer_derives_sidecar_from_report_path(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-full-corpus-audit.yml").read_text(encoding="utf-8")
        report_var = "$SCALA39_TASTY_PARENT_AUDIT_REPORT"

        self.assertIn(
            "SCALA39_TASTY_PARENT_AUDIT_REPORT: ${{ runner.temp }}/typer-audit-report/"
            "typer-classpath-corpus-audit-post-tasty-parents-3.9.0.md",
            workflow,
        )
        self.assertIn('"${SCALA39_TASTY_PARENT_AUDIT_REPORT%.md}-tasty-parent-source-provenance.tsv"', workflow)
        self.assertEqual(workflow.count(report_var), 2)
        self.assertNotIn(
            '"$RUNNER_TEMP/typer-classpath-corpus-audit-post-tasty-parents-3.9.0-tasty-parent-source-provenance.tsv"',
            workflow,
        )


if __name__ == "__main__":
    unittest.main()
