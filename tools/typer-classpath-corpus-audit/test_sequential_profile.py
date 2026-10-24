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
AUDIT_METHOD_TIMING file=compiler/A.scala tree=8 line=12 method=foo elapsed_ms=44
AUDIT_METHOD_RESULT file=compiler/A.scala tree=8 result=success
AUDIT_MATCH_PROBES available=6 executed=0
AUDIT_MATCH_PROBE file=compiler/A.scala method_symbol=17 case_tree=21 family=literal
AUDIT_MATCH_RESULT file=compiler/A.scala method_symbol=17 case_tree=21 family=literal result=success
AUDIT_MATCH_PHASE file=compiler/A.scala elapsed_ms=46 selectors=1 probes=1
AUDIT_MATCH_RESOLVER file=compiler/A.scala status=captured package_calls=1 member_calls=4 module_identity_calls=2 module_completion_calls=0 class_completion_calls=0 classfile_completion_calls=0 tasty_completion_calls=0 package_inner_us=100 member_inner_us=11700 module_identity_inner_us=200 module_completion_inner_us=0 class_completion_inner_us=0 classfile_completion_inner_us=0 tasty_completion_inner_us=0 rollback_count=1 rollback_operation_observations=2 rollback_member_observations=3 classpath_requests=8 classpath_inner_calls=7 classpath_positive=4 classpath_negative=3 classpath_errors=0 classpath_negative_cache_hits=1 classpath_returned_bytes=900 classpath_inner_wall_us=70
AUDIT_MATCH_RESOLVER_WRAPPER file=compiler/A.scala operation=member_lookup calls=4 wall_us=12000 delegated_us=11700 exclusive_us=300
AUDIT_RESOLVER_WRAPPER operation=member_lookup calls=3 wall_us=500 delegated_us=300 exclusive_us=200
AUDIT_CHEAP_RESOLVER scope=source operation=member_lookup calls=5 wall_us=900 delegated_us=700 exclusive_us=200
AUDIT_CHEAP_RESOLVER_OUTCOMES scope=source operation=member_lookup ok=2 none=2 errors=1
AUDIT_ROLLBACK count=2 operation_observations=4 member_observations=6
AUDIT_CLASSLOADER_RESOLVER_COUNTS member_lookups=7 successes=3 misses=3 errors=1 rollbacks=4
AUDIT_RESOLVER_STAGE stage=delegate_target_resolution calls=7 elapsed_us=120
AUDIT_MATCH_CLASSLOADER_RESOLVER_COUNTS file=compiler/A.scala member_lookups=5 successes=2 misses=3 errors=0 rollbacks=3
AUDIT_MATCH_RESOLVER_STAGE file=compiler/A.scala stage=delegate_target_resolution calls=5 elapsed_us=80
    space=Prefix::prefix=ModuleClass::namespace=term: requests=12, known_requests=10, distinct_keys=4, unknown_identity_requests=2, owners=3, names=5, files=1, repeated_hit_factor=3.000, known_repeated_hit_factor=2.500, outcomes={"none": 12}
