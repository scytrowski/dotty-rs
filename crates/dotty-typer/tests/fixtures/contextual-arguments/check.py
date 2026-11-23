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
    if len(sys.argv) in (2, 3):
        client = Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else None
        verify_rust_library(toolchain, Path(sys.argv[1]).resolve(), client)
        return
    with tempfile.TemporaryDirectory() as output:
        for source, package, classes in (
            ("Evidence.scala", "evidence", ("Box.tasty", "TC.tasty")),
            ("Defaults.scala", "defaults", ("Reader.tasty",)),
            ("PairReader.scala", "defaults", ("PairReader.tasty",)),
            ("ClauseReader.scala", "defaults", ("ClauseReader.tasty",)),
            ("Structural.scala", "structural", ("Box.tasty", "TC.tasty", "Builders.tasty", "Builders$boxes.class")),
        ):
            toolchain.compiler(["-sourceroot", ".", "-d", output, source], cwd=HERE)
            for name in classes:
                fixture = Path(package, name)
                if (Path(output) / fixture).read_bytes() != (HERE / "binary" / fixture).read_bytes():
                    raise SystemExit(f"{fixture} differs from the pinned Scala compiler output")
        for source, code, required in (
            ("NestedContextClient.scala", 0, (
                "this.build.apply(using 7).apply(using true)",
                "this.build.apply(number).apply(flag)", "this.build.apply(using 7).apply(flag)",
                "this.build.apply(using 7).apply(contextual$3)",
            )),
            ("NestedContextClientErrors.scala", 1, (
                "[E172]", "this.build.apply(n).apply(/* missing */summon[Boolean])",
                "No given instance of type Boolean was found for parameter of (Boolean) ?=> Int",
                "both parameter right and parameter left match type Boolean of parameter of (Boolean) ?=> Int",
            )),
            ("StructuralClauses.scala", 0, (
                "this.box(structuralclauses.Makers.simple[Int])",
                "this.mark(structuralclauses.Makers.mono(seed))",
            )),
            ("Calls.scala", 0, (
                "def forward(using n: Int): Int = this.need(n)",
                "def explicit(using n: Int): Int = this.need(using n)",
                "this.f(1)(instance)", "this.legacy(9)", "this.need(using 9)",
                "def read: Base = this.need(this.broad)",
                "this.absent(scala.util.NotGiven.value)",
                "def pick(using n: Int): Int = this.one(n)", "this.from(1)(b)",
                "this.read(Result.direct)", "this.provider(n).apply(2)",
                "Implicit parameters should be provided with a `using` clause",
            )),
            ("Structural.scala", 0, (
                "given class boxes[A", "final given def boxes[A",
                "new structural.Builders.boxes[A](using base)()",
                "this.need(structural.Builders.boxes[Int](leaf))",
                "structural.Builders.boxes[Int](using leaf).tag",
            )),
            ("StructuralBound.scala", 0, (
                "private[this] given val base: bound.TC[A]",
                "final given def boxes[A",
                "this.need(bound.Builders.boxes[Int](leaf))",
            )),
            ("Paths.scala", 0, (
                "Client.this.p.read(Client.this.p.member)", "Client.this.q.read(Client.this.q.member)",
                "a.family.read(", "a.family.member)",
            )),
            ("PathErrors.scala", 1, ("[E007]", "Found:    Client.this.q.Item", "Required: Client.this.p.Item")),
            ("Anchors.scala", 0, (
                "this.useAlias(anchorsearch.Target.targetKey)", "this.useToken(anchorsearch.Tokens.Token.tokenKey)",
                "anchorsearch.Tokens.useHere(raw)",
            )),
            ("OpaqueError.scala", 1, ("[E172]", "No given instance of type opaqueerror.Key[opaqueerror.Tokens.Token]")),
            ("Dependent.scala", 0, ("this.read(p)(p.member)", "this.read(using p)(p.member)",
                "this.needs(dependentclauses.Witness.derive(p)(p.member))")),
            ("NestedEvidence.scala", 0, (
                "using contextual$2: Boolean) => this.read(contextual$1, contextual$2)",
                "contextual$4: Int) => this.readOne(contextual$4)",
                "contextual$5: Boolean) => (using contextual$6: Long) => x",
            )),
            ("NestedEvidenceMissing.scala", 1, (
                "[E172]", "No given instance of type Long was found for parameter third of method read",
                "this.read(contextual$3, contextual$4, third)", "1 error found",
            )),
            ("Recursive.scala", 0, (
                "final class $_lazy_implicit_$2() extends Object(), Serializable",
                "recursivesearch.Evidence.loop(this.$_lazy_implicit_$1,",
                "this.$_lazy_implicit_$1)", "$_lazy_implicit_$3.$_lazy_implicit_$1",
            )),
            ("StrictRecursion.scala", 1, ("[E172]", "strictsearch.Evidence.loop(/* missing */summon[strictsearch.Evidence])")),
            ("Defaults.scala", 0, (
                "r.read(n)", "r.read(using r.read$default$1)",
                "r.offset(x)(using r.offset$default$2(x))",
                "this.need(this.box(using this.box$default$1))",
            )),
            ("LegacyDefaults.scala", 0, (
                "def legacy$default$1: Int @uncheckedVariance = 7",
                "def fallback(): Int = this.legacy(using this.legacy$default$1)",
                "def supplied(using chosen: Int): Int = this.legacy(chosen)",
                "def explicit(using a: Int, b: Int): Int = this.legacy(9)",
                "Implicit parameters should be provided with a `using` clause",
            )),
            ("LegacyDefaultAmbiguous.scala", 1, (
                "[E172]", "Ambiguous given instances",
                "both parameter b and parameter a match type Int of parameter n of method legacy",
            )),
            ("ByNameGrowth.scala", 0, (
                "this.ask(this.grow[Int](this.grow[growth.Box[Int]](this.growingLeaf)))",
                "this.shrink[growth.Box[Int]](this.shrink[Int](this.shrinkingLeaf))",
            )),
            ("Inherited.scala", 0, ("this.need(TC.instance[Int])",)),
            ("Invalid.scala", 1, (
                "[E050]", "[E172]", "does not take more parameters",
                "No given instance of type scala.util.NotGiven[Int]",
                "No given instance of type Int", "Ambiguous given instances",
            )),
        ):
            check_typer_output(toolchain, output, source, code, required, toolchain.classpath)

        # Compile clients separately from the libraries providing the default getters.
        client_output = Path(output) / "client"
        client_output.mkdir()
        classpath = os.pathsep.join((output, toolchain.classpath))
        check_contextual_exports(toolchain, output, client_output, classpath)
        check_chained_exports(toolchain, output, client_output, classpath)
        check_inherited_exports(toolchain, output, client_output, classpath)
        check_provider_export_clauses(toolchain, output, client_output, classpath)
        check_within_clause(toolchain, output, client_output, classpath)
        check_named_using(toolchain, output, client_output, classpath)
        check_poly_named_using(toolchain, output, client_output, classpath)
        check_omitted_witnesses(toolchain, output, client_output, classpath)
        check_typer_output(toolchain, client_output, "BinaryDefaultsClient.scala", 0, (
            "val x$1: Int = this.next()",
            "reader.offset(x$1)(using reader.offset$default$2(x$1))",
            "reader.offset(this.next())(chosen)", "reader.offset(7)(using 9)",
            "reader.read(using reader.read$default$1)",
        ), classpath)
        check_typer_output(toolchain, client_output, "BinaryPairClient.scala", 0, (
            "val $1$: defaults.PairReader = this.reader\n"
            "        val x$1: Int = this.left()\n"
            "        val y$1: Int = this.right()",
            "$1$.pick(x$1, y$1)(using $1$.pick$default$3(x$1, y$1))",
        ), classpath)
        check_typer_output(toolchain, client_output, "AppliedPairClient.scala", 0, (
            "this.reader().pick(1, 2)(using 9)",
            "val $1$: defaults.PairReader = this.reader()\n"
            "        val x$1: Int = this.left()\n"
            "        val y$1: Int = this.right()\n"
            "        $1$.pick(x$1, y$1)(using $1$.pick$default$3(x$1, y$1))",
            "this.reader().pick(this.left(), this.right())(n)",
            "this.reader().pick(this.left(), this.right())(using b)",
            "r.choose(seed)(r.choose$default$2(seed))",
            "val $2$: defaults.PairReader = this.reader()\n"
            "        $2$.choose(seed)($2$.choose$default$2(seed))",
            "this.reader().choose(using seed)(7)",
            "val $3$: defaults.PairReader = this.reader()\n"
            "        val x$2: Int = this.left()\n"
            "        val y$2: Int = this.right()\n"
            "        $3$.legacy(x$2, y$2)(using $3$.legacy$default$3(x$2, y$2))",
            "val x$3: Int = this.left()\n"
            "        val y$3: Int = this.right()\n"
            "        r.legacy(x$3, y$3)(using r.legacy$default$3(x$3, y$3))",
            "this.reader().legacy(this.left(), this.right())(n)",
            "r.legacy(this.left(), this.right())(n)",
            "this.reader().legacy(this.left(), this.right())(b)",
            "r.legacy(this.left(), this.right())(using b)",
            "Implicit parameters should be provided with a `using` clause",
        ), classpath)
        ambiguous = "both parameter b and parameter a match type Int of parameter"
        check_typer_output(toolchain, client_output, "AppliedPairAmbiguous.scala", 1, (
            "[E172]", "5 errors found", f"{ambiguous} n of method pick",
            "No given instance of type Int was found for parameter seed of method choose",
            f"{ambiguous} seed of method choose", f"{ambiguous} n of method legacy",
            "this.reader().legacy(1, 2)(\n        /* ambiguous:",
            "r.legacy(1, 2)(\n        /* ambiguous:",
        ), classpath)
        check_typer_output(toolchain, client_output, "ClauseClient.scala", 0, (
            "r.fold(n)(3)(7, r.fold$default$4(n)(3))",
            "r.fold(n)(3)(7, 8)",
            "r.fold(n)(3)(r.fold$default$3(n)(3), r.fold$default$4(n)(3))",
            "val $1$: defaults.ClauseReader = this.reader()\n"
            "        val seed$1: Int = this.seed()\n"
            "        val x$1: Int = this.next()\n"
            "        $1$.fold(using seed$1)(x$1)($1$.fold$default$3(seed$1)(x$1),\n"
            "          $1$.fold$default$4(seed$1)(x$1))",
            "r.finish(n)(3)()(r.finish$default$3(n)(3)())(flag)",
            "val $2$: defaults.ClauseReader = this.reader()\n"
            "        val x$2: Int = this.next()\n"
            "        val y$1: Int = this.next()\n"
            "        $2$.legacy(x$2)(y$1)(using $2$.legacy$default$3(x$2)(y$1))",
            "r.legacy(1)(2)(n)", "r.legacy(1)(2)(using b)",
        ), classpath)
        check_typer_output(toolchain, client_output, "ClauseErrors.scala", 1, (
            "[E172]", "5 errors found",
            "No given instance of type Int was found for parameter seed of method fold",
            f"{ambiguous} seed of method fold",
            "both parameter b and parameter a match type Boolean of parameter flag of method finish",
            f"{ambiguous} n of method legacy",
            "r.finish(n)(3)()(r.finish$default$3(n)(3)())(\n        /* ambiguous:",
            "$1$.finish(n)(x$1)()($1$.finish$default$3(n)(x$1)())\n      }(\n        /* ambiguous:",
        ), classpath)
        check_typer_output(toolchain, client_output, "BinaryDefaultsAmbiguous.scala", 1, (
            "[E172]", "Ambiguous given instances", "of parameter n of method offset",
        ), classpath)


