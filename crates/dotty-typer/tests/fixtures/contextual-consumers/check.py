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
        for sources, status, *expected in [
            (["LegacyMemberConversions.scala"], 0,
             "legacymembers.Box.wrap[Int](x).show", "this.local[Int](x).show",
             "def keep(x: legacymembers.Box[Int]): Int = x.direct", "this.modern.apply(x).show",
             "this.mono(x).show"),
            (["LegacyMemberConversions.scala", "LegacyMemberRejected.scala"], 1,
             "value hidden is not a member of legacymembers.Box[Int]",
             "value absent is not a member of legacymembers.Box[Int]",
             "both method first in class Ambiguous and method second in class Ambiguous provide an extension method `show`",
             "3 errors found"),
            (["MemberConversionScope.scala"], 0, "memberscope.From.render.apply(x).show"),
            (["MemberConversionScope.scala"], 0, "chosen.apply(x).show"),
            (["MemberConversions.scala"], 0, "Int = x.show"),
            (["MemberConversions.scala"], 0, "Int = via.apply(x).show"),
            (["MemberConversions.scala", "MemberConversionMissing.scala"], 1,
             "value show is not a member of memberconversions.From"),
            (["MemberConversions.scala", "MemberConversionAmbiguous.scala"], 1,
             "both parameter second and parameter first provide an extension method `show`"),
            (["Conversions.scala"], 0, "this.accept(inner.apply(x))"),
            (["Conversions.scala", "ConversionsAmbiguous.scala"], 1, "implicit conversions cannot be applied because they are ambiguous"),
            (["GenericLegacyConversions.scala"], 0,
             "def result(x: genericconversions.Box[Int]): genericconversions.Wrapped[Int] = this.wrap[Int](x)",
             "this.accept(this.wrap[Int](x))",
             "def inherited(x: genericconversions.Child): genericconversions.Wrapped[Int] = this.wrap[Int](x)",
             "this.make[Int, Boolean](x)", "this.special(x)", "this.pair[Int, Boolean](x)"),
            (["GenericLegacyConversions.scala", "GenericLegacyRejected.scala"], 1,
             "def outsideBound(x: Box[Int]): Wrapped[Int] = x",
             "def wrongResult(x: Box[Int]): Wrapped[Boolean] = x",
             "both method first in class Ambiguous and method second in class Ambiguous",
             "3 errors found"),
            (["LegacyConversions.scala"], 0, "this.old(x)"),
            (["LegacyConversions.scala", "LegacyConversionsAmbiguous.scala"], 1, "implicit conversions cannot be applied because they are ambiguous"),
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
            output = " ".join(result.stdout.split())
            if result.returncode != status or any(needle not in output for needle in expected):
                raise SystemExit(f"{sources} disagrees with pinned Scala 3.9\n{result.stdout}")


if __name__ == "__main__":
    main()
