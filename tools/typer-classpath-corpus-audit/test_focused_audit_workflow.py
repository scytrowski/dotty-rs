import hashlib
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class FocusedAuditWorkflowTests(unittest.TestCase):
    def test_recursive_acceptance_evidence_paths_trigger_the_workflow(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")
        pull_request = workflow.split("  pull_request:\n", 1)[1].split("  workflow_dispatch:", 1)[0]
        paths_section = pull_request.split("    paths:\n", 1)[1]
        patterns = re.findall(r"^      - ['\"]?([^'\"\n]+)['\"]?\s*$", paths_section, re.MULTILINE)

        def github_path_matches(pattern: str, path: str) -> bool:
            expression = ""
            index = 0
            while index < len(pattern):
                if pattern.startswith("**", index):
                    expression += ".*"
                    index += 2
                elif pattern[index] == "*":
                    expression += "[^/]*"
                    index += 1
                elif pattern[index] == "?":
                    expression += "[^/]"
                    index += 1
                else:
                    expression += re.escape(pattern[index])
                    index += 1
            return re.fullmatch(expression, path) is not None

        evidence_files = (
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv",
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv",
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/positive-roots.tsv",
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/negative-rollback.tsv",
            "docs/typer-lexical-closure-acceptance-recovery-3.9.0/stdlib-cohorts.tsv",
        )
        for path in evidence_files:
            with self.subTest(path=path):
                self.assertTrue(any(github_path_matches(pattern, path) for pattern in patterns))

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

    def test_sealed_rollback_outputs_are_verified_and_uploaded_from_runner_scratch(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")
        runner = (
            repo / "crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run-rollback-matrix"
        ).read_text(encoding="utf-8")
        step = workflow.split("- name: Run pinned sealed Event negative and rollback matrix\n", 1)[1].split(
            "\n      - name:", 1
        )[0]

        self.assertIn(
            "DOTTY_SEALED_ADT_ROLLBACK_TMP_ROOT: ${{ github.workspace }}/target/source-sealed-adt-match-rollback-oracle",
            step,
        )
        self.assertIn("scratch=\"${DOTTY_SEALED_ADT_ROLLBACK_TMP_ROOT:", runner)
        self.assertIn('test -s "$DOTTY_SEALED_ADT_ROLLBACK_TMP_ROOT/scala-failure-matrix.tsv"', step)
        self.assertIn("diff -u crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/scala-failure-matrix.tsv", step)
        self.assertIn("-name '*.out' | wc -l)\" -eq 9", step)
        self.assertIn("source-sealed-adt-match-rollback-oracle/", workflow.split("- name: Upload focused report", 1)[1])

    def test_nested_recursive_adt_release_gate_executes_and_bundles_three_sidecars(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")
        report_writer = (repo / "tools/write-typer-focused-audit-report.py").read_text(encoding="utf-8")
        for prerequisite in (
            "7165bfc6f02c66706a86b5ea814ca49a757187b7",
            "eb4ff37f704e8d1a112a46a294403d92a5a07767",
            "73748e6dec188fb60c030d61d7b33a2b0706b2e7",
            "e50e2cf465d833049b4c202919548198bc41fd8c",
            "0fc457776374f69d6b16187c007721a85b2557a0",
            "918b4e3d757585799fbbe88d0f56b7fc44bbb43c",
        ):
            with self.subTest(prerequisite=prerequisite):
                self.assertIn(prerequisite, workflow)
        for required in (
            "Run pinned nested recursive ADT Scala 3.9 oracle",
            "nested_recursive_adt_original_roots_are_attempted_in_two_fresh_sessions",
            "nested_late_failures_restore_and_retry_before_original_roots_recover",
            "Verify nested recursive ADT execution receipts",
            "nested-recursive-adt-positive.tsv",
            "nested-recursive-adt-negative-rollback.tsv",
            "nested-recursive-adt-protected-cohorts.tsv",
        ):
            with self.subTest(required=required):
                self.assertIn(required, workflow)
        self.assertIn("nested-recursive-adt-e2e-report.md", report_writer)
        self.assertIn("independent review of this exact PR head is pending", report_writer)
        self.assertIn("actions/upload-artifact@v4", workflow)

    def test_nested_report_binds_pins_sidecar_hashes_and_unmet_review_disposition(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        prerequisites = (
            "7165bfc6f02c66706a86b5ea814ca49a757187b7",
            "eb4ff37f704e8d1a112a46a294403d92a5a07767",
            "73748e6dec188fb60c030d61d7b33a2b0706b2e7",
            "e50e2cf465d833049b4c202919548198bc41fd8c",
            "0fc457776374f69d6b16187c007721a85b2557a0",
            "918b4e3d757585799fbbe88d0f56b7fc44bbb43c",
        )
        with tempfile.TemporaryDirectory() as directory:
            report_dir = Path(directory)
            (report_dir / "environment.tsv").write_text(
                "issue2138_baseline_sha\t" + "a" * 40 + "\n"
                "issue2138_tested_sha\t" + "b" * 40 + "\n"
                "issue2138_pr_head_sha\t" + "b" * 40 + "\n",
                encoding="utf-8",
            )
            sidecars = (
                "nested-recursive-adt-positive.tsv",
                "nested-recursive-adt-negative-rollback.tsv",
                "nested-recursive-adt-protected-cohorts.tsv",
            )
            for filename in sidecars:
                (report_dir / filename).write_text("receipt\n", encoding="utf-8")
            environment = os.environ.copy()
            environment.update(
                DOTTY_AUDIT_REPORT_DIR=str(report_dir),
                SCALA39_ROOT=str(repo),
                JAVA_HOME=environment.get("JAVA_HOME", "/missing-jdk"),
                DOTTY_AUDIT_JOB_STATUS="success",
                GITHUB_SERVER_URL="https://github.com",
                GITHUB_REPOSITORY="scytrowski/dotty-rs",
                GITHUB_RUN_ID="12345",
                DOTTY_PR_URL="https://github.com/scytrowski/dotty-rs/pull/9999",
            )
            subprocess.run(
                [sys.executable, "tools/write-typer-focused-audit-report.py"],
                cwd=repo,
                env=environment,
                check=True,
                capture_output=True,
                text=True,
            )
            report = (report_dir / "nested-recursive-adt-e2e-report.md").read_text(encoding="utf-8")
        self.assertIn("SPRINT GOAL UNMET", report)
        self.assertIn("actions/runs/12345", report)
        self.assertIn("pull/9999", report)
        self.assertIn("`" + "a" * 40 + "`", report)
        self.assertIn("`" + "b" * 40 + "`", report)
        self.assertIn("18 exact source roots", report)
        self.assertIn("Unchanged exact 16/25 main and 15/25 analysis roots", report)
        for sha in prerequisites:
            self.assertIn(sha, report)
        for filename in sidecars:
            self.assertIn(hashlib.sha256(b"receipt\n").hexdigest(), report)


if __name__ == "__main__":
    unittest.main()