def check_contextual_exports(toolchain, output, client_output, classpath):
    check_typer_output(toolchain, output, "ExportLibrary.scala", 0, (
        "final given def ints: (exportsprint.Instances.ints : exportsprint.TC[Int])",
        "final given def selected:", "final given def flags:",
        "final implicit def old: (exportsprint.Legacy.old : exportsprint.TC[Int])",
        "final given def boxed[A](using e: exportsprint.TC[A]):",
        "exportsprint.TC[exportsprint.Box[A]] = exportsprint.Instances.boxed[A](e)",
    ), toolchain.classpath)
    check_typer_output(toolchain, client_output, "ExportClient.scala", 0, (
        "this.need[Int](exportsprint.Instances.ints)",
        "def named: exportsprint.TC[Int] = exportsprint.Named.ints",
        "def aliasSelection: exportsprint.TC[Int] = exportsprint.Alias.selected",
        "this.need[Int](exportsprint.Alias.selected)",
        "this.need[Boolean](exportsprint.All.flags)",
        "this.need[Int](exportsprint.Only.ints)", "this.need[Int](exportsprint.Old.old)",
        "exportsprint.All.boxed[Int](exportsprint.All.ints)",
    ), classpath)
    check_typer_output(toolchain, client_output, "ExportErrors.scala", 1, (
        "[E172]", "6 errors found",
        "No given instance of type exportsprint.TC[Boolean] was found for parameter e",
        "No given instance of type exportsprint.TC[Int] was found for parameter e",
        "No given instance of type exportsprint.TC[Long] was found for parameter e",
        "given instance another in object Other and given instance ints in object All",
        "exportsprint.Conditional.boxed[Int](/* missing */summon[exportsprint.TC[Int]])",
        "both parameter b and parameter a match type exportsprint.TC[Int]",
    ), classpath)


