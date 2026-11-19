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
        ("CurriedDefault.scala", 0, ["this.narrow(c)(this.narrow$default$2(c))",
          "def narrow$default$2(using c: Int): Int @uncheckedVariance = this.root(c)"]),
        ("ExplicitContextFunction.scala", 0, ["this.ExplicitContextFunction$$inline$bar.apply(using 4):Int",
          "private[this] def bar: (Int) ?=> Int = (using contextual$1: Int) => 3"]),
        ("OwnerAmbiguity.scala", 1, ["[E172]", "both given instance a1 in object Cases and given "
          "instance a2 in object Cases match type A of parameter a of method g in object Cases",
          "1 error found"]),
    ]
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
        ["cargo", "test", "--locked", "-p", "dotty-typer", "--lib", MODULE, "--",
         "--ignored", "--show-output", "--test-threads=1"],
        cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    passed = set(re.findall(r"^test (\S+) \.\.\. ok$", result.stdout, re.MULTILINE))
    sources = set(re.findall(r"^AUDIT_SOURCE\t(\S+)\t(\S+)$", result.stdout, re.MULTILINE))
    outcomes = re.findall(r"^AUDIT_OUTCOME\t(\S+)\t(\S+)$", result.stdout, re.MULTILINE)
    statuses = dict(outcomes)
    if (result.returncode or passed != rows.keys()
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
    open_rows = sum(s in {"typer-gap", "blocked"} for s in statuses.values())
    print(f"{len(rows)} observations verified; {open_rows} open for v1")
    return int(args.require_parity and open_rows > 0)


if __name__ == "__main__":
    raise SystemExit(main())
