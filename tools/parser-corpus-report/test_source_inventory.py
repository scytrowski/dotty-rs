import pathlib
import subprocess
import tempfile
import unittest
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from source_inventory import (
    kyo_scala3_production_roots,
    shapeless3_compile_roots,
    tracked_scala_sources,
)


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

    def test_kyo_plugin_exclusions_work_when_checkout_path_is_a_symlink(self):
        production = self.repository / "kyo-core" / "shared" / "src" / "main" / "scala"
        scala_2_plugin = self.repository / "kyo-test" / "sbt" / "src" / "main" / "scala"
        production.mkdir(parents=True)
        scala_2_plugin.mkdir(parents=True)
        checkout_link = self.repository / "checkout-link"
        checkout_link.symlink_to(".", target_is_directory=True)

        roots = kyo_scala3_production_roots(
            checkout_link, [production.resolve(), scala_2_plugin.resolve()]
        )

        self.assertEqual(roots, [production.resolve()])

    def test_shapeless_compile_roots_include_test_support_main_sources(self):
        production = self.repository / "modules/deriving/src/main/scala"
        test_support = self.repository / "modules/test/src/main/scala"
        test_fixture = self.repository / "modules/test/src/test/scala"
        production.mkdir(parents=True)
        test_support.mkdir(parents=True)
        test_fixture.mkdir(parents=True)

        roots = shapeless3_compile_roots(self.repository, [production])

        self.assertEqual(roots, [production.resolve(), test_support.resolve()])


if __name__ == "__main__":
    unittest.main()