def check_chained_exports(toolchain, output, client_output, classpath):
    check_typer_output(toolchain, output, "ChainedExports.scala", 0, (
        "def control: chain.TC[Int] = chain.Middle.first",
        "def named: chain.TC[Int] = chain.Renamed.selected",
        "final given def selected:\n      (chain.Middle.first : => (chain.Leaf.ints : chain.TC[Int])) =\n      chain.Middle.first",
        "final implicit def old:\n      (chain.OldMiddle.old : => (chain.Legacy.old : chain.TC[Int])) =\n      chain.OldMiddle.old",
        "chain.Public.want[Int](chain.Public.first)", "this.need[Boolean](chain.Public.flags)",
        "this.need[Int](chain.Public.first)", "this.need[Int](chain.Filtered.first)",
        "this.need[Int](chain.OldPublic.old)", "this.need[Boolean](chain.WithoutFirst.flags)",
        "this.need[chain.Box[Int]](chain.Public.boxed[Int](chain.Public.first))",
        "chain.Middle.boxed[A](e)", "chain.Leaf.boxed[A](e)",
    ), toolchain.classpath)
    check_typer_output(toolchain, client_output, "ChainedErrors.scala", 1, (
        "[E172]", "7 errors found",
        "given instance other in object Rival and given instance first in object Public",
        "import chain.Filtered.given\n        this.ask[Boolean](/* missing */summon[chain.TC[Boolean]])",
        "import chain.PlainStar.given\n        this.ask[Int](/* missing */summon[chain.TC[Int]])",
        "import chain.WithoutFirst.given\n        this.ask[Int](/* missing */summon[chain.TC[Int]])",
        "import chain.Public.given\n        this.ask[Long](/* missing */summon[chain.TC[Long]])",
        "chain.Conditional.boxed[Int](/* missing */summon[chain.TC[Int]])",
        "both parameter right and parameter left match type chain.TC[Int]",
    ), classpath)
    check_typer_output(toolchain, client_output, "ChainedCycle.scala", 1, (
        "[E172]", "1 error found", "No given instance of type cycles.TC[Int] was found for parameter e",
        "this.need[Int](/* missing */summon[cycles.TC[Int]])",
    ), toolchain.classpath)


