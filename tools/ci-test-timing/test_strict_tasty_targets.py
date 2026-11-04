import json
import re
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
        (self.root / "tests" / "future_required_consumer.rs").touch()
        workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
        workflow.parent.mkdir(parents=True)
        self.base_workflow = """STRICT_REQUIRED_TARGETS: >-
 first_target second_target
STRICT_SUPPORT_TARGETS: support_target
STRICT_REQUIRED_ENVS: DOTTY_REQUIRE_FIRST
- name: Run manifest-driven strict Scala and TASTy shard
  id: strict_tests
  env:
    DOTTY_REQUIRE_FIRST: '1'
  run: |
    test_args=()
    test_args+=(--test "$target")
    cargo test "${test_args[@]}"
- name: Run manifest-driven strict Scala and TASTy shard
  id: strict_tests
  env:
    DOTTY_REQUIRE_FIRST: '1'
  run: |
    test_args=()
    test_args+=(--test "$target")
    cargo test "${test_args[@]}"
"""
        workflow.write_text(self.base_workflow, encoding="utf-8")

    def tearDown(self):
        self.temporary_directory.cleanup()

    def fixture(self, name):
        return json.loads((FIXTURES / name).read_text(encoding="utf-8"))

    def valid_manifest(self):
        manifest = self.fixture("strict-targets-duplicate.json")
        manifest["sharding"] = {
            "shard_count": 2,
            "shards": [
                {"id": "shard-1", "estimated_test_seconds": 1.0, "targets": ["first_target"]},
                {"id": "shard-2", "estimated_test_seconds": 2.0, "targets": ["second_target"]},
            ],
        }
        return manifest

    def append_run_step(self, command):
        workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
        text = workflow.read_text(encoding="utf-8")
        workflow.write_text(
            text + "\n- name: Additional required consumer invocation\n  run: |\n" +
            "\n".join("    " + line for line in command.splitlines()) + "\n",
            encoding="utf-8",
        )

    def validate_real_manifest_with_run_step(self, command, block=False):
        repository = Path(__file__).parents[2]
        manifest = json.loads((repository / "tools/ci/strict_tasty_targets.json").read_text(encoding="utf-8"))
        workflow_text = (repository / manifest["source"]["workflow"]).read_text(encoding="utf-8")
        steps_match = re.search(r"(?m)^(\s*)steps:\s*$", workflow_text)
        self.assertIsNotNone(steps_match)
        step_indent = len(steps_match.group(1)) + 2
        indent = " " * step_indent
        if block:
            step = f"{indent}- run: |\n{indent}  {command}\n"
        else:
            step = f"{indent}- run: {command}\n"
        insertion = steps_match.end() + 1
        workflow_text = workflow_text[:insertion] + step + workflow_text[insertion:]

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow_path = root / manifest["source"]["workflow"]
            workflow_path.parent.mkdir(parents=True)
            workflow_path.write_text(workflow_text, encoding="utf-8")
            (root / "tests").mkdir()
            for target in manifest["targets"]:
                (root / "tests" / f"{target['name']}.rs").touch()
            validate_manifest(manifest, root)

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
        with self.assertRaisesRegex(ManifestError, "not in workflow inventory: replacement_target"):
            validate_manifest(self.fixture("strict-targets-replaced.json"), self.root)

    def test_rejects_removal_when_count_and_shards_are_adjusted(self):
        with self.assertRaisesRegex(ManifestError, "workflow declares 2 strict targets; manifest expects 1"):
            validate_manifest(self.fixture("strict-targets-removed.json"), self.root)

    def test_rejects_duplicate_workflow_inventory(self):
        workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
        workflow.write_text("strict_targets=(\n first_target\n first_target\n second_target\n)\n", encoding="utf-8")
        with self.assertRaisesRegex(ManifestError, "workflow is missing independent STRICT_REQUIRED_TARGETS declaration"):
            validate_manifest(self.fixture("strict-targets-duplicate.json"), self.root)

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

    def test_rejects_workflow_inventory_drift_from_independent_baseline(self):
        workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
        text = workflow.read_text(encoding="utf-8").replace("first_target second_target", "first_target")
        workflow.write_text(text, encoding="utf-8")
        with self.assertRaisesRegex(ManifestError, "workflow declares 1 strict targets"):
            validate_manifest(self.fixture("strict-targets-missing.json"), self.root)

    def test_manifest_remains_valid_after_workflow_array_is_removed(self):
        workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
        workflow.write_text("name: sharded workflow\n", encoding="utf-8")
        manifest = self.fixture("strict-targets-duplicate.json")
        manifest["sharding"] = {
            "shard_count": 2,
            "shards": [
                {"id": "shard-1", "estimated_test_seconds": 1.0, "targets": ["first_target"]},
                {"id": "shard-2", "estimated_test_seconds": 2.0, "targets": ["second_target"]},
            ],
        }
        with self.assertRaisesRegex(ManifestError, "missing independent STRICT_REQUIRED_TARGETS"):
            validate_manifest(manifest, self.root)

    def test_rejects_new_required_consumer_invocation_absent_from_manifest(self):
        forms = (
            'cargo test --test future_required_consumer',
            'cargo test --test "future_required_consumer"',
            "cargo test --test 'future_required_consumer'",
            'cargo test --test=future_required_consumer',
            'cargo test --test \\\n              future_required_consumer',
        )
        for command in forms:
            with self.subTest(command=command):
                workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
                workflow.write_text(self.base_workflow, encoding="utf-8")
                validate_manifest(self.valid_manifest(), self.root)
                self.append_run_step(command)
                with self.assertRaisesRegex(ManifestError, "future_required_consumer"):
                    validate_manifest(self.valid_manifest(), self.root)

    def test_rejects_duplicate_strict_target_invocations_in_shell_argument_forms(self):
        forms = (
            'cargo test --test first_target',
            'cargo test --test "first_target"',
            "cargo test --test 'first_target'",
            'cargo test --test=first_target',
            'cargo test --test \\\n              first_target',
        )
        for command in forms:
            with self.subTest(command=command):
                workflow = self.root / ".github" / "workflows" / "jvm-tasty-factory-scala-consumer.yml"
                workflow.write_text(self.base_workflow, encoding="utf-8")
                self.append_run_step(command)
                with self.assertRaisesRegex(ManifestError, "strict targets invoked outside"):
                    validate_manifest(self.valid_manifest(), self.root)

    def test_rejects_dynamic_required_test_target_invocation(self):
        self.append_run_step('cargo test --test "$FUTURE_REQUIRED_TARGET"')
        with self.assertRaisesRegex(ManifestError, "unsupported dynamic Cargo --test target"):
            validate_manifest(self.valid_manifest(), self.root)

    def test_real_inventory_rejects_unassigned_targets_in_sequence_run_steps(self):
        repository = Path(__file__).parents[2]
        manifest = json.loads((repository / "tools/ci/strict_tasty_targets.json").read_text(encoding="utf-8"))
        commands = (
            "cargo +1.98.1 test --test future_required_consumer --locked",
            "cargo +1.98.1 test --test future_required_consumer --locked",
        )
        for block, command in zip((False, True), commands):
            with self.subTest(block=block):
                with self.assertRaisesRegex(ManifestError, "future_required_consumer"):
                    self.validate_real_manifest_with_run_step(command, block=block)

    def test_real_inventory_rejects_duplicate_strict_targets_in_sequence_run_steps(self):
        repository = Path(__file__).parents[2]
        manifest = json.loads((repository / "tools/ci/strict_tasty_targets.json").read_text(encoding="utf-8"))
        target = manifest["targets"][0]["name"]
        command = f"cargo +1.98.1 test --test {target} --locked"
        for block in (False, True):
            with self.subTest(block=block):
                with self.assertRaisesRegex(ManifestError, "strict targets invoked outside"):
                    self.validate_real_manifest_with_run_step(command, block=block)

    def test_real_inventory_rejects_dynamic_cargo_subcommand_construction(self):
        commands = (
            "CARGO_COMMAND=test; cargo \"$CARGO_COMMAND\" --test future_required_consumer",
            'cargo "$CARGO_COMMAND" --test future_required_consumer',
            'cargo t"${CARGO_COMMAND_SUFFIX:-est}" --test future_required_consumer',
        )
        for command in commands:
            with self.subTest(command=command):
                with self.assertRaisesRegex(ManifestError, "unsupported dynamic"):
                    self.validate_real_manifest_with_run_step(command, block=True)

    def test_real_inventory_scans_test_invocations_after_cargo_global_options(self):
        repository = Path(__file__).parents[2]
        manifest = json.loads((repository / "tools/ci/strict_tasty_targets.json").read_text(encoding="utf-8"))
        target = manifest["targets"][0]["name"]
        commands = (
            "cargo +1.98.1 --color always test --test future_required_consumer --locked",
            f"cargo +1.98.1 --color always test --test {target} --locked",
        )
        expected_errors = ("future_required_consumer", "strict targets invoked outside")
        for block in (False, True):
            for command, expected_error in zip(commands, expected_errors):
                with self.subTest(block=block, command=command):
                    with self.assertRaisesRegex(ManifestError, expected_error):
                        self.validate_real_manifest_with_run_step(command, block=block)

    def test_real_inventory_rejects_unsupported_cargo_global_options(self):
        command = "cargo --unreviewed-global-option test --test future_required_consumer"
        with self.assertRaisesRegex(ManifestError, "unsupported Cargo global option"):
            self.validate_real_manifest_with_run_step(command, block=False)

    def test_shard_selection_returns_manifest_order(self):
        manifest = self.fixture("strict-targets-duplicate.json")
        manifest["sharding"] = {
            "shard_count": 2,
            "shards": [
                {"id": "shard-1", "estimated_test_seconds": 1.0, "targets": ["second_target"]},
                {"id": "shard-2", "estimated_test_seconds": 2.0, "targets": ["first_target"]},
            ],
        }
        from validate_strict_tasty_targets import shard_targets

        self.assertEqual(shard_targets(manifest, "shard-1"), ["second_target"])
        self.assertEqual(shard_targets(manifest, "shard-2"), ["first_target"])

    def test_shard_selection_rejects_invalid_id(self):
        from validate_strict_tasty_targets import shard_targets

        with self.assertRaisesRegex(ManifestError, "unknown shard 'shard-3'"):
            shard_targets(self.fixture("strict-targets-duplicate.json"), "shard-3")

    def test_workflow_cli_rejects_unconfigured_third_shard(self):
        manifest = self.fixture("strict-targets-duplicate.json")
        manifest["sharding"] = {
            "shard_count": 3,
            "shards": [
                {"id": "shard-1", "estimated_test_seconds": 1.0, "targets": ["first_target"]},
                {"id": "shard-2", "estimated_test_seconds": 2.0, "targets": ["second_target"]},
                {"id": "shard-3", "estimated_test_seconds": 0.0, "targets": []},
            ],
        }
        path = self.root / "three-shards.json"
        path.write_text(json.dumps(manifest), encoding="utf-8")
        validator = Path(__file__).parents[1] / "ci" / "validate_strict_tasty_targets.py"
        result = subprocess.run(
            [sys.executable, str(validator), "--manifest", str(path), "--root", str(self.root), "--shard", "shard-1"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly the ordered shard ids", result.stderr)

    def test_rejects_unknown_shard_target(self):
        manifest = self.fixture("strict-targets-duplicate.json")
        manifest["sharding"] = {
            "shard_count": 1,
            "shards": [{"id": "shard-1", "estimated_test_seconds": 3.0, "targets": ["first_target", "rogue_target"]}],
        }
        with self.assertRaisesRegex(ManifestError, "unknown assigned targets: rogue_target"):
            validate_manifest(manifest, self.root)


if __name__ == "__main__":
    unittest.main()
