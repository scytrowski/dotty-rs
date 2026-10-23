import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

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

    def test_cargo_target_cache_state_tracks_compiled_audit_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            target_dir = Path(temporary)
            self.assertEqual(sequential_profile.cargo_target_cache_state(target_dir), "cold")
            deps_dir = target_dir / "debug" / "deps"
            deps_dir.mkdir(parents=True)
            audit_binary = deps_dir / "local_definition_audit-abc123"
            audit_binary.write_text("binary")
            audit_binary.chmod(0o755)
            self.assertEqual(sequential_profile.cargo_target_cache_state(target_dir), "warm")

    def test_profile_log_extracts_file_phases_methods_resolver_and_run_setup(self):
        log = """AUDIT_FILE_PHASE file=compiler/A.scala phase=parse ms=12
AUDIT_METHOD_TIMING file=compiler/A.scala tree=8 method=foo elapsed_ms=44
AUDIT_METHOD_RESULT file=compiler/A.scala tree=8 result=success
AUDIT_MATCH_PROBES available=6 executed=0
AUDIT_MATCH_PROBE file=compiler/A.scala method_symbol=17 case_tree=21 family=literal
AUDIT_MATCH_RESULT file=compiler/A.scala method_symbol=17 case_tree=21 family=literal result=success
AUDIT_RESOLVER_WRAPPER operation=member_lookup calls=3 wall_us=500 delegated_us=300 exclusive_us=200
AUDIT_ROLLBACK count=2 operation_observations=4 member_observations=6
AUDIT_CLASSPATH_RESOURCES requests=10 inner_calls=7 positive=4 negative=3 errors=0 negative_cache_hits=3 returned_bytes=900 inner_wall_us=70 bytes_semantics=logical_resource_bytes_not_physical_io
AUDIT_METHOD_RESOLVER file=compiler/A.scala tree=8 module_identity_calls=2 module_identity_inner_ms=3 module_identity_inner_us=3400 module_completion_calls=1 module_completion_inner_ms=6 module_completion_inner_us=6400 class_completion_calls=2 class_completion_inner_ms=8 class_completion_inner_us=8200 classfile_completion_calls=1 classfile_completion_inner_us=5100 tasty_completion_calls=1 tasty_completion_inner_us=3100 member_calls=4 member_inner_ms=11 member_inner_us=11700 package_calls=1 package_inner_ms=1 package_inner_us=1500
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
        self.assertEqual(parsed["outcomes"][("compiler/A.scala", "8")], "success")
        self.assertEqual(parsed["match_probes"][0], {"available": "6", "executed": "0"})
        self.assertEqual(parsed["match_probe_ids"], [("compiler/A.scala", "17", "21", "literal")])
        self.assertEqual(parsed["match_outcomes"][("compiler/A.scala", "17", "21", "literal")], "success")
        self.assertEqual(parsed["wrappers"]["member_lookup"]["exclusive_us"], 200)
        self.assertEqual(parsed["rollback"]["operation_observations"], 4)
        self.assertEqual(parsed["classpath_resources"]["returned_bytes"], "900")
        self.assertEqual(parsed["resolver"][("compiler/A.scala", "8")]["member_calls"], 4)
        self.assertEqual(parsed["resolver"][("compiler/A.scala", "8")]["member_inner_us"], 11700)
        self.assertEqual(parsed["resolver"][("compiler/A.scala", "8")]["classfile_completion_calls"], 1)
        self.assertEqual(parsed["resolver"][("compiler/A.scala", "8")]["tasty_completion_inner_us"], 3100)
        self.assertEqual(parsed["loads"]["completed_cache_hits"], 1)
        self.assertEqual(parsed["resolver_counts"]["resolver_member_requests"], 4)
        self.assertEqual(parsed["load_roots"][0]["root"], "scala/Function1")
        self.assertEqual(parsed["load_roots"][0]["distinct_classes"], 7)

    def test_report_timer_includes_sidecar_writes_and_is_recorded_in_summary(self):
        with tempfile.TemporaryDirectory() as temporary:
            original_write_text = Path.write_text

            def delayed_write(path, data, *args, **kwargs):
                if path.name == "resolver-counts.tsv":
                    time.sleep(0.03)
                return original_write_text(path, data, *args, **kwargs)

            with patch.object(Path, "write_text", delayed_write):
                sequential_profile.write_report(
                    Path(temporary),
                    [{"files": {}, "methods": {}, "resolver": {}, "summary": {"wall_ms": "0"}, "loads": {},
                      "resolver_counts": {}, "load_roots": []}],
                    [0], ["cold"], [{"mode": "evicted", "before_bytes": 1024, "after_evict_bytes": 0}],
                    [], "0" * 40, Path(temporary), Path(temporary), "21.0.8", Path(temporary),
                )

            summary = (Path(temporary) / "summary.md").read_text()
            timing_line = next(line for line in summary.splitlines() if "profile report generation" in line)
            self.assertNotIn("PROFILE_REPORT_", summary)
            self.assertGreaterEqual(int(timing_line.split("took about ", 1)[1].split(" ms", 1)[0]), 30)


if __name__ == "__main__":
    unittest.main()