def check_inherited_exports(toolchain, output, client_output, classpath):
    check_typer_output(toolchain, output, "InheritedExports.scala", 0, (
        "def direct: inheritexport.TC[Int] = inheritexport.Ints.key",
        "this.need[Int](inheritexport.Ints.key)",
        "def named: inheritexport.TC[Int] = inheritexport.Named.selected",
        "final given def selected: (inheritexport.Ints.key : inheritexport.TC[Int])",
        "this.need[Int](inheritexport.Api.key)",
        "this.need[Int](inheritexport.Only.key)",
        "this.need[Boolean](inheritexport.BoolOnly.key)",
        "inheritexport.Api.pair[Int](inheritexport.Api.key)",
        "inheritexport.TC[inheritexport.Pair[Int, B]] =\n      inheritexport.Ints.pair[B](e)",
        "this.need[Int](inheritexport.Second.key)", "this.need[Int](inheritexport.Both.old)",
        "final implicit def old: (inheritexport.Diamond.old : inheritexport.TC[Int])",
        "this.need[Int](inheritexport.High.high)", "this.need[Int](inheritexport.Shared.key)",
        "export inheritexport.Ints.given\n  }\n  class SharedClient",
    ), toolchain.classpath)
    check_typer_output(toolchain, client_output, "InheritedErrors.scala", 1, (
        "[E172]", "5 errors found",
        "given instance other in object Rival and given instance key in object Api",
        "given instance high in object Ranked", "given instance low in object Ranked",
        "import inheritexport.Only.given\n        this.need[Boolean](/* missing */summon[inheritexport.TC[Boolean]])",
        "import inheritexport.Api.given\n        this.need[Long](/* missing */summon[inheritexport.TC[Long]])",
        "import inheritexport.OldApi.*\n        this.need[Int](/* missing */summon[inheritexport.TC[Int]])",
    ), classpath)
    check_typer_output(toolchain, client_output, "InheritedSharedNamed.scala", 1, (
        "1 error found", "no eligible member key at inheritexport.Ints",
        "inheritexport.Ints.key cannot be exported because it is already a member of object SharedNamed",
    ), classpath)


