import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1] / "ci"))
from validate_strict_tasty_targets import ManifestError, validate_manifest


FIXTURES = Path(__file__).parent / "fixtures"


class StrictTastyTargetManifestTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name)
        (self.root / "tests").mkdir()
        (self.root / "tests" / "first_target.rs").touch()
        (self.root / "tests" / "second_target.rs").touch()
        (self.root / "tests" / "replacement_target.rs").touch()
        workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text(
            "strict_targets=(\n first_target\n second_target\n)\n", encoding="utf-8"
        )

    def tearDown(self):
        self.temporary_directory.cleanup()

    def fixture(self, name):
        return json.loads((FIXTURES / name).read_text(encoding="utf-8"))

    def test_command_rejects_invalid_fixture_manifests(self):
        validator = Path(__file__).parents[1] / "ci" / "validate_strict_tasty_targets.py"
        for fixture in (
            "strict-targets-missing.json",
            "strict-targets-duplicate.json",
            "strict-targets-replaced.json",
            "strict-targets-removed.json",
        ):
            with self.subTest(fixture=fixture):
                result = subprocess.run(
                    [sys.executable, str(validator), "--manifest", str(FIXTURES / fixture), "--root", str(self.root)],
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_rejects_target_missing_from_shard_union(self):
        with self.assertRaisesRegex(ManifestError, "unassigned targets: second_target"):
            validate_manifest(self.fixture("strict-targets-missing.json"), self.root)

    def test_rejects_target_assigned_to_multiple_shards(self):
        with self.assertRaisesRegex(ManifestError, "assigned more than once: first_target"):
            validate_manifest(self.fixture("strict-targets-duplicate.json"), self.root)

    def test_rejects_replacement_in_inventory_and_shards(self):
        with self.assertRaisesRegex(ManifestError, "not in frozen workflow inventory: replacement_target"):
            validate_manifest(self.fixture("strict-targets-replaced.json"), self.root)

    def test_rejects_removal_when_count_and_shards_are_adjusted(self):
        with self.assertRaisesRegex(ManifestError, "workflow inventory has 2 targets; manifest expects 1"):
            validate_manifest(self.fixture("strict-targets-removed.json"), self.root)

    def test_rejects_manifest_repointing_its_inventory_source(self):
        manifest = self.fixture("strict-targets-replaced.json")
        manifest["source"]["workflow"] = "another-workflow.yml"
        with self.assertRaisesRegex(ManifestError, "source.workflow must remain"):
            validate_manifest(manifest, self.root)

    def test_rejects_missing_test_file(self):
        (self.root / "tests" / "second_target.rs").unlink()
        manifest = self.fixture("strict-targets-duplicate.json")
        manifest["sharding"] = {
            "shard_count": 1,
            "shards": [{"id": "shard-1", "estimated_test_seconds": 3.0, "targets": ["first_target", "second_target"]}],
        }
        with self.assertRaisesRegex(ManifestError, "missing tests/<target>.rs files: second_target"):
            validate_manifest(manifest, self.root)


if __name__ == "__main__":
    unittest.main()
