"""Environment-independent harness controls, also run by the Rust test target."""

import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import run as oracle


class HarnessTests(unittest.TestCase):
    def test_checked_in_source_and_tasty_hashes_match_provenance(self):
        oracle.verify_provenance()

    def test_provenance_detects_changed_fixture_without_regeneration(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "tasty").mkdir()
            (root / "provenance.json").write_text(json.dumps({
                "sources_sha256": {"Absent.scala": "bad"}, "tasty_sha256": {}}))
            with mock.patch.object(oracle, "FIXTURES", root):
                with self.assertRaisesRegex(RuntimeError, "SHA-256 provenance"):
                    oracle.verify_provenance()

    def test_explicit_oracle_requires_configuration(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "SCALA39_ROOT is required"):
                oracle.Toolchain()

    def test_incompatible_checkout_fails(self):
        with mock.patch.dict(os.environ, {k: "/unused" for k in
                                          ("SCALA39_ROOT", "JAVA_HOME", "COURSIER_MAVEN_ROOT")}, clear=True):
            with mock.patch.object(oracle, "execute", return_value="wrong-revision"):
                with self.assertRaisesRegex(RuntimeError, "expected " + oracle.REVISION):
                    oracle.Toolchain()

    def test_missing_java_executable_is_a_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            env = {k: temporary for k in ("SCALA39_ROOT", "JAVA_HOME", "COURSIER_MAVEN_ROOT")}
            with mock.patch.dict(os.environ, env, clear=True):
                with mock.patch.object(oracle, "execute", return_value=oracle.REVISION):
                    with self.assertRaisesRegex(RuntimeError, "missing JDK Java executable"):
                        oracle.Toolchain()

    def fake_cache(self, root):
        (root / "bin").mkdir()
        for executable in ("java", "javac", "javap"):
            (root / "bin" / executable).touch()
        (root / "release").write_text('JAVA_VERSION="21"\nIMPLEMENTOR="test"\n'
                                      'IMPLEMENTOR_VERSION="test"\nOS_ARCH="test"\n')
        for group, artifact, version in oracle.ARTIFACTS:
            path = root / group / artifact / version / f"{artifact}-{version}.jar"
            path.parent.mkdir(parents=True)
            path.touch()

    def test_runtime_tools_and_jdk_21_are_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fake_cache(root)
            env = {k: temporary for k in ("SCALA39_ROOT", "JAVA_HOME", "COURSIER_MAVEN_ROOT")}
            with mock.patch.dict(os.environ, env, clear=True):
                with mock.patch.object(oracle, "execute", return_value=oracle.REVISION):
                    (root / "bin/javac").unlink()
                    with self.assertRaisesRegex(RuntimeError, "missing JDK javac executable"):
                        oracle.Toolchain()
                    (root / "bin/javac").touch()
                    (root / "release").write_text('JAVA_VERSION="17.0.12"\nIMPLEMENTOR="test"\n'
                                                  'IMPLEMENTOR_VERSION="test"\nOS_ARCH="test"\n')
                    with mock.patch.object(oracle, "execute", side_effect=[oracle.REVISION,
                                                                           "Scala compiler version 3.9.0 -- test"]):
                        with self.assertRaisesRegex(RuntimeError, "JDK 21 or newer is required"):
                            oracle.Toolchain()

    def test_wrong_compiler_version_and_unpinned_artifacts_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fake_cache(root)
            version_fixtures = root / "version-fixtures"
            version_fixtures.mkdir()
            jars = {f"{artifact}-{version}.jar": oracle.digest(
                root / group / artifact / version / f"{artifact}-{version}.jar")
                for group, artifact, version in oracle.ARTIFACTS}
            (version_fixtures / "provenance.json").write_text(json.dumps({
                "compiler_artifacts_sha256": jars}))
            env = {k: temporary for k in ("SCALA39_ROOT", "JAVA_HOME", "COURSIER_MAVEN_ROOT")}
            for version, error in [("3.8.4", "expected Scala 3.9.0"),
                                   ("3.9.0", "artifacts differ")]:
                with self.subTest(version=version):
                    with mock.patch.dict(os.environ, env, clear=True):
                        with mock.patch.object(oracle, "execute", side_effect=[
                                oracle.REVISION, f"Scala compiler version {version} -- test"]) as execute:
                            with mock.patch.object(oracle, "FIXTURES",
                                                   version_fixtures if version == "3.8.4" else oracle.FIXTURES):
                                with self.assertRaisesRegex(RuntimeError, error):
                                    oracle.Toolchain()
                            if version == "3.9.0":
                                self.assertEqual(execute.call_count, 1)  # git only, no unverified JVM invocation

    def test_nonzero_process_exit_is_observable(self):
        failed = subprocess.CompletedProcess(["compiler"], 17, "specific reader failure")
        with mock.patch.object(oracle.subprocess, "run", return_value=failed):
            with self.assertRaisesRegex(oracle.CommandFailure, "specific reader failure"):
                oracle.execute(["compiler"])

    def test_timeout_is_a_harness_failure_not_a_reader_negative_control(self):
        with mock.patch.object(oracle.subprocess, "run", side_effect=
                               subprocess.TimeoutExpired(["compiler"], 180)):
            with self.assertRaisesRegex(RuntimeError, "timed out"):
                oracle.expect_failure(lambda: oracle.execute(["compiler"]), "invalid candidate")

    def test_negative_controls_cannot_mask_missing_tools(self):
        with self.assertRaisesRegex(RuntimeError, "configuration unavailable"):
            oracle.expect_failure(lambda: (_ for _ in ()).throw(
                RuntimeError("configuration unavailable")), "bad candidate")
        with self.assertRaisesRegex(RuntimeError, "unexpectedly succeeded"):
            oracle.expect_failure(lambda: None, "bad candidate")

    def test_candidate_wire_validation_precedes_compiler_invocation(self):
        with mock.patch.object(oracle, "inspector", return_value=Path("inspector")):
            with mock.patch.object(oracle, "snapshot", side_effect=RuntimeError("invalid wire")):
                with mock.patch.object(oracle, "Toolchain") as toolchain:
                    with mock.patch("sys.argv", ["run.py", "verify", "bad.tasty"]):
                        with self.assertRaisesRegex(RuntimeError, "invalid wire"):
                            oracle.main()
                    toolchain.assert_not_called()

    def test_verify_stages_only_tasty_and_requires_new_output_class(self):
        structure = json.loads((oracle.FIXTURES / "snapshots/example/Answer.json").read_text())
        candidate = oracle.FIXTURES / "tasty/example/Answer.tasty"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            toolchain = mock.Mock()

            def compiler(args, **kwargs):
                if "-d" in args:
                    output = args[args.index("-d") + 1] / "example/Answer.class"
                    output.parent.mkdir()
                    output.write_bytes(b"mock class output")
                return "independent tree"

            toolchain.compiler.side_effect = compiler
            result = oracle.verify_candidate(toolchain, Path("unused-inspector"), candidate,
                                              root, structure)
            self.assertTrue(result["fresh_class"])
            self.assertEqual([p.name for p in (root / "candidate").rglob("*") if p.is_file()],
                             ["Answer.tasty"])
            self.assertTrue(toolchain.compiler.call_args_list[0].kwargs["decompile"])
            self.assertIn("-from-tasty", toolchain.compiler.call_args_list[1].args[0])
        with tempfile.TemporaryDirectory() as temporary:
            toolchain = mock.Mock()
            toolchain.compiler.return_value = "tree but no class"
            with self.assertRaisesRegex(RuntimeError, "no fresh class"):
                oracle.verify_candidate(toolchain, Path("unused"), candidate, Path(temporary), structure)

    def test_candidate_path_is_derived_from_package_and_rejects_traversal(self):
        structure = json.loads((oracle.FIXTURES / "snapshots/example/Answer.json").read_text())
        self.assertEqual(oracle.artifact_path(structure), Path("example/Answer.tasty"))
        bad = copy.deepcopy(structure)
        next(n for n in bad["nodes"] if n["tag"] == "TYPEDEF")["name"] = {"utf8": "../Answer"}
        with self.assertRaisesRegex(RuntimeError, "unsafe artifact path"):
            oracle.artifact_path(bad)


if __name__ == "__main__":
    unittest.main()
