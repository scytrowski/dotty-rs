"""Focused tests for the #1336 read-only classpath evidence collector."""

import csv
import importlib.util
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("unresolved_import_roots.py")
spec = importlib.util.spec_from_file_location("unresolved_import_roots", SCRIPT)
assert spec and spec.loader
audit = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = audit
spec.loader.exec_module(audit)


def tasty(major: int, minor: int, experimental: int) -> bytes:
    # TASTy base-128 integers use the high bit to mark the final group.
    return bytes((0x5C, 0xA1, 0xAB, 0x1F, major | 0x80, minor | 0x80, experimental | 0x80))


class UnresolvedImportRootTests(unittest.TestCase):
    def test_frozen_observation_cohort_denominators(self):
        root = SCRIPT.parents[2]
        report = root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0.md"
        rows = audit.profile.extract_observations(report.read_text(encoding="utf-8"))
        self.assertEqual(len(rows), 1063)
        self.assertEqual(len({row["source_file"] for row in rows}), 81)
        self.assertEqual(len({row["target_key"] for row in rows}), 31)
        self.assertEqual(len({row["blocker_origin"] for row in rows}), 393)
        candidates = audit.render_target_candidates(rows)
        self.assertEqual(candidates.splitlines()[0], "target_key\tpriority\tcandidate_symbol\torigin")
        self.assertEqual(len({line.split("\t", 1)[0] for line in candidates.splitlines()[1:]}), 31)
        self.assertIn("Character::<first-segment-unresolved>\t3\tjava.lang.Character\tscala_default_import", candidates)
        self.assertIn("collection::<first-segment-unresolved>\t4\tscala.collection\tscala_default_import", candidates)

    def test_parses_pinned_28_8_and_28_9_headers(self):
        self.assertEqual(audit.tasty_header(tasty(28, 8, 0)), "28.8.0")
        self.assertEqual(audit.tasty_header(tasty(28, 9, 0)), "28.9.0")
        self.assertEqual(audit.tasty_header(b"bad!"), "invalid-magic")

    def test_reports_exact_resources_and_keeps_absence_distinct(self):
        with tempfile.TemporaryDirectory() as temp:
            jar = Path(temp) / "compiler.jar"
            with zipfile.ZipFile(jar, "w") as archive:
                archive.writestr("dotty/tools/dotc/core/Types.tasty", tasty(28, 9, 0))
                archive.writestr("dotty/tools/dotc/core/Types$.class", b"class-bytes")
            row = {
                "target_key": "Types::<first-segment-unresolved>",
                "source_file": "compiler/src/example.scala",
                "method": "example",
                "line": "1",
                "blocker_origin": "inherited:compiler/src/example.scala#tree=1",
                "import_tree": "compiler/src/example.scala#tree=1",
                "context_owner": "example.Owner",
                "context_package": "dotty.tools.dotc.core",
                "first_failing_segment": "Types",
                "request_kind": "resolve_package",
                "source_package_registry_lookup": "relative=missing@dotty.tools.dotc.core.Types;enclosing=none;root=missing",
            }
            rows, summary, _ = audit.scan([jar], [row])
            self.assertIn("dotty/tools/dotc/core/Types.tasty", rows)
            self.assertIn("28.9.0", rows)
            self.assertIn("candidate_resource_present_identity_requires_semantic_oracle", rows)
            self.assertIn("TASTy 28.9.0", summary)
            self.assertEqual((rows, summary), audit.scan([jar], [row])[:2])

            missing = dict(
                row,
                target_key="Absent::<first-segment-unresolved>",
                first_failing_segment="Absent",
                source_package_registry_lookup="relative=missing@dotty.tools.dotc.core.Absent;enclosing=none;root=missing",
            )
            rows, _, _ = audit.scan([jar], [missing])
            self.assertIn("no_candidate_resource_in_supplied_classpath", rows)

    def test_package_level_quotes_helper_uses_synthetic_package_resource(self):
        root = SCRIPT.parents[2]
        report = root / "docs/typer-classpath-import-prefix-profile-1180-3.9.0.md"
        observations = audit.profile.extract_observations(report.read_text(encoding="utf-8"))
        row = next(row for row in observations if row["target_key"].startswith("quotes::<"))
        with tempfile.TemporaryDirectory() as temp:
            jar = Path(temp) / "scala-library.jar"
            with zipfile.ZipFile(jar, "w") as archive:
                archive.writestr("scala/quoted/Quotes$package.tasty", tasty(28, 9, 0))
                archive.writestr("scala/quoted/Quotes$package$.class", b"class-bytes")
            rows, _, totals = audit.scan([jar], [row])
            self.assertIn("scala/quoted/Quotes$package.tasty", rows)
            self.assertIn("package_level_helper", rows)
            self.assertIn("candidate_resource_present_identity_requires_semantic_oracle", rows)
            self.assertEqual(totals["candidate_resource_present_identity_requires_semantic_oracle"], 1)

    def test_compiler_object_source_kinds_match_term_resolver_results(self):
        root = SCRIPT.parents[2]
        source_path = root / "docs/typer-classpath-import-roots-1336-target-sources.tsv"
        resolver_path = root / "docs/typer-classpath-import-roots-1336-resolver-results.tsv"
        with source_path.open(encoding="utf-8") as source_file:
            sources = {
                row["target_key"].split("::<", 1)[0]: row["declaration_kind"]
                for row in csv.DictReader(source_file, delimiter="\t")
            }
        with resolver_path.open(encoding="utf-8") as resolver_file:
            results = list(csv.DictReader(resolver_file, delimiter="\t"))
        for name in ("Parsers", "Scanners", "Settings", "SymDenotations"):
            self.assertIn("object", sources[name].lower())
            self.assertTrue(
                any(
                    row["target_key"].startswith(name + "::<")
                    and row["request_kind"] == "package_member"
                    and row["namespace"] == "Term"
                    and row["resolver_result_kind"] == "symbol:Object"
                    for row in results
                ),
                name,
            )

    def test_scans_jmod_class_entries_for_default_java_lang_candidate(self):
        with tempfile.TemporaryDirectory() as temp:
            jmod = Path(temp) / "java.base.jmod"
            with zipfile.ZipFile(jmod, "w") as archive:
                archive.writestr("classes/java/lang/Character.class", b"class-bytes")
            row = {
                "target_key": "Character::<first-segment-unresolved>",
                "source_file": "library/src/scala/sys/process/BasicIO.scala",
                "method": "readCharacter",
                "line": "1",
                "blocker_origin": "inherited:library/src/scala/sys/process/BasicIO.scala#tree=1",
                "import_tree": "library/src/scala/sys/process/BasicIO.scala#tree=1",
                "context_owner": "scala.sys.process.BasicIO",
                "context_package": "scala.sys.process",
                "first_failing_segment": "Character",
                "request_kind": "resolve_package",
                "source_package_registry_lookup": "relative=missing@scala.sys.process.Character;enclosing=none;root=missing",
            }
            rows, _, _ = audit.scan([jmod], [row])
            self.assertIn("java/lang/Character.class", rows)
            self.assertIn("java.base.jmod", rows)
            self.assertIn("candidate_resource_present_identity_requires_semantic_oracle", rows)


if __name__ == "__main__":
    unittest.main()
