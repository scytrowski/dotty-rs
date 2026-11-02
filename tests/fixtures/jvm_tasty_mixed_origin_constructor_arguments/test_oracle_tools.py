#!/usr/bin/env python3
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


HERE = pathlib.Path(__file__).resolve().parent
NORMALIZER = HERE / "normalize-dump.py"
COMPARATOR = HERE / "compare-manifests.py"
SCRATCH = HERE.parents[2] / "target/issue-2011/oracle-tool-tests"


class OracleToolTests(unittest.TestCase):
    def normalize(self, kind: str, path: str, logical: str, text: str) -> str:
        result = subprocess.run(
            [sys.executable, str(NORMALIZER), kind, path, logical],
            input=text,
            text=True,
            capture_output=True,
            check=True,
        )
        return result.stdout

    def test_normalizes_checkout_paths_and_javap_dates_only(self) -> None:
        javap = (
            "Classfile /arbitrary/worktree/out/example/Factory.class\n"
            "  Last modified Jan 4, 2031; size 717 bytes\n"
            "  SHA-256 checksum abc123\n"
            "      13: invokespecial #26 // Method example/Pair.<init>:()V\n"
        )
        normalized = self.normalize(
            "javap",
            "/arbitrary/worktree/out/example/Factory.class",
            "<producer>/example/Factory.class",
            javap,
        )
        self.assertEqual(
            normalized,
            "Classfile <producer>/example/Factory.class\n"
            "  Last modified <normalized>; size 717 bytes\n"
            "  SHA-256 checksum abc123\n"
            "      13: invokespecial #26 // Method example/Pair.<init>:()V\n",
        )

        tasty = "/another/checkout/out/example/Factory.tasty\nTREE 1\n"
        self.assertEqual(
            self.normalize(
                "tasty",
                "/another/checkout/out/example/Factory.tasty",
                "<producer>/example/Factory.tasty",
                tasty,
            ),
            "<producer>/example/Factory.tasty\nTREE 1\n",
        )

    def test_manifest_comparison_ignores_only_informational_jar_hash(self) -> None:
        SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=SCRATCH) as temp:
            first = pathlib.Path(temp) / "first.json"
            second = pathlib.Path(temp) / "second.json"
            baseline = {
                "oracle": {"scalac_jar_sha256_informational": "first", "javap_version": "21"},
                "orders": {"one": {"snapshot_hash": "same"}},
            }
            changed_jar = json.loads(json.dumps(baseline))
            changed_jar["oracle"]["scalac_jar_sha256_informational"] = "different"
            first.write_text(json.dumps(baseline))
            second.write_text(json.dumps(changed_jar))
            subprocess.run([sys.executable, str(COMPARATOR), str(first), str(second)], check=True)

            changed_evidence = json.loads(json.dumps(baseline))
            changed_evidence["orders"]["one"]["snapshot_hash"] = "different"
            second.write_text(json.dumps(changed_evidence))
            rejected = subprocess.run(
                [sys.executable, str(COMPARATOR), str(first), str(second)],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("manifest differs", rejected.stderr)


if __name__ == "__main__":
    unittest.main()
