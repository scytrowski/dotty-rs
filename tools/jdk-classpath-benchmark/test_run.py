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


if __name__ == "__main__":
    unittest.main()
