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


class AcceptanceIVCloseoutTests(unittest.TestCase):
    def write_closeout_inputs(
        self,
        directory: Path,
        typed_main: int = 17,
        target: bool = True,
        measured_target_status: str = "blocked::IfBranchJoinUnsupported",
    ):
        measured = directory / "measured-2179.tsv"
        integration = directory / "integration-2183.tsv"
        main_required = directory / "required-main.tsv"
        analysis_required = directory / "required-analysis.tsv"
        manifests = {"main": directory / "main-manifest.tsv", "analysis": directory / "analysis-manifest.tsv"}
        reports = {"main": directory / "main.tsv", "analysis": directory / "analysis.tsv"}
        measured_rows = []
        integration_rows = []
        required_rows = {"main": [], "analysis": []}
        report_rows = {"main": [], "analysis": []}
        protected_count = {"main": 16, "analysis": 15}
        for cohort in ("main", "analysis"):
            manifest_rows = []
            for index in range(25):
                identity = (
                    "library/src/scala/Option.scala#tree=214"
                    if cohort == "main" and index == 24
                    else "library/src/scala/Tuple.scala#tree=23"
                    if cohort == "main" and index == 23
                    else f"library/src/{cohort}/Method{index}.scala#tree={index}"
                )
                source_tree = (
                    214 if cohort == "main" and index == 24
                    else 23 if cohort == "main" and index == 23
                    else index
                )
                protected = index < protected_count[cohort]
                status = "typed" if (protected and (cohort == "analysis" or index < 16)) else "blocked::MemberNotFound"
                result = "Int" if status == "typed" else "not_available"
                if cohort == "main" and index == 24:
                    status = "typed" if target else "blocked::IfBranchJoinUnsupported"
                    result = "scala.Option[typeparam#0]" if target else "not_available"
                expected_result = (
                    "scala.Option[typeparam#0]"
                    if cohort == "main" and index == 24
                    else "Int"
                )
                if cohort == "main" and index == 23:
                    status = "blocked::ApplicationArgumentConformanceUnsupported"
                    result = "not_available"
                first_status = status if protected else "blocked::MemberNotFound"
                first_result = result if protected else "not_available"
                if cohort == "main" and index == 24:
                    first_status = measured_target_status
                    first_result = "not_available" if first_status.startswith("blocked::") else "scala.Option[typeparam#0]"
                measured_rows.append({
                    "cohort": cohort,
                    "identity": identity,
                    "source_tree": str(source_tree),
                    "first_status": first_status,
                    "second_status": first_status,
                    "first_actual_result": first_result,
                    "second_actual_result": first_result,
                    "expected_result": expected_result,
                    "scala_revision": MODULE.PINNED_SCALA_REVISION,
                    "measured_revision": MODULE.ACCEPTANCE_IV_BASELINE_REVISION,
                    "workflow_head_sha": MODULE.ACCEPTANCE_IV_BASELINE_WORKFLOW_HEAD,
                    "workflow_run_id": MODULE.ACCEPTANCE_IV_BASELINE_RUN_ID,
                    "artifact_id": MODULE.ACCEPTANCE_IV_BASELINE_ARTIFACT_ID,
                    "artifact_sha256": MODULE.ACCEPTANCE_IV_BASELINE_ARTIFACT_SHA256,
                })
                if protected:
                    required_rows[cohort].append(f"{identity}\t{index}\tInt\n")
                report_rows[cohort].append([
                    identity, cohort, f"method{index}", "1", "0..1", "[]", expected_result,
                    result, "1", status,
                    "214" if cohort == "main" and index == 24 else "23" if cohort == "main" and index == 23 else str(index),
                    str(index + 100) if status == "typed" else "-",
                ])
                manifest_rows.append(f"{identity}\t{source_tree}\n")
                integration_status = status
                integration_result = result
                if (cohort == "main" and index == 16) or (cohort == "analysis" and index == 15):
                    integration_status = "typed"
                    integration_result = "Int"
                integration_rows.append({
                    "cohort": cohort,
                    "identity": identity,
                    "source_tree": str(source_tree),
                    # This historical label deliberately conflicts with the
                    # measured first/second outcomes in the regression case.
                    "baseline_status": "blocked",
                    "gate": "observe",
                    "first_status": integration_status,
                    "second_status": integration_status,
                    "actual_result": integration_result,
                    "protected_success": "false",
                    "two_pass_equal": "true",
                })
            manifests[cohort].write_text(
                "identity\tsource_tree\n" + "".join(manifest_rows), encoding="utf-8"
            )
        with measured.open("w", encoding="utf-8", newline="") as output:
            writer = csv.DictWriter(output, fieldnames=list(measured_rows[0]), delimiter="\t", lineterminator="\n")
            writer.writeheader()
            writer.writerows(measured_rows)
        with integration.open("w", encoding="utf-8", newline="") as output:
            writer = csv.DictWriter(output, fieldnames=list(integration_rows[0]), delimiter="\t", lineterminator="\n")
            writer.writeheader()
            writer.writerows(integration_rows)
        for cohort, path in (("main", main_required), ("analysis", analysis_required)):
            required_rows[cohort] = [row.replace("\t", "\trequired_typed\t", 1) for row in required_rows[cohort]]
            path.write_text("identity\tgate\tsource_tree\toracle_result\n" + "".join(required_rows[cohort]), encoding="utf-8")
            with reports[cohort].open("w", encoding="utf-8", newline="") as output:
                typed = sum(row[9] == "typed" for row in report_rows[cohort])
                output.write(
                    "schema=1\npinned_scala_revision=777528f19a58e794c9954a42f433373472ec57f8\n"
                    "selected=25\nattempted=25\n"
                    f"typed={typed}\nblocked={25 - typed}\nexcluded=0\nnot_attempted=0\n"
                )
                writer = csv.writer(output, delimiter="\t", lineterminator="\n")
                writer.writerow(MODULE.REPORT_COLUMNS)
                writer.writerows(report_rows[cohort])
        return reports, measured, integration, main_required, analysis_required, manifests

    def test_closeout_requires_threshold_and_exact_target_in_ordinary_report(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(Path(temporary))
            failures, rows = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration, main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertEqual(failures, [])
            self.assertEqual(len(rows), 50)
            option = next(row for row in rows if row["identity"].endswith("Option.scala#tree=214"))
            self.assertTrue(option["measured_first_status"].startswith("blocked::"))
            self.assertEqual(option["measured_second_status"], option["measured_first_status"])
            self.assertEqual(option["integration_status"], "typed")
            self.assertEqual(option["after_status"], "typed")
            report = Path(temporary) / "closeout.md"
            MODULE._write_acceptance_iv_markdown(
                report, rows, True, "tested-sha", "pr-head-sha", "https://actions/run/1"
            )
            closeout = report.read_text(encoding="utf-8")
            self.assertIn("Outcome: SPRINT GOAL UNMET pending independent review", closeout)
            self.assertIn("Hosted acceptance gate: PASS", closeout)

    def test_closeout_rejects_unprepared_tuple_proof_without_ordinary_target(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(
                Path(temporary), typed_main=16, target=False
            )
            failures, _ = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration, main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertTrue(any("requires at least 17/25" in failure for failure in failures))
            self.assertTrue(any("no exact original" in failure for failure in failures))

    def test_closeout_rejects_changed_protected_result(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(Path(temporary))
            text = reports["main"].read_text(encoding="utf-8")
            reports["main"].write_text(text.replace("\tInt\t1\ttyped\t0\t100\n", "\tString\t1\ttyped\t0\t100\n", 1), encoding="utf-8")
            failures, _ = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration, main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertTrue(any("protected identity or result changed" in failure for failure in failures))

    def test_closeout_rejects_target_with_unexpected_result_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(Path(temporary))
            text = reports["main"].read_text(encoding="utf-8")
            reports["main"].write_text(
                text.replace(
                    "\tscala.Option[typeparam#0]\t1\ttyped\t214\t124\n",
                    "\tscala.Option[String]\t1\ttyped\t214\t124\n",
                    1,
                ),
                encoding="utf-8",
            )
            failures, _ = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration, main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertTrue(any("typed target has unexpected result shape" in failure for failure in failures))

    def test_closeout_uses_measured_baseline_not_historical_manifest_status(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(
                Path(temporary), measured_target_status="typed"
            )
            failures, _ = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration, main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertTrue(any("already typed in the #2179 baseline" in failure for failure in failures))
            self.assertTrue(any("no exact original" in failure for failure in failures))

    def test_closeout_rejects_nondeterministic_measured_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(Path(temporary))
            text = measured.read_text(encoding="utf-8")
            text = text.replace(
                "blocked::IfBranchJoinUnsupported\tblocked::IfBranchJoinUnsupported",
                "typed\tblocked::IfBranchJoinUnsupported",
                1,
            )
            measured.write_text(text, encoding="utf-8")
            failures, _ = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration,
                main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertTrue(any("#2179 measured attempts disagree" in failure for failure in failures))

    def test_closeout_rejects_nondeterministic_integration_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            reports, measured, integration, main_required, analysis_required, manifests = self.write_closeout_inputs(Path(temporary))
            text = integration.read_text(encoding="utf-8")
            text = text.replace(
                "blocked\tobserve\ttyped\ttyped\tBoolean",
                "blocked\tobserve\ttyped\tblocked::Failure\tBoolean",
                1,
            )
            text = text.replace("\ttrue\n", "\tfalse\n", 1)
            integration.write_text(text, encoding="utf-8")
            failures, _ = MODULE.verify_acceptance_iv(
                reports["main"], reports["analysis"], measured, integration,
                main_required, analysis_required,
                main_manifest_path=manifests["main"], analysis_manifest_path=manifests["analysis"],
            )
            self.assertTrue(any("#2183 integration attempts disagree" in failure for failure in failures))


if __name__ == "__main__":
    unittest.main()