def check_provider_export_clauses(toolchain, output, client_output, classpath):
    check_typer_output(toolchain, output, "ProviderExports.scala", 0, (
        "this.need[multi.Pair[Int, Int]](multi.Records.mono(k))",
        "multi.Records.flat[Int](k, t)", "multi.Records.poly[Int, Boolean](k, t)",
        "multi.Records.curried[Int, Boolean](k)(t)", "multi.Legacy.old[Int, Boolean](k, t)",
        "this.need[multi.Pair[Int, Int]](multi.Mono.mono(k))", "multi.Flat.flat[Int](k, t)",
        "multi.Poly.poly[Int, Boolean](k, t)", "multi.Curried.curried[Int, Boolean](k)(t)",
        "multi.Again.derived[Int, Boolean](k)(t)", "multi.Old.old[Int, Boolean](k, t)",
        "final given def mono(using k: multi.Key[Int]): multi.Pair[Int, Int] =\n      multi.Records.mono(k)",
        "multi.Records.flat[A](k, t)", "multi.Records.poly[A, B](k, t)",
        "multi.Records.curried[A, B](k)(t)", "multi.Curried.curried[A, B](k)(t)",
        "final implicit def old[A, B]", "multi.Legacy.old[A, B](k, t)",
    ), toolchain.classpath)
    required = ["[E172]", "6 errors found"]
    for method in ("multi.Curried.curried", "multi.Records.curried"):
        required.extend((
            f"{method}[Int, Boolean](/* missing */summon[multi.Key[Int]])",
            f"{method}[Int, Boolean](k)(/* missing */summon[multi.Tag[Boolean]])",
        ))
    for method in ("multi.Again.derived", "multi.Records.curried"):
        required.append(
            f"{method}[Int, Boolean](k)(\n"
            "            /* ambiguous: both parameter right and parameter left match type multi.Tag[Boolean] */"
        )
    check_typer_output(toolchain, client_output, "ProviderExportErrors.scala", 1, required, classpath)


def check_within_clause(toolchain, output, client_output, classpath):
    check_typer_output(toolchain, output, "WithinClause.scala", 0, (
        "this.curried(p)(v)", "def explicit(using p: within.Family, v: p.Out): p.Out =\n      this.read(using p, v)",
        "this.read(p, v)", "this.need(within.Makers.product(p, v))",
        "this.need(within.Api.product(p, v))", "this.need(within.Again.derived(p, v))",
        "within.Api.product(using p, v)", "def partial(using p: within.Family, v: p.Out): p.Out = this.read(using p, v)",
        "this.need(within.Legacy.old(p, v))", "this.need(within.Old.old(p, v))",
        "within.Generic.lifted[Int](using p, v, tag)",
        "within.Makers.product(f, v)", "within.Api.product(f, v)", "within.Makers.lifted[A](f, v, tag)",
        "final implicit def old(implicit f: within.Family, implicit v: f.Out)",
    ), toolchain.classpath)
    check_typer_output(toolchain, client_output, "WithinClauseErrors.scala", 1, (
        "[E007]", "Found:    (v : q.Out)", "Required: p.Out", "[E172]", "7 errors found",
        "within.Api.product(/* missing */summon[within.Family], f.Out)",
        "within.Api.product(p, /* missing */summon[p.Out])",
        "within.Makers.product(p, /* missing */summon[p.Out])",
        "within.Again.derived(p,\n            /* ambiguous: both parameter b and parameter a match type p.Out",
        "this.read(using p, /* missing */summon[p.Out])",
        "this.read(using p,\n        /* ambiguous: both parameter b and parameter a match type p.Out",
    ), classpath)


