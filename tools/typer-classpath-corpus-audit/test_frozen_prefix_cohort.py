import unittest

from frozen_prefix_cohort import classify


def report(origin="source", count=2):
    return (
        "    f.scala#tree=1::p.f: requests=2, distinct_request_outcomes=1\n"
        f"      prefix=p.B::namespace=term::name=value::space=Prefix::outcome=class member scope unavailable: incomplete class info={count}\n"
        f"    state=Missing::origin={origin}::identity=p.B=2\n"
    )


class FrozenPrefixTests(unittest.TestCase):
    def test_source_state_is_attributed_without_claiming_classpath_progress(self):
        rows = classify(report())
        self.assertEqual(rows[0][:6], ("f.scala#tree=1::p.f", "p.B", "term", "value", "Prefix", 2))
        self.assertEqual(rows[0][-1], "remains_unresolved_unowned_source_prefix")

    def test_external_origin_requires_owned_session_replay(self):
        self.assertEqual(classify(report("classloader"))[0][-1], "requires_owned_session_replay")

    def test_missing_attribution_and_inconsistent_counts_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "counts"):
            classify(report(count=1))
        with self.assertRaisesRegex(ValueError, "no attributed"):
            classify("")
        with self.assertRaisesRegex(ValueError, "unattributed"):
            classify(report().replace("prefix=p.B", "prefix=p.C"))

    def test_conflicting_origin_for_same_prefix_is_not_guessed(self):
        with self.assertRaisesRegex(ValueError, "ambiguous"):
            classify(report() + "    state=Missing::origin=classloader::identity=p.B=1\n")


if __name__ == "__main__":
    unittest.main()
