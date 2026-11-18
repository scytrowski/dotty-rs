"""Run with the same pinned toolchain environment as tools/tasty-pickler-oracle."""

from pathlib import Path
import runpy
import subprocess
import shutil
import os
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]


def main():
    toolchain = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))["Toolchain"]()
    if len(sys.argv) == 2:
        verify_rust_library(toolchain, Path(sys.argv[1]).resolve())
        return
    with tempfile.TemporaryDirectory() as output:
        toolchain.compiler(["-sourceroot", ".", "-d", output, "Evidence.scala"], cwd=HERE)
        for fixture in ("Box.tasty", "TC.tasty"):
            generated = Path(output) / "evidence" / fixture
            if generated.read_bytes() != (HERE / "binary/evidence" / fixture).read_bytes():
                raise SystemExit(f"{fixture} differs from the pinned Scala compiler output")
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


def verify_rust_library(toolchain, candidate):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        library = root / "library"
        library.mkdir()
        toolchain.compiler(["-from-tasty", "-d", library, candidate])
        shutil.copyfile(candidate, library / "example/Evidence.tasty")
        client = root / "Client.scala"
        client.write_text(
            "object Client { def main(args: Array[String]): Unit = { "
            "given Int = 41; val lib = new example.Evidence; "
            "assert(lib.read == 41); assert(lib.old(using 7) == 7) } }"
        )
        classpath = os.pathsep.join((str(library), toolchain.classpath))
        toolchain.compiler(["-d", root, client], classpath=classpath)
        subprocess.run([toolchain.java, "-Xverify:all", "-cp",
                        os.pathsep.join((str(root), classpath)), "Client"], check=True)


if __name__ == "__main__":
    main()
