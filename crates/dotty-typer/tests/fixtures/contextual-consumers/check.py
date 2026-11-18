"""Check the synthesized contextual applications with the pinned Scala 3.9 typer."""

from pathlib import Path
import runpy
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]


def main():
    toolchain = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))["Toolchain"]()
    with tempfile.TemporaryDirectory() as out:
        for sources, status, expected in [
            (["Conversions.scala"], 0, "this.accept(inner.apply(x))"),
            (["Conversions.scala", "ConversionsAmbiguous.scala"], 1, "implicit conversions cannot be applied because they are ambiguous"),
            (["Extensions.scala"], 0, "inner.render(box)(3)(fmt)"),
            (["Extensions.scala", "ExtensionsAmbiguous.scala"], 1, "implicit extension methods cannot be applied because they are ambiguous"),
            (["Extractors.scala"], 0, "case contextualextractors.Allowed()(inner, mode)"),
            (["Extractors.scala", "ExtractorsAmbiguous.scala"], 1, "Ambiguous given instances"),
        ]:
            result = subprocess.run(
                [toolchain.java, "-cp", toolchain.classpath, "dotty.tools.dotc.Main",
                 "-classpath", toolchain.classpath, "-color:never", "-Vprint:typer",
                 "-d", out, *sources],
                cwd=HERE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            )
            if result.returncode != status or expected not in " ".join(result.stdout.split()):
                raise SystemExit(f"{sources} disagrees with pinned Scala 3.9\n{result.stdout}")


if __name__ == "__main__":
    main()
