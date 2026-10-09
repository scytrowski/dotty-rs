import importlib.util
import json
import pathlib
import subprocess
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("oracle_batch.py")
SPEC = importlib.util.spec_from_file_location("oracle_batch", MODULE_PATH)
oracle_batch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(oracle_batch)


class OracleBatchTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary_directory.name)
        self.manifest = self.root / "manifest"
        self.counts = self.root / "counts"
        self.manifest.write_text(
            "".join(f"compilation\t/source/{index}.scala\n" for index in range(5)),
            encoding="utf-8",
        )
        self.counts.write_text("scala3\t2\ncats\t3\n", encoding="utf-8")

    def tearDown(self):
        self.temporary_directory.cleanup()

    def test_batches_records_and_attributes_failures_across_source_set_boundary(self):
        calls = []

        def fake_run(command, **kwargs):
            calls.append(command)
            entries = pathlib.Path(command[2]).read_text(encoding="utf-8").splitlines()
            records = []
            for entry in entries:
                path = entry.split("\t", 1)[1]
                kind = "OracleFailure" if path.endswith(("1.scala", "4.scala")) else "Ident"
                records.append(json.dumps({"kind": kind, "path": path}))
            return subprocess.CompletedProcess(command, 0, "\n".join(records) + "\n", "")

        result = oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2, fake_run)

        self.assertEqual(result, (5, 2))
        self.assertEqual(len(calls), 3)
        self.assertEqual(self.counts.read_text(encoding="utf-8"), "scala3\t2\t1\ncats\t3\t1\n")

    def test_unicode_line_separators_inside_json_do_not_split_records(self):
        def unicode_run(command, **kwargs):
            entries = pathlib.Path(command[2]).read_text(encoding="utf-8").splitlines()
            records = [
                json.dumps(
                    {"kind": "PackageDef", "text": "before\u2028middle\u2029after"},
                    ensure_ascii=False,
                )
                for _ in entries
            ]
            return subprocess.CompletedProcess(command, 0, "\n".join(records) + "\n", "")

        self.assertEqual(
            oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2, unicode_run),
            (5, 0),
        )

    def test_counts_every_oracle_failure_in_each_batch(self):
        def failing_run(command, **kwargs):
            entries = pathlib.Path(command[2]).read_text(encoding="utf-8").splitlines()
            records = [json.dumps({"kind": "OracleFailure"}) for _ in entries]
            return subprocess.CompletedProcess(command, 0, "\n".join(records) + "\n", "")

        self.assertEqual(
            oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2, failing_run),
            (5, 5),
        )
        self.assertEqual(self.counts.read_text(encoding="utf-8"), "scala3\t2\t2\ncats\t3\t3\n")

    def test_rejects_truncated_batch_without_replacing_source_counts(self):
        original_counts = self.counts.read_text(encoding="utf-8")

        def truncated_run(command, **kwargs):
            return subprocess.CompletedProcess(command, 0, '{"kind":"Ident"}\n', "")

        with self.assertRaisesRegex(ValueError, "returned 1 records for 2 inputs"):
            oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2, truncated_run)
        self.assertEqual(self.counts.read_text(encoding="utf-8"), original_counts)

    def test_rejects_invalid_json(self):
        def invalid_run(command, **kwargs):
            entries = pathlib.Path(command[2]).read_text(encoding="utf-8").splitlines()
            return subprocess.CompletedProcess(command, 0, "not json\n" * len(entries), "")

        with self.assertRaisesRegex(ValueError, "invalid Scala oracle JSON"):
            oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2, invalid_run)

    def test_rejects_nonzero_oracle_exit(self):
        def failed_run(command, **kwargs):
            return subprocess.CompletedProcess(command, 1, "", "sbt failed")

        with self.assertRaisesRegex(RuntimeError, "sbt failed"):
            oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2, failed_run)

    def test_rejects_manifest_source_set_count_mismatch(self):
        self.counts.write_text("scala3\t1\ncats\t3\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "manifest/source-set count mismatch"):
            oracle_batch.run_batches(self.manifest, self.counts, "oracle", 2)


if __name__ == "__main__":
    unittest.main()
