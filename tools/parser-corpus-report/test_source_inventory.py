import pathlib
import subprocess
import tempfile
import unittest
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from source_inventory import tracked_scala_sources


class TrackedScalaSourcesTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.repository = pathlib.Path(self.temp_dir.name)
        subprocess.run(["git", "init", "-q", str(self.repository)], check=True)
        subprocess.run(
            ["git", "-C", str(self.repository), "config", "user.email", "test@example.com"],
            check=True,
        )
        subprocess.run(
            ["git", "-C", str(self.repository), "config", "user.name", "Corpus Test"],
            check=True,
        )
        self.source_root = self.repository / "src" / "main" / "scala"
        self.source_root.mkdir(parents=True)
        (self.repository / ".gitignore").write_text(
            "src/main/scala/Extra.scala\n", encoding="utf-8"
        )
        self.source = self.source_root / "Example.scala"
        self.source.write_text("object Example\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(self.repository), "add", "."], check=True)
        subprocess.run(
            ["git", "-C", str(self.repository), "commit", "-qm", "pin source"],
            check=True,
        )

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_rejects_modified_tracked_source_at_same_head(self):
        head = subprocess.check_output(
            ["git", "-C", str(self.repository), "rev-parse", "HEAD"], text=True
        ).strip()
        self.source.write_text("object Changed\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "differ from pinned HEAD"):
            tracked_scala_sources(self.repository, [self.source_root])
        self.assertEqual(
            subprocess.check_output(
                ["git", "-C", str(self.repository), "rev-parse", "HEAD"], text=True
            ).strip(),
            head,
        )

    def test_rejects_additional_scala_source_at_same_head(self):
        head = subprocess.check_output(
            ["git", "-C", str(self.repository), "rev-parse", "HEAD"], text=True
        ).strip()
        extra = self.source_root / "Extra.scala"
        extra.write_text("object Extra\n", encoding="utf-8")
        self.assertEqual(
            subprocess.run(
                ["git", "-C", str(self.repository), "check-ignore", "-q", str(extra)],
                check=False,
            ).returncode,
            0,
        )
        with self.assertRaisesRegex(ValueError, "untracked Scala source"):
            tracked_scala_sources(self.repository, [self.source_root])
        self.assertEqual(
            subprocess.check_output(
                ["git", "-C", str(self.repository), "rev-parse", "HEAD"], text=True
            ).strip(),
            head,
        )

    def test_returns_sources_tracked_by_pinned_head(self):
        self.assertEqual(
            tracked_scala_sources(self.repository, [self.source_root]), [self.source.resolve()]
        )


if __name__ == "__main__":
    unittest.main()
