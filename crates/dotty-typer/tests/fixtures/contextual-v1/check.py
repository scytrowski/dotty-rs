"""Compare targeted typer observations on unchanged upstream Scala 3.9 test files."""

import argparse
import csv
import os
from pathlib import Path
import re
import runpy
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
MODULE = "typer::tests::contextual_v1::"
SUPPLEMENTAL_CONTROLS = (
    MODULE + "lookup::binary_fractional_companion_search",
    MODULE + "inherited_nested::fractional_numeric_ops_uses_the_subclass_prefix",
    "typer::tests::contextual_arguments::envelope_copy_reads_the_original_field_and_forwards_two_new_witnesses",
    "typer::tests::contextual_arguments::envelope_calls_diagnose_each_context_argument_at_the_call_site",
    "typer::tests::contextual_arguments::envelope_apply_passes_both_context_arguments_to_the_constructor",
    "typer::tests::contextual_arguments::envelope_constructor_merges_searched_and_explicit_arguments_in_scala_order",
    "typer::tests::contextual_arguments::ordinary_class_bound_uses_private_evidence_in_the_body_and_caller_evidence_at_new",
    "typer::tests::contextual_arguments::ordinary_class_bound_reports_missing_and_ambiguous_evidence_at_new",
    "typer::tests::contextual_arguments::case_class_bound_infers_int_and_passes_the_callers_witness_to_apply",
    "typer::tests::contextual_arguments::case_class_bound_reports_missing_and_ambiguous_evidence_at_apply",
    "typer::tests::contextual_arguments::case_class_copy_defaults_the_field_and_forwards_the_call_site_witness",
    "typer::tests::contextual_arguments::case_class_copy_reports_missing_and_ambiguous_call_site_evidence",
)


def cases():
    rows = {}
    for manifest in sorted(HERE.glob("*.tsv")):
        with manifest.open() as stream:
            for case, source, code, needles in csv.reader(stream, delimiter="\t"):
                name = f"{MODULE}{manifest.stem}::{case}"
                if name in rows or not needles or code not in {"0", "1"}:
                    raise ValueError(f"invalid or duplicate case {name}")
                rows[name] = (source, int(code), needles.split(" ~~ "))
    if not rows:
        raise ValueError("no upstream cases")
    return rows


def scala_observation(tc, checkout, source, expected_exit, needles, out):
    pinned_bytes = subprocess.check_output(["git", "-C", checkout, "show", f"HEAD:{source}"])
    path = checkout / source
    if pinned_bytes != path.read_bytes():
        raise ValueError(f"working copy differs from the pinned source: {source}")
    check_compiler(tc, path, expected_exit, needles, out)


