import importlib.machinery
import importlib.util
from argparse import Namespace
from pathlib import Path
import unittest


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

    def test_missing_jmods_path_is_actionable(self):
        with self.assertRaisesRegex(RuntimeError, "install/provide a JDK 21"):
            benchmark.jdk_path(Namespace(jmods="/definitely/missing/jdk/jmods"))


if __name__ == "__main__":
    unittest.main()
