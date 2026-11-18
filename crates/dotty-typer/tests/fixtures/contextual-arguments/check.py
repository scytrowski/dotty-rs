"""Run with the same pinned toolchain environment as tools/tasty-pickler-oracle."""

from pathlib import Path
import runpy
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]


def main():
    toolchain = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))["Toolchain"]()
    with tempfile.TemporaryDirectory() as output:
        for source, code, required in (
            ("Calls.scala", 0, (
                "def forward(using n: Int): Int = this.need(n)",
                "def explicit(using n: Int): Int = this.need(using n)",
                "this.f(1)(instance)", "this.legacy(9)", "this.need(using 9)",
                "def read: Base = this.need(this.broad)",
                "Implicit parameters should be provided with a `using` clause",
            )),
            ("Invalid.scala", 1, (
                "[E050]", "[E172]", "does not take more parameters",
                "No given instance of type Int", "Ambiguous given instances",
            )),
        ):
            result = subprocess.run(
                [toolchain.java, "-cp", toolchain.classpath, "dotty.tools.dotc.Main",
                 "-classpath", toolchain.classpath, "-color:never", "-Vprint:typer",
                 "-d", output, HERE / source],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False,
            )
            if result.returncode != code or any(item not in result.stdout for item in required):
                raise SystemExit(f"{source} disagrees with Scala 3.9 controls\n{result.stdout}")


if __name__ == "__main__":
    main()