def check_compiler(tc, path, expected_exit, needles, out):
    result = subprocess.run(
        [tc.java, "-XX:-UsePerfData", "-cp", tc.classpath, "dotty.tools.dotc.Main",
         "-classpath", tc.runtime_classpath, "-color:never", "-Vprint:typer",
         "-d", out, path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    output = " ".join(result.stdout.split())
    if result.returncode != expected_exit or any(" ".join(n.split()) not in output for n in needles):
        raise RuntimeError(f"unexpected Scala observation for {path}\n{result.stdout}")


def check_controls(tc, checkout):
    controls = [
        ("FractionalStandard.scala", 0, ["def standard: scala.math.Fractional[Double] = scala.math.Numeric.DoubleIsFractional",
          "def single: scala.math.Fractional[Float] = scala.math.Numeric.FloatIsFractional"]),
        ("FractionalInherited.scala", 0, ["def inherited(v: Slice.this.NumericOps): Slice.this.NumericOps = v"]),
        ("FractionalMissing.scala", 1, ["[E172]", "No given instance of type Fractional[T] was found", "1 error found"]),
        ("Envelope.scala", 0, [
            "def apply[A](value: A)(using evidence$1: TC[A], trace: Trace): Envelope[A] = "
            "new Envelope[A](value)(using evidence$1, trace)",
            "Envelope.apply[Int](1)(tc, log)",
            "def copy[A](value: A)(using evidence$1: TC[A], trace: Trace): Envelope[A] = "
            "new Envelope[A](value)(using evidence$1, trace)",
            "def copy$default$1[A]: A = Envelope.this.value",
            "Envelope.apply[Int](1)(using initial, previous)",
            "original.copy[Int](original.copy$default$1[Int])(current, active)",
            "new Envelope[Int](1)(tc, log)",
            "new Envelope[Int](1)(using tc, log)",
        ]),
        ("CaseClassCopy.scala", 0, [
            "def copy[A](value: A)(using evidence$1: TC[A]): Box[A] = new Box[A](value)(using evidence$1)",
            "def copy$default$1[A]: A = Box.this.value",
            "Box.apply[Int](1)(using initial)",
            "box.copy[Int](box.copy$default$1[Int])(current)",
        ]),
        ("CaseClassCopyMissing.scala", 1, ["[E172]", "No given instance of type TC[Int] was found "
          "for a context parameter of method copy in class Box", "1 error found"]),
        ("CaseClassCopyAmbiguous.scala", 1, ["[E172]", "both parameter second and parameter first "
          "match type TC[Int] of a context parameter of method copy in class Box", "1 error found"]),
        ("CaseClassBound.scala", 0, [
            "def apply[A](value: A)(using evidence$1: TC[A]): Box[A] = new Box[A](value)(using evidence$1)",
            "def evidence: TC[Box.this.A] = Box.this.evidence$1",
            "def make(using leaf: TC[Int]): Box[Int] = Box.apply[Int](1)(leaf)",
        ]),
        ("CaseClassBoundMissing.scala", 1, ["[E172]", "No given instance of type TC[Int] was found "
          "for a context parameter of method apply in object Box", "1 error found"]),
        ("CaseClassBoundAmbiguous.scala", 1, ["[E172]", "both parameter second and parameter first "
          "match type TC[Int] of a context parameter of method apply in object Box", "1 error found"]),
        ("ClassBound.scala", 0, [
            "class Box[A >: Nothing <: Any](using evidence$1: TC[A])",
            "private[this] given val evidence$1: TC[A]",
            "def evidence: TC[Box.this.A] = Box.this.evidence$1",
            "new Box[Int](leaf)()",
        ]),
        ("ClassBoundMissing.scala", 1, ["[E172]", "No given instance of type TC[Int] was found "
          "for a context parameter of constructor Box in class Box", "1 error found"]),
        ("ClassBoundAmbiguous.scala", 1, ["[E172]", "both parameter second and parameter first "
          "match type TC[Int] of a context parameter of constructor Box in class Box", "1 error found"]),
        ("CurriedDefault.scala", 0, ["this.narrow(c)(this.narrow$default$2(c))",
          "def narrow$default$2(using c: Int): Int @uncheckedVariance = this.root(c)"]),
        ("DependentWitnessDefault.scala", 0, [
            "def read[A >: Nothing <: Any](using tc: Ev[A])(v: tc.Out)(using n: Int): Int",
            "def read$default$3[A >: Nothing <: Any](using tc: Ev[A])(v: tc.Out): Int @uncheckedVariance = 1",
            "def shadow[tc >: Nothing <: Any](using tc: Ev[tc])(v: tc.Out)(using n: Int)",
            "def shadow$default$3[tc >: Nothing <: Any](using tc: Ev[tc])(v: tc.Out)",
            "def project[A >: Nothing <: Any](v: tc#Out)(using tc: Ev[A], n: Int): Int",
            "def project$default$3[A >: Nothing <: Any](v: tc#Out): Int @uncheckedVariance = 1",
            "this.read[Int](using e)(v)(using 1)",
            "this.read[Int](using e)(v)(using this.read$default$3[Int](e)(v))",
        ]),
        ("DifferentWitness.scala", 1, ["[E007]", "Found: (v : e2.Out)", "Required: e1.Out", "1 error found"]),
        ("ExplicitContextFunction.scala", 0, ["this.ExplicitContextFunction$$inline$bar.apply(using 4):Int",
          "private[this] def bar: (Int) ?=> Int = (using contextual$1: Int) => 3"]),
        ("OwnerAmbiguity.scala", 1, ["[E172]", "both given instance a1 in object Cases and given "
          "instance a2 in object Cases match type A of parameter a of method g in object Cases",
          "1 error found"]),
    ]
    for file, message in [
        ("EnvelopeMissing.scala", "No given instance of type {goal} was found for {param} of {callee}"),
        ("EnvelopeAmbiguous.scala", "both parameter second and parameter first match type {goal} of {param} of {callee}"),
    ]:
        messages = [
            message.format(goal=goal, param=param, callee=callee)
            for goal, param in [("TC[Int]", "a context parameter"), ("Trace", "parameter trace")]
            for callee in ["constructor Envelope in class Envelope", "method apply in object Envelope",
                           "method copy in class Envelope"]
        ]
        controls.append((file, 1, ["[E172]", "6 errors found", *messages]))
    for file, code, needles in controls:
        with tempfile.TemporaryDirectory() as out:
            check_compiler(tc, HERE / "controls" / file, code, needles, out)
    variants = [
        ("tests/pos/given-priority.scala", [("import language.`3.6`", "")],
         ["val x: NormalImplicits.g2.type = NormalImplicits.g2"]),
        ("tests/run/implicit-disambiguation.scala",
         [("def f(using B, C): String", "def f(using B, C): A"), ("summon[A].show", "summon[A]")],
         ["final lazy given val a: A = x$1", "a:A"]),
    ]
    for file, replacements, needles in variants:
        with tempfile.TemporaryDirectory() as out:
            text = (checkout / file).read_text()
            for before, after in replacements:
                if text.count(before) != 1:
                    raise ValueError(f"expected one {before!r} in {file}")
                text = text.replace(before, after)
            path = Path(out) / Path(file).name
            path.write_text(text)
            check_compiler(tc, path, 0, needles, out)


def rust_observations(tc, rows):
    env = {**os.environ, "SCALA39_CLASSPATH": tc.runtime_classpath}
    result = subprocess.run(
        ["cargo", "test", "--locked", "-p", "dotty-typer", "--lib", "--",
         "--include-ignored", "--show-output", "--test-threads=1", MODULE, *SUPPLEMENTAL_CONTROLS],
        cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    passed = set(re.findall(r"^test (\S+) \.\.\. ok$", result.stdout, re.MULTILINE))
    sources = set(re.findall(r"^AUDIT_SOURCE\t(\S+)\t(\S+)$", result.stdout, re.MULTILINE))
    outcomes = re.findall(r"^AUDIT_OUTCOME\t(\S+)\t(\S+)$", result.stdout, re.MULTILINE)
    statuses = dict(outcomes)
    if (result.returncode or passed != rows.keys() | set(SUPPLEMENTAL_CONTROLS)
            or sources != {(name, row[0]) for name, row in rows.items()}
            or len(outcomes) != len(rows) or statuses.keys() != rows.keys()
            or not set(statuses.values()) <= {"match", "typer-gap", "blocked"}):
        raise RuntimeError(f"Rust observations missing, duplicated or changed\n{result.stdout}")
    return statuses


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--require-parity", action="store_true",
                        help="fail also on known typer gaps and blocked observations")
    args = parser.parse_args()
    tc = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))["Toolchain"]()
    if tc.jdk_release["JAVA_VERSION"].split(".")[0] != "21":
        raise RuntimeError("this audit uses JDK 21 on both sides")
    checkout = Path(os.environ["SCALA39_ROOT"]).resolve()
    print(f"{tc.compiler_version}; JDK {tc.jdk_release['JAVA_VERSION']}", flush=True)
    rows = cases()
    for source, code, needles in rows.values():
        with tempfile.TemporaryDirectory() as out:
            scala_observation(tc, checkout, source, code, needles, out)
    check_controls(tc, checkout)
    statuses = rust_observations(tc, rows)
    for name, (source, _, _) in rows.items():
        print(f"{statuses[name]:10} {name.rsplit('::', 1)[1]:32} {source}")
    print("3 ordinary-class, 3 case-class apply and 3 case-class copy controls matched; Envelope new/apply/copy and all 12 error sites matched")
    open_rows = sum(s in {"typer-gap", "blocked"} for s in statuses.values())
    print(f"{len(rows)} observations verified; {open_rows} open for v1")
    return int(args.require_parity and open_rows > 0)


if __name__ == "__main__":
    raise SystemExit(main())
