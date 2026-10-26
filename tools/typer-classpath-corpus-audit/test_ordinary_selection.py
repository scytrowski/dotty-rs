import tempfile
import unittest
from pathlib import Path

import ordinary_selection


class OrdinarySelectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "scala"
        source = self.root / "library" / "src" / "scala"
        source.mkdir(parents=True)
        (source / "A.scala").write_text("object A", encoding="utf-8")
        (self.root / "compiler" / "src").mkdir(parents=True)
        self.manifest = Path(self.temporary.name) / "selection.txt"

    def tearDown(self):
        self.temporary.cleanup()

    def load(self, content):
        self.manifest.write_text(content, encoding="utf-8")
        return ordinary_selection.load_selection(self.manifest, self.root)

    def test_returns_sorted_exact_tree_identities(self):
        self.assertEqual(
            self.load("library/src/scala/A.scala#tree=12\nlibrary/src/scala/A.scala#tree=3\n"),
            ["library/src/scala/A.scala#tree=12", "library/src/scala/A.scala#tree=3"],
        )

    def test_reads_identity_column_from_cohort_tsv(self):
        self.assertEqual(
            self.load(
                "identity\tstatus\n"
                "library/src/scala/A.scala#tree=3\tblocked\n"
                "library/src/scala/A.scala#tree=12\ttyped\n"
            ),
            ["library/src/scala/A.scala#tree=12", "library/src/scala/A.scala#tree=3"],
        )

    def test_rejects_malformed_and_duplicate_identities(self):
        for value in (
            "library/src/scala/A.scala",
            "library/src/scala/A.scala#tree=-1",
            "library/src/scala/A.scala#tree=01",
            "library/src/scala/A.scala#tree=3\nlibrary/src/scala/A.scala#tree=3",
        ):
            with self.subTest(value=value), self.assertRaises(ordinary_selection.SelectionError):
                self.load(value)

    def test_rejects_missing_and_outside_sources(self):
        for value in (
            "library/src/scala/Missing.scala#tree=1",
            "../outside.scala#tree=1",
            "/tmp/A.scala#tree=1",
        ):
            with self.subTest(value=value), self.assertRaises(ordinary_selection.SelectionError):
                self.load(value)

    def test_rejects_empty_selection(self):
        with self.assertRaises(ordinary_selection.SelectionError):
            self.load("# no selections\n")


if __name__ == "__main__":
    unittest.main()
