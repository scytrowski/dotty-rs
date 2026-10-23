import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class AuditRunDirectoryTests(unittest.TestCase):
    def test_recorded_run_directory_drives_workflow_timing_verification(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        tools = repo / "tools/typer-classpath-corpus-audit"
        directory_tool = tools / "audit_run_directory.py"
        verify_tool = tools / "verify_focused_gate.py"
        workflow = (repo / ".github/workflows/typer-focused-audit.yml").read_text(encoding="utf-8")
        runner = (tools / "run").read_text(encoding="utf-8")

        self.assertIn("audit_run_directory.py\" \\\n        record", runner)
        self.assertIn("AUDIT_RUN_DIR=", workflow)
        self.assertIn("--timing-log \"$AUDIT_RUN_DIR/first.log\"", workflow)
        self.assertIn("--timing-rss \"$AUDIT_RUN_DIR/first.rss\"", workflow)
        self.assertIn("--timing-log \"$AUDIT_RUN_DIR/second.log\"", workflow)
        self.assertIn("--timing-rss \"$AUDIT_RUN_DIR/second.rss\"", workflow)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "intermediates"
            run_dir = root / "run.test1234"
            run_dir.mkdir(parents=True)
            subprocess.run(
                [sys.executable, str(directory_tool), "record", str(root), str(run_dir)],
                check=True,
            )
            resolved = subprocess.run(
                [sys.executable, str(directory_tool), "resolve", str(root)],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            self.assertEqual(Path(resolved), run_dir.resolve())

            timing = (
                "FOCUSED_TIMING files=3 wall_ms=17 classpath_setup_ms=2 "
                "file_discovery_ms=1 source_read_ms=2 source_processing_and_merge_ms=12\n"
            )
            for sample in ("first", "second"):
                (run_dir / f"{sample}.log").write_text(timing, encoding="utf-8")
                (run_dir / f"{sample}.rss").write_text("MAX_RSS_KB=4096\n", encoding="utf-8")

            report = Path(temporary) / "focused-report.md"
            baseline = Path(temporary) / "baseline.txt"
            timings = Path(temporary) / "focused-timings.tsv"
            selected = "files=3\nmethods_attempted=31\nmethods_typed=0\n"
            report.write_text(
                "## Selected audit output\n\n```text\n" + selected + "```\n\n",
                encoding="utf-8",
            )
            baseline.write_text(selected, encoding="utf-8")
            command = [
                sys.executable,
                str(verify_tool),
                "--report",
                str(report),
                "--baseline",
                str(baseline),
                "--timing-output",
                str(timings),
            ]
            for sample in ("first", "second"):
                command.extend(("--timing-log", str(Path(resolved) / f"{sample}.log")))
                command.extend(("--timing-rss", str(Path(resolved) / f"{sample}.rss")))
            subprocess.run(command, check=True, capture_output=True, text=True)
            rows = timings.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(rows), 3)
            self.assertIn("\t4096", rows[1])
            self.assertIn("\t4096", rows[2])


if __name__ == "__main__":
    unittest.main()
