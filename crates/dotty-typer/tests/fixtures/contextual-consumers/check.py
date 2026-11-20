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
            (["ExtensionUsingOverloads.scala"], 0,
             "this.ops[Int](tc).render(x)(3)(using c)", "this.ops[Int](tc).render(x)(3)(using d)"),
            (["ExtensionUsingOverloads.scala", "ExtensionUsingOverloadsRejected.scala"], 1,
             "def probeOne", "def probeBoth", "Ambiguous overload.", "both match arguments",
             "def probeWrong", "this.ops[Int](tc).render()", "3 errors found"),
            (["OverloadedExtensions.scala"], 0,
             "this.ops[Int](tc).render(x)(3)(fmt)", "this.ops[Int](tc).render(x)(true)",
             "extnext.Convertible.view.apply(x).render(3L)"),
            (["OverloadedExtensions.scala", "OverloadedExtensionsRejected.scala"], 1,
             "[E172]", "No given instance of type extnext.Format was found for parameter fmt",
             "[E008]", "value render is not a member of extnext.Box[Int]",
             "this.ops[Int](tc).render()", "2 errors found"),
            (["ConditionalExtensions.scala"], 0,
             "this.ops[Int](tc).render(x)(3)", "conditionalext.Parcel.fallback.render(x)(3)",
             "conditionalext.Converted.view.apply(x).render(3)",
             "this.formatted[Int](tc).render(x)(3)(fmt)",
             "this.formatted[Int](tc).render(x)(3)(using chosen)"),
            (["ConditionalExtensions.scala", "ConditionalExtensionsRejected.scala"], 1,
             "value render is not a member of conditionalext.Box[Int]",
             "this.ops[Int](/* missing */summon[conditionalext.TC[Int]]).render()",
             "both parameter second and parameter first match type conditionalext.TC[Int]",
             "this.formatted[Int](/* missing */summon[conditionalext.TC[Int]]).render()",
             "No given instance of type conditionalext.Format was found for parameter fmt of method render in trait FormattedOps",
             "both parameter second and parameter first match type conditionalext.Format of parameter fmt of method render in trait FormattedOps",
             "5 errors found"),
            (["TrailingImplicitConversions.scala"], 0,
             "this.wrap[Int](x)(tc).show", "this.accept(this.wrap[Int](x)(tc))",
             "trailingviews.Parcel.fallback(x).show", "this.accept(trailingviews.Parcel.fallback(x))"),
            (["TrailingImplicitConversions.scala", "TrailingImplicitRejected.scala"], 1,
             "value show is not a member of trailingviews.Box[Int]",
             "Found: (x : trailingviews.Box[Int])", "Required: trailingviews.Wrapped[Int]",
             "Required: ?{ show: ? }", "both parameter second and parameter first match type trailingviews.TC[Int]",
             "4 errors found"),
            (["ConditionalMemberConversion.scala"], 0,
             "this.wrap[Int](tc).apply(x).show", "this.modern[Int](tc).apply(x).show",
             "this.inheritedAlternative(x).show",
             "def withoutEvidence(x: conditionalviews.Alternate[Int]): Int = conditionalviews.Alternate.fallback.apply(x).show",
             "second: conditionalviews.TC[Int]): Int = conditionalviews.Alternate.fallback.apply(x).show"),
            (["ConditionalMemberConversion.scala", "ConditionalMemberRejected.scala"], 1,
             "value show is not a member of conditionalviews.Box[Int]",
             "/* missing */summon[conditionalviews.TC[Int]]",
             "both parameter second and parameter first match type conditionalviews.TC[Int]",
             "2 errors found"),
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
             "genericconversions.Parcel.fromParcel[Int](x)", "genericconversions.Custom.implicitConversions.apply(x)",
             "this.make[Int, Boolean](x)", "this.special(x)", "this.pair[Int, Boolean](x)"),
            (["GenericLegacyConversions.scala", "GenericLegacyRejected.scala"], 1,
             "def outsideBound(x: Box[Int]): Wrapped[Int] = x",
             "def wrongResult(x: Box[Int]): Wrapped[Boolean] = x",
             "def wrongCompanionResult(x: Parcel[Int]): Wrapped[Boolean] = x",
             "Required: genericconversions.Wrapped[Boolean]",
             "both method first in class Ambiguous and method second in class Ambiguous",
             "4 errors found"),
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
