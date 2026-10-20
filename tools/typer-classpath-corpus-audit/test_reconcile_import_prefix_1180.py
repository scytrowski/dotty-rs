import unittest
import json
from pathlib import Path

from reconcile_import_prefix_1180 import (
    CURRENT_COLUMNS,
    frozen_rows,
    import_identity,
    read_tsv,
    reconcile,
)


class ImportPrefix1180ReconciliationTests(unittest.TestCase):
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
        self.assertTrue(all(row["pinned_classpath_evidence"] for row in refreshed))

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
        evidence = json.loads(refreshed[0]["current_object_module_evidence"])[0]
        self.assertEqual(evidence["classifications"], ["module_shell_not_completed"])
        self.assertEqual(evidence["member_outcomes"], ["error|rolled_back"])


if __name__ == "__main__":
    unittest.main()