def check_named_using(toolchain, output, client_output, classpath):
    # Tie repeated trees to individual methods so one working call cannot mask another.
    check_typer_output(toolchain, output, "NamedUsing.scala", 0, (
        "this.pick(using i, b, t)", "this.pick(using 7, b, t)",
        "def ordered(i: Int, b: Boolean, t: namedctx.Tag): Int =\n      this.pick(using n = i, flag = b, text = t)",
        "def reverse(i: Int, b: Boolean, t: namedctx.Tag): Int =\n      this.pick(using n = i, flag = b, text = t)",
        "val flag$1: Boolean = this.tickBool()\n        val n$1: Int = this.tickInt()\n"
        "        this.legacy(using n = n$1, flag = flag$1)",
        "this.pick(using i, flag = true, t)", "this.pick(using 7, b, text = t)",
        "def useDefault(): Int =\n      this.fallback(using this.fallback$default$1, flag = true)",
        "def preferGiven(using i: Int): Int =\n      this.fallback(using this.fallback$default$1, flag = true)",
        "[E220]", "def preferGiven(using i: Int): Int = fallback(using flag = true)", "1 warning found",
        "this.fallback(using n = i, flag = true)", "this.read(using f = p, v = w)", "this.read(using p, v = w)",
        "namedctx.Api.selected(using f = p, v = w)", "namedctx.Makers.product(f, v)", "this.legacy(using i, flag = b)",
    ), toolchain.classpath)
    check_typer_output(toolchain, client_output, "NamedUsingErrors.scala", 1, (
        "does not have a parameter bogus", "parameter n of method pick", "is already instantiated",
        "No given instance of type Int was found", "both parameter j and parameter i match type Int",
        "positional after named argument", "[E007]", "Found:    (w : q.Out)", "Required: p.Out", "6 errors found",
    ), classpath)


def check_poly_named_using(toolchain, output, client_output, classpath):
    full = "inference.Box[Int] = this.choose[Int](using value = b, fmt = f)"
    getter = "this.fallback[Int](using value = b, this.fallback$default$2[Int])"
    check_typer_output(toolchain, output, "PolyNamed.scala", 0, (
        "this.choose[Int](using b, f)",
        *(f"def {site}(b: inference.Box[Int], f: inference.Fmt[Int]):\n      {full}"
          for site in ("explicit", "ordered", "reversed")),
        "this.pair[Int, Boolean](using x = b, y = c)",
        "val y$1: inference.Box[Boolean] = this.boolean()\n"
        "        val x$1: inference.Box[Int] = this.integer()\n"
        "        this.pair[Int, Boolean](using x = x$1, y = y$1)",
        "inference.Box[Int] = this.choose[Int](using value = b, f)",
        "this.front[Int](using f, value = b)", "this.middle[Int](using b, f, flag = true)",
        "def wide(b: inference.Box[Int])(using f: inference.Fmt[Int]): Any =\n      this.choose[Int](using value = b, f)",
        f"def defaulted(b: inference.Box[Int]): inference.Box[Int] =\n      {getter}",
        f"def shadow(b: inference.Box[Int])(using f: inference.Fmt[Int]):\n      inference.Box[Int] =\n      {getter}",
        "[E220]", "def shadow(b: Box[Int])(using f: Fmt[Int]): Box[Int] = fallback(using value = b)", "1 warning found",
        "this.fallback[Int](using value = b, fmt = f)", "this.bounded[inference.Dog](using value = d)",
        "inference.Api.selected[Int](using value = b, f)", "inference.Providers.entry[A](value, fmt)",
        "this.old[Int](using value = b, f)",
    ), toolchain.classpath)
    check_typer_output(toolchain, client_output, "PolyNamedErrors.scala", 1, (
        "does not have a parameter bogus", "parameter value of method choose", "is already instantiated",
        "No given instance of type inference.Fmt[Int] was found",
        "both parameter r and parameter l match type inference.Fmt[Int]",
        "Found:    (f : inference.Fmt[Boolean])", "Required: inference.Fmt[Int]",
        "Found:    (b : inference.Box[Int])", "A is a type variable with constraint <: inference.Animal",
        "6 errors found",
    ), classpath)


