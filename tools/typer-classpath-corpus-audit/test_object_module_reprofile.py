import unittest

from object_module_reprofile import (
    BASELINE_BLOCKER,
    classify,
    method_attempts,
    preserve_rows,
    render,
    stable_attempt_key,
    target_counts,
)


def report(rows, operations=(), packages=()):
    attempts = "\n".join(f"  {row}" for row in rows)
    operation_rows = "\n".join(f"  {row}" for row in operations)
    package_rows = "\n".join(f"  {row}" for row in packages)
    return (
        "local_method_attempts:\n"
        + attempts
        + "\nmodule_package_term_observations:\n"
        + "  observations=0\n"
        + "  request_key\tsource_file\tenclosing_attempt\tresolution\tobject_identity\tmodule_class_identity\n"
        + package_rows
        + "\nmodule_operation_observations:\n"
        + "  observations=0\n"
        + "  operation\tsource_file\tenclosing_attempt\tresult\tresult_state\terror_category\tobject_identity\tmodule_class_identity\n"
        + operation_rows
        + "\n"
    )


class ObjectModuleReprofileTests(unittest.TestCase):
    def test_stable_attempt_key_discards_only_unstable_detail(self):
        self.assertEqual(
            stable_attempt_key("compiler/Foo.scala#tree=42::method=run"),
            "compiler/Foo.scala#tree=42",
        )
        self.assertEqual(stable_attempt_key("<none>"), "")

    def test_validated_external_object_with_module_class_is_separate_from_member_success(self):
        self.assertEqual(
            classify(["resolved"], [], [], BASELINE_BLOCKER),
            "module_shell_not_completed",
        )
        self.assertEqual(
            classify(["resolved"], ["completed|Complete(ClassInfo)"], [], BASELINE_BLOCKER),
            "module_completed_member_unattempted",
        )
        self.assertEqual(
            classify(["resolved"], ["completed|Complete(ClassInfo)"], ["success"], BASELINE_BLOCKER),
            "module_member_resolved",
        )

    def test_incomplete_shell_and_invalid_completed_class_info_are_distinct(self):
        self.assertEqual(
            classify(["resolved"], ["completed|Deferred"], [], BASELINE_BLOCKER),
            "module_completion_invalid_class_info",
        )
        self.assertEqual(
            classify(["resolved"], ["error|Error"], [], BASELINE_BLOCKER),
            "module_completion_failed",
        )

    def test_ok_none_and_resolver_error_are_distinct(self):
        self.assertEqual(
            classify(["unresolved"], [], [], BASELINE_BLOCKER),
            "object_missing_expected_module_class",
        )
        self.assertEqual(
            classify(["error"], [], [], BASELINE_BLOCKER),
            "module_lookup_error",
        )

    def test_direct_and_inherited_first_blockers_use_stable_tree_indexes(self):
        direct = "method=compiler/Foo.scala#tree=10::site=compiler/Foo.scala:2:run::attempt=compiler/Foo.scala#tree=10::blocker=ObjectModuleClassUnavailable::typed=false"
        inherited = "method=compiler/Foo.scala#tree=11::site=compiler/Foo.scala:3=child::attempt=compiler/Foo.scala#tree=10::blocker=ObjectModuleClassUnavailable::typed=false"
        parsed = method_attempts(report([direct, inherited]))
        self.assertEqual(parsed["compiler/Foo.scala#tree=10"]["attempt"], "compiler/Foo.scala#tree=10")
        self.assertEqual(parsed["compiler/Foo.scala#tree=11"]["attempt"], "compiler/Foo.scala#tree=10")

    def test_moved_or_missing_current_rows_are_preserved(self):
        frozen = []
        current = []
        for index in range(576):
            source = f"compiler/group{index % 77}/File{index % 77}.scala"
            tree = index + 1
            old = (
                f"method={source}#tree={tree}::site={source}:1:m{index}"
                f"::attempt={source}#tree={tree}::blocker={BASELINE_BLOCKER}::typed=false"
            )
            new_blocker = "ImportQualifierNotFound" if index == 0 else BASELINE_BLOCKER
            new = old.replace(f"blocker={BASELINE_BLOCKER}", f"blocker={new_blocker}")
            frozen.append(old)
            current.append(new)
        rows = preserve_rows(report(frozen), report(current[:-1]))
        self.assertEqual(len(rows), 576)
        moved = next(row for row in rows if row["method_tree"] == "1")
        missing = next(row for row in rows if row["method_tree"] == "576")
        self.assertEqual(moved["classification"], "earlier_import_package_blocker")
        self.assertEqual(missing["current_blocker"], "method_not_observed")
        self.assertEqual(missing["fully_typed"], "false")

    def test_repeated_requests_do_not_inflate_distinct_targets(self):
        self.assertEqual(
            target_counts([
                {"request_keys": ["scala/Source", "scala/Source"]},
                {"request_keys": ["scala/Source", "scala/Predef"]},
            ]),
            (4, 2),
        )

    def test_normalized_output_is_byte_identical(self):
        row = {column: "stable" for column in (
            "baseline_key", "source_file", "method_tree", "enclosing_attempt", "attribution",
            "baseline_blocker", "current_blocker", "object_origin", "object_identities",
            "module_class_identities", "module_lookup", "completion", "member_lookup",
            "classification", "fully_typed",
        )}
        self.assertEqual(render([row]), render([row]))


if __name__ == "__main__":
    unittest.main()
