"""Focused tests for the #1336 read-only classpath evidence collector."""

import importlib.util
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("unresolved_import_roots.py")
spec = importlib.util.spec_from_file_location("unresolved_import_roots", SCRIPT)
assert spec and spec.loader
audit = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = audit
spec.loader.exec_module(audit)


def tasty(major: int, minor: int, experimental: int) -> bytes:
    # TASTy base-128 integers use the high bit to mark the final group.
    return bytes((0x5C, 0xA1, 0xAB, 0x1F, major | 0x80, minor | 0x80, experimental | 0x80))


class UnresolvedImportRootTests(unittest.TestCase):
    def test_parses_pinned_28_8_and_28_9_headers(self):
        self.assertEqual(audit.tasty_header(tasty(28, 8, 0)), "28.8.0")
        self.assertEqual(audit.tasty_header(tasty(28, 9, 0)), "28.9.0")
        self.assertEqual(audit.tasty_header(b"bad!"), "invalid-magic")

    def test_reports_exact_resources_and_keeps_absence_distinct(self):
        with tempfile.TemporaryDirectory() as temp:
            jar = Path(temp) / "compiler.jar"
            with zipfile.ZipFile(jar, "w") as archive:
                archive.writestr("dotty/tools/dotc/core/Types.tasty", tasty(28, 9, 0))
                archive.writestr("dotty/tools/dotc/core/Types$.class", b"class-bytes")
            row = {
                "target_key": "Types::<first-segment-unresolved>",
                "source_file": "compiler/src/example.scala",
                "method": "example",
                "line": "1",
                "blocker_origin": "inherited:compiler/src/example.scala#tree=1",
                "import_tree": "compiler/src/example.scala#tree=1",
                "context_owner": "example.Owner",
                "context_package": "dotty.tools.dotc.core",
                "first_failing_segment": "Types",
                "request_kind": "resolve_package",
                "source_package_registry_lookup": "relative=missing@dotty.tools.dotc.core.Types;enclosing=none;root=missing",
            }
            rows, summary, _ = audit.scan([jar], [row])
            self.assertIn("dotty/tools/dotc/core/Types.tasty", rows)
            self.assertIn("28.9.0", rows)
            self.assertIn("candidate_resource_present_identity_requires_semantic_oracle", rows)
            self.assertIn("TASTy 28.9.0", summary)

            missing = dict(
                row,
                target_key="Absent::<first-segment-unresolved>",
                first_failing_segment="Absent",
                source_package_registry_lookup="relative=missing@dotty.tools.dotc.core.Absent;enclosing=none;root=missing",
            )
            rows, _, _ = audit.scan([jar], [missing])
            self.assertIn("no_candidate_resource_in_supplied_classpath", rows)


if __name__ == "__main__":
    unittest.main()
