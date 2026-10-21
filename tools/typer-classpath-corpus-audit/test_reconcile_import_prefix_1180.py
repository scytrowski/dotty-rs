import unittest
from pathlib import Path

from reconcile_import_prefix_1180 import (
    CURRENT_COLUMNS,
    frozen_rows,
    import_identity,
    read_tsv,
    reconcile,
)


class ImportPrefix1180ReconciliationTests(unittest.TestCase):
    def test_rust_probe_and_runner_share_a_nonempty_1180_section(self):
        root = Path(__file__).resolve().parents[2]
        rust = (root / "crates/dotty-typer/tests/local_definition_audit.rs").read_text()
        probe = rust.split("fn pinned_scala39_import_prefix_typing_probe()", 1)[1].split(
            "fn print_singleton_reference_profile", 1
        )[0]
        begin = 'println!("AUDIT_IMPORT_PREFIX_1180_RECONCILIATION_BEGIN");'
        end = 'println!("AUDIT_IMPORT_PREFIX_1180_RECONCILIATION_END");'
        self.assertIn("reconcile_import_prefix_observations(\n        &latest_baseline_rows", probe)
        self.assertIn("assert_eq!(latest_reconciliation.len(), 1_063)", probe)
        self.assertIn(begin, probe)
        self.assertIn("print_import_prefix_reconciliation(&latest_reconciliation)", probe)
        self.assertIn(end, probe)

        runner = (root / "tools/typer-classpath-corpus-audit/run").read_text()
        extraction = (
            "sed -n '/^AUDIT_IMPORT_PREFIX_1180_RECONCILIATION_BEGIN$/,"
            "/^AUDIT_IMPORT_PREFIX_1180_RECONCILIATION_END$/p'"
        )
        self.assertIn(extraction, runner)
        self.assertIn(
            'if [[ ! -s "$tmp_dir/$run.1180-reconciliation" ]]; then', runner
        )
        self.assertLess(
            runner.index('if [[ ! -s "$tmp_dir/$run.1180-reconciliation" ]]; then'),
            runner.index('python3 "$script_dir/reconcile_import_prefix_1180.py"'),
        )

    def test_joins_all_frozen_rows_and_preserves_original_evidence(self):
        root = Path(__file__).resolve().parents[2]
        profile = (root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0.md").read_text()
        frozen = frozen_rows(profile)
        current = []
        for row in frozen:
            source, method, origin, imported = import_identity(row)
            current.append(
                {
                    "source_file": source,
                    "method_tree": method,
                    "blocker_origin_tree": origin,
                    "import_tree": imported,
                    "final_import_identity": "-",
                    **{column: "current-value" for column in CURRENT_COLUMNS},
                }
            )

        refreshed = reconcile(
            profile,
            read_tsv(root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0-import-prefix-targets.tsv"),
            read_tsv(root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0-import-prefix-classpath-evidence.tsv"),
            current,
        )

        self.assertEqual(len(refreshed), 1063)
        self.assertEqual(len({row["stable_key"] for row in refreshed}), 1063)
        self.assertEqual({row["current_method_blocker"] for row in refreshed}, {"current-value"})
        self.assertTrue(all(row["source_package_registry_lookup"] for row in refreshed))
        self.assertTrue(all(int(row["target_unique_blocker_origins"]) > 0 for row in refreshed))
        self.assertTrue(all(int(row["classpath_evidence_rows"]) > 0 for row in refreshed))

        current[0]["final_import_identity"] = (
            "outcomes::qualifier=resolved::kind=Object::origin=tasty::"
            "path=dotty.tools.dotc.ast.Trees;*=absent"
        )
        module_attempt = {
            "current_object_identities": "dotty.tools.dotc.ast.Trees",
            "current_module_class_identities": "dotty/tools/dotc/ast/Trees$",
            "classification": "module_shell_not_completed",
            "module_lookup": "resolved|rolled_back",
            "completion": "unobserved",
            "member_lookup": "error|rolled_back",
            "module_member_attempts": "1",
            "distinct_module_member_requests": "member=Trees",
        }
        refreshed = reconcile(
            profile,
            read_tsv(root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0-import-prefix-targets.tsv"),
            read_tsv(root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0-import-prefix-classpath-evidence.tsv"),
            current,
            [module_attempt],
        )
        self.assertEqual(refreshed[0]["current_qualifier_kind"], "Object")
        self.assertEqual(refreshed[0]["current_object_module_evidence_status"], "matched_object_module_evidence")
        self.assertEqual(refreshed[0]["current_object_module_evidence_status"], "matched_object_module_evidence")


if __name__ == "__main__":
    unittest.main()
