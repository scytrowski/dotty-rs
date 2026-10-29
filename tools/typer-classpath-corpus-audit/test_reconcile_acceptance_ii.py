import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("reconcile_acceptance_ii.py")
SPEC = importlib.util.spec_from_file_location("reconcile_acceptance_ii", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class AcceptanceReconciliationTests(unittest.TestCase):
    def test_reports_new_success_and_session_local_tree_id_change(self):
        manifest = [{
            "identity": "library/src/scala/A.scala#tree=7",
            "owner_path": "scala.A",
            "method_name": "value",
            "tier": "A0",
        }]
        before = {manifest[0]["identity"]: {
            "status": "blocked::MemberNotFound", "actual_result": "not_available",
            "source_tree": "7", "typed_tree": "-", "owner_path": "scala.A", "method_name": "value",
        }}
        after = {manifest[0]["identity"]: {
            "status": "typed", "actual_result": "Int", "source_tree": "7", "typed_tree": "12",
            "owner_path": "scala.A", "method_name": "value",
        }}
        row = MODULE.reconcile("main", manifest, before, after)[0]
        self.assertEqual(row["disposition"], "newly_typed")
        self.assertEqual(row["baseline_blocker"], "MemberNotFound")
        self.assertEqual(row["baseline_cause"], "blocked::MemberNotFound")
        self.assertEqual(row["final_cause"], "typed")
        self.assertEqual(row["typed_tree_id_changed"], "true")

    def test_rejects_a_lost_accepted_method(self):
        manifest = [{"identity": "A.scala#tree=1", "source_tree": "1", "gate": "required_typed", "oracle_result": "Int"}]
        before = {"A.scala#tree=1": {"status": "typed", "actual_result": "Int", "source_tree": "1", "typed_tree": "2"}}
        after = {"A.scala#tree=1": {"status": "blocked::MissingMember", "actual_result": "not_available", "source_tree": "1", "typed_tree": "-"}}
        with self.assertRaises(MODULE.ReconcileError):
            MODULE.reconcile("main", manifest, before, after)


if __name__ == "__main__":
    unittest.main()
