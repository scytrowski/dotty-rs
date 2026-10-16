import importlib.machinery
import importlib.util
from argparse import Namespace
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


RUNNER = Path(__file__).with_name("run")
loader = importlib.machinery.SourceFileLoader("jdk_classpath_benchmark", str(RUNNER))
spec = importlib.util.spec_from_loader(loader.name, loader)
benchmark = importlib.util.module_from_spec(spec)
loader.exec_module(benchmark)


class SummaryTests(unittest.TestCase):
    def test_percentile_uses_nearest_rank(self):
        self.assertEqual(benchmark.percentile95(list(range(1, 11))), 10)

    def test_summary_keeps_lookup_categories_separate(self):
        samples = []
        for sample in range(1, 11):
            samples.append({
                "rss_kib": 100 + sample,
                "measurements": {
                    "init_ns": sample * 1_000_000,
                    "fd_count_after_init": 3 + sample,
                    "fd_count_after_workload": 4 + sample,
                    "class_lookups": [{
                        "class": "java/lang/Object",
                        "first_lookup_ns": sample * 1_000_000,
                        "warm_iterations": 2,
                        "warm_total_ns": sample * 2_000_000,
                    }],
                    "negative_lookup": {
                        "first_lookup_ns": sample * 3_000_000,
                        "warm_iterations": 2,
                        "warm_total_ns": sample * 4_000_000,
                    },
                    "package_lookups": [{
                        "query": "contains_package",
                        "package": "java/lang",
                        "iterations": 2,
                        "total_ns": sample * 5_000_000,
                    }],
                    "concurrent_lookups": [{
                        "module": "java.net.http",
                        "class": "java/net/http/HttpClient",
                        "threads": 4,
                        "iterations_per_thread": 10,
                        "total_ns": sample * 40_000_000,
                    }],
                },
            })

        result = benchmark.summarize(samples)
        self.assertEqual(result["init_ms"], {"median": 5.5, "p95": 10.0})
        self.assertEqual(result["peak_rss_kib"], {"median": 105.5, "p95": 110})
        self.assertEqual(result["negative_lookup"]["first_lookup_ms"], {"median": 16.5, "p95": 30.0})
        self.assertEqual(
            result["package_lookup_ms_per_op"]["contains_package:java/lang"],
            {"median": 13.75, "p95": 25.0},
        )
        self.assertEqual(
            result["concurrent_lookup_ms_per_op"]["java.net.http:java/net/http/HttpClient"],
            {"median": 5.5, "p95": 10.0},
        )
        self.assertEqual(
            result["open_file_descriptors"]["fd_count_after_init"],
            {"median": 8.5, "p95": 13},
        )

    def test_missing_jmods_path_is_actionable(self):
        with self.assertRaisesRegex(RuntimeError, "install/provide a JDK 21"):
            benchmark.jdk_path(Namespace(jmods="/definitely/missing/jdk/jmods"))

    def test_build_driver_uses_cargo_artifact_from_custom_target_dir(self):
        with tempfile.TemporaryDirectory() as temporary:
            target_dir = Path(temporary) / "custom-target"
            executable = target_dir / "release/examples/jdk_classpath_benchmark"
            artifact = {
                "reason": "compiler-artifact",
                "target": {"name": "jdk_classpath_benchmark", "kind": ["example"]},
                "executable": str(executable),
            }
            result = SimpleNamespace(stdout=json.dumps(artifact) + "\n")
            environment = os.environ.copy()
            environment["CARGO_TARGET_DIR"] = str(target_dir)
            with mock.patch.object(benchmark, "run_checked", return_value=result) as run_checked:
                found = benchmark.build_driver(env=environment)

        self.assertEqual(found, executable.resolve())
        self.assertIn("--message-format=json-render-diagnostics", run_checked.call_args.args[0])
        self.assertEqual(run_checked.call_args.kwargs["env"]["CARGO_TARGET_DIR"], str(target_dir))

    def test_relative_jmods_path_is_stable_when_caller_directory_differs_from_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            caller = Path(temporary) / "jdk-home"
            jmods = caller / "jmods"
            jmods.mkdir(parents=True)
            (jmods / "java.base.jmod").touch()
            old_cwd = Path.cwd()
            try:
                os.chdir(caller)
                path, modules = benchmark.jdk_path(Namespace(jmods="jmods"))
            finally:
                os.chdir(old_cwd)

        self.assertEqual(path, jmods.resolve())
        self.assertTrue(path.is_absolute())
        self.assertEqual([module.name for module in modules], ["java.base.jmod"])
        measurements = json.dumps({"peak_rss_kib": 123})
        with mock.patch.object(
            benchmark.subprocess,
            "run",
            return_value=SimpleNamespace(returncode=0, stdout=measurements, stderr=""),
        ) as run:
            benchmark.run_samples(Path("/benchmark-driver"), path, 1)
        command = run.call_args.args[0]
        self.assertEqual(command[2], str(jmods.resolve()))
        self.assertEqual(run.call_args.kwargs["cwd"], benchmark.ROOT)


