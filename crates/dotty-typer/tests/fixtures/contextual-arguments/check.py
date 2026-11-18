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
        check_typer_output(toolchain, client_output, "BinaryDefaultsAmbiguous.scala", 1, (
            "[E172]", "Ambiguous given instances", "of parameter n of method offset",
        ), classpath)


def check_typer_output(toolchain, output, source, code, required, classpath):
    result = subprocess.run(
        [toolchain.java, "-cp", toolchain.classpath, "dotty.tools.dotc.Main",
         "-classpath", classpath, "-color:never", "-Vprint:typer",
         "-d", output, HERE / source],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False,
    )
    if result.returncode != code or any(item not in result.stdout for item in required):
        raise SystemExit(f"{source} disagrees with Scala 3.9 controls\n{result.stdout}")


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
