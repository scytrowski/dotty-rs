import unittest
from pathlib import Path


class FocusedAuditWorkflowTests(unittest.TestCase):
    def test_ordinary_member_mode_is_separate_from_frozen_local_cohorts(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        runner = (repo / "tools/typer-classpath-corpus-audit/run").read_text(encoding="utf-8")
        audit = (
            repo / "crates/dotty-typer/tests/support/ordinary_member_audit_support.rs"
        ).read_text(encoding="utf-8")

        self.assertIn("--method-kind ordinary --method-manifest MANIFEST", runner)
        self.assertIn("pinned_scala39_ordinary_member_method_audit", runner)
        self.assertIn("DOTTY_ORDINARY_METHOD_SELECTION", audit)
        self.assertIn("type_method_definition", audit)
        ordinary_mode = runner.split('if [[ "$method_kind" == ordinary ]]; then', 1)[1].split(
            'if [[ -n "$focused_manifest" || -n "$focused_cohort" ]]', 1
        )[0]
        self.assertNotIn("DOTTY_AUDIT_FOCUSED_METHODS", ordinary_mode)

    def test_target_and_scratch_are_scoped_to_the_workspace(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")
        job_config = workflow.split("\njobs:\n", 1)[1].split("\n    steps:\n", 1)[0]
        audit_step = workflow.split("- name: Run deterministic 25 plus 25 ordinary-method cohorts\n", 1)[1]
        audit_step = audit_step.split("\n      - name:", 1)[0]

        self.assertIn("CARGO_TARGET_DIR: ${{ github.workspace }}/target", job_config)
        self.assertIn("TMPDIR: ${{ github.workspace }}/target/tmp", job_config)
        self.assertNotIn("${{ runner.temp }}", workflow)
        self.assertIn("COURSIER_MAVEN_ROOT: ${{ github.workspace }}/target/coursier", audit_step)
        self.assertIn("DOTTY_AUDIT_TMP_ROOT: ${{ github.workspace }}/target/typer-focused-intermediates", audit_step)
        self.assertIn("DOTTY_AUDIT_REPORT_DIR: ${{ github.workspace }}/target/typer-focused-report", audit_step)

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
        self.assertIn("verify_lexical_closure_acceptance.py inputs", workflow)
        self.assertIn("--method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv", workflow)
        self.assertIn("--method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv", workflow)
        self.assertIn("--require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv", workflow)
        self.assertIn("--require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv", workflow)
        self.assertIn("if-no-files-found: error", workflow)
        self.assertIn("--verify-determinism", workflow)


if __name__ == "__main__":
    unittest.main()
