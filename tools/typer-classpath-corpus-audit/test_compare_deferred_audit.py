import unittest

from compare_deferred_audit import cohort_records, gate, matching_methods, requests, scalar


def report(depth=3, fanout=8, name='B'):
    return f'''dependency_load_profile:
  p95_dependency_depth={depth}
  max_distinct_fully_completed_classes_per_request={fanout}
resolver_581_comparison:
  member_requests_by_enclosing_method:
    f.scala#tree=1::p.f: requests=1, distinct_request_outcomes=1
      prefix=p::namespace=type::name={name}::space=Prefix::outcome=external class success=1
'''


class DeferredAuditComparisonTests(unittest.TestCase):
    def test_gate_compares_full_completions_and_retains_frozen_keys(self):
        self.assertEqual(gate(report(), report()), (3, 8))
        with self.assertRaisesRegex(ValueError, 'lost'):
            gate(report(), report(name='C'))
        for text in (report(depth=71), report(fanout=525)):
            with self.assertRaisesRegex(ValueError, 'reduction'):
                gate(report(), text)

    def test_boundary_reduction_is_accepted(self):
        self.assertEqual(gate(report(), report(depth=70, fanout=524)), (70, 524))

    def test_missing_or_duplicate_metrics_are_rejected(self):
        for text in ('', report() + 'dependency_load_profile:\n  p95_dependency_depth=3\n'):
            with self.assertRaisesRegex(ValueError, 'missing or duplicate'):
                scalar(text, 'dependency_load_profile', 'p95_dependency_depth')

    def test_requests_keep_method_identity_and_count(self):
        rows = requests(report())
        self.assertEqual(sum(rows['f.scala#tree=1::p.f'].values()), 1)
        with self.assertRaisesRegex(ValueError, 'no method'):
            requests(report().replace('    f.scala#tree=1::p.f: requests=1, distinct_request_outcomes=1\n', ''))

    def test_nested_rows_use_recorded_enclosing_attempt(self):
        method = 'p/f.scala#tree=1::p.f'
        self.assertEqual(matching_methods('method=p/f.scala#tree=2:f', [method],
                                         {'p/f.scala#tree=2': ('p/f.scala#tree=1', '')}), [method])
        self.assertEqual(matching_methods('method=p/f.scala:4:f', [method], {},
                                         {'p/f.scala:4:f': ['p/f.scala#tree=1']}), [method])

    def test_duplicate_frozen_rows_are_rejected(self):
        row = '  f.scala#tree=1 first_blocker=ImportQualifierNotFound\n'
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            cohort_records('singleton_projection_baseline:\n' + row * 2)


if __name__ == '__main__':
    unittest.main()
