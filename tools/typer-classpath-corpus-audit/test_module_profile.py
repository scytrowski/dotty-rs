import csv
import tempfile
import unittest
import zipfile
from pathlib import Path

from module_profile import evidence_rows, freeze, request_rows


KEY = "prefix=fixture::namespace=term::name=Only::space=Prefix"
OBSERVATION_HEADER = (
    "request_key\tprefix_kind\tpackage\tname\tmember_space\tselector\tsource_file\t"
    "enclosing_attempt\tresolver_outcome\ttype_resource\tcompanion_resource"
)
EVIDENCE_HEADER = (
    "request_key\tselected_resource\tclassification\tobject_identity\t"
    "module_class_identity\tclass_identity\tvisibility\tdetail"
)
BLOCKER_HEADER = (
    "blocker\tfile\tmethod\tattribution\trequest_attempts\tfailed_qualifier\t"
    "request_targets"
)


def report():
    lines = [
        "package_term_request_rows:",
        "  observations=1",
        "  distinct_request_keys=1",
        f"  {OBSERVATION_HEADER}",
        f"  {KEY}\tPackage\tfixture\tOnly\tPrefix\tUnique\tfixture/Use.scala\tfixture/Use.scala#tree=1\tpackage term member unsupported\ttasty@only.jar\tclass@only.jar",
        "package_term_module_evidence:",
        "  distinct_request_keys=1",
        f"  {EVIDENCE_HEADER}",
        f"  {KEY}\ttasty@only.jar\tstandalone object\tfixture.Only\tfixture.Only$\t\tPublic\t",
        "package_term_first_blocker_rows:",
        "  observations=369",
        f"  {BLOCKER_HEADER}",
    ]
    lines.extend(
        f"  ImportQualifierNotFound\tfixture/Use.scala\tfixture/Use.scala#tree={index}\t"
        "inherited\tfixture/Use.scala#tree=1\tfixture.Only\t"
        f"{KEY}"
        for index in range(361)
    )
    lines.extend(
        f"  TermNameNotFound\tfixture/Use.scala\tfixture/Use.scala#tree={index}\tdirect\t"
        "fixture/Use.scala#tree=1\tfixture.Only\t"
        f"{KEY}"
        for index in range(8)
    )
    return "\n".join(lines) + "\n"


class ModuleProfileTests(unittest.TestCase):
    def test_package_request_shape_is_checked_and_sorted(self):
        rows = request_rows(report())
        self.assertEqual(len(rows), 1)
        with self.assertRaisesRegex(ValueError, "unexpected package-term outcome"):
            request_rows(report().replace("package term member unsupported", "external class success"))

    def test_freeze_joins_exact_resource_formats_to_all_requests_and_blockers(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "only.jar"
            with zipfile.ZipFile(archive, "w") as jar:
                jar.writestr("fixture/Only.tasty", b"tasty")
                jar.writestr("fixture/Only.class", b"class")
                jar.writestr("fixture/Only$.class", b"module")
            jmods = root / "jmods"
            jmods.mkdir()
            report_path = root / "audit.md"
            report_path.write_text(report())
            observations = root / "profile.tsv"
            blockers = root / "blockers.tsv"

            summary = freeze(
                report_path,
                observations,
                blockers,
                [archive],
                jmods,
                expected=(1, 1),
            )

            with observations.open() as stream:
                rows = list(csv.DictReader(stream, delimiter="\t"))
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]["type_tasty_resources"], "only.jar!fixture/Only.tasty")
            self.assertEqual(rows[0]["type_class_resources"], "only.jar!fixture/Only.class")
            self.assertEqual(rows[0]["companion_tasty_resources"], "absent")
            self.assertEqual(rows[0]["companion_class_resources"], "only.jar!fixture/Only$.class")
            self.assertEqual(rows[0]["module_classification"], "standalone object")
            with blockers.open() as stream:
                blocker_rows = list(csv.DictReader(stream, delimiter="\t"))
            self.assertEqual(len(blocker_rows), 369)
            self.assertIn("361 `ImportQualifierNotFound`", summary)
            self.assertIn("8 `TermNameNotFound`", summary)

    def test_frozen_baseline_rejects_request_volume_or_key_drift(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "only.jar"
            with zipfile.ZipFile(archive, "w") as jar:
                jar.writestr("fixture/Only.tasty", b"tasty")
            jmods = root / "jmods"
            jmods.mkdir()
            report_path = root / "audit.md"
            report_path.write_text(report())
            with self.assertRaisesRegex(ValueError, "baseline package-term volume changed"):
                freeze(report_path, root / "out.tsv", root / "blockers.tsv", [archive], jmods)

    def test_companion_dollar_class_does_not_create_object_evidence(self):
        no_object_report = report().replace(
            f"  {KEY}\ttasty@only.jar\tstandalone object\tfixture.Only\tfixture.Only$\t\tPublic\t",
            f"  {KEY}\tclass@only.jar\tTASTy has no matching object definition\t\t\t\t\t\t",
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "only.jar"
            with zipfile.ZipFile(archive, "w") as jar:
                jar.writestr("fixture/Only$.class", b"not module proof")
            jmods = root / "jmods"
            jmods.mkdir()
            report_path = root / "audit.md"
            report_path.write_text(no_object_report)
            observations = root / "profile.tsv"
            blockers = root / "blockers.tsv"

            freeze(report_path, observations, blockers, [archive], jmods, expected=(1, 1))

            with observations.open() as stream:
                row = next(csv.DictReader(stream, delimiter="\t"))
            self.assertEqual(row["companion_class_resources"], "only.jar!fixture/Only$.class")
            self.assertEqual(row["module_classification"], "TASTy has no matching object definition")
            self.assertEqual(row["tasty_object_identity"], "-")

    def test_duplicate_tasty_evidence_is_rejected(self):
        duplicate = report().replace(
            "package_term_first_blocker_rows:",
            f"  {KEY}\ttasty@only.jar\tstandalone object\tfixture.Only\tfixture.Only$\t\tPublic\t\n"
            "package_term_first_blocker_rows:",
        )
        with self.assertRaisesRegex(ValueError, "duplicate module-evidence target"):
            evidence_rows(duplicate)


if __name__ == "__main__":
    unittest.main()
