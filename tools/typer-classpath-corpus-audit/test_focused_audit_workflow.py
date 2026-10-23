import unittest
from pathlib import Path


class FocusedAuditWorkflowTests(unittest.TestCase):
    def test_pr_gate_is_pinned_bounded_and_never_runs_full_corpus(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")

        self.assertIn("pull_request:", workflow)
        self.assertIn("timeout-minutes: 30", workflow)
        self.assertIn("java-version: '21.0.8+9'", workflow)
        self.assertIn("777528f19a58e794c9954a42f433373472ec57f8", workflow)
        self.assertIn("--focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv", workflow)
        self.assertIn("--verify-determinism", workflow)
        self.assertIn("verify_focused_gate.py", workflow)
        self.assertIn("actions/upload-artifact@v4", workflow)
        self.assertNotIn("tools/typer-classpath-corpus-audit/run --verify-determinism", workflow)


if __name__ == "__main__":
    unittest.main()
