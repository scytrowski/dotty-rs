import csv
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
        counters = {
            "typed": 1 if status == "typed" else 0,
            "blocked": 1 if status.startswith("blocked::") else 0,
            "excluded": 0,
            "not_attempted": 0,
        }
        report.write_text(
            "schema=1\n"
            "pinned_scala_revision=777528f19a58e794c9954a42f433373472ec57f8\n"
            "selected=1\n"
            f"attempted={counters['typed'] + counters['blocked']}\n"
            f"typed={counters['typed']}\n"
            f"blocked={counters['blocked']}\n"
            f"excluded={counters['excluded']}\n"
            f"not_attempted={counters['not_attempted']}\n"
            "method\towner_path\tmethod_name\tline\trange\tmodifiers\t"
            "expected_result\tactual_result\tcomplexity\tstatus\tsource_tree\ttyped_tree\n"
            f"scala/Example.scala#tree=7\tExample\tanswer\t1\t0..1\t[]\tInt\t"
            f"{actual_result}\t1\t{status}\t7\t{typed_tree if status == 'typed' else '-'}\n",
            encoding="utf-8",
        )
        return cohort, report

    def write_report(self, path: Path, rows: list[list[str]], **overrides: int) -> None:
        typed = sum(row[9] == "typed" for row in rows)
        blocked = sum(row[9].startswith("blocked::") for row in rows)
        excluded = sum(row[9].startswith("excluded::") for row in rows)
        not_attempted = len(rows) - typed - blocked - excluded
        counts = {
            "selected": len(rows),
            "attempted": typed + blocked,
            "typed": typed,
            "blocked": blocked,
            "excluded": excluded,
            "not_attempted": not_attempted,
            **overrides,
        }
        with path.open("w", encoding="utf-8", newline="") as output:
            output.write("schema=1\n")
            output.write("pinned_scala_revision=777528f19a58e794c9954a42f433373472ec57f8\n")
            for key, value in counts.items():
                output.write(f"{key}={value}\n")
            writer = csv.writer(output, delimiter="\t", lineterminator="\n")
            writer.writerow(MODULE.REPORT_COLUMNS)
            writer.writerows(rows)

    def report_row(
        self, identity: str, status: str, source_tree: str, typed_tree: str = "-"
    ) -> list[str]:
        actual_result = "Int" if status == "typed" else "not_available"
        return [
            identity,
            "Example",
            "answer",
            "1",
            "0..1",
            "[]",
            "Int",
            actual_result,
            "1",
            status,
            source_tree,
            typed_tree,
        ]

    def test_gate_accepts_the_frozen_typed_result_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed")
            self.assertEqual(MODULE.verify(cohort, report), [])

    def test_gate_rejects_a_regression_to_blocked(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(
                Path(temporary), "blocked::MemberNotFound", "not_available"
            )
            self.assertIn("expected fully typed", MODULE.verify(cohort, report)[0])

    def test_gate_rejects_a_missing_frozen_method(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed")
            lines = report.read_text(encoding="utf-8").splitlines()
            report.write_text("\n".join(lines[:-1]) + "\n", encoding="utf-8")
            failures = MODULE.verify(cohort, report)
            self.assertTrue(any("missing from ordinary audit report" in item for item in failures))

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
            failures = MODULE.verify(cohort, report)
            self.assertTrue(any("expected fully typed" in item for item in failures))

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
                + (
                    "scala/Example.scala#tree=9\tExample\textra\t1\t0..1\t[]\t"
                    "Int\tInt\t1\ttyped\t9\t3\n"
                ),
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

    def test_actual_analysis_set_root_cannot_be_reported_not_attempted(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            set_identity = "library/src/scala/collection/Set.scala#tree=726"
            manifest = directory / "manifest.tsv"
            manifest.write_text(
                "identity\tsource_tree\n"
                f"{set_identity}\t726\n"
                "scala/Example.scala#tree=7\t7\n",
                encoding="utf-8",
            )
            self.write_report(
                report,
                [
                    self.report_row(set_identity, "not_attempted::namer_error::reproduction", ""),
                    self.report_row("scala/Example.scala#tree=7", "typed", "7", "2"),
                ],
                selected=2,
                attempted=1,
                typed=1,
                blocked=0,
                excluded=0,
                not_attempted=1,
            )
            failures = MODULE.verify(cohort, report, manifest)
            self.assertTrue(
                any("Set.scala#tree=726: expected typed or blocked" in item for item in failures)
            )
            self.assertTrue(
                any("not_attempted counter must be zero" in item for item in failures)
            )

    def test_gate_rejects_excluded_manifest_outcome(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            self.write_report(
                report,
                [
                    self.report_row(
                        "scala/Example.scala#tree=7", "excluded::missing_namer_symbol", ""
                    )
                ],
                selected=1,
                attempted=0,
                typed=0,
                blocked=0,
                excluded=1,
                not_attempted=0,
            )
            failures = MODULE.verify(cohort, report)
            self.assertTrue(any("expected typed or blocked" in item for item in failures))
            self.assertTrue(any("excluded counter must be zero" in item for item in failures))

    def test_gate_rejects_malformed_outcome_and_invalid_tree_mapping(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            self.write_report(
                report,
                [self.report_row("scala/Example.scala#tree=7", "typed-ish", "8", "2")],
            )
            failures = MODULE.verify(cohort, report)
            self.assertTrue(any("expected typed or blocked" in item for item in failures))
            self.assertTrue(any("invalid source-tree mapping" in item for item in failures))

    def test_gate_rejects_inconsistent_attempt_counters(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cohort, report = self.write_fixture(directory, "typed")
            self.write_report(
                report,
                [self.report_row("scala/Example.scala#tree=7", "typed", "7", "2")],
                attempted=0,
                typed=0,
            )
            failures = MODULE.verify(cohort, report)
            self.assertTrue(
                any("typed counter is 0, but report rows contain 1" in item for item in failures)
            )
            self.assertTrue(any("selected=1 but attempted=0" in item for item in failures))

    def test_gate_rejects_malformed_report_row_width(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cohort, report = self.write_fixture(Path(temporary), "typed")
            report.write_text(report.read_text(encoding="utf-8") + "extra\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "malformed ordinary audit row width"):
                MODULE.verify(cohort, report)


if __name__ == "__main__":
    unittest.main()
