import unittest

from check_list_fanout_profile import EXPECTED, validate_rows


def profile_fixture():
    roots = []
    edges = []
    classes = []
    hierarchy = [("hierarchy", f"owner/H{i}", f"parent/P{i}", "1") for i in range(81)]
    member = [
        ("member_type_identity", f"owner/M{i}", f"member/T{i}", str(4 if i < 102 else 3))
        for i in range(111)
    ]
    for run in range(1, 4):
        for method in EXPECTED:
            roots.append({
                "run": str(run), "method": method,
                "request": "namespace=type::name=List::space=Prefix::List",
                "root": "scala/collection/immutable/List", "depth": "7",
                "distinct_classes": "63", "full_completions": "29",
                "shells_created": "30", "shells_reused": "154",
                "member_type_references": "300", "hierarchy_loads": "81",
                "completed_cache_hits": "53",
            })
            for reason, owner, dependency, requests in hierarchy + member:
                edges.append({
                    "run": str(run), "method": method,
                    "root": "scala/collection/immutable/List",
                    "reason": reason, "owner": owner, "dependency": dependency,
                    "requests": requests,
                })
            for index in range(63):
                classes.append({
                    "run": str(run), "method": method,
                    "root": "scala/collection/immutable/List", "class": f"class/C{index}",
                    "identity_shell_seen": str(int(index < 33)),
                    "fully_completed": str(int(index < 29)),
                })
    return roots, edges, classes


class ListFanoutProfileTests(unittest.TestCase):
    def test_checks_exact_roots_and_identical_dependency_graphs(self):
        validate_rows(*profile_fixture())

    def test_rejects_an_over_limit_completed_fanout(self):
        roots, edges, classes = profile_fixture()
        roots[0]["full_completions"] = "28"
        with self.assertRaisesRegex(ValueError, "unexpected fanout metrics"):
            validate_rows(roots, edges, classes)

    def test_rejects_a_changed_root_edge_graph(self):
        roots, edges, classes = profile_fixture()
        edges[0]["dependency"] = "wrong/Parent"
        with self.assertRaisesRegex(ValueError, "dependency graph differs"):
            validate_rows(roots, edges, classes)


if __name__ == "__main__":
    unittest.main()
