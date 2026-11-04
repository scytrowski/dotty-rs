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

    def tearDown(self):
        self.temporary_directory.cleanup()

    def fixture(self, name):
        return json.loads((FIXTURES / name).read_text(encoding="utf-8"))

    def test_command_rejects_missing_and_duplicate_fixture_manifests(self):
        validator = Path(__file__).parents[1] / "ci" / "validate_strict_tasty_targets.py"
        for fixture in ("strict-targets-missing.json", "strict-targets-duplicate.json"):
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
