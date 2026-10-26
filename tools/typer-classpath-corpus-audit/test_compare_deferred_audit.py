import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from compare_deferred_audit import (
    cohort_records,
    gate,
    main,
    matching_methods,
    requests,
    scalar,
    sprint_start_gate_baseline,
)
from test_fanout_policy import pinned_report_profiles


def report(depth=3, fanout=8, name='B', high_profiles=None):
    high_profiles = high_profiles or []
    return f'''resolver_metrics:
  external_class_symbol_successes=358
audit_v1_comparison:
  local_methods=3297
dependency_load_profile:
  p95_dependency_depth={depth}
  max_distinct_fully_completed_classes_per_request={fanout}
  completed_fanout_over_16_requests={len(high_profiles)}
  high_completed_fanout_profiles:
{''.join('    high_fanout_profile=' + line + chr(10) for line in high_profiles)}
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
        with self.assertRaisesRegex(ValueError, 'depth gate'):
            gate(report(), report(depth=7))
        with self.assertRaisesRegex(ValueError, 'profile evidence is missing'):
            gate(report(), report(fanout=17))

    def test_gate_accepts_only_oracle_verified_high_fanout_exceptions(self):
        self.assertEqual(
            gate(report(fanout=16), report(fanout=29, high_profiles=pinned_report_profiles())),
            (3, 29),
        )
        with self.assertRaisesRegex(ValueError, 'depth gate'):
            gate(
                report(fanout=16),
                report(depth=7, fanout=29, high_profiles=pinned_report_profiles()),
            )

    def test_cli_reports_raw_and_proven_fanout_separately(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            baseline_path = Path(temp_dir) / 'baseline.md'
            current_path = Path(temp_dir) / 'current.md'
            baseline_path.write_text(report(fanout=16), encoding='utf-8')
            current_path.write_text(
                report(fanout=29, high_profiles=pinned_report_profiles()), encoding='utf-8')
            output = io.StringIO()
            with patch('sys.argv', [
                'compare_deferred_audit.py', str(current_path), '--baseline', str(baseline_path)
            ]):
                with redirect_stdout(output):
                    self.assertEqual(main(), 0)
            rendered = output.getvalue()
            self.assertIn('raw maximum completed fanout 29', rendered)
            self.assertIn('raw completed=29; proven hierarchy=29; unexpected=0', rendered)
            self.assertIn('max depth=7; distinct identities=55', rendered)

    def test_success_counts_and_denominator_cannot_regress_silently(self):
        with self.assertRaisesRegex(ValueError, 'count regressed'):
            gate(report(), report().replace('successes=358', 'successes=357'))
        with self.assertRaisesRegex(ValueError, 'denominator'):
            gate(report(), report().replace('local_methods=3297', 'local_methods=3296'))

    def test_deferred_loading_hard_caps_are_inclusive(self):
        self.assertEqual(gate(report(), report(depth=6, fanout=16)), (6, 16))

    def test_missing_or_duplicate_metrics_are_rejected(self):
        for text in ('', report() + 'dependency_load_profile:\n  p95_dependency_depth=3\n'):
            with self.assertRaisesRegex(ValueError, 'missing or duplicate'):
                scalar(text, 'dependency_load_profile', 'p95_dependency_depth')

    def test_cli_writes_cohort_sidecar_when_regression_gate_fails(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            baseline_path = Path(temp_dir) / 'baseline.md'
            current_path = Path(temp_dir) / 'current.md'
            rows_path = Path(temp_dir) / 'cohorts.tsv'
            baseline_path.write_text(report(), encoding='utf-8')
            current_path.write_text(report(name='C'), encoding='utf-8')
            argv = ['compare_deferred_audit.py', str(current_path),
                    '--baseline', str(baseline_path), '--rows', str(rows_path)]
            with patch('sys.argv', argv):
                self.assertEqual(main(), 1)
            self.assertTrue(rows_path.is_file())
            self.assertIn('cohort\tfrozen_row', rows_path.read_text(encoding='utf-8'))

    def test_sprint_start_gate_baseline_reads_targets_and_success_count(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            baseline_path = Path(temp_dir) / 'baseline.tsv'
            baseline_path.write_text(
                'kind\tkey\tvalue\n'
                'metric\texternal_class_symbol_successes\t12\n'
                'successful_target_key\tprefix=scala\t1\n',
                encoding='utf-8',
            )
            self.assertEqual(
                sprint_start_gate_baseline(baseline_path),
                ({'prefix=scala'}, 12),
            )

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