def check_omitted_witnesses(toolchain, output, client_output, classpath):
    positive = check_typer_output(toolchain, output, "OmittedWitness.scala", 0, (
        "[E220]", "def shadow(using b: Box[Int], f: Fmt[Int])", "1 warning found",
    ), toolchain.classpath)
    getter = "this.fallback[Int](using b, this.fallback$default$2[Int], flag = true)"
    calls = {
        "explicit": "this.read[Int](using b, f, flag = true)",
        "written": "this.read[Int](using b = b, f, flag = true)",
        "single": "this.one[Int](using b, flag = true)",
        "trailing": "this.last[Boolean](using flag = true, b)",
        "both": "this.read[Int](using b, f, flag = true)",
        "reverse": "this.flip[Int](using f, b, flag = true)",
        "twoVars": "this.pair[Int, Boolean](using b, f, flag = true)",
        "partial": "this.pair[Int, Boolean](using b = b, f, flag = true)",
        "earlyFilter": "this.flip[Int](using f, a, flag = true)",
        "boundOk": "this.bounded[omitted.Dog](using d, flag = true)",
        "boundMix": "this.bounded[omitted.Dog](using d, flag = true)",
        "imported": "omitted.Imports.imported this.one[Boolean](using b$1, flag = true)",
        "companion": "omitted.Anchor.chosen this.anchor[Boolean](using a$1, flag = true)",
        "exported": "omitted.Api.selected[Int](using b, f, flag = true)",
        "legacy": "this.old[Int](using b, f, flag = true)",
        "defaulted": getter, "shadow": getter,
    }
    methods = positive.split("\n    def ")
    for site, expected in calls.items():
        bodies = [body for body in methods if body.startswith(site + "(")]
        if len(bodies) != 1 or expected not in " ".join(bodies[0].split()):
            raise SystemExit(f"OmittedWitness.{site} expected {expected}\n{positive}")

    rejected = check_typer_output(toolchain, client_output, "OmittedWitnessErrors.scala", 1,
                                  ("7 errors found",), classpath)
    errors = rejected.split("[[syntax trees", 1)[0].split("-- [E172]")[1:]
    missing = "No given instance of type omitted."
    ambiguous = "both parameter b and parameter a match type omitted.Box[A]"
    for site, expected in {
        "absent": missing + "Box[A]", "noSecond": missing + "Fmt[Int]",
        "mismatch": missing + "Fmt[Int]", "ambSame": ambiguous, "ambDiff": ambiguous,
        "lateFilter": ambiguous, "boundBad": missing + "Box[A]",
    }.items():
        if not any(f"def {site}(" in err and expected in err for err in errors):
            raise SystemExit(f"OmittedWitnessErrors.{site} expected {expected}\n{rejected}")


def check_typer_output(toolchain, output, source, code, required, classpath):
    result = subprocess.run(
        [toolchain.java, "-cp", toolchain.classpath, "dotty.tools.dotc.Main",
         "-classpath", classpath, "-color:never", "-Vprint:typer",
         "-d", output, HERE / source],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False,
    )
    if result.returncode != code or any(item not in result.stdout for item in required):
        raise SystemExit(f"{source} disagrees with Scala 3.9 controls\n{result.stdout}")
    return result.stdout


def verify_rust_library(toolchain, candidate, client=None):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        library = root / "library"
        library.mkdir()
        toolchain.compiler(["-from-tasty", "-d", library, candidate])
        shutil.copyfile(candidate, library / "example" / candidate.name)
        if client is None:
            client = root / "Client.scala"
            client.write_text(
                "object Client { def main(args: Array[String]): Unit = { "
                "given Int = 41; val lib = new example.Evidence; "
                "assert(lib.read == 41); assert(lib.old(using 7) == 7); "
                "assert(lib.forward == 41); assert(lib.explicit(7) == 7) } }"
            )
        classpath = os.pathsep.join((str(library), toolchain.classpath))
        toolchain.compiler(["-d", root, client], classpath=classpath)
        subprocess.run([toolchain.java, "-Xverify:all", "-cp",
                        os.pathsep.join((str(root), classpath)), "Client"], check=True)


if __name__ == "__main__":
    main()
