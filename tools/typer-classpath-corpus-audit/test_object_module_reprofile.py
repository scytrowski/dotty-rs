import unittest

from object_module_reprofile import (
    BASELINE_BLOCKER,
    classify,
    format_summary,
    frozen_rows,
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
        + "  request_key\tsource_file\tenclosing_attempt\tresolution\tobject_origin\tobject_identity\tmodule_class_identity\n"
        + package_rows
        + "\nmodule_operation_observations:\n"
        + "  observations=0\n"
        + "  operation\tsource_file\tenclosing_attempt\tresult\tresult_state\tmodule_state\ttransaction_state\terror_category\tobject_origin\tobject_identity\tmodule_class_identity\n"
        + operation_rows
        + "\nmodule_member_observations:\n"
        + "  observations=0\n"
        + "  source_file\tenclosing_attempt\tmodule_class_identity\trequest_key\tresult\tresult_symbol\tmodule_state_before\tmodule_state_after\ttransaction_state\n"
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
        self.assertEqual(
            classify(
                ["resolved"],
                ["completed_during_member_lookup|Complete(ClassInfo)|observed"],
                ["success|observed"],
                BASELINE_BLOCKER,
            ),
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
        self.assertEqual(
            classify(["resolved"], ["completed|Complete(ClassInfo)|rolled_back"], [], BASELINE_BLOCKER),
            "module_completion_rolled_back",
        )

    def test_ok_none_and_resolver_error_are_distinct(self):
        self.assertEqual(
            classify(["unresolved"], [], [], BASELINE_BLOCKER),
            "module_lookup_returned_none_origin_unknown",
        )
        self.assertEqual(
            classify(["unresolved"], [], [], BASELINE_BLOCKER, "source_local"),
            "source_object_missing_module_class_mapping",
        )
        self.assertEqual(
            classify(["unresolved"], [], [], BASELINE_BLOCKER, "classpath_tasty"),
            "classpath_object_module_lookup_returned_none",
        )
        self.assertEqual(
            classify(["error"], [], [], BASELINE_BLOCKER),
            "module_lookup_error",
        )

    def test_module_stage_evidence_survives_a_moved_first_blocker(self):
        self.assertEqual(
            classify(["resolved"], [], [], "ObjectTermReferenceDeferred"),
            "module_shell_not_completed",
        )
        self.assertEqual(
            classify(
                ["resolved"],
                ["completed|Complete(ClassInfo)"],
                ["failure|observed"],
                "ObjectTermReferenceDeferred",
            ),
            "module_member_lookup_failed",
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

    def test_frozen_baseline_retains_all_rows_and_exact_request_targets(self):
        entries = []
        for index in range(576):
            source = f"compiler/group{index % 77}/File{index % 77}.scala"
            tree = index + 1
            attempt_tree = tree if index % 2 == 0 else tree - 1
            entries.append(
                f"method={source}#tree={tree}::site={source}:1:m{index}"
                f"::attempt={source}#tree={attempt_tree}"
                f"::blocker={BASELINE_BLOCKER}::typed=false"
            )
        package = (
            "prefix=scala.io::namespace=term::name=Source::space=Prefix\t"
            "compiler/group0/File0.scala\tcompiler/group0/File0.scala#tree=1::m0\t"
            "validated_module_identity\tclasspath_tasty\tscala.io.Source\tscala.io.Source$"
        )
        rows = frozen_rows(report(entries, packages=[package]))
        self.assertEqual(len(rows), 576)
        first = next(row for row in rows if row["method_tree"] == "1")
        self.assertEqual(first["object_origin"], "classpath_tasty")
        self.assertEqual(first["resolution_requests"], "prefix=scala.io::namespace=term::name=Source::space=Prefix")
        self.assertEqual(first["request_observations"], "1")
        self.assertEqual(first["distinct_resolution_targets"], "1")
        self.assertIn("inherited", {row["attribution"] for row in rows})
        self.assertIn("direct", {row["attribution"] for row in rows})

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
            "baseline_blocker", "current_blocker", "baseline_object_origin",
            "baseline_object_identities", "baseline_module_class_identities",
            "baseline_resolution_requests", "baseline_request_observations",
            "baseline_distinct_resolution_targets", "current_object_origin",
            "current_object_identities", "current_module_class_identities",
            "current_resolution_requests", "current_request_observations",
            "current_distinct_resolution_targets", "module_lookup", "completion",
            "member_lookup", "member_request_keys", "module_member_attempts",
            "distinct_module_member_requests", "classification", "fully_typed",
        )}
        self.assertEqual(render([row]), render([row]))
        self.assertEqual(format_summary([row]), format_summary([row]))

    def test_source_and_classpath_origins_are_observable_not_guessed(self):
        from object_module_reprofile import identity_origin

        self.assertEqual(
            identity_origin(
                [{"object_origin": "source_local"}],
                [{"object_origin": "classpath_tasty"}],
            ),
            "classpath_tasty,source_local",
        )
        self.assertEqual(identity_origin([], []), "unknown")


if __name__ == "__main__":
    unittest.main()
