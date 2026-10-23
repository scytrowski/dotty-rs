import csv
import hashlib
from pathlib import Path
import unittest


class SprintStartSnapshotConsistencyTests(unittest.TestCase):
    def test_gate_targets_match_movement_and_compact_provenance(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        gate_path = repo / "docs/typer-performance-sprint-start-gate-3.9.0.tsv"
        movement_path = repo / "docs/typer-performance-target-movement-3.9.0.tsv"
        metrics_path = repo / "docs/typer-performance-full-audit-3.9.0.tsv"

        with gate_path.open(encoding="utf-8", newline="") as stream:
            gate_rows = list(csv.DictReader(stream, delimiter="\t"))
        gate_metrics = {row["key"]: row["value"] for row in gate_rows if row["kind"] == "metric"}
        gate_targets = {row["key"] for row in gate_rows if row["kind"] == "successful_target_key"}
        with movement_path.open(encoding="utf-8", newline="") as stream:
            movement_rows = list(csv.DictReader(stream, delimiter="\t"))
        movement_start_targets = {
            row["target_key"] for row in movement_rows if int(row["sprint_start_success_count"]) > 0
        }
        movement_current_targets = {
            row["target_key"] for row in movement_rows if int(row["current_success_count"]) > 0
        }
        with metrics_path.open(encoding="utf-8", newline="") as stream:
            compact = {
                row["metric"]: row["value"]
                for row in csv.DictReader(stream, delimiter="\t")
            }

        self.assertEqual(gate_metrics["sprint_start_commit"], compact["sprint_start_sha"])
        self.assertEqual(gate_targets, movement_start_targets)
        self.assertEqual(len(gate_targets), int(compact["successful_target_keys_start"]))
        self.assertEqual(len(movement_current_targets), int(compact["successful_target_keys_current"]))
        self.assertEqual(
            int(gate_metrics["external_class_symbol_successes"]),
            int(compact["external_class_symbol_successes_start"]),
        )
        self.assertEqual(
            sum(int(row["sprint_start_success_count"]) for row in movement_rows),
            int(compact["external_class_symbol_successes_start"]),
        )
        self.assertEqual(
            sum(int(row["current_success_count"]) for row in movement_rows),
            int(compact["external_class_symbol_successes_current"]),
        )
        key_hash = hashlib.sha256(("\n".join(sorted(gate_targets)) + "\n").encode()).hexdigest()
        self.assertEqual(gate_metrics["successful_target_keys_sha256"], key_hash)
        self.assertEqual(compact["successful_target_keys_start_sha256"], key_hash)
        current_key_hash = hashlib.sha256(
            ("\n".join(sorted(movement_current_targets)) + "\n").encode()
        ).hexdigest()
        self.assertEqual(compact["successful_target_keys_current_sha256"], current_key_hash)


if __name__ == "__main__":
    unittest.main()
