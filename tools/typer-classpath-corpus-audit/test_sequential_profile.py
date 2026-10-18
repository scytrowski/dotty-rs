import tempfile
import unittest
from pathlib import Path

import sequential_profile


class SequentialProfileTests(unittest.TestCase):
    def test_manifest_accepts_only_existing_files_under_pinned_roots(self):
        with tempfile.TemporaryDirectory() as temporary:
            scala_root = Path(temporary)
            source = scala_root / "compiler/src/dotty/tools/dotc/ast/tpd.scala"
            source.parent.mkdir(parents=True)
            source.write_text("object tpd")
            manifest = scala_root / "manifest.tsv"
            manifest.write_text("path\trole\ncompiler/src/dotty/tools/dotc/ast/tpd.scala\texpensive\n")

            self.assertEqual(
                sequential_profile.read_manifest(manifest, scala_root),
                ["compiler/src/dotty/tools/dotc/ast/tpd.scala"],
            )

    def test_manifest_rejects_missing_paths_instead_of_silently_dropping_them(self):
        with tempfile.TemporaryDirectory() as temporary:
            scala_root = Path(temporary)
            (scala_root / "compiler").mkdir()
            manifest = scala_root / "manifest.tsv"
            manifest.write_text("path\trole\ncompiler/missing.scala\tcontrol\n")

            with self.assertRaises(SystemExit):
                sequential_profile.read_manifest(manifest, scala_root)

    def test_manifest_rejects_paths_outside_the_pinned_source_roots(self):
        with tempfile.TemporaryDirectory() as temporary:
            scala_root = Path(temporary)
            (scala_root / "other").mkdir()
            (scala_root / "other/source.scala").write_text("object Other")
            manifest = scala_root / "manifest.tsv"
            manifest.write_text("path\trole\nother/source.scala\tcontrol\n")

            with self.assertRaises(SystemExit):
                sequential_profile.read_manifest(manifest, scala_root)

    def test_profile_log_extracts_file_phases_methods_resolver_and_run_setup(self):
        log = """AUDIT_FILE_PHASE file=compiler/A.scala phase=parse ms=12
AUDIT_METHOD_TIMING file=compiler/A.scala tree=8 method=foo elapsed_ms=44
AUDIT_METHOD_RESOLVER file=compiler/A.scala tree=8 module_identity_calls=2 module_identity_inner_ms=3 module_identity_inner_us=3400 module_completion_calls=1 module_completion_inner_ms=6 module_completion_inner_us=6400 class_completion_inner_ms=8 class_completion_inner_us=8200 member_calls=4 member_inner_ms=11 member_inner_us=11700 package_calls=1 package_inner_ms=1 package_inner_us=1500
AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete parse_ms=12 inventory_ms=5 namer_ms=2 typer_setup_ms=3 typing_ms=45 postprocess_ms=1 total_ms=68
AUDIT_PROFILE_SUMMARY files=1 expected_files=1 wall_ms=75 classpath_setup_ms=10 file_discovery_ms=2 source_read_ms=1 source_audit_merge_ms=68
  full_completions=2
  completed_cache_hits=1
  resolver_member_requests=4
    root=scala/Function1::file=compiler/A.scala::method=compiler/A.scala#tree=8::Foo::request=prefix=scala::name=Function1::depth=1::distinct_classes=7::full_completions=2::shells_created=4::shells_reused=9::member_type_references=15::hierarchy_loads=1::completed_cache_hits=0::completed_classes=[scala/Function1]
"""
        parsed = sequential_profile.parse_run(log)

        self.assertEqual(parsed["summary"]["classpath_setup_ms"], "10")
        self.assertEqual(parsed["files"]["compiler/A.scala"]["typing_ms"], 45)
        self.assertEqual(parsed["methods"][("compiler/A.scala", "8")]["method"], "foo")
        self.assertEqual(parsed["resolver"][("compiler/A.scala", "8")]["member_calls"], 4)
        self.assertEqual(parsed["resolver"][("compiler/A.scala", "8")]["member_inner_us"], 11700)
        self.assertEqual(parsed["loads"]["completed_cache_hits"], 1)
        self.assertEqual(parsed["resolver_counts"]["resolver_member_requests"], 4)
        self.assertEqual(parsed["load_roots"][0]["root"], "scala/Function1")
        self.assertEqual(parsed["load_roots"][0]["distinct_classes"], 7)


if __name__ == "__main__":
    unittest.main()
