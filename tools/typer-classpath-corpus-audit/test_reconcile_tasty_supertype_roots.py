import csv
import tempfile
import unittest
from pathlib import Path

from reconcile_tasty_supertype_roots import reconcile


def write_tsv(path: Path, rows: list[dict[str, str]]) -> None:
    with path.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=rows[0].keys(), delimiter="\t", lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


class ReconcileTastySupertypeRootsTests(unittest.TestCase):
    def test_preserves_all_frozen_roots_and_surfaces_observation_delta(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = root / "baseline.tsv"
            historical = root / "historical.tsv"
            parents = root / "parents.tsv"
            provenance = root / "provenance.tsv"
            baseline_rows = [
                {
                    "requested_class_root": f"compiler/Root{index}",
                    "weighted_observations": "2",
                    "resource_path": f"compiler/Root{index}.tasty",
                    "artifact": "scala3-compiler_3-3.9.0.jar",
                    "artifact_version": "3.9.0",
                    "artifact_sha256": "abc",
                }
                for index in range(50)
            ]
            parent_rows = [
                {
                    "requested_class_root": row["requested_class_root"],
                    "template_wire_address": "100",
                    "parent_index": "0",
                    "shape": "simple",
                    "resolution_stage": "semantic",
                    "resolved_binary_name": "scala/Parent",
                    "deepest_typed_cause": "none",
                    "failing_binary_name": "",
                    "reference_tag": "IDENTtpt",
                    "reference_name": "Parent",
                    "reference_prefix": "scala",
                    "raw_tree": "Ident",
                }
                for row in baseline_rows
            ]
            historical_rows = [
                {
                    "requested_class_root": row["requested_class_root"],
                    "template_wire_address": "100",
                    "parent_index": "0",
                    "shape": "old-simple",
                    "resolution_stage": "unresolved",
                    "resolved_binary_name": "",
                    "standalone_decode_parent_index": "0",
                    "standalone_decode_cause": "tasty.unresolved_supertype",
                }
                for row in baseline_rows
            ]
            write_tsv(baseline, baseline_rows)
            write_tsv(historical, historical_rows)
            write_tsv(parents, parent_rows)
            write_tsv(
                provenance,
                [
                    {
                        "root_cause_key": "root_cause=InvalidTastyFile::requested=compiler/Root0::root=tasty.unresolved_supertype",
                        "source_file": "compiler/Foo.scala",
                        "observations": "1",
                    },
                    {
                        "root_cause_key": "root_cause=InvalidTastyFile::requested=compiler/Root0::root=tasty.unresolved_supertype",
                        "source_file": "compiler/Other.scala",
                        "observations": "1",
                    },
                    {
                        "root_cause_key": "root_cause=InvalidTastyFile::requested=compiler/NewRoot::root=tasty.unresolved_supertype",
                        "source_file": "compiler/Bar.scala",
                        "observations": "3",
                    },
                ],
            )

            rows, summary = reconcile(baseline, historical, parents, provenance)

            self.assertEqual(len(rows), 52)  # header + 50 frozen roots + one new root origin
            self.assertTrue(all(len(row) == len(rows[0]) for row in rows))
            self.assertEqual(rows[1][0], "compiler/Root0")
            self.assertEqual(rows[1][7], "2")
            self.assertEqual(rows[1][10], "0")
            self.assertEqual(rows[1][11], "100")
            self.assertEqual(rows[1][12], "old-simple")
            self.assertEqual(rows[1][8], "compiler/Foo.scala;compiler/Other.scala")
            self.assertTrue(rows[1][9].endswith("=2"))
            self.assertEqual(rows[-1][0], "compiler/NewRoot")
            self.assertIn("frozen_weighted_observations=100", summary)
            self.assertIn("current_frozen_root_source_observations=2", summary)
            self.assertIn("historical_to_current_weight_delta=98", summary)
            self.assertIn("new_current_roots=1", summary)

    def test_fails_when_a_frozen_root_is_missing_from_current_profile(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = root / "baseline.tsv"
            historical = root / "historical.tsv"
            parents = root / "parents.tsv"
            provenance = root / "provenance.tsv"
            baseline_rows = [
                {
                    "requested_class_root": f"root/{index}",
                    "weighted_observations": "1",
                    "resource_path": f"root/{index}.tasty",
                    "artifact": "compiler.jar",
                    "artifact_version": "3.9.0",
                    "artifact_sha256": "abc",
                }
                for index in range(50)
            ]
            write_tsv(baseline, baseline_rows)
            write_tsv(
                historical,
                [
                    {
                        "requested_class_root": row["requested_class_root"],
                        "template_wire_address": "100",
                        "parent_index": "0",
                        "shape": "simple",
                        "resolution_stage": "unresolved",
                        "resolved_binary_name": "",
                        "standalone_decode_parent_index": "0",
                        "standalone_decode_cause": "tasty.unresolved_supertype",
                    }
                    for row in baseline_rows
                ],
            )
            write_tsv(
                parents,
                [
                    {
                        "requested_class_root": "root/0",
                        "template_wire_address": "100",
                        "parent_index": "0",
                        "shape": "simple",
                        "resolution_stage": "unresolved",
                        "resolved_binary_name": "",
                        "deepest_typed_cause": "tasty.unresolved_supertype",
                        "failing_binary_name": "root/0",
                    }
                ],
            )
            write_tsv(provenance, [{"root_cause_key": "requested=root/0", "source_file": "A.scala", "observations": "1"}])

            with self.assertRaisesRegex(ValueError, "dropped frozen roots"):
                reconcile(baseline, historical, parents, provenance)


if __name__ == "__main__":
    unittest.main()
