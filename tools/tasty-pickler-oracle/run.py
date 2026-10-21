#!/usr/bin/env python3
"""Opt-in, offline Scala 3.9 fixture generation and independent verification."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/tasty_pickler"
REVISION = "777528f19a58e794c9954a42f433373472ec57f8"
ARTIFACTS = [
    ("org/scala-lang", artifact, "3.9.0")
    for artifact in ("scala3-library_3", "scala-library", "scala3-compiler_3",
                     "tasty-core_3", "scala3-interfaces")
] + [
    ("org/scala-lang/modules", "scala-asm", "9.9.0-scala-1"),
    ("org/scala-sbt", "compiler-interface", "1.12.0"),
    ("org/scala-sbt", "util-interface", "1.11.5"),
]


class CommandFailure(RuntimeError):
    """A command ran and failed, distinct from unavailable tools/configuration."""


def execute(args: list[str | Path], *, cwd: Path = ROOT) -> str:
    try:
        result = subprocess.run([str(a) for a in args], cwd=cwd, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=180)
    except subprocess.TimeoutExpired as error:
        # A timeout is a harness failure, never an expected reader rejection.
        raise RuntimeError(f"command timed out after 180 seconds: {args}") from error
    if result.returncode:
        raise CommandFailure(f"command failed ({result.returncode}): {args}\n{result.stdout}")
    return result.stdout


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_provenance() -> None:
    pinned = json.loads((FIXTURES / "provenance.json").read_text())
    sources = {p.name: digest(p) for p in FIXTURES.glob("*.scala")}
    tasty = {str(p.relative_to(FIXTURES / "tasty")): digest(p)
             for p in (FIXTURES / "tasty").rglob("*.tasty")}
    if sources != pinned["sources_sha256"] or tasty != pinned["tasty_sha256"]:
        raise RuntimeError("checked-in fixture inventory/content differs from SHA-256 provenance")


class Toolchain:
    def __init__(self) -> None:
        required = ("SCALA39_ROOT", "JAVA_HOME", "COURSIER_MAVEN_ROOT")
        for key in required:
            if not os.environ.get(key):
                raise RuntimeError(f"{key} is required for the explicitly requested oracle")
        scala_root = Path(os.environ["SCALA39_ROOT"])
        actual = execute(["git", "-C", scala_root, "rev-parse", "HEAD"]).strip()
        if actual != REVISION:
            raise RuntimeError(f"Scala source revision {actual}; expected {REVISION}")
        java_home = Path(os.environ["JAVA_HOME"])
        self.java = java_home / "bin/java"
        self.javac = java_home / "bin/javac"
        self.javap = java_home / "bin/javap"
        for executable, label in ((self.java, "Java"), (self.javac, "javac"),
                                  (self.javap, "javap")):
            if not executable.is_file():
                raise RuntimeError(f"missing JDK {label} executable: {executable}")
        release = dict(re.findall(r'^([A-Z_]+)="(.*)"$',
                                  (java_home / "release").read_text(), re.MULTILINE))
        self.jdk_release = {key: release[key] for key in
                            ("JAVA_VERSION", "IMPLEMENTOR", "IMPLEMENTOR_VERSION", "OS_ARCH")}
        java_version = re.match(r"(\d+)", self.jdk_release["JAVA_VERSION"])
        if java_version is None or int(java_version.group(1)) < 21:
            raise RuntimeError(f"JDK 21 or newer is required, got {self.jdk_release['JAVA_VERSION']}")
        base = Path(os.environ["COURSIER_MAVEN_ROOT"])
        jars = [base / group / artifact / version / f"{artifact}-{version}.jar"
                for group, artifact, version in ARTIFACTS]
        for jar in jars:
            if not jar.is_file():
                raise RuntimeError(f"missing cached compiler dependency: {jar}")
        self.classpath = os.pathsep.join(str(p) for p in jars)
        self.artifacts = {p.name: digest(p) for p in jars}
        pinned = json.loads((FIXTURES / "provenance.json").read_text())
        if self.artifacts != pinned["compiler_artifacts_sha256"]:
            raise RuntimeError("compiler artifacts differ from pinned SHA-256 provenance")
        version = self.compiler(["-version"])
        if not re.search(r"Scala compiler version 3\.9\.0 --", version):
            raise RuntimeError(f"expected Scala 3.9.0 compiler, got: {version}")

    def compiler(self, args: list[str | Path], *, decompile: bool = False,
                 cwd: Path = ROOT) -> str:
        main = "dotty.tools.dotc.decompiler.Main" if decompile else "dotty.tools.dotc.Main"
        return execute([self.java, "-XX:-UsePerfData", "-cp", self.classpath, main,
                        "-classpath", self.classpath, "-color:never", *args], cwd=cwd)


def inspector() -> Path:
    configured = os.environ.get("TASTY_PICKLER_INSPECTOR")
    if configured:
        binary = Path(configured).resolve()
    else:
        execute(["cargo", "build", "--locked", "-p", "tasty-pickler-oracle"])
        # Cargo honors CARGO_TARGET_DIR; do not assume target/debug for custom CI.
        metadata = json.loads(execute(["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"]))
        binary = Path(metadata["target_directory"]) / "debug/tasty-pickler-oracle"
    if not binary.is_file():
        raise RuntimeError(f"missing Rust inspector executable: {binary}")
    return binary


def snapshot(binary: Path, candidate: Path) -> dict:
    return json.loads(execute([binary, "inspect", candidate]))


def generate(toolchain: Toolchain, binary: Path, output: Path, check: bool) -> None:
    sources = sorted(FIXTURES.glob("*.scala"))
    toolchain.compiler(["-sourceroot", ".", "-Yprint-tasty", "-d", output,
                        *(p.name for p in sources)], cwd=FIXTURES)
    expected_paths = {"Empty.tasty", *(f"example/{p.stem}.tasty" for p in sources if p.stem != "Empty")}
    paths = {str(p.relative_to(output)) for p in output.rglob("*.tasty")}
    if paths != expected_paths:
        raise RuntimeError(f"unexpected compiler artifact inventory: {paths}")
    for relative in sorted(paths):
        candidate = output / relative
        actual = snapshot(binary, candidate)
        expected = FIXTURES / "snapshots" / Path(relative).with_suffix(".json")
        if check:
            if actual != json.loads(expected.read_text()):
                raise RuntimeError(f"structural snapshot differs: {relative}")
        else:
            expected.parent.mkdir(parents=True, exist_ok=True)
            expected.write_text(json.dumps(actual, indent=2, sort_keys=True) + "\n")
            destination = FIXTURES / "tasty" / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(candidate, destination)
    manifest = {
        "schema": 1, "scala_version": "3.9.0", "tasty_version": [28, 9, 0],
        "scala_source_revision": REVISION,
        "compiler_artifacts_sha256": toolchain.artifacts,
        "jdk_release": toolchain.jdk_release,
        "sources_sha256": {p.name: digest(p) for p in sources},
        "tasty_sha256": {p: digest(FIXTURES / "tasty" / p) for p in sorted(paths)},
        "generation": "run.py generate; dotty.tools.dotc.Main -classpath <pinned-jars> -color:never -sourceroot . -Yprint-tasty -d <fresh-output> *.scala (cwd=tests/fixtures/tasty_pickler)",
    }
    if not check:
        (FIXTURES / "provenance.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"Scala-generated structural snapshots {'checked' if check else 'recorded'}: {len(paths)}")


def plain_name(name: dict) -> str:
    if "utf8" in name:
        return name["utf8"]
    if "qualified" in name:
        return ".".join(plain_name(part) for part in name["qualified"])
    raise RuntimeError("candidate output boundary requires a plain name")


def artifact_path(structure: dict) -> Path:
    nodes = structure["nodes"]
    roots = [node for node in nodes if node["parent"] is None]
    if len(roots) != 1 or roots[0]["tag"] != "PACKAGE":
        raise RuntimeError("candidate must have one top-level PACKAGE")
    package_id = roots[0]["id"]
    packages = [node for node in nodes if node["parent"] == package_id
                and node["tag"] == "TERMREFpkg"]
    classes = [node for node in nodes if node["parent"] == package_id
               and node["tag"] == "TYPEDEF"]
    if len(packages) != 1 or len(classes) != 1:
        raise RuntimeError("candidate must contain exactly one top-level class")
    package = plain_name(packages[0]["name"])
    components = ([] if package == "<empty>" else package.split("."))
    components.append(plain_name(classes[0]["name"]))
    if any(not re.fullmatch(r"[\w$]+", component) for component in components):
        raise RuntimeError("candidate has an unsupported/unsafe artifact path")
    return Path(*components).with_suffix(".tasty")


def verify_candidate(toolchain: Toolchain, binary: Path, candidate: Path,
                     directory: Path, structure: dict | None = None,
                     runtime: bool = False) -> dict:
    # Rust wire validation always precedes candidate use by an independent reader.
    structure = snapshot(binary, candidate) if structure is None else structure
    relative = artifact_path(structure)
    staged = directory / "candidate" / relative
    compiled = directory / "compiled"
    staged.parent.mkdir(parents=True)
    compiled.mkdir()
    shutil.copyfile(candidate, staged)
    # Classpath is only the pinned compiler/libraries. The compiler adds the
    # package-aligned candidate root; no original output, source or class file.
    decompiled = toolchain.compiler([staged], decompile=True)
    if not decompiled.strip():
        raise RuntimeError("independent decompiler returned no tree")
    if relative.as_posix() == "example/PrivateFields.tasty":
        expected_members = (
            "private[this] val seed: scala.Int = 42",
            "private[this] val enabled: scala.Boolean = true",
            "private[this] var counter: scala.Int = 0",
            "def constant(): scala.Int = 1",
            "def seedValue(): scala.Int = PrivateFields.this.seed",
            "def seedViaThis(): scala.Int = this.seed",
            "def enabledValue(): scala.Boolean = PrivateFields.this.enabled",
            "def shadowedSeed(): scala.Int =",
            "def enabledBranch(): scala.Int =",
            "def enabledViaThis(): scala.Boolean = this.enabled",
            "def disabled(): scala.Boolean =",
            "def seedBranch(flag: scala.Boolean): scala.Int =",
            "def seedViaThisBranch(",
            "def seedParenBranch(",
        )
        if not all(member in decompiled for member in expected_members):
            raise RuntimeError(f"fresh PrivateFields TASTy lost a field or method: {decompiled}")
    toolchain.compiler(["-from-tasty", "-d", compiled, staged])
    emitted = compiled / relative.with_suffix(".class")
    if not emitted.is_file() or emitted.stat().st_size == 0:
        raise RuntimeError(f"compiler produced no fresh class: {relative}")
    if list((directory / "candidate").rglob("*.class")):
        raise RuntimeError("candidate root unexpectedly contains a class file")
    result = {"artifact": str(relative), "candidate_sha256": digest(candidate),
              "rust_wire": True, "scala_decompile": True, "scala_from_tasty": True,
              "fresh_class": True, "adjacent_input_classfiles": False}
    if runtime:
        runtime_cases = {
            "example/Logic.tasty": (
                "example.Logic", r"public boolean safeAnd\(\)", "LogicRuntime.java",
                """public final class LogicRuntime {
  public static void main(String[] args) {
    example.Logic logic = new example.Logic();
    if (logic.negate(true) || !logic.negate(false) ||
        !logic.both(true, true) || logic.both(true, false) ||
        logic.both(false, true) || logic.both(false, false) ||
        !logic.either(true, true) || !logic.either(true, false) ||
        !logic.either(false, true) || logic.either(false, false) ||
        logic.safeAnd() || !logic.safeOr())
      throw new AssertionError("unexpected Boolean results or eager short-circuit RHS");
    try { logic.evaluatedAnd(true); throw new AssertionError("&& RHS was not evaluated"); }
    catch (ArithmeticException expected) { }
    try { logic.evaluatedOr(false); throw new AssertionError("|| RHS was not evaluated"); }
    catch (ArithmeticException expected) { }
    System.out.println("Logic = Boolean truth tables and short-circuit");
  }
}
""", "Logic = Boolean truth tables and short-circuit", 1,
            ),
            "example/Arithmetic.tasty": (
                "example.Arithmetic", r"public int add\(int, int\)", "ArithmeticRuntime.java",
                """public final class ArithmeticRuntime {
  public static void main(String[] args) {
    example.Arithmetic arithmetic = new example.Arithmetic();
    int add = arithmetic.add(Integer.MAX_VALUE, 1);
    int subtract = arithmetic.subtract(Integer.MIN_VALUE, 1);
    int multiply = arithmetic.multiply(1_073_741_824, 2);
    int divide = arithmetic.divide(-7, 3);
    int overflowDivide = arithmetic.divide(Integer.MIN_VALUE, -1);
    int remainder = arithmetic.remainder(-7, 3);
    int overflowRemainder = arithmetic.remainder(Integer.MIN_VALUE, -1);
    int nested = arithmetic.nested(2, 3, 4);
    int throughCall = arithmetic.throughCall(6, 7);
    int local = arithmetic.local(6, 7);
    int compute = arithmetic.compute(2, 3);
    if (add != Integer.MIN_VALUE || subtract != Integer.MAX_VALUE ||
        multiply != Integer.MIN_VALUE || divide != -2 ||
        overflowDivide != Integer.MIN_VALUE || remainder != -1 ||
        overflowRemainder != 0 || nested != 14 || throughCall != 13 || local != 13 || compute != 9)
      throw new AssertionError("unexpected Arithmetic results");
    try {
      arithmetic.divide(1, 0);
      throw new AssertionError("division by zero did not throw");
    } catch (ArithmeticException expected) { }
    System.out.println("Arithmetic = " + add + ", " + subtract + ", " + multiply +
        ", " + divide + ", " + overflowDivide + ", " + remainder + ", " +
        overflowRemainder + ", " + nested + ", " + throughCall + ", " + local + ", " + compute);
  }
}
""", "Arithmetic = -2147483648, 2147483647, -2147483648, -2, -2147483648, -1, 0, 14, 13, 13, 9", 14,
            ),
            "example/Comparisons.tasty": (
                "example.Comparisons", r"public boolean positive\(int\)", "ComparisonsRuntime.java",
                """public final class ComparisonsRuntime {
  public static void main(String[] args) {
    example.Comparisons c = new example.Comparisons();
    if (c.positive(Integer.MIN_VALUE) || c.positive(-1) || c.positive(0) ||
        !c.positive(1) || !c.positive(Integer.MAX_VALUE) ||
        !c.before(Integer.MIN_VALUE, -1) || c.before(-1, Integer.MIN_VALUE) ||
        !c.atMost(-1, -1) || c.atMost(0, -1) ||
        !c.atLeast(Integer.MAX_VALUE, Integer.MAX_VALUE) || c.atLeast(0, 1) ||
        !c.nested(1, 2) || !c.nested(0, 0) || c.choose(-1) != 0 || c.choose(1) != 1)
      throw new AssertionError("unexpected Comparisons results");
    System.out.println("Comparisons = signed Int boundaries and Boolean conditions");
  }
}
""", "Comparisons = signed Int boundaries and Boolean conditions", 1,
            ),
            "example/Answer.tasty": (
                "example.Answer", r"public int value\(\)", "AnswerRuntime.java",
                """public final class AnswerRuntime {
  public static void main(String[] args) {
    int actual = new example.Answer().value();
    if (actual != 42) throw new AssertionError("Answer.value() = " + actual);
    System.out.println("Answer.value() = " + actual);
  }
}
""", "Answer.value() = 42", 42,
            ),
            "example/PrivateFields.tasty": (
                "example.PrivateFields", r"public int seedValue\(\)",
                "PrivateFieldsRuntime.java",
                """public final class PrivateFieldsRuntime {
  public static void main(String[] args) throws Exception {
    example.PrivateFields values = new example.PrivateFields();
    if (values.seedValue() != 42 || values.seedViaThis() != 42 ||
        !values.enabledValue() || values.shadowedSeed() != 7 || values.constant() != 1 ||
        values.enabledBranch() != 1 || !values.enabledViaThis() || values.disabled() ||
        values.seedBranch(true) != 42 || values.seedBranch(false) != 0 ||
        values.seedViaThisBranch(true) != 42 || values.seedViaThisBranch(false) != 0 ||
        values.seedParenBranch(true) != 42 || values.seedParenBranch(false) != 0)
      throw new AssertionError("PrivateFields field reads or shadowing changed");
    System.out.println("PrivateFields field reads and shadowing passed");
  }
}
""", "PrivateFields field reads and shadowing passed", 1,
            ),
            "example/Parameters.tasty": (
                "example.Parameters", r"public int pick\(int, int\)", "ParametersRuntime.java",
                """public final class ParametersRuntime {
  public static void main(String[] args) {
    int actual = new example.Parameters().pick(11, 42);
    if (actual != 42) throw new AssertionError("Parameters.pick() = " + actual);
    System.out.println("Parameters.pick() = " + actual);
  }
}
""", "Parameters.pick() = 42", 42,
            ),
            "example/Flags.tasty": (
                "example.Flags", r"public boolean enabled\(\)", "FlagsRuntime.java",
                """public final class FlagsRuntime {
  public static void main(String[] args) {
    example.Flags flags = new example.Flags();
    boolean enabled = flags.enabled();
    boolean disabled = flags.disabled();
    boolean passTrue = flags.passthrough(true);
    boolean passFalse = flags.passthrough(false);
    int keepTrue = flags.keep(true, 17);
    int keepFalse = flags.keep(false, 42);
    if (!enabled || disabled || !passTrue || passFalse || keepTrue != 17 || keepFalse != 42)
      throw new AssertionError("unexpected Flags results");
    System.out.println("Flags = " + enabled + ", " + disabled + ", " +
        passTrue + ", " + passFalse + ", " + keepTrue + ", " + keepFalse);
  }
}
""", "Flags = true, false, true, false, 17, 42", 42,
            ),
            "example/Conditionals.tasty": (
                "example.Conditionals", r"public int choose\(boolean, int, int\)", "ConditionalsRuntime.java",
                """public final class ConditionalsRuntime {
  public static void main(String[] args) {
    example.Conditionals values = new example.Conditionals();
    int chooseTrue = values.choose(true, 17, 42);
    int chooseFalse = values.choose(false, 17, 42);
    int nestedTrueTrue = values.nested(true, true);
    int nestedTrueFalse = values.nested(true, false);
    int nestedFalse = values.nested(false, true);
    int constant = values.constant();
    int mixedTrue = values.mixed(true, 19);
    int mixedFalse = values.mixed(false, 19);
    int callTrue = values.callBranch(true, 31);
    int callFalse = values.callBranch(false, 31);
    if (chooseTrue != 17 || chooseFalse != 42 || nestedTrueTrue != 1 ||
        nestedTrueFalse != 2 || nestedFalse != 3 || constant != 17 ||
        mixedTrue != 19 || mixedFalse != 42 || callTrue != 31 || callFalse != 42)
      throw new AssertionError("unexpected Conditionals results");
    System.out.println("Conditionals = " + chooseTrue + ", " + chooseFalse + ", " +
        nestedTrueTrue + ", " + nestedTrueFalse + ", " + nestedFalse + ", " + constant +
        ", " + mixedTrue + ", " + mixedFalse + ", " + callTrue + ", " + callFalse);
  }
}
""", "Conditionals = 17, 42, 1, 2, 3, 17, 19, 42, 31, 42", 17,
            ),
            "example/Blocks.tasty": (
                "example.Blocks", r"public int ordered\(boolean, int\)", "BlocksRuntime.java",
                """public final class BlocksRuntime {
  public static void main(String[] args) {
    example.Blocks blocks = new example.Blocks();
    int orderedTrue = blocks.ordered(true, 42);
    int orderedFalse = blocks.ordered(false, 42);
    int single = blocks.single();
    int calls = blocks.calls();
    if (orderedTrue != 42 || orderedFalse != 42 || single != 7 || calls != 3)
      throw new AssertionError("unexpected Blocks results");
    System.out.println("Blocks = " + orderedTrue + ", " + orderedFalse + ", " + single + ", " + calls);
  }
}
""", "Blocks = 42, 42, 7, 3", 3,
            ),
            "example/Mutable.tasty": (
                "example.Mutable", r"public int overwrite\(\)", "MutableRuntime.java",
                """public final class MutableRuntime {
  public static void main(String[] args) {
    example.Mutable values = new example.Mutable();
    int overwrite = values.overwrite();
    int increment = values.increment(41);
    int sequential = values.sequential();
    int overflow = values.signed(Integer.MAX_VALUE);
    int negative = values.signed(-1);
    if (overwrite != 7 || increment != 42 || sequential != 8 ||
        overflow != Integer.MIN_VALUE || negative != 0)
      throw new AssertionError("mutable local assignment results were not preserved");
    System.out.println("Mutable = " + overwrite + ", " + increment + ", " +
        sequential + ", " + overflow + ", " + negative);
  }
}
""", "Mutable = 7, 42, 8, -2147483648, 0", 42,
            ),
            "example/Locals.tasty": (
                "example.Locals", r"public int copy\(int\)", "LocalsRuntime.java",
                """public final class LocalsRuntime {
  public static void main(String[] args) {
    example.Locals locals = new example.Locals();
    int copy = locals.copy(17);
    int literal = locals.literal();
    int call = locals.call(23);
    int multiple = locals.multiple(31);
    if (copy != 17 || literal != 42 || call != 23 || multiple != 7)
      throw new AssertionError("unexpected Locals results");
    System.out.println("Locals = " + copy + ", " + literal + ", " + call + ", " + multiple);
  }
}
""", "Locals = 17, 42, 23, 7", 42,
            ),
            "example/Decisions.tasty": (
                "example.Decisions", r"public boolean enabled\(\)", "DecisionsRuntime.java",
                """public final class DecisionsRuntime {
  public static void main(String[] args) {
    example.Decisions decisions = new example.Decisions();
    decisions.unit();
    boolean enabled = decisions.enabled();
    boolean disabled = decisions.disabled();
    int chooseTrue = decisions.choose(true, 11, 22);
    int chooseFalse = decisions.choose(false, 11, 22);
    int nestedTrueTrue = decisions.nested(true, true);
    int nestedTrueFalse = decisions.nested(true, false);
    int nestedFalse = decisions.nested(false, true);
    int storedTrue = decisions.stored(true, 17);
    int storedFalse = decisions.stored(false, 17);
    if (!enabled || disabled || chooseTrue != 11 || chooseFalse != 22 ||
        nestedTrueTrue != 1 || nestedTrueFalse != 2 || nestedFalse != 3 ||
        storedTrue != 17 || storedFalse != 0)
      throw new AssertionError("unexpected Decisions results");
    System.out.println("Decisions = " + enabled + ", " + disabled + ", " +
        chooseTrue + ", " + chooseFalse + ", " + nestedTrueTrue + ", " +
        nestedTrueFalse + ", " + nestedFalse + ", " + storedTrue + ", " + storedFalse);
  }
}
""", "Decisions = true, false, 11, 22, 1, 2, 3, 17, 0", 17,
            ),
            "example/References.tasty": (
                "example.References", r"public int first\(\)", "ReferencesRuntime.java",
                """public final class ReferencesRuntime {
  public static void main(String[] args) {
    int actual = new example.References().first();
    if (actual != 42) throw new AssertionError("References.first() = " + actual);
    System.out.println("References.first() = " + actual);
  }
}
""", "References.first() = 42", 42,
            ),
            "example/Operations.tasty": (
                "example.Operations", r"public int (use\(\)|identity\(int\)|first\(\)|zero\(\))", "OperationsRuntime.java",
                """public final class OperationsRuntime {
  public static void main(String[] args) {
    example.Operations operations = new example.Operations();
    int use = operations.use();
    int identity = operations.identity(17);
    int first = operations.first();
    int zero = operations.zero();
    if (use != 42) throw new AssertionError("Operations.use() = " + use);
    if (identity != 17) throw new AssertionError("Operations.identity(17) = " + identity);
    if (first != 7) throw new AssertionError("Operations.first() = " + first);
    if (zero != 0) throw new AssertionError("Operations.zero() = " + zero);
    System.out.println("Operations = " + use + ", " + identity + ", " + first + ", " + zero);
  }
}
""", "Operations = 42, 17, 7, 0", 42,
            ),
            "example/Algorithms.tasty": (
                "example.Algorithms", r"public int sumTo\(int\)", "AlgorithmsRuntime.java",
                """public final class AlgorithmsRuntime {
  public static void main(String[] args) {
    example.Algorithms a = new example.Algorithms();
    if (a.add(2, 3) != 5 || a.add(-2, 3) != 1 ||
        a.add(Integer.MAX_VALUE, 1) != Integer.MIN_VALUE ||
        !a.positive(1) || a.positive(0) || a.positive(-1) ||
        !a.both(true, true) || a.both(true, false) ||
        a.safeAnd() || !a.safeOr() || a.sumTo(0) != 0 ||
        a.sumTo(1) != 0 || a.sumTo(5) != 10 || a.sumTo(-1) != 0)
      throw new AssertionError("unexpected Algorithms results");
    try { a.divisionByZero(); throw new AssertionError("division did not throw"); }
    catch (ArithmeticException expected) { }
    System.out.println("Algorithms = arithmetic, booleans, short-circuit, sumTo(5)=10");
  }
}
""", "Algorithms = arithmetic, booleans, short-circuit, sumTo(5)=10", 10,
            ),
            "example/Counter.tasty": (
                "example.Counter", r"public int increment\(\)", "CounterRuntime.java",
                """public final class CounterRuntime {
  public static void main(String[] args) {
    example.Counter counter = new example.Counter(5);
    if (counter.get() != 5) throw new AssertionError("constructor field initializer");
    counter.set(8);
    if (counter.increment() != 9 || counter.get() != 9)
      throw new AssertionError("mutating getter/setter/increment");
    counter.set(-2);
    if (counter.increment() != -1 || counter.get() != -1)
      throw new AssertionError("negative mutable field state");
    System.out.println("Counter = 5, 9, -1");
  }
}
""", "Counter = 5, 9, -1", 9,
            ),
            "example/Loop.tasty": (
                "example.Loop", r"public int sumTo\(int\)", "LoopRuntime.java",
                """public final class LoopRuntime {
  public static void main(String[] args) {
    example.Loop loop = new example.Loop();
    int zero = loop.sumTo(0);
    int one = loop.sumTo(1);
    int five = loop.sumTo(5);
    int negative = loop.sumTo(-1);
    if (zero != 0 || one != 0 || five != 10 || negative != 0)
      throw new AssertionError("unexpected Loop results");
    System.out.println("Loop = " + zero + ", " + one + ", " + five + ", " + negative);
  }
}
""", "Loop = 0, 0, 10, 0", 10,
            ),
        }
        case = runtime_cases.get(relative.as_posix())
        if case is None:
            raise RuntimeError("--runtime requires a supported example TASTy artifact")
        class_name = case[0]
        javap = execute([toolchain.javap, "-classpath", compiled, class_name])
        if relative.as_posix() == "example/Operations.tasty":
            sprint_two_methods = (
                r"public int use\(\)", r"public int identity\(int\)",
                r"public int first\(\)", r"public int zero\(\)",
            )
            call_methods = (
                r"public int use\(\)", r"public int propagate\(int\)",
                r"public int picks\(\)", r"public int nested\(\)",
            )
            if all(re.search(pattern, javap) for pattern in sprint_two_methods):
                scenario = "sprint-two"
            elif all(re.search(pattern, javap) for pattern in call_methods):
                scenario = "calls"
            else:
                raise RuntimeError(f"fresh Operations class matches no supported runtime profile: {javap}")
        else:
            _, method_pattern, *_ = case
            if not re.search(method_pattern, javap):
                raise RuntimeError(f"fresh class does not have the expected method: {javap}")
            if relative.as_posix() == "example/Flags.tasty":
                signatures = (
                    r"public boolean enabled\(\)", r"public boolean disabled\(\)",
                    r"public boolean passthrough\(boolean\)", r"public int keep\(boolean, int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Flags class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Conditionals.tasty":
                signatures = (
                    r"public int choose\(boolean, int, int\)",
                    r"public int nested\(boolean, boolean\)",
                    r"public int constant\(\)",
                    r"public int mixed\(boolean, int\)",
                    r"public int callBranch\(boolean, int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Conditionals class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Blocks.tasty":
                signatures = (
                    r"public int ordered\(boolean, int\)",
                    r"public int single\(\)",
                    r"public int calls\(\)",
                    r"public int identity\(int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Blocks class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Mutable.tasty":
                signatures = (
                    r"public int overwrite\(\)", r"public int increment\(int\)",
                    r"public int sequential\(\)", r"public int signed\(int\)",
                    r"public int immutableTarget\(\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Mutable class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Locals.tasty":
                signatures = (
                    r"public int copy\(int\)", r"public int literal\(\)",
                    r"public int call\(int\)", r"public int multiple\(int\)",
                    r"public int identity\(int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Locals class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Decisions.tasty":
                signatures = (
                    r"public void unit\(\)", r"public boolean enabled\(\)",
                    r"public boolean disabled\(\)",
                    r"public int choose\(boolean, int, int\)",
                    r"public int nested\(boolean, boolean\)",
                    r"public int stored\(boolean, int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Decisions class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Algorithms.tasty":
                signatures = (
                    r"public int add\(int, int\)", r"public boolean positive\(int\)",
                    r"public boolean both\(boolean, boolean\)",
                    r"public boolean safeAnd\(\)",
                    r"public boolean safeOr\(\)",
                    r"public boolean divisionByZero\(\)",
                    r"public int sumTo\(int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Algorithms class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Counter.tasty":
                signatures = (
                    r"public example\.Counter\(int\)",
                    r"public int get\(\)", r"public void set\(int\)",
                    r"public int increment\(\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Counter class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Arithmetic.tasty":
                signatures = (
                    r"public int add\(int, int\)", r"public int subtract\(int, int\)",
                    r"public int multiply\(int, int\)", r"public int divide\(int, int\)",
                    r"public int remainder\(int, int\)",
                    r"public int nested\(int, int, int\)", r"public int throughCall\(int, int\)",
                    r"public int local\(int, int\)", r"public int compute\(int, int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Arithmetic class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Comparisons.tasty":
                signatures = (
                    r"public boolean positive\(int\)", r"public boolean before\(int, int\)",
                    r"public boolean atMost\(int, int\)", r"public boolean atLeast\(int, int\)",
                    r"public boolean nested\(int, int\)", r"public int choose\(int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Comparisons class has unexpected signatures: {javap}")
            if relative.as_posix() == "example/Logic.tasty":
                signatures = (
                    r"public boolean negate\(boolean\)",
                    r"public boolean both\(boolean, boolean\)",
                    r"public boolean either\(boolean, boolean\)",
                    r"public boolean safeAnd\(\)",
                    r"public boolean safeOr\(\)",
                    r"public boolean evaluatedAnd\(boolean\)",
                    r"public boolean evaluatedOr\(boolean\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Logic class has unexpected signatures: {javap}")
            scenario = "standard"

        if scenario == "calls":
            harness_name = "OperationsCallsRuntime.java"
            harness_source = """public final class OperationsCallsRuntime {
  public static void main(String[] args) {
    example.Operations operations = new example.Operations();
    int use = operations.use();
    int propagated = operations.propagate(17);
    int picked = operations.picks();
    int nested = operations.nested();
    if (use != 42) throw new AssertionError("Operations.use() = " + use);
    if (propagated != 17) throw new AssertionError("Operations.propagate(17) = " + propagated);
    if (picked != 42) throw new AssertionError("Operations.picks() = " + picked);
    if (nested != 42) throw new AssertionError("Operations.nested() = " + nested);
    System.out.println("Operations = " + use + ", " + propagated + ", " + picked + ", " + nested);
  }
}
"""
            expected_output = "Operations = 42, 17, 42, 42"
            expected_value = 42
        else:
            _, _, harness_name, harness_source, expected_output, expected_value = case
        harness = directory / harness_name
        harness.write_text(harness_source)
        execute([toolchain.javac, "-classpath", compiled, "-d", compiled, harness])
        runtime_class = harness.stem
        run = execute([toolchain.java, "-XX:-UsePerfData", "-classpath", compiled, runtime_class])
        if expected_output not in run:
            raise RuntimeError(f"runtime harness did not confirm {expected_output}: {run}")
        bad = directory / "corrupt.tasty"
        corrupt = bytearray(candidate.read_bytes())
        corrupt[0] ^= 1
        bad.write_bytes(corrupt)
        expect_failure(lambda: snapshot(binary, bad), "Rust rejects corrupted producer output")
        rejected = directory / "corrupt-output"
        rejected.mkdir()
        expect_failure(lambda: toolchain.compiler(["-from-tasty", "-d", rejected, bad]),
                       "Scala rejects corrupted producer output")
        truncated = directory / "truncated.tasty"
        truncated.write_bytes(candidate.read_bytes()[:-1])
        expect_failure(lambda: snapshot(binary, truncated), "Rust rejects truncated producer output")
        truncated_output = directory / "truncated-output"
        truncated_output.mkdir()
        expect_failure(lambda: toolchain.compiler(["-from-tasty", "-d", truncated_output, truncated]),
                       "Scala rejects truncated producer output")
        result.update({"javap": True, "runtime_value": expected_value,
                       "runtime_profile": scenario,
                       "rust_corrupt_rejection": True, "scala_corrupt_rejection": True,
                       "rust_truncated_rejection": True, "scala_truncated_rejection": True})
    return result


def expect_failure(action, label: str) -> None:
    try:
        action()
    except CommandFailure:
        return
    raise RuntimeError(f"negative control unexpectedly succeeded: {label}")


def reject_source(toolchain: Toolchain, directory: Path, name: str,
                  source: str, diagnostic: str) -> None:
    case = directory / "source-negative" / name
    case.mkdir(parents=True)
    source_file = case / f"{name}.scala"
    source_file.write_text(source)
    output = case / "classes"
    output.mkdir()
    try:
        toolchain.compiler(["-d", output, source_file])
    except CommandFailure as error:
        if not re.search(diagnostic, str(error), re.IGNORECASE):
            raise RuntimeError(f"{name} failed for an unexpected reason: {error}") from error
        return
    raise RuntimeError(f"negative source unexpectedly compiled: {name}")


def check_oracle(toolchain: Toolchain, binary: Path, directory: Path) -> list[dict]:
    verify_provenance()
    generated = directory / "regenerated"
    generated.mkdir()
    generate(toolchain, binary, generated, check=True)
    results = []
    for name, source, diagnostic in [
        ("DuplicateField", "class DuplicateField { private val item: Int = 1; private val item: Int = 2 }", "already defined|duplicate"),
        ("MissingInitializer", "class MissingInitializer { private val item: Int }", "not defined|abstract"),
        ("WrongInitializer", "class WrongInitializer { private val item: Int = true }", "Found:|Required:|type mismatch"),
    ]:
        reject_source(toolchain, directory, name, source, diagnostic)
        results.append({"negative_source": name, "scala_compile_rejected": True})
    algorithm_classes = directory / "algorithm-source-reference"
    algorithm_classes.mkdir()
    toolchain.compiler(["-d", algorithm_classes, FIXTURES / "AlgorithmsReference.scala"])
    algorithm_harness = directory / "AlgorithmsSourceRuntime.java"
    algorithm_harness.write_text("""public final class AlgorithmsSourceRuntime {
  public static void main(String[] args) {
    example.AlgorithmsReference a = new example.AlgorithmsReference();
    if (a.shortCircuitAnd() || !a.shortCircuitOr() || a.sumTo(5) != 10)
      throw new AssertionError("Scala source reference failed");
    try { a.eagerAndEffect(); throw new AssertionError("& skipped its RHS"); }
    catch (ArithmeticException expected) { }
    try { a.eagerOrEffect(); throw new AssertionError("| skipped its RHS"); }
    catch (ArithmeticException expected) { }
    System.out.println("Algorithms reference = short-circuit, eager, sumTo(5)=10");
  }
}
""")
    execute([toolchain.javac, "-classpath", algorithm_classes, "-d", algorithm_classes,
             algorithm_harness])
    algorithm_source_run = execute([toolchain.java, "-XX:-UsePerfData", "-classpath",
                                    algorithm_classes, "AlgorithmsSourceRuntime"])
    expected_algorithm_source = "Algorithms reference = short-circuit, eager, sumTo(5)=10"
    if expected_algorithm_source not in algorithm_source_run:
        raise RuntimeError(f"pinned Scala Algorithms source returned unexpected values: {algorithm_source_run}")
    results.append({"source_reference": "example.AlgorithmsReference", "scala_source_compile": True,
                    "source_runtime_output": expected_algorithm_source})
    # Independently compile and execute the original Scala source. This is a
    # source-level reference: it does not consume the checked-in TASTy fixture.
    source_classes = directory / "source-reference"
    source_classes.mkdir()
    toolchain.compiler(["-d", source_classes, FIXTURES / "Decisions.scala"])
    source_harness = directory / "DecisionsSourceRuntime.java"
    source_harness.write_text("""public final class DecisionsSourceRuntime {
  public static void main(String[] args) {
    example.Decisions decisions = new example.Decisions();
    decisions.unit();
    boolean enabled = decisions.enabled();
    boolean disabled = decisions.disabled();
    int chooseTrue = decisions.choose(true, 11, 22);
    int chooseFalse = decisions.choose(false, 11, 22);
    int nestedTrueTrue = decisions.nested(true, true);
    int nestedTrueFalse = decisions.nested(true, false);
    int nestedFalse = decisions.nested(false, true);
    int storedTrue = decisions.stored(true, 17);
    int storedFalse = decisions.stored(false, 17);
    if (!enabled || disabled || chooseTrue != 11 || chooseFalse != 22 ||
        nestedTrueTrue != 1 || nestedTrueFalse != 2 || nestedFalse != 3 ||
        storedTrue != 17 || storedFalse != 0) {
      throw new AssertionError("unexpected Scala source results");
    }
    System.out.println("Decisions source = " + enabled + ", " + disabled + ", " +
        chooseTrue + ", " + chooseFalse + ", " + nestedTrueTrue + ", " +
        nestedTrueFalse + ", " + nestedFalse + ", " + storedTrue + ", " + storedFalse);
  }
}
""")
    execute([toolchain.javac, "-classpath", source_classes, "-d", source_classes, source_harness])
    source_run = execute([toolchain.java, "-XX:-UsePerfData", "-classpath", source_classes,
                          "DecisionsSourceRuntime"])
    expected_source_output = "Decisions source = true, false, 11, 22, 1, 2, 3, 17, 0"
    if expected_source_output not in source_run:
        raise RuntimeError(f"pinned Scala source reference returned unexpected values: {source_run}")
    results.append({"source_reference": "example.Decisions", "scala_source_compile": True,
                    "source_runtime_output": expected_source_output})
    for candidate in sorted((FIXTURES / "tasty").rglob("*.tasty")):
        results.append(verify_candidate(toolchain, binary, candidate,
                                        directory / f"verify-{candidate.stem}",
                                        runtime=candidate.name in {"Algorithms.tasty", "Counter.tasty"}))
    answer = FIXTURES / "tasty/example/Answer.tasty"
    reduced = directory / "Reduced.tasty"
    execute([binary, "reduce-answer", answer, reduced])
    result = verify_candidate(toolchain, binary, reduced, directory / "reduced")
    result["omits_positions_comments_and_sourcefile_annotation"] = True
    results.append(result)
    corrupt = directory / "corrupt.tasty"
    data = bytearray(answer.read_bytes())
    data[0] ^= 1
    corrupt.write_bytes(data)
    expect_failure(lambda: snapshot(binary, corrupt), "Rust rejects invalid magic")
    # Also exercise the independent compiler rejection itself, not just Rust's
    # validator. This is deliberately outside the normal validate-first pipeline.
    bad_wire_output = directory / "invalid-wire-output"
    bad_wire_output.mkdir()
    expect_failure(lambda: toolchain.compiler(["-from-tasty", "-d", bad_wire_output, corrupt]),
                   "Scala rejects invalid magic")
    misplaced = directory / "wrong-layout/Answer.tasty"
    misplaced.parent.mkdir()
    shutil.copyfile(answer, misplaced)
    bad_output = directory / "wrong-layout-output"
    bad_output.mkdir()
    expect_failure(lambda: toolchain.compiler(["-from-tasty", "-d", bad_output, misplaced]),
                   "Scala rejects missing example/ package layout")
    print("Negative controls passed: Rust/Scala invalid wire; Scala wrong package layout")
    return results


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    generation = subcommands.add_parser("generate", help="explicitly record/check independent fixtures")
    generation.add_argument("--check", action="store_true", help="compare regenerated structure without writing fixtures")
    verify = subcommands.add_parser("verify", help="validate and independently compile a candidate")
    verify.add_argument("candidate", type=Path)
    verify.add_argument("--runtime", action="store_true",
                        help="inspect, decompile, compile, and execute a supported fresh fixture class")
    subcommands.add_parser("check", help="regenerate/check fixtures, compile originals/reduced Answer, run negative controls")
    args = parser.parse_args()
    binary = inspector()
    structure = snapshot(binary, args.candidate) if args.command == "verify" else None
    toolchain = Toolchain()
    with tempfile.TemporaryDirectory(prefix="tasty-pickler-oracle-") as temporary:
        output = Path(temporary)
        if args.command == "generate":
            generate(toolchain, binary, output, args.check)
        elif args.command == "verify":
            print(json.dumps(verify_candidate(toolchain, binary, args.candidate,
                                             output, structure, runtime=args.runtime), indent=2, sort_keys=True))
        else:
            print(json.dumps(check_oracle(toolchain, binary, output), indent=2, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError) as error:
        print(f"tasty-pickler-oracle: {error}", file=sys.stderr)
        sys.exit(1)