AUDIT_CLASSLOAD_TIMINGS resource_lookup_ns=1000 classfile_decode_ns=2000 tasty_decode_ns=3000 load_uncached_inclusive_ns=9000
AUDIT_MATCH_CLASSLOAD_TIMINGS file=compiler/A.scala resource_lookup_ns=400 classfile_decode_ns=500 tasty_decode_ns=600 load_uncached_inclusive_ns=700
AUDIT_CLASSPATH_RESOURCES requests=10 inner_calls=7 positive=4 negative=3 errors=0 negative_cache_hits=3 returned_bytes=900 inner_wall_us=70 bytes_semantics=logical_resource_bytes_not_physical_io
AUDIT_METHOD_RESOLVER file=compiler/A.scala tree=8 module_identity_calls=2 module_identity_inner_ms=3 module_identity_inner_us=3400 module_completion_calls=1 module_completion_inner_ms=6 module_completion_inner_us=6400 class_completion_calls=2 class_completion_inner_ms=8 class_completion_inner_us=8200 classfile_completion_calls=1 classfile_completion_inner_us=5100 tasty_completion_calls=1 tasty_completion_inner_us=3100 member_calls=4 member_inner_ms=11 member_inner_us=11700 package_calls=1 package_inner_ms=1 package_inner_us=1500
AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete parse_ms=12 inventory_ms=5 namer_ms=2 typer_setup_ms=3 typing_ms=45 postprocess_ms=1 match_ms=46 total_ms=68
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
        self.assertEqual(parsed["methods"][("compiler/A.scala", "8")]["line"], 12)
        self.assertEqual(parsed["outcomes"][("compiler/A.scala", "8")], "success")
        self.assertEqual(parsed["match_probes"][0], {"available": "6", "executed": "0"})
        self.assertEqual(parsed["match_probe_ids"], [("compiler/A.scala", "17", "21", "literal")])
        self.assertEqual(parsed["match_outcomes"][("compiler/A.scala", "17", "21", "literal")], "success")
        self.assertEqual(parsed["files"]["compiler/A.scala"]["match_ms"], 46)
        self.assertEqual(parsed["match_phases"][0]["selectors"], "1")
        self.assertEqual(parsed["match_resolver"][0]["member_calls"], "4")
        self.assertEqual(parsed["match_resolver"][0]["classpath_positive"], "4")
        self.assertEqual(parsed["match_resolver_wrappers"][0]["exclusive_us"], "300")
        self.assertEqual(parsed["wrappers"]["member_lookup"]["exclusive_us"], 200)
        self.assertEqual(parsed["cheap_resolver"][("source", "member_lookup")]["calls"], 5)
        self.assertEqual(parsed["cheap_resolver_outcomes"][("source", "member_lookup")]["none"], 2)
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
        self.assertEqual(parsed["resolver_profile"]["member_lookups"], 7)
        self.assertEqual(parsed["resolver_profile"]["rollbacks"], 4)
        self.assertEqual(parsed["match_resolver_profiles"][0]["member_lookups"], 5)
        self.assertEqual(parsed["match_resolver_profiles"][0]["file"], "compiler/A.scala")
        self.assertEqual(parsed["repeated_requests"][0]["distinct_keys"], 4)
        self.assertEqual(parsed["repeated_requests"][0]["repeated_hit_factor"], 3.0)
        self.assertEqual(parsed["classload_timings"]["tasty_decode_ns"], 3000)
        self.assertEqual(parsed["match_classload_timings"][0]["file"], "compiler/A.scala")
        self.assertEqual(parsed["match_classload_timings"][0]["tasty_decode_ns"], 600)
        self.assertEqual(parsed["resolver_stages"][-1]["stage"], "delegate_target_resolution")
        self.assertEqual(parsed["match_resolver_stages"][-1]["stage"], "delegate_target_resolution")

    def test_new_rollback_producer_fields_reach_report_with_exact_counts(self):
        log = """AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete total_ms=10
AUDIT_ROLLBACK count=350 checkpoint_count=727 checkpoint_max_depth=0 operation_appended=1444 member_appended=722 operation_rollback_coverage=1444 member_rollback_coverage=722 operation_rows_scanned=1444 member_rows_scanned=722 operation_status_changes=1444 member_status_changes=0
AUDIT_PROFILE_SUMMARY files=1 expected_files=1 wall_ms=10 timing_mode=full
"""
        run = sequential_profile.parse_run(log)

        with tempfile.TemporaryDirectory() as temporary:
            sequential_profile.write_report(
                Path(temporary), [run], [10], ["warm"],
                [{"mode": "warm-read-through", "before_bytes": 0}], [], "0" * 40,
                Path(temporary), Path(temporary), "21.0.8", Path(temporary), 0,
            )

            rollback = (Path(temporary) / "rollback.tsv").read_text()

        self.assertEqual(
            rollback,
            "run\tcount\tcheckpoint_count\tcheckpoint_max_depth\toperation_appended\tmember_appended\t"
            "operation_observations\tmember_observations\toperation_rows_scanned\tmember_rows_scanned\t"
            "operation_status_changes\tmember_status_changes\n"
            "1\t350\t727\t0\t1444\t722\t1444\t722\t1444\t722\t1444\t0\n",
        )

    def test_legacy_rollback_producer_marks_new_metrics_unavailable(self):
        log = """AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete total_ms=10
AUDIT_ROLLBACK count=350 operation_observations=1444 member_observations=722
AUDIT_PROFILE_SUMMARY files=1 expected_files=1 wall_ms=10 timing_mode=full
"""
        run = sequential_profile.parse_run(log)

        with tempfile.TemporaryDirectory() as temporary:
            sequential_profile.write_report(
                Path(temporary), [run], [10], ["warm"],
                [{"mode": "warm-read-through", "before_bytes": 0}], [], "0" * 40,
                Path(temporary), Path(temporary), "21.0.8", Path(temporary), 0,
            )

            rollback = (Path(temporary) / "rollback.tsv").read_text().splitlines()

        self.assertEqual(
            rollback[1],
            "1\t350\tnot_captured\tnot_captured\tnot_captured\tnot_captured\t"
            "1444\t722\tnot_captured\tnot_captured\tnot_captured\tnot_captured",
        )

    def test_cheap_match_counters_sum_all_files_independent_of_order(self):
        blocks = [
            "AUDIT_CHEAP_RESOLVER scope=match operation=member_lookup calls=560 wall_us=1000 delegated_us=900 exclusive_us=100",
            "AUDIT_CHEAP_RESOLVER_OUTCOMES scope=match operation=member_lookup ok=18 none=541 errors=1",
            "AUDIT_CHEAP_RESOLVER scope=match operation=member_lookup calls=470 wall_us=2000 delegated_us=1800 exclusive_us=200",
            "AUDIT_CHEAP_RESOLVER_OUTCOMES scope=match operation=member_lookup ok=5 none=464 errors=1",
            "AUDIT_CHEAP_RESOLVER scope=match operation=member_lookup calls=1687 wall_us=3000 delegated_us=2900 exclusive_us=100",
            "AUDIT_CHEAP_RESOLVER_OUTCOMES scope=match operation=member_lookup ok=18 none=1668 errors=1",
        ]
        permutations = [
            [*blocks, "AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete total_ms=0", "AUDIT_PROFILE_SUMMARY files=1 expected_files=1 wall_ms=0"],
            [*reversed(blocks), "AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete total_ms=0", "AUDIT_PROFILE_SUMMARY files=1 expected_files=1 wall_ms=0"],
            [*blocks[2:], *blocks[:2], "AUDIT_FILE_TIMING file=compiler/A.scala outcome=complete total_ms=0", "AUDIT_PROFILE_SUMMARY files=1 expected_files=1 wall_ms=0"],
        ]

        results = [
            sequential_profile.parse_run("\n".join(lines))
            for lines in permutations
        ]

        for parsed in results:
            self.assertEqual(
                parsed["cheap_resolver"][("match", "member_lookup")],
                {"calls": 2717, "wall_us": 6000, "delegated_us": 5600, "exclusive_us": 400},
            )
            self.assertEqual(
                parsed["cheap_resolver_outcomes"][("match", "member_lookup")],
                {"ok": 41, "none": 2673, "errors": 3},
            )

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
                    [], "0" * 40, Path(temporary), Path(temporary), "21.0.8", Path(temporary), 0,
                )

            summary = (Path(temporary) / "summary.md").read_text()
            timing_line = next(line for line in summary.splitlines() if "profile report generation" in line)
            self.assertNotIn("PROFILE_REPORT_", summary)
            self.assertGreaterEqual(int(timing_line.split("took about ", 1)[1].split(" ms", 1)[0]), 30)

    def test_timing_only_report_marks_resolver_attribution_uncaptured(self):
        with tempfile.TemporaryDirectory() as temporary:
            run = {
                "files": {"compiler/A.scala": {"total_ms": 20, "outcome": "complete", "typing_ms": 20}},
                "methods": {("compiler/A.scala", "8"): {"elapsed_ms": 20, "line": 12, "method": "foo"}},
                "resolver": {("compiler/A.scala", "8"): {"member_calls": 0, "member_inner_us": 0}},
                "summary": {"wall_ms": "20", "timing_mode": "semantic_only", "method_limit_per_file": "1"},
                "loads": {}, "resolver_counts": {"resolver_member_requests": 0}, "load_roots": [],
                "outcomes": {}, "match_probes": [], "match_probe_ids": [], "match_outcomes": {},
            }
            sequential_profile.write_report(
                Path(temporary), [run], [20], ["warm"], [{"mode": "warm-read-through", "before_bytes": 0}],
                ["compiler/A.scala"], "0" * 40, Path(temporary), Path(temporary), "21.0.8", Path(temporary), 0,
            )

            summary = (Path(temporary) / "summary.md").read_text()
            methods = (Path(temporary) / "methods.tsv").read_text()
            self.assertIn("Detailed resolver counters and timings are not captured outside full mode", summary)
            self.assertIn("Detailed per-method attribution is only available in full mode", summary)
            self.assertIn("not_captured", methods)
            self.assertNotIn("Three confirmed hot method roots", summary)

    def test_cheap_counter_and_full_subphase_rows_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            run = {
                "files": {"compiler/A.scala": {"total_ms": 20, "outcome": "complete", "typing_ms": 20}},
                "methods": {}, "resolver": {}, "summary": {"wall_ms": "20", "timing_mode": "cheap_counters"},
                "loads": {}, "resolver_counts": {}, "load_roots": [], "cheap_resolver": {
                    ("source", "member_lookup"): {"calls": 5, "wall_us": 900, "delegated_us": 700, "exclusive_us": 200},
                    ("match", "member_lookup"): {"calls": 17, "wall_us": 300, "delegated_us": 250, "exclusive_us": 50},
                }, "cheap_resolver_outcomes": {
                    ("source", "member_lookup"): {"ok": 2, "none": 2, "errors": 1},
                    ("match", "member_lookup"): {"ok": 3, "none": 13, "errors": 1},
                }, "resolver_stages": [{"stage": "observation_collection", "calls": "5", "elapsed_us": "300"}],
            }
            sequential_profile.write_report(
                Path(temporary), [run], [20], ["warm"], [{"mode": "warm-read-through", "before_bytes": 0}],
                ["compiler/A.scala"], "0" * 40, Path(temporary), Path(temporary), "21.0.8", Path(temporary), 0,
            )

            cheap = (Path(temporary) / "cheap-resolver.tsv").read_text()
            stages = (Path(temporary) / "resolver-stages.tsv").read_text()
            summary = (Path(temporary) / "summary.md").read_text()
            self.assertIn("source\tmember_lookup\t5\t900\t700\t200\t2\t2\t1", cheap)
            self.assertIn("match\tmember_lookup\t17\t300\t250\t50\t3\t13\t1", cheap)
            self.assertIn("source\tobservation_collection\t5\t300", stages)
            self.assertIn("Cheap counter adapter", summary)
            self.assertIn("Full-mode AuditResolver subphases", summary)


if __name__ == "__main__":
    unittest.main()
