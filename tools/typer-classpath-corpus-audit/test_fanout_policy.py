import csv
import unittest
from collections import Counter
from pathlib import Path

from fanout_policy import (
    EDGES_PATH,
    CLASSES_PATH,
    ROOTS_PATH,
    serialize_profile_record,
    verify_high_fanout_profiles,
    verify_hierarchy_graph,
)


def read_tsv(path):
    with path.open(encoding="utf-8", newline="") as stream:
        return list(csv.DictReader(stream, delimiter="\t"))


def pinned_report_profiles():
    roots = read_tsv(ROOTS_PATH)
    edges = read_tsv(EDGES_PATH)
    classes = read_tsv(CLASSES_PATH)
    lines = []
    for root in roots:
        method = root["source_method"]
        method_edges = [row for row in edges if row["source_method"] == method]
        method_classes = [row for row in classes if row["source_method"] == method]
        lines.append(serialize_profile_record(
            {**root, "method": method}, method_edges, method_classes
        ))
    return lines


def hierarchy_evidence(nodes, edges):
    return Counter(("hierarchy", owner, dependency, 1) for owner, dependency in edges)


class FanoutPolicyTests(unittest.TestCase):
    def test_accepts_current_high_fanout_exceptions_with_exact_graphs(self):
        profiles = verify_high_fanout_profiles(pinned_report_profiles(), 5)
        self.assertEqual(len(profiles), 5)
        self.assertEqual(sorted(p.completed for p in profiles), [25, 29, 29, 29, 29])
        self.assertTrue(all(p.completed == p.proven_hierarchy for p in profiles))
        self.assertTrue(all(p.unexpected == 0 for p in profiles))
        self.assertEqual(
            {(p.depth, p.identities) for p in profiles}, {(6, 46), (7, 55)}
        )

    def test_rejects_fake_hierarchy_label_and_wrong_identity_graph(self):
        lines = pinned_report_profiles()
        lines[0] = lines[0].replace("hierarchy:", "member_type_identity:", 1)
        with self.assertRaisesRegex(ValueError, "pinned List oracle"):
            verify_high_fanout_profiles(lines, 5)

    def test_rejects_unknown_high_fanout_request(self):
        lines = pinned_report_profiles()
        lines[0] = lines[0].replace("tree=2721", "tree=999")
        with self.assertRaisesRegex(ValueError, "all pinned requests"):
            verify_high_fanout_profiles(lines, 5)

    def test_accepts_a_wide_oracle_verified_acyclic_hierarchy_once(self):
        nodes = {f"Wide{index}" for index in range(40)}
        edges = {(f"Wide{index}", f"Wide{index + 1}") for index in range(39)}
        evidence = hierarchy_evidence(nodes, edges)
        proven, unexpected = verify_hierarchy_graph(
            root="Wide0",
            classes=nodes,
            completed=nodes,
            observed_edges=evidence,
            trusted_edges=evidence.copy(),
            max_depth=39,
        )
        self.assertEqual((proven, unexpected), (40, 0))

    def test_rejects_profile_edge_that_disagrees_with_trusted_parent_graph(self):
        nodes = {"Root", "ActualParent", "FakeParent"}
        trusted = hierarchy_evidence(nodes, {("Root", "ActualParent")})
        observed = hierarchy_evidence(nodes, {("Root", "FakeParent")})
        with self.assertRaisesRegex(ValueError, "trusted hierarchy/member oracle"):
            verify_hierarchy_graph(
                root="Root", classes=nodes, completed={"Root", "FakeParent"},
                observed_edges=observed, trusted_edges=trusted, max_depth=1,
            )

    def test_rejects_member_identity_only_completion(self):
        edges = Counter({
            ("hierarchy", "Root", "Parent", 1): 1,
            ("member_type_identity", "Root", "Member", 1): 1,
        })
        with self.assertRaisesRegex(ValueError, "unexpected non-hierarchy completions"):
            verify_hierarchy_graph(
                root="Root", classes={"Root", "Parent", "Member"},
                completed={"Root", "Parent", "Member"},
                observed_edges=edges, trusted_edges=edges.copy(), max_depth=1,
            )

    def test_rejects_missing_parent_completion(self):
        edges = hierarchy_evidence({"Root", "Parent"}, {("Root", "Parent")})
        with self.assertRaisesRegex(ValueError, "not fully completed"):
            verify_hierarchy_graph(
                root="Root", classes={"Root", "Parent"}, completed={"Root"},
                observed_edges=edges, trusted_edges=edges.copy(), max_depth=1,
            )

    def test_rejects_hierarchy_cycle(self):
        edges = hierarchy_evidence({"A", "B"}, {("A", "B"), ("B", "A")})
        with self.assertRaisesRegex(ValueError, "inheritance cycle"):
            verify_hierarchy_graph(
                root="A", classes={"A", "B"}, completed={"A", "B"},
                observed_edges=edges, trusted_edges=edges.copy(), max_depth=2,
            )

    def test_shared_ancestor_is_counted_once(self):
        nodes = {"Root", "Left", "Right", "Shared"}
        edges = hierarchy_evidence(nodes, {
            ("Root", "Left"), ("Root", "Right"), ("Left", "Shared"), ("Right", "Shared")
        })
        self.assertEqual(
            verify_hierarchy_graph(
                root="Root", classes=nodes, completed=nodes, observed_edges=edges,
                trusted_edges=edges.copy(), max_depth=2,
            ),
            (4, 0),
        )

    def test_rejects_runaway_profile_breadth_and_depth(self):
        nodes = {f"N{index}" for index in range(257)}
        edges = hierarchy_evidence(nodes, {(f"N{i}", f"N{i + 1}") for i in range(256)})
        with self.assertRaisesRegex(ValueError, "identity budget"):
            verify_hierarchy_graph(
                root="N0", classes=nodes, completed=nodes, observed_edges=edges,
                trusted_edges=edges.copy(), max_depth=256,
            )
        deep_nodes = {f"D{index}" for index in range(258)}
        deep_edges = hierarchy_evidence(
            deep_nodes, {(f"D{i}", f"D{i + 1}") for i in range(257)}
        )
        with self.assertRaisesRegex(ValueError, "hierarchy depth budget"):
            verify_hierarchy_graph(
                root="D0", classes=deep_nodes, completed=deep_nodes,
                observed_edges=deep_edges, trusted_edges=deep_edges.copy(), max_depth=257,
            )
        wide_nodes = {f"W{index}" for index in range(33)}
        wide_edges = hierarchy_evidence(
            wide_nodes,
            {(f"W{left}", f"W{right}") for left in range(33) for right in range(left + 1, 33)},
        )
        with self.assertRaisesRegex(ValueError, "edge budget"):
            verify_hierarchy_graph(
                root="W0", classes=wide_nodes, completed=wide_nodes,
                observed_edges=wide_edges, trusted_edges=wide_edges.copy(), max_depth=32,
            )


if __name__ == "__main__":
    unittest.main()
