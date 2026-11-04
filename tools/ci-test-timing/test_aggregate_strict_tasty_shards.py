import os
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1] / "ci"))
from aggregate_strict_tasty_shards import REQUIRED_RESULTS, failed_results


class StrictTastyAggregateTests(unittest.TestCase):
    def setUp(self):
        self.success = {name: "success" for name in REQUIRED_RESULTS}

    def test_all_shards_and_followups_success_is_green(self):
        self.assertEqual(failed_results(self.success), {})

    def test_failure_cancellation_and_skip_are_red(self):
        for result in ("failure", "cancelled", "skipped"):
            with self.subTest(result=result):
                statuses = self.success.copy()
                statuses["shard-1-job"] = result
                self.assertEqual(failed_results(statuses), {"shard-1-job": result})

    def test_every_required_result_must_be_success(self):
        for name in REQUIRED_RESULTS:
            for result in ("failure", "cancelled", "skipped", "unknown"):
                with self.subTest(name=name, result=result):
                    statuses = self.success.copy()
                    statuses[name] = result
                    self.assertEqual(failed_results(statuses), {name: result})

    def test_each_target_shard_failure_is_red(self):
        for name in ("shard-1-tests", "shard-2-tests", "shard-3-tests", "shard-4-tests"):
            with self.subTest(name=name):
                statuses = self.success.copy()
                statuses[name] = "failure"
                self.assertIn(name, failed_results(statuses))

    def test_early_mandatory_gate_failure_and_later_skips_are_red(self):
        statuses = self.success.copy()
        statuses["constructor-alias"] = "failure"
        statuses["loader-rejection"] = "skipped"
        statuses["dual-reference-reader"] = "skipped"
        statuses["java-verifier"] = "skipped"
        self.assertEqual(
            failed_results(statuses),
            {
                "constructor-alias": "failure",
                "loader-rejection": "skipped",
                "dual-reference-reader": "skipped",
                "java-verifier": "skipped",
            },
        )

    def test_missing_result_is_red(self):
        statuses = self.success.copy()
        del statuses["dual-reference-reader"]
        self.assertEqual(failed_results(statuses), {"dual-reference-reader": "missing"})

    def test_cli_fails_closed_for_unset_result(self):
        result = subprocess.run(
            [sys.executable, str(Path(__file__).parents[1] / "ci" / "aggregate_strict_tasty_shards.py")],
            env={},
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("shard-1-job: missing", result.stdout)

    def test_cli_accepts_only_all_explicit_success_results(self):
        environment = os.environ.copy()
        environment.update(
            {
                f"STRICT_TASTY_{name.upper().replace('-', '_')}": "success"
                for name in REQUIRED_RESULTS
            }
        )
        result = subprocess.run(
            [sys.executable, str(Path(__file__).parents[1] / "ci" / "aggregate_strict_tasty_shards.py"), "--report"],
            env=environment,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("all four strict TASTy shards", result.stdout)


if __name__ == "__main__":
    unittest.main()
