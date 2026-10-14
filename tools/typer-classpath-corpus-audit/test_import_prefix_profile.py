import tempfile
import unittest
import zipfile
from pathlib import Path

from import_prefix_profile import (
    OBSERVATION_HEADER,
    classify_target,
    logical_resources,
    package_resource_evidence,
    render_profile,
)


class ImportPrefixProfileTest(unittest.TestCase):
    def make_archive(self, root: Path, name: str, entries: dict[str, bytes]) -> Path:
        path = root / name
        with zipfile.ZipFile(path, "w") as archive:
            for entry, contents in entries.items():
                archive.writestr(entry, contents)
        return path

    def test_direct_only_package_resource(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.make_archive(
                Path(temporary), "direct.jar", {"a/b/Direct.class": b"class"}
            )
            resources, versions, directories = logical_resources(archive, 21)
            result = package_resource_evidence(
                ["a", "b"], [("JAR", resources, versions, directories)]
            )
        self.assertEqual(result["direct"], [("JAR", "a/b/Direct.class", 0)])
        self.assertEqual(result["descendants"], [])

    def test_descendant_only_intermediate_two_levels_and_multiple_roots(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.make_archive(root, "first.jar", {"a/b/c/deep/Leaf.class": b"class"})
            second = self.make_archive(root, "second.jar", {"a/b/d/Other.tasty": b"tasty"})
            first_resources, first_versions, first_directories = logical_resources(first, 21)
            second_resources, second_versions, second_directories = logical_resources(second, 21)
            evidence = package_resource_evidence(
                ["a", "b"],
                [
                    ("JAR:first", first_resources, first_versions, first_directories),
                    ("JAR:second", second_resources, second_versions, second_directories),
                ],
            )
        self.assertEqual(evidence["direct"], [])
        self.assertEqual(
            {row[0] for row in evidence["descendants"]}, {"JAR:first", "JAR:second"}
        )
        self.assertEqual({row[2] for row in evidence["descendants"]}, {1, 2})

    def test_multi_release_jar_uses_pinned_effective_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.make_archive(
                Path(temporary),
                "mr.jar",
                {
                    "META-INF/MANIFEST.MF": b"Manifest-Version: 1.0\nMulti-Release: true\n\n",
                    "a/b/Leaf.class": b"base",
                    "META-INF/versions/17/a/b/Leaf.class": b"17",
                    "META-INF/versions/22/a/b/Leaf.class": b"22",
                },
            )
            resources, versions, _ = logical_resources(archive, 21)
        self.assertEqual(resources, {"a/b/Leaf.class"})
        self.assertEqual(versions["a/b/Leaf.class"], 17)

    def test_directory_entry_without_class_or_tasty_is_not_a_package_resource(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.make_archive(Path(temporary), "directory.jar", {"a/b/": b""})
            resources, versions, directories = logical_resources(archive, 21)
            evidence = package_resource_evidence(
                ["a", "b"], [("JAR", resources, versions, directories)]
            )
        self.assertEqual(evidence["direct"], [])
        self.assertEqual(evidence["descendants"], [])
        self.assertTrue(evidence["directory_only"])

    def test_source_owned_package_is_classified_separately(self):
        row = self.observation(
            target_key="a::<first-segment-unresolved>",
            qualifier_segments="a.b.C",
            source_package_registry_lookup="found:Package",
        )
        result = classify_target([row], [])
        self.assertIn("source_package_scope_resolver_registry_gap", result)

    def test_object_or_class_qualifier_is_not_misclassified_as_package(self):
        row = self.observation(
            target_key="a::<first-segment-unresolved>",
            source_lookup="found:Class@a.Owner",
        )
        result = classify_target([row], [])
        self.assertIn("qualifier_is_non_package_symbol", result)

    def test_missing_path_and_unknown_evidence_remain_explicit(self):
        missing = self.observation(target_key="absent::<first-segment-unresolved>")
        unknown = self.observation(
            target_key="unknown::<first-segment-unresolved>",
            source_lookup="unavailable",
            source_package_registry_lookup="unavailable",
            context_package="<unavailable>",
        )
        self.assertIn("absent_from_configured_classpath", classify_target([missing], []))
        self.assertIn("unknown_or_ambiguous_source_lookup", classify_target([unknown], []))

    def test_root_qualified_source_candidate_is_preserved(self):
        row = self.observation(
            target_key="a::<first-segment-unresolved>",
            qualifier_segments="_root_.a.b.C",
        )
        result = classify_target([row], [])
        self.assertIn("root_qualified", result)
        self.assertNotIn("source_relative_candidate", result)

    def test_normalized_profile_is_independent_of_input_order(self):
        first = self.observation(
            source_file="b.scala", target_key="b::<first-segment-unresolved>"
        )
        second = self.observation(
            source_file="a.scala", target_key="a::<first-segment-unresolved>"
        )
        forward = render_profile([first, second], [])
        reverse = render_profile([second, first], [])
        self.assertEqual(forward, reverse)

    @staticmethod
    def observation(**changes):
        row = dict.fromkeys(OBSERVATION_HEADER.split("\t"), "-")
        row.update(
            {
                "source_file": "a.scala",
                "method": "a.scala#tree=1",
                "blocker_origin": "inherited:a.scala#tree=2",
                "qualifier_source": "a.b.C",
                "qualifier_segments": "a.b.C",
                "context_owner": "a",
                "context_package": "a",
                "namespace": "Term",
                "source_lookup": "missing",
                "source_package_registry_lookup": "missing",
                "resolved_prefix": "-",
                "first_failing_segment": "a",
                "request_kind": "resolve_package",
                "resolver_outcome": "Ok(None)::package unresolved::first segment/package unresolved",
                "target_key": "a::<first-segment-unresolved>",
            }
        )
        row.update(changes)
        return row


if __name__ == "__main__":
    unittest.main()
