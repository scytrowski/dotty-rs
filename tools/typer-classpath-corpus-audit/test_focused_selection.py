import tempfile
import unittest
from pathlib import Path

from focused_selection import SelectionError, load_manifest, normalize_manifest_path


class FocusedSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name)
        self.library = self.base / "library" / "src"
        self.compiler = self.base / "compiler" / "src"
        self.library.mkdir(parents=True)
        self.compiler.mkdir(parents=True)
        (self.library / "A.scala").write_text("object A\n", encoding="utf-8")
        (self.compiler / "B.scala").write_text("object B\n", encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def manifest(self, content: str) -> Path:
        path = self.base / "selection.txt"
        path.write_text(content, encoding="utf-8")
        return path

    def roots(self):
        return self.base

    def test_comments_blanks_and_duplicates_produce_sorted_unique_selection(self):
        manifest = self.manifest("# smoke cohort\n\ncompiler/src/B.scala\nlibrary/src/A.scala\ncompiler/src/B.scala\n")
        self.assertEqual(
            load_manifest(manifest, self.roots()),
            ["compiler/src/B.scala", "library/src/A.scala"],
        )

    def test_rejects_non_normalized_or_unsafe_paths(self):
        for value in ("/tmp/A.scala", "../A.scala", "src/../A.scala", "src//A.scala", "src\\A.scala"):
            with self.subTest(value=value), self.assertRaises(SelectionError):
                normalize_manifest_path(value, line_number=1)

    def test_rejects_missing_or_empty_selection(self):
        with self.assertRaisesRegex(SelectionError, "missing or outside"):
            load_manifest(self.manifest("src/Missing.scala\n"), self.roots())
        with self.assertRaisesRegex(SelectionError, "selected no source files"):
            load_manifest(self.manifest("# only a comment\n\n"), self.roots())

    def test_rejects_source_file_outside_corpus(self):
        (self.base / "docs").mkdir()
        (self.base / "docs" / "A.scala").write_text("object A\n", encoding="utf-8")
        with self.assertRaisesRegex(SelectionError, "escapes the corpus"):
            load_manifest(self.manifest("docs/A.scala\n"), self.roots())

    def test_rejects_symlink_escaping_corpus(self):
        outside = self.base / "outside.scala"
        outside.write_text("object Outside\n", encoding="utf-8")
        (self.library / "Escape.scala").symlink_to(outside)
        with self.assertRaisesRegex(SelectionError, "escapes the corpus"):
            load_manifest(self.manifest("library/src/Escape.scala\n"), self.roots())


if __name__ == "__main__":
    unittest.main()
