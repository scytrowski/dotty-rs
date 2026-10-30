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
    def write_fixture(
        self, directory: Path, status: str, actual_result: str = "Int", typed_tree: str = "2"
    ) -> tuple[Path, Path]:
        cohort = directory / "cohort.tsv"
        cohort.write_text(
            "identity\tgate\toracle_result\tsource_tree\ttyped_tree\n"
            "scala/Example.scala#tree=7\trequired_typed\tInt\t7\t2\n"
            "scala/Example.scala#tree=8\tobserve\t-\t8\t-\n",
            encoding="utf-8",
        )
        report = directory / "audit.tsv"
        report.write_text(
            "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
            "expected_result\tactual_result\tcomplexity\tstatus\tsource_tree\ttyped_tree\n"
            f"scala/Example.scala#tree=7\tExample\tanswer\t1\t0..1\t[]\tInt\t"
            f"{actual_result}\t1\t{status}\t7\t{typed_tree if status == 'typed' else '-'}\n",
            encoding="utf-8",
        )
        return cohort, report

    def test_gate_accepts_the_frozen_typed_result_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed")
            self.assertEqual(MODULE.verify(cohort, report), [])

    def test_gate_rejects_a_regression_to_blocked(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "blocked::MemberNotFound", "not_available")
            self.assertIn("expected fully typed", MODULE.verify(cohort, report)[0])

    def test_gate_rejects_a_missing_frozen_method(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed")
            report.write_text(report.read_text(encoding="utf-8").splitlines()[0] + "\n", encoding="utf-8")
            self.assertIn("missing from ordinary audit report", MODULE.verify(cohort, report)[0])

    def test_gate_rejects_a_changed_result_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed", "String")
            self.assertIn("result shape changed", MODULE.verify(cohort, report)[0])

    def test_gate_accepts_a_session_local_typed_tree_id_change(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed", typed_tree="9")
            self.assertEqual(MODULE.verify(cohort, report), [])

    def test_gate_rejects_a_missing_typed_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed")
            report.write_text(
                report.read_text(encoding="utf-8").replace("\t7\t2\n", "\t7\t-\n"),
                encoding="utf-8",
            )
            self.assertIn("expected fully typed", MODULE.verify(cohort, report)[0])

    def test_gate_requires_every_frozen_manifest_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            manifest = directory / "manifest.tsv"
            manifest.write_text(
                "identity\tsource_tree\n"
                "scala/Example.scala#tree=7\t7\n"
                "scala/Example.scala#tree=8\t8\n",
                encoding="utf-8",
            )
            failures = MODULE.verify(cohort, report, manifest)
            self.assertIn(
                "scala/Example.scala#tree=8: missing from ordinary audit report",
                failures,
            )

    def test_gate_accepts_a_complete_exact_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            manifest = directory / "manifest.tsv"
            manifest.write_text(
                "identity\tsource_tree\nscala/Example.scala#tree=7\t7\n",
                encoding="utf-8",
            )
            self.assertEqual(MODULE.verify(cohort, report, manifest), [])

    def test_gate_rejects_an_unlisted_report_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            manifest = directory / "manifest.tsv"
            manifest.write_text(
                "identity\tsource_tree\nscala/Example.scala#tree=7\t7\n",
                encoding="utf-8",
            )
            report.write_text(
                report.read_text(encoding="utf-8")
                + "scala/Example.scala#tree=9\tExample\textra\t1\t0..1\t[]\tInt\tInt\t1\ttyped\t9\t3\n",
                encoding="utf-8",
            )
            failures = MODULE.verify(cohort, report, manifest)
            self.assertIn(
                "scala/Example.scala#tree=9: unexpected in ordinary audit report",
                failures,
            )

    def test_gate_rejects_duplicate_report_roots(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            report.write_text(
                report.read_text(encoding="utf-8")
                + report.read_text(encoding="utf-8").splitlines()[-1]
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "duplicate ordinary-method identity"):
                MODULE.verify(cohort, report)


if __name__ == "__main__":
    unittest.main()
