import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("verify_ordinary_cohort.py")
SPEC = importlib.util.spec_from_file_location("verify_ordinary_cohort", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class OrdinaryCohortGateTests(unittest.TestCase):
    def write_fixture(self, directory: Path, status: str) -> tuple[Path, Path]:
        cohort = directory / "cohort.tsv"
        cohort.write_text(
            "identity\tgate\toracle_result\n"
            "scala/Example.scala#tree=7\trequired_typed\tInt\n"
            "scala/Example.scala#tree=8\tobserve\t\n",
            encoding="utf-8",
        )
        report = directory / "audit.tsv"
        report.write_text(
            "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
            "expected_result\tcomplexity\tstatus\tattempted_tree\n"
            f"scala/Example.scala#tree=7\tExample\tanswer\t1\t0..1\t[]\tInt\t1\t{status}\t7\n",
            encoding="utf-8",
        )
        return cohort, report

    def test_gate_accepts_the_frozen_typed_result_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed::result=Int")
            self.assertEqual(MODULE.verify(cohort, report), [])

    def test_gate_rejects_a_regression_to_blocked(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "blocked::MemberNotFound")
            self.assertIn("expected fully typed", MODULE.verify(cohort, report)[0])

    def test_gate_rejects_a_missing_frozen_method(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed::result=Int")
            report.write_text(report.read_text(encoding="utf-8").splitlines()[0] + "\n", encoding="utf-8")
            self.assertIn("missing from ordinary audit report", MODULE.verify(cohort, report)[0])

    def test_gate_rejects_a_changed_result_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed::result=String")
            self.assertIn("result shape changed", MODULE.verify(cohort, report)[0])


if __name__ == "__main__":
    unittest.main()