class MetadataTests(unittest.TestCase):
    @staticmethod
    def sample(sessions=1, factor=1):
        row = {
            "sessions": sessions, "module_count": 69,
            "catalog_string_bytes_per_session": 80_000,
            "init_ns": factor * sessions * 1_000_000,
            "init_per_session_ns": [factor * 1_000_000] * sessions,
            "first_catalog_per_session_ns": [factor * 2_000_000] * sessions,
            "module_lookups": [dict(module=name, first_snapshot_lookup_ns=factor * 100,
                                    snapshot_iterations=10_000, snapshot_total_ns=factor * 1_000_000,
                                    fresh_iterations=20, fresh_total_ns=factor * 40_000_000)
                               for name in benchmark.MODULE_QUERIES],
            "enumeration": {"iterations": 10_000, "total_ns": factor * 2_000_000},
            "concurrent": [dict(mode=mode, threads=4, iterations_per_thread=10, total_ns=factor * 40_000_000)
                           for mode in ("shared_session", "independent_sessions")],
        }
        for phase in benchmark.RESOURCE_PHASES:
            row[phase] = {"fd_count": 4 + sessions * 69, "rss_kib": 100 * sessions, "peak_rss_kib": 200 * sessions}
        return row

    def test_summaries_separate_snapshots_fresh_catalogs_and_amortized_concurrency(self):
        rows = [self.sample(2, factor) for factor in range(1, 11)]
        for row in rows:
            benchmark.validate_metadata_sample(row, 2, 69)
        result = benchmark.summarize_metadata(rows)
        self.assertEqual(result["init_ms"], {"median": 11, "p95": 20})
        self.assertEqual(result["first_catalog_ms_per_session"], {"median": 11, "p95": 20})
        self.assertEqual(result["module_lookups"]["java.base"]["fresh_catalog_and_get_ms_per_op"], {"median": 11, "p95": 20})
        self.assertAlmostEqual(result["module_lookups"]["java.base"]["snapshot_ms_per_op"]["median"], 0.00055)
        self.assertEqual(result["module_lookups"]["java.base"]["snapshot_ms_per_op"]["p95"], 0.001)
        self.assertEqual(result["concurrent_amortized_ms_per_op"]["shared_session"], {"median": 5.5, "p95": 10})
        self.assertEqual(result["resources"]["after_init"]["fd_count"], {"median": 142, "p95": 142})

    def test_rejects_missing_fields_and_wrong_counts(self):
        mutations = [
            lambda row: row.pop("after_calls"),
            lambda row: row.update(sessions=2),
            lambda row: row.update(module_count=68),
            lambda row: row["module_lookups"].pop(),
            lambda row: row["concurrent"].pop(),
            lambda row: row["first_catalog_per_session_ns"].append(1),
            lambda row: row["after_init"].update(extra=1),
            lambda row: row["module_lookups"][0].update(fresh_iterations=0),
            lambda row: row["concurrent"][0].update(threads=0),
            lambda row: row["after_init"].update(rss_kib=True),
            lambda row: row.update(init_ns=-1),
        ]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                row = self.sample()
                mutation(row)
                with self.assertRaisesRegex(RuntimeError, "invalid metadata sample"):
                    benchmark.validate_metadata_sample(row, 1, 69)

    def test_samples_are_fresh_processes_and_cohorts_are_interleaved(self):
        def run(command):
            if "--metadata-check" in command:
                return SimpleNamespace(stdout='{"metadata_check": true}')
            sessions = int(command[-1])
            return SimpleNamespace(stdout=json.dumps(self.sample(sessions)))
        with mock.patch.object(benchmark, "run_checked", side_effect=run) as checked:
            cohorts = benchmark.run_metadata_samples(Path("/driver"), Path("/jmods"), 10, 69)
        self.assertEqual(len(checked.call_args_list), 31)
        self.assertEqual([int(call.args[0][-1]) for call in checked.call_args_list[1:7]], [1, 2, 4, 1, 2, 4])
        self.assertEqual([cohorts[key]["sample_count"] for key in ("1", "2", "4")], [10, 10, 10])

    def test_child_failure_is_reported(self):
        with mock.patch.object(benchmark.subprocess, "run", return_value=SimpleNamespace(returncode=2, stdout="", stderr="bad descriptor")):
            with self.assertRaisesRegex(RuntimeError, "bad descriptor"):
                benchmark.run_metadata_samples(Path("/driver"), Path("/jmods"), 10, 69)

    def test_metadata_smoke_builds_fixture_jmods_without_java(self):
        def run(command):
            directory = Path(command[-1])
            with benchmark.zipfile.ZipFile(directory / "fixture.alpha.jmod") as archive:
                self.assertEqual(archive.namelist(), ["classes/module-info.class"])
                self.assertTrue(archive.read("classes/module-info.class").startswith(b"\xca\xfe\xba\xbe"))
            return SimpleNamespace(stdout='{"metadata_smoke": true, "module_count": 3}')
        with mock.patch.object(benchmark, "run_checked", side_effect=run):
            benchmark.metadata_smoke(Path("/driver"))


if __name__ == "__main__":
    unittest.main()
