import subprocess
import unittest
from pathlib import Path


class FocusedAuditWorkflowTests(unittest.TestCase):
    def test_runner_temp_is_scoped_to_steps_not_job_environment(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")
        job_config = workflow.split("\njobs:\n", 1)[1].split("\n    steps:\n", 1)[0]
        audit_step = workflow.split("- name: Run deterministic focused audit and compare baseline\n", 1)[1]
        audit_step = audit_step.split("\n      - name:", 1)[0]

        self.assertNotIn("${{ runner.temp }}", job_config)
        self.assertIn("COURSIER_MAVEN_ROOT: ${{ runner.temp }}", audit_step)
        self.assertIn("DOTTY_AUDIT_TMP_ROOT: ${{ runner.temp }}", audit_step)
        self.assertIn("DOTTY_AUDIT_REPORT_DIR: ${{ runner.temp }}", audit_step)

    def test_pr_gate_is_pinned_bounded_and_never_runs_full_corpus(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")

        self.assertIn("pull_request:", workflow)
        self.assertIn("timeout-minutes: 30", workflow)
        self.assertIn("java-version: '21.0.8+9.0.LTS'", workflow)
        self.assertIn("777528f19a58e794c9954a42f433373472ec57f8", workflow)
        self.assertIn("--focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv", workflow)
        self.assertIn("--verify-determinism", workflow)
        self.assertIn("verify_focused_gate.py", workflow)
        self.assertIn("actions/upload-artifact@v4", workflow)
        self.assertNotIn("tools/typer-classpath-corpus-audit/run --verify-determinism", workflow)

    def test_root_method_outcomes_are_limited_to_focused_manifests(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        runner = repo / "tools/typer-classpath-corpus-audit/run"
        result = subprocess.run(
            [str(runner), "--include-root-method-outcomes"],
            capture_output=True,
            check=False,
            text=True,
        )

        self.assertEqual(result.returncode, 2)
        self.assertIn(
            "--include-root-method-outcomes requires --focused-manifest",
            result.stderr,
        )


if __name__ == "__main__":
    unittest.main()
