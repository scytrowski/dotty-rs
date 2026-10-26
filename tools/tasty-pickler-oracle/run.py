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
import zipfile


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

    def __init__(self, message: str, output: str = "") -> None:
        super().__init__(message)
        self.output = output


def execute(args: list[str | Path], *, cwd: Path = ROOT) -> str:
    try:
        result = subprocess.run([str(a) for a in args], cwd=cwd, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=180)
    except subprocess.TimeoutExpired as error:
        # A timeout is a harness failure, never an expected reader rejection.
        raise RuntimeError(f"command timed out after 180 seconds: {args}") from error
    if result.returncode:
        raise CommandFailure(f"command failed ({result.returncode}): {args}\n{result.stdout}",
                             result.stdout)
    return result.stdout


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _class_tasty_link(class_bytes: bytes) -> bytes | None:
    """Read the class-level TASTY attribute from a bounded classfile."""
    offset = 0

    def take(length: int) -> bytes:
        nonlocal offset
        end = offset + length
        if length < 0 or end > len(class_bytes):
            raise RuntimeError("truncated classfile while reading TASTY link")
        result = class_bytes[offset:end]
        offset = end
        return result

    def u1() -> int:
        return take(1)[0]

    def u2() -> int:
        return int.from_bytes(take(2), "big")

    def u4() -> int:
        return int.from_bytes(take(4), "big")

    if take(4) != b"\xca\xfe\xba\xbe":
        raise RuntimeError("invalid classfile magic while reading TASTY link")
    take(4)  # minor and major version
    cp_count = u2()
    utf8: dict[int, bytes] = {}
    index = 1
    while index < cp_count:
        tag = u1()
        if tag == 1:
            utf8[index] = take(u2())
        elif tag in (3, 4):
            take(4)
        elif tag in (5, 6):
            take(8)
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            take(2)
        elif tag in (9, 10, 11, 12, 17, 18):
            take(4)
        elif tag == 15:
            take(3)
        else:
            raise RuntimeError(f"unknown classfile constant-pool tag {tag}")
        index += 1

    take(6)  # access flags, this class, super class
    for _ in range(u2()):
        take(2)  # interfaces

    def skip_members() -> None:
        for _ in range(u2()):
            take(6)  # access flags, name, descriptor
            for _ in range(u2()):
                take(2)
                take(u4())

    skip_members()  # fields
    skip_members()  # methods
    links = []
    for _ in range(u2()):
        name_index = u2()
        payload = take(u4())
        if utf8.get(name_index) == b"TASTY":
            links.append(payload)
    if offset != len(class_bytes):
        raise RuntimeError("trailing bytes after classfile attributes")
    if len(links) > 1:
        raise RuntimeError("classfile has duplicate class-level TASTY attributes")
    return links[0] if links else None


def _tasty_header_uuid(tasty_bytes: bytes) -> bytes:
    """Decode the pinned TASTy header fields needed to authenticate a link."""
    offset = 0

    def take(length: int) -> bytes:
        nonlocal offset
        end = offset + length
        if length < 0 or end > len(tasty_bytes):
            raise RuntimeError("truncated TASTy header")
        value = tasty_bytes[offset:end]
        offset = end
        return value

    def nat() -> int:
        value = 0
        while True:
            byte = take(1)[0]
            value = (value << 7) | (byte & 0x7f)
            if byte & 0x80 != 0:
                return value

    if take(4) != bytes.fromhex("5ca1ab1f"):
        raise RuntimeError("invalid TASTy magic")
    major, minor, experimental = nat(), nat(), nat()
    tooling_length = nat()
    take(tooling_length)
    uuid = take(16)
    if major != 28 or minor != 9 or experimental != 0:
        raise RuntimeError(f"unexpected TASTy header version {major}.{minor}.{experimental}")
    return uuid


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
        self.runtime_jars = [p for p in jars if p.name in
                             ("scala3-library_3-3.9.0.jar", "scala-library-3.9.0.jar")]
        self.runtime_classpath = os.pathsep.join(str(p) for p in self.runtime_jars)
        self.artifacts = {p.name: digest(p) for p in jars}
        pinned = json.loads((FIXTURES / "provenance.json").read_text())
        if self.artifacts != pinned["compiler_artifacts_sha256"]:
            raise RuntimeError("compiler artifacts differ from pinned SHA-256 provenance")
        version = self.compiler(["-version"])
        if not re.search(r"Scala compiler version 3\.9\.0 --", version):
            raise RuntimeError(f"expected Scala 3.9.0 compiler, got: {version}")

    def compiler(self, args: list[str | Path], *, decompile: bool = False,
                 classpath: str | None = None, cwd: Path = ROOT) -> str:
        main = "dotty.tools.dotc.decompiler.Main" if decompile else "dotty.tools.dotc.Main"
        compile_classpath = self.classpath if classpath is None else classpath
        return execute([self.java, "-XX:-UsePerfData", "-cp", self.classpath, main,
                        "-classpath", compile_classpath, "-color:never", *args], cwd=cwd)


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
            "def seedBlockBranch(",
            "def seedBlockAfterLocal(",
        )
        if not all(member in decompiled for member in expected_members):
            raise RuntimeError(f"fresh PrivateFields TASTy lost a field or method: {decompiled}")
    if relative.as_posix() == "example/MutableFieldWrites.tasty":
        expected_members = (
            "private[this] var current: scala.Int = 0",
            "private[this] var enabled: scala.Boolean = false",
            "def get(): scala.Int =",
            "def set(value: scala.Int): scala.Unit =",
            "def isEnabled(): scala.Boolean =",
            "def setEnabled(",
            "def increment(): scala.Int =",
        )
        if not all(member in decompiled for member in expected_members):
            raise RuntimeError(f"fresh Mutable TASTy lost a field or method: {decompiled}")
    if relative.as_posix() == "example/Counter.tasty":
        expected_members = (
            "private[this] var current: scala.Int = start",
            "def get(): scala.Int = Counter.this.current",
            "def set(value: scala.Int): scala.Unit = Counter.this.current = value",
            "def increment(): scala.Int =",
        )
        if not all(member in decompiled for member in expected_members):
            raise RuntimeError(f"fresh Counter TASTy lost its field or methods: {decompiled}")
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
            "example/Box.tasty": (
                "example.Box", r"public int answer\(\)", "BoxRuntime.java",
                """public final class BoxRuntime {
  public static void main(String[] args) {
    int actual = new example.Box(17).answer();
    if (actual != 42) throw new AssertionError("Box.answer() = " + actual);
    System.out.println("Box(17).answer() = " + actual);
  }
}
""", "Box(17).answer() = 42", 42,
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
        values.seedParenBranch(true) != 42 || values.seedParenBranch(false) != 0 ||
        values.seedBlockBranch(true) != 42 || values.seedBlockBranch(false) != 0 ||
        values.seedBlockAfterLocal(true) != 42 || values.seedBlockAfterLocal(false) != 0)
      throw new AssertionError("PrivateFields field reads or shadowing changed");
    System.out.println("PrivateFields field reads and shadowing passed");
  }
}
""", "PrivateFields field reads and shadowing passed", 1,
            ),
            "example/MutableFieldWrites.tasty": (
                "example.MutableFieldWrites", r"public int get\(\)",
                "MutableRuntime.java",
                """public final class MutableRuntime {
  public static void main(String[] args) {
    example.MutableFieldWrites first = new example.MutableFieldWrites();
    example.MutableFieldWrites second = new example.MutableFieldWrites();
    if (first.get() != 0 || second.get() != 0 || first.isEnabled() || second.isEnabled())
      throw new AssertionError("Mutable initial field state changed");
    first.set(8);
    first.setEnabled(true);
    if (first.get() != 8 || second.get() != 0 || !first.isEnabled() || second.isEnabled())
      throw new AssertionError("Mutable.set changed the wrong instance");
    if (first.increment() != 9 || first.get() != 9 || second.get() != 0
        || !first.isEnabled() || second.isEnabled())
      throw new AssertionError("Mutable.increment changed the wrong instance");
    System.out.println("Mutable set/get/increment and instance state passed");
  }
}
""", "Mutable set/get/increment and instance state passed", 1,
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
    example.Counter a = new example.Counter(4);
    example.Counter b = new example.Counter(10);
    if (a.get() != 4 || b.get() != 10)
      throw new AssertionError("constructor field initializer or independent instances");
    a.set(8);
    if (a.get() != 8 || b.get() != 10)
      throw new AssertionError("setter changed the wrong instance");
    if (a.increment() != 9 || a.get() != 9 || b.get() != 10)
      throw new AssertionError("increment changed the wrong instance");
    if (b.increment() != 11 || b.get() != 11 || a.get() != 9)
      throw new AssertionError("instances do not retain independent state");
    example.Counter negative = new example.Counter(-2);
    if (negative.get() != -2)
      throw new AssertionError("negative constructor input was not retained");
    System.out.println("Counter = independent state, 9, 11, -2");
  }
}
""", "Counter = independent state, 9, 11, -2", 9,
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
            if relative.as_posix() == "example/Box.tasty":
                signatures = (r"public example\.Box\(int\)", r"public int answer\(\)")
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Box class has unexpected signatures: {javap}")
                if re.search(r"public (?:final )?int start\(\)", javap):
                    raise RuntimeError(f"plain constructor input became a public accessor: {javap}")
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


def consume_rust_pair(toolchain: Toolchain, pair_root: Path, tasty_link_uuid: str) -> dict:
    """Compile a Scala client against a Rust linked pair and prove TASTy lookup."""
    expected = {
        "fresh-a": ("example/Counter.class", "example/Counter.tasty"),
        "fresh-b": ("example/Counter.class", "example/Counter.tasty"),
        "with-tasty-no-get": ("example/Counter.class", "example/Counter.tasty"),
        "classfile-only-no-get": ("example/Counter.class",),
        "uuid-mismatch": ("example/Counter.class", "example/Counter.tasty"),
        "unrecognized-link": ("example/Counter.class", "example/Counter.tasty"),
    }
    for name, files in expected.items():
        root = pair_root / name
        actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
        if actual != set(files):
            raise RuntimeError(f"{name} artifact inventory differs: {sorted(actual)}")

    first = pair_root / "fresh-a"
    second = pair_root / "fresh-b"
    try:
        link_bytes = bytes.fromhex(tasty_link_uuid)
    except ValueError as error:
        raise RuntimeError("TASTy link UUID must be 32 hexadecimal characters") from error
    if len(link_bytes) != 16:
        raise RuntimeError("TASTy link UUID must be 32 hexadecimal characters")
    for relative in expected["fresh-a"]:
        if digest(first / relative) != digest(second / relative):
            raise RuntimeError(f"fresh Rust pair bytes differ: {relative}")
    for pair in (first, second):
        class_bytes = (pair / "example/Counter.class").read_bytes()
        tasty_bytes = (pair / "example/Counter.tasty").read_bytes()
        class_link = _class_tasty_link(class_bytes)
        if class_link != link_bytes or _tasty_header_uuid(tasty_bytes) != class_link:
            raise RuntimeError("Rust classfile TASTY link does not match its TASTy header UUID")

    semantic_pair = pair_root / "with-tasty-no-get"
    semantic_link = _class_tasty_link((semantic_pair / "example/Counter.class").read_bytes())
    if semantic_link is None or _tasty_header_uuid(
            (semantic_pair / "example/Counter.tasty").read_bytes()) != semantic_link:
        raise RuntimeError("TASTy semantic control is missing a valid matching class-level link")

    unrecognized = pair_root / "unrecognized-link"
    try:
        unrecognized_link = _class_tasty_link((unrecognized / "example/Counter.class").read_bytes())
        if unrecognized_link is None or _tasty_header_uuid(
                (unrecognized / "example/Counter.tasty").read_bytes()) != unrecognized_link:
            raise RuntimeError("classfile is missing a matching recognized TASTY attribute")
    except RuntimeError as error:
        if "TASTY" not in str(error):
            raise
    else:
        raise RuntimeError("unrecognized TASTY link negative control unexpectedly passed")

    consumer = """package consumer
import example.Counter

object Consumer:
  def main(args: Array[String]): Unit = run()
  def run(): Unit =
    val first = new Counter(start = 7)
    val second = new Counter(start = 2)
    println(first.get())
    println(second.get())
    first.set(value = 13)
    second.set(value = -42)
    println(first.get())
    println(second.get())
    first.set(value = -42)
    println(first.get())
    println(second.get())
"""
    probe = """package consumer
import example.Counter

object TastyProbe:
  def read(counter: Counter): Int = counter.get()
"""
    with tempfile.TemporaryDirectory(prefix="dotty-scala-consumer-") as temporary:
        work = Path(temporary)
        source = work / "src/Consumer.scala"
        source.parent.mkdir()
        source.write_text(consumer)
        probe_source = work / "src/TastyProbe.scala"
        probe_source.write_text(probe)

        for jar in (Path(path) for path in toolchain.classpath.split(os.pathsep)):
            with zipfile.ZipFile(jar) as archive:
                if "example/Counter.class" in archive.namelist():
                    raise RuntimeError(f"Scala compiler/runtime dependency contains Counter.class: {jar}")

        fresh_runtime_outputs = []
        artifact_hashes = {}
        for index, pair in enumerate((first, second)):
            client = work / f"client-{index}"
            client.mkdir()
            class_before = digest(pair / "example/Counter.class")
            artifact_hashes[pair.name] = {
                "class_sha256": class_before,
                "tasty_sha256": digest(pair / "example/Counter.tasty"),
            }
            toolchain.compiler(["-d", client, "-classpath", os.pathsep.join(
                (str(pair), toolchain.classpath)), source])
            consumer_class = client / "consumer/Consumer.class"
            consumer_module = client / "consumer/Consumer$.class"
            if not consumer_class.is_file() or not consumer_module.is_file():
                raise RuntimeError("Scala consumer compilation produced no expected client classes")
            if list(client.rglob("Counter.class")):
                raise RuntimeError("Scala compiler emitted or copied Counter.class into client output")
            if digest(pair / "example/Counter.class") != class_before:
                raise RuntimeError("Scala consumer compilation changed the staged Rust Counter.class")

            javap = execute([toolchain.javap, "-classpath",
                             os.pathsep.join((str(client), str(pair))), "-verbose",
                             "consumer.Consumer$"])
            for reference in ('example/Counter."<init>":(I)V', "example/Counter.set:(I)V",
                              "example/Counter.get:()I"):
                if reference not in javap:
                    raise RuntimeError(f"consumer bytecode lacks Methodref {reference}: {javap}")

            runtime = execute([toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                               os.pathsep.join((str(pair), str(client), toolchain.classpath)),
                               "consumer.Consumer"])
            result = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if result != ["7", "2", "13", "-42", "-42", "-42"]:
                raise RuntimeError(f"Scala consumer returned an unexpected runtime result: {runtime}")
            if digest(pair / "example/Counter.class") != class_before:
                raise RuntimeError("runtime did not use the unchanged Rust Counter.class")
            fresh_runtime_outputs.extend(result)

        semantic_output = work / "tasty-probe-output"
        semantic_output.mkdir()
        semantic = pair_root / "with-tasty-no-get"
        toolchain.compiler(["-d", semantic_output, "-classpath", os.pathsep.join(
            (str(semantic), toolchain.classpath)), probe_source])
        if not (semantic_output / "consumer/TastyProbe$.class").is_file():
            raise RuntimeError("TASTy semantic control produced no probe class")

        classfile_only_output = work / "classfile-only-output"
        classfile_only_output.mkdir()
        try:
            toolchain.compiler(["-d", classfile_only_output, "-classpath", os.pathsep.join(
                (str(pair_root / "classfile-only-no-get"), toolchain.classpath)), probe_source])
        except CommandFailure as error:
            classfile_only_failure = error.output
        else:
            raise RuntimeError("classfile-only control unexpectedly resolved the TASTy-only get method")
        if not re.search(r"(?i)(get.*(not found|not a member)|no member.*get)",
                         classfile_only_failure):
            raise RuntimeError(f"classfile-only control failed for an unrelated reason: {classfile_only_failure}")

        mismatch_output = work / "uuid-mismatch-output"
        mismatch_output.mkdir()
        try:
            toolchain.compiler(["-Werror", "-d", mismatch_output, "-classpath", os.pathsep.join(
                (str(pair_root / "uuid-mismatch"), toolchain.classpath)), probe_source])
        except CommandFailure as error:
            mismatch_failure = error.output
        else:
            raise RuntimeError("Scala compiler accepted the mismatched TASTy UUID with -Werror")
        if not re.search(r"(?i)(tasty.*(uuid|identifier)|(uuid|identifier).*tasty)", mismatch_failure):
            raise RuntimeError(f"UUID control failed for an unrelated reason: {mismatch_failure}")

        classfile_diagnostic = next(
            line.strip() for line in classfile_only_failure.splitlines()
            if re.search(r"(?i)(get.*(not found|not a member)|no member.*get)", line)
        )
        uuid_diagnostic = next(
            line.strip() for line in mismatch_failure.splitlines()
            if re.search(r"(?i)(tasty.*(uuid|identifier)|(uuid|identifier).*tasty)", line)
        )

        return {
            "scala_consumer": True,
            "fresh_runtime_outputs": fresh_runtime_outputs,
            "fresh_pair_hashes": artifact_hashes,
            "counter_source_sha256": digest(ROOT / "tests/fixtures/jvm_tasty_interop/Counter.scala"),
            "rust_class_sha256": class_before,
            "fresh_pair_byte_identical": True,
            "tasty_link_uuid": tasty_link_uuid,
            "consumer_methodrefs": ["<init>(I)V", "set(I)V", "get()I"],
            "tasty_only_method_resolved": True,
            "classfile_only_control_rejected": True,
            "unrecognized_link_control_rejected": True,
            "uuid_mismatch_control_rejected": True,
            "classfile_only_diagnostic": classfile_diagnostic,
            "uuid_mismatch_diagnostic": uuid_diagnostic,
            "java_version": toolchain.jdk_release["JAVA_VERSION"],
            "compiler_artifacts_sha256": toolchain.artifacts,
        }


def consume_rust_batch(toolchain: Toolchain, batch_root: Path) -> dict:
    """Consume a complete Rust Counter/Gauge batch and prove each TASTy sidecar is used."""
    relative_paths = {
        "example/Counter.class",
        "example/Counter.tasty",
        "example/Gauge.class",
        "example/Gauge.tasty",
    }
    roots = {"fresh-a": relative_paths, "fresh-b": relative_paths}
    for class_name in ("Counter", "Gauge"):
        class_file = f"example/{class_name}.class"
        tasty_file = f"example/{class_name}.tasty"
        roots[f"tasty-only-{class_name}"] = relative_paths
        roots[f"uuid-mismatch-{class_name}"] = relative_paths
        roots[f"classfile-only-{class_name}"] = relative_paths - {tasty_file}
        roots[f"missing-sidecar-{class_name}"] = relative_paths - {tasty_file}
        roots[f"xasty-{class_name}"] = relative_paths - {tasty_file}
        roots[f"missing-marker-{class_name}"] = relative_paths - {tasty_file}
        roots[f"missing-classfile-{class_name}"] = relative_paths - {class_file}
        roots[f"wrong-path-{class_name}"] = (
            relative_paths - {tasty_file} | {f"misplaced/{class_name}.tasty"}
        )
        roots[f"corrupt-sidecar-{class_name}"] = relative_paths

    for name, expected in roots.items():
        root = batch_root / name
        actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
        if actual != expected:
            raise RuntimeError(f"{name} artifact inventory differs: {sorted(actual)}")

    first = batch_root / "fresh-a"
    second = batch_root / "fresh-b"
    hashes: dict[str, dict[str, str]] = {}
    links: dict[str, str] = {}
    for class_name in ("Counter", "Gauge"):
        class_path = f"example/{class_name}.class"
        tasty_path = f"example/{class_name}.tasty"
        first_class = first / class_path
        first_tasty = first / tasty_path
        if first_class.read_bytes()[6:8] != (65).to_bytes(2, "big"):
            raise RuntimeError(f"Rust {class_name}.class is not classfile major 65")
        class_link = _class_tasty_link(first_class.read_bytes())
        tasty_link = _tasty_header_uuid(first_tasty.read_bytes())
        if class_link is None or class_link != tasty_link:
            raise RuntimeError(f"Rust {class_name} classfile and TASTy UUIDs do not match")
        if digest(first_class) != digest(second / class_path):
            raise RuntimeError(f"fresh Rust classfile bytes differ: {class_path}")
        if digest(first_tasty) != digest(second / tasty_path):
            raise RuntimeError(f"fresh Rust TASTy bytes differ: {tasty_path}")
        hashes[class_name] = {
            "class_sha256": digest(first_class),
            "tasty_sha256": digest(first_tasty),
        }
        links[class_name] = class_link.hex()
    if links["Counter"] == links["Gauge"]:
        raise RuntimeError("Counter and Gauge unexpectedly share one TASTy UUID")

    consumer_source = ROOT / "tests/fixtures/jvm_tasty_multiclass/Consumer.scala"
    probes = {
        "Counter": """package consumer
object CounterTastyProbe:
  def read(counter: example.Counter): Int = counter.get()
""",
        "Gauge": """package consumer
object GaugeTastyProbe:
  def read(gauge: example.Gauge): Int = gauge.get()
""",
    }
    compiler_jars = [Path(path) for path in toolchain.classpath.split(os.pathsep)]
    for jar in compiler_jars:
        with zipfile.ZipFile(jar) as archive:
            names = set(archive.namelist())
        for class_name in ("Counter", "Gauge"):
            if f"example/{class_name}.class" in names:
                raise RuntimeError(f"Scala compiler/runtime dependency contains {class_name}.class: {jar}")

    runtime_outputs: list[str] = []
    negative_control_diagnostics: dict[str, str] = {}
    rust_hashes_before = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }
    with tempfile.TemporaryDirectory(prefix="dotty-scala-multiclass-consumer-") as temporary:
        work = Path(temporary)
        client_source = work / "src/Consumer.scala"
        client_source.parent.mkdir()
        shutil.copyfile(consumer_source, client_source)
        for index, pair_root in enumerate((first, second)):
            client = work / f"client-{index}"
            client.mkdir()
            toolchain.compiler(["-d", client, client_source],
                               classpath=os.pathsep.join((str(pair_root), toolchain.classpath)))
            classfiles = sorted(
                path.relative_to(client).as_posix()
                for path in client.rglob("*.class")
            )
            expected_client_classes = ["consumer/Consumer$.class", "consumer/Consumer.class"]
            if classfiles != expected_client_classes:
                raise RuntimeError(f"Scala consumer emitted unexpected classes: {classfiles}")
            for path in client.rglob("*"):
                if path.is_file() and not path.relative_to(client).as_posix().startswith("consumer/"):
                    raise RuntimeError(f"Scala consumer output escaped its package: {path}")
            for class_name in ("Counter", "Gauge"):
                if list(client.rglob(f"{class_name}.class")):
                    raise RuntimeError(f"Scala compiler emitted or copied {class_name}.class")

            javap = execute([
                toolchain.javap,
                "-classpath",
                os.pathsep.join((str(client), str(pair_root))),
                "-verbose",
                "consumer.Consumer$",
            ])
            for owner in ("Counter", "Gauge"):
                for reference in (
                    f'example/{owner}."<init>":(I)V',
                    f"example/{owner}.set:(I)V",
                    f"example/{owner}.get:()I",
                ):
                    if reference not in javap:
                        raise RuntimeError(f"consumer bytecode lacks Methodref {reference}: {javap}")

            runtime = execute([toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                               os.pathsep.join((str(pair_root), str(client), toolchain.classpath)),
                               "consumer.Consumer"])
            output = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if output != ["1329"]:
                raise RuntimeError(f"Scala consumer returned unexpected output: {runtime}")
            runtime_outputs.extend(output)

        for class_name in ("Counter", "Gauge"):
            probe = work / f"{class_name}TastyProbe.scala"
            probe.write_text(probes[class_name])
            output = work / f"probe-{class_name}"
            output.mkdir()
            tasty_only = batch_root / f"tasty-only-{class_name}"
            toolchain.compiler(["-d", output, probe],
                               classpath=os.pathsep.join((str(tasty_only), toolchain.classpath)))
            generated = output / f"consumer/{class_name}TastyProbe$.class"
            if not generated.is_file():
                raise RuntimeError(f"{class_name} TASTy-only semantic probe emitted no class")

            classfile_output = work / f"classfile-only-{class_name}-output"
            classfile_output.mkdir()
            try:
                toolchain.compiler(["-d", classfile_output, probe],
                                   classpath=os.pathsep.join((
                                       str(batch_root / f"classfile-only-{class_name}"),
                                       toolchain.classpath,
                                   )))
            except CommandFailure as error:
                classfile_failure = error.output
            else:
                raise RuntimeError(f"{class_name} classfile-only control resolved get()")
            if not re.search(r"(?i)(get.*(not found|not a member)|no member.*get)", classfile_failure):
                raise RuntimeError(f"{class_name} classfile-only control failed for another reason: {classfile_failure}")
            negative_control_diagnostics[f"{class_name}.classfile_only"] = classfile_failure.strip()

            missing_output = work / f"missing-sidecar-{class_name}-output"
            missing_output.mkdir()
            try:
                toolchain.compiler(["-Werror", "-d", missing_output, probe],
                                   classpath=os.pathsep.join((
                                       str(batch_root / f"missing-sidecar-{class_name}"),
                                       toolchain.classpath,
                                   )))
            except CommandFailure as error:
                missing_failure = error.output
            else:
                raise RuntimeError(f"{class_name} missing-sidecar control unexpectedly compiled")
            if not re.search(r"(?i)(tasty.*missing|missing.*tasty|tasty.*not found)", missing_failure):
                raise RuntimeError(f"{class_name} missing-sidecar control failed for another reason: {missing_failure}")
            negative_control_diagnostics[f"{class_name}.missing_sidecar"] = missing_failure.strip()

            mismatch_output = work / f"uuid-mismatch-{class_name}-output"
            mismatch_output.mkdir()
            try:
                toolchain.compiler(["-Werror", "-d", mismatch_output, probe],
                                   classpath=os.pathsep.join((
                                       str(batch_root / f"uuid-mismatch-{class_name}"),
                                       toolchain.classpath,
                                   )))
            except CommandFailure as error:
                mismatch_failure = error.output
            else:
                raise RuntimeError(f"{class_name} stale UUID control unexpectedly compiled")
            if not re.search(
                r"(?i)(tasty.*(uuid|identifier|sync)|(uuid|identifier|sync).*tasty|out of sync.*tasty)",
                mismatch_failure,
            ):
                raise RuntimeError(f"{class_name} UUID control failed for another reason: {mismatch_failure}")
            negative_control_diagnostics[f"{class_name}.uuid_mismatch"] = mismatch_failure.strip()

            for marker_kind in ("xasty", "missing-marker"):
                marker_output = work / f"{marker_kind}-{class_name}-output"
                marker_output.mkdir()
                marker_root = batch_root / f"{marker_kind}-{class_name}"
                try:
                    toolchain.compiler(["-Werror", "-d", marker_output, probe],
                                       classpath=os.pathsep.join((str(marker_root), toolchain.classpath)))
                except CommandFailure as error:
                    marker_failure = error.output
                else:
                    raise RuntimeError(f"{class_name} {marker_kind} control unexpectedly resolved get()")
                if not re.search(r"(?i)(get.*(not found|not a member)|no member.*get)", marker_failure):
                    raise RuntimeError(f"{class_name} {marker_kind} control failed for another reason: {marker_failure}")
                negative_control_diagnostics[f"{class_name}.{marker_kind}"] = marker_failure.strip()

            wrong_path_output = work / f"wrong-path-{class_name}-output"
            wrong_path_output.mkdir()
            wrong_path_root = batch_root / f"wrong-path-{class_name}"
            try:
                toolchain.compiler(["-Werror", "-d", wrong_path_output, probe],
                                   classpath=os.pathsep.join((str(wrong_path_root), toolchain.classpath)))
            except CommandFailure as error:
                wrong_path_failure = error.output
            else:
                raise RuntimeError(f"{class_name} wrong-path sidecar control unexpectedly compiled")
            if not re.search(r"(?i)(tasty.*missing|missing.*tasty|tasty.*not found)", wrong_path_failure):
                raise RuntimeError(f"{class_name} wrong-path control failed for another reason: {wrong_path_failure}")
            negative_control_diagnostics[f"{class_name}.wrong_path"] = wrong_path_failure.strip()

            corrupt_output = work / f"corrupt-sidecar-{class_name}-output"
            corrupt_output.mkdir()
            corrupt_root = batch_root / f"corrupt-sidecar-{class_name}"
            try:
                toolchain.compiler(["-Werror", "-d", corrupt_output, probe],
                                   classpath=os.pathsep.join((str(corrupt_root), toolchain.classpath)))
            except CommandFailure as error:
                corrupt_failure = error.output
            else:
                raise RuntimeError(f"{class_name} corrupt-sidecar control unexpectedly compiled")
            if not re.search(r"(?i)(tasty|malformed|magic|invalid)", corrupt_failure):
                raise RuntimeError(f"{class_name} corrupt sidecar failed for another reason: {corrupt_failure}")
            negative_control_diagnostics[f"{class_name}.corrupt_sidecar"] = corrupt_failure.strip()

            missing_class_root = batch_root / f"missing-classfile-{class_name}"
            missing_class_client = work / f"missing-classfile-{class_name}-client"
            missing_class_client.mkdir()
            toolchain.compiler(["-d", missing_class_client, client_source],
                               classpath=os.pathsep.join((str(missing_class_root), toolchain.classpath)))
            missing_class_runtime = [toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                                     os.pathsep.join((str(missing_class_root), str(missing_class_client),
                                                      toolchain.classpath)), "consumer.Consumer"]
            try:
                execute(missing_class_runtime)
            except CommandFailure as error:
                missing_class_failure = error.output
            else:
                raise RuntimeError(f"{class_name} missing-classfile control unexpectedly executed")
            if not re.search(
                rf"(?i)(NoClassDefFoundError|ClassNotFoundException).*example[/.]{class_name}",
                missing_class_failure,
            ):
                raise RuntimeError(
                    f"{class_name} missing-classfile control failed for another reason: {missing_class_failure}"
                )
            negative_control_diagnostics[f"{class_name}.missing_classfile"] = missing_class_failure.strip()

    rust_hashes_after = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }
    if rust_hashes_after != rust_hashes_before:
        raise RuntimeError("Scala compile/runtime changed a staged Rust artifact")

    return {
        "scala_consumer": True,
        "scala_source_sha256": digest(consumer_source),
        "fresh_batches_byte_identical": True,
        "fresh_batch_hashes": hashes,
        "tasty_link_uuids": links,
        "runtime_outputs": runtime_outputs,
        "counter_tasty_only_method_resolved": True,
        "gauge_tasty_only_method_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "gauge_classfile_only_control_rejected": True,
        "counter_missing_sidecar_control_rejected": True,
        "gauge_missing_sidecar_control_rejected": True,
        "counter_uuid_mismatch_control_rejected": True,
        "gauge_uuid_mismatch_control_rejected": True,
        "counter_xasty_marker_control_rejected": True,
        "gauge_xasty_marker_control_rejected": True,
        "counter_missing_marker_control_rejected": True,
        "gauge_missing_marker_control_rejected": True,
        "counter_missing_classfile_control_rejected": True,
        "gauge_missing_classfile_control_rejected": True,
        "counter_wrong_path_control_rejected": True,
        "gauge_wrong_path_control_rejected": True,
        "counter_corrupt_sidecar_control_rejected": True,
        "gauge_corrupt_sidecar_control_rejected": True,
        "negative_control_diagnostics": negative_control_diagnostics,
        "rust_classfiles_unchanged_after_compile_and_run": True,
        "consumer_classes": ["consumer/Consumer$.class", "consumer/Consumer.class"],
        "consumer_methodrefs": [
            'example/Counter."<init>":(I)V',
            "example/Counter.set:(I)V",
            "example/Counter.get:()I",
            'example/Gauge."<init>":(I)V',
            "example/Gauge.set:(I)V",
            "example/Gauge.get:()I",
        ],
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "compiler_artifacts_sha256": toolchain.artifacts,
    }


def consume_cross_class_batch(toolchain: Toolchain, batch_root: Path) -> dict:
    """Consume the Rust Counter/Reader pair and prove both TASTy sidecars are used."""
    relative_paths = {
        "example/Counter.class",
        "example/Counter.tasty",
        "example/Reader.class",
        "example/Reader.tasty",
    }
    roots: dict[str, set[str]] = {"fresh-a": relative_paths, "fresh-b": relative_paths}
    for class_name in ("Counter", "Reader"):
        tasty_path = f"example/{class_name}.tasty"
        roots[f"tasty-only-{class_name}"] = relative_paths
        roots[f"uuid-mismatch-{class_name}"] = relative_paths
        roots[f"classfile-only-{class_name}"] = relative_paths - {tasty_path}
        roots[f"missing-sidecar-{class_name}"] = relative_paths - {tasty_path}
        roots[f"wrong-path-{class_name}"] = (
            relative_paths - {tasty_path} | {f"misplaced/{class_name}.tasty"}
        )
        roots[f"corrupt-sidecar-{class_name}"] = relative_paths

    for name, expected in roots.items():
        root = batch_root / name
        actual = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
        if actual != expected:
            raise RuntimeError(f"{name} artifact inventory differs: {sorted(actual)}")

    first = batch_root / "fresh-a"
    second = batch_root / "fresh-b"
    hashes: dict[str, dict[str, str]] = {}
    links: dict[str, str] = {}
    for class_name in ("Counter", "Reader"):
        class_path = first / f"example/{class_name}.class"
        tasty_path = first / f"example/{class_name}.tasty"
        class_bytes = class_path.read_bytes()
        tasty_bytes = tasty_path.read_bytes()
        if class_bytes[6:8] != (65).to_bytes(2, "big"):
            raise RuntimeError(f"Rust {class_name}.class is not classfile major 65")
        class_link = _class_tasty_link(class_bytes)
        tasty_link = _tasty_header_uuid(tasty_bytes)
        if class_link is None or class_link != tasty_link:
            raise RuntimeError(f"Rust {class_name} classfile and TASTy UUIDs do not match")
        for path in (class_path, tasty_path):
            other = second / path.relative_to(first)
            if digest(path) != digest(other):
                raise RuntimeError(f"fresh Rust artifact bytes differ: {path.relative_to(first)}")
        hashes[class_name] = {
            "class_sha256": digest(class_path),
            "tasty_sha256": digest(tasty_path),
        }
        links[class_name] = class_link.hex()
    if links["Counter"] == links["Reader"]:
        raise RuntimeError("Counter and Reader unexpectedly share one TASTy UUID")

    compiler_jars = [Path(path) for path in toolchain.classpath.split(os.pathsep)]
    for jar in compiler_jars:
        with zipfile.ZipFile(jar) as archive:
            names = set(archive.namelist())
        for class_name in ("Counter", "Reader"):
            if f"example/{class_name}.class" in names:
                raise RuntimeError(f"Scala compiler/runtime dependency contains {class_name}.class: {jar}")

    consumer_source = ROOT / "tests/fixtures/jvm_tasty_cross_class/Consumer.scala"
    probes = {
        "Counter": (
            """package consumer
object CounterTastyProbe:
  def read(counter: example.Counter): Int = counter.get()
""",
            "get",
        ),
        "Reader": (
            """package consumer
object ReaderTastyProbe:
  def read(reader: example.Reader, counter: example.Counter): Int = reader.read(counter)
""",
            "read",
        ),
    }
    runtime_outputs: list[str] = []
    negative_diagnostics: dict[str, str] = {}
    artifact_hashes_before = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }

    def compile_probe(work: Path, class_name: str, label: str, classpath_root: Path,
                      source: Path, *, werror: bool = False) -> None:
        output = work / f"{label}-{class_name}-output"
        output.mkdir()
        args: list[str | Path] = []
        if werror:
            args.append("-Werror")
        args.extend(("-d", output, source))
        toolchain.compiler(
            args,
            classpath=os.pathsep.join((str(classpath_root), toolchain.classpath)),
        )

    with tempfile.TemporaryDirectory(prefix="dotty-scala-cross-class-consumer-") as temporary:
        work = Path(temporary)
        client_source = work / "src/Consumer.scala"
        client_source.parent.mkdir()
        shutil.copyfile(consumer_source, client_source)
        for index, pair_root in enumerate((first, second)):
            client = work / f"client-{index}"
            client.mkdir()
            toolchain.compiler(
                ["-d", client, client_source],
                classpath=os.pathsep.join((str(pair_root), toolchain.classpath)),
            )
            classfiles = sorted(path.relative_to(client).as_posix() for path in client.rglob("*.class"))
            expected_client = ["consumer/Consumer$.class", "consumer/Consumer.class"]
            if classfiles != expected_client:
                raise RuntimeError(f"Scala consumer emitted unexpected classfiles: {classfiles}")
            if list(client.rglob("Counter.class")) or list(client.rglob("Reader.class")):
                raise RuntimeError("Scala compiler emitted or copied a Rust dependency class")

            javap = execute([
                toolchain.javap,
                "-classpath",
                os.pathsep.join((str(client), str(pair_root))),
                "-verbose",
                "consumer.Consumer$",
            ])
            methodrefs = (
                'example/Counter."<init>":(I)V',
                'example/Reader."<init>":()V',
                "example/Reader.read:(Lexample/Counter;)I",
                "example/Counter.set:(I)V",
            )
            for reference in methodrefs:
                if reference not in javap:
                    raise RuntimeError(f"Scala consumer bytecode lacks Methodref {reference}: {javap}")

            runtime = execute([
                toolchain.java,
                "-Xverify:all",
                "-XX:-UsePerfData",
                "-classpath",
                os.pathsep.join((str(pair_root), str(client), toolchain.classpath)),
                "consumer.Consumer",
            ])
            output = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if output != ["713"]:
                raise RuntimeError(f"Scala consumer returned unexpected output: {runtime}")
            runtime_outputs.extend(output)

        for class_name, (probe_text, member) in probes.items():
            probe = work / f"{class_name}TastyProbe.scala"
            probe.write_text(probe_text)
            tasty_only = batch_root / f"tasty-only-{class_name}"
            try:
                compile_probe(work, class_name, "tasty-only", tasty_only, probe)
            except CommandFailure as error:
                raise RuntimeError(f"{class_name} TASTy-only lookup failed: {error.output}") from error
            generated = work / f"tasty-only-{class_name}-output/consumer/{class_name}TastyProbe$.class"
            if not generated.is_file():
                raise RuntimeError(f"{class_name} TASTy-only probe emitted no client class")

            classfile_only = batch_root / f"classfile-only-{class_name}"
            try:
                compile_probe(work, class_name, "classfile-only", classfile_only, probe)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"{class_name} classfile-only control resolved {member}")
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(f"{class_name} classfile-only failed for another reason: {failure}")
            negative_diagnostics[f"{class_name}.classfile_only"] = failure.strip()

            for control, pattern in (
                ("uuid-mismatch", r"(?i)out of sync with its TASTy file"),
                ("missing-sidecar", r"(?i)this `?\.tasty`? file is missing"),
                ("wrong-path", r"(?i)this `?\.tasty`? file is missing"),
                ("corrupt-sidecar", r"(?is)could not be read.*not a TASTy file"),
            ):
                try:
                    compile_probe(
                        work,
                        class_name,
                        control,
                        batch_root / f"{control}-{class_name}",
                        probe,
                        werror=True,
                    )
                except CommandFailure as error:
                    failure = error.output
                else:
                    raise RuntimeError(f"{class_name} {control} control unexpectedly resolved {member}")
                if not re.search(pattern, failure):
                    raise RuntimeError(
                        f"{class_name} {control} did not report its expected first-stage diagnostic: {failure}"
                    )
                negative_diagnostics[f"{class_name}.{control}"] = failure.strip()

    artifact_hashes_after = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }
    if artifact_hashes_after != artifact_hashes_before:
        raise RuntimeError("Scala compile/runtime changed a staged Rust artifact")

    return {
        "scala_consumer": True,
        "scala_source_sha256": digest(consumer_source),
        "fresh_batches_byte_identical": True,
        "fresh_batch_hashes": hashes,
        "tasty_link_uuids": links,
        "runtime_outputs": runtime_outputs,
        "counter_tasty_only_get_resolved": True,
        "reader_tasty_only_read_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "reader_classfile_only_control_rejected": True,
        "counter_uuid_mismatch_control_rejected": True,
        "reader_uuid_mismatch_control_rejected": True,
        "counter_missing_sidecar_control_rejected": True,
        "reader_missing_sidecar_control_rejected": True,
        "counter_wrong_path_control_rejected": True,
        "reader_wrong_path_control_rejected": True,
        "counter_corrupt_sidecar_control_rejected": True,
        "reader_corrupt_sidecar_control_rejected": True,
        "negative_control_diagnostics": negative_diagnostics,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "consumer_classes": ["consumer/Consumer$.class", "consumer/Consumer.class"],
        "consumer_methodrefs": [
            'example/Counter."<init>":(I)V',
            'example/Reader."<init>":()V',
            "example/Reader.read:(Lexample/Counter;)I",
            "example/Counter.set:(I)V",
        ],
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "compiler_artifacts_sha256": toolchain.artifacts,
    }


def consume_factory_batch(toolchain: Toolchain, batch_root: Path) -> dict:
    """Consume Rust Counter/CounterFactory TASTy with pinned Scala 3.9."""
    relative_paths = {
        "example/Counter.class",
        "example/Counter.tasty",
        "example/CounterFactory.class",
        "example/CounterFactory.tasty",
    }
    roots: dict[str, set[str]] = {"fresh-a": relative_paths, "fresh-b": relative_paths}
    for class_name in ("Counter", "CounterFactory"):
        tasty_path = f"example/{class_name}.tasty"
        roots[f"tasty-only-{class_name}"] = relative_paths
        roots[f"uuid-mismatch-{class_name}"] = relative_paths
        roots[f"classfile-only-{class_name}"] = relative_paths - {tasty_path}
        roots[f"missing-classfile-{class_name}"] = relative_paths - {f"example/{class_name}.class"}
        roots[f"missing-sidecar-{class_name}"] = relative_paths - {tasty_path}
        roots[f"wrong-path-{class_name}"] = (
            relative_paths - {tasty_path} | {f"misplaced/{class_name}.tasty"}
        )
        roots[f"corrupt-sidecar-{class_name}"] = relative_paths

    for name, expected in roots.items():
        root = batch_root / name
        actual = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
        if actual != expected:
            raise RuntimeError(f"{name} artifact inventory differs: {sorted(actual)}")

    first = batch_root / "fresh-a"
    second = batch_root / "fresh-b"
    classes = ("Counter", "CounterFactory")
    hashes: dict[str, dict[str, str]] = {}
    links: dict[str, str] = {}
    for class_name in classes:
        class_path = first / f"example/{class_name}.class"
        tasty_path = first / f"example/{class_name}.tasty"
        class_bytes = class_path.read_bytes()
        tasty_bytes = tasty_path.read_bytes()
        if class_bytes[6:8] != (65).to_bytes(2, "big"):
            raise RuntimeError(f"Rust {class_name}.class is not classfile major 65")
        class_link = _class_tasty_link(class_bytes)
        tasty_link = _tasty_header_uuid(tasty_bytes)
        if class_link is None or class_link != tasty_link:
            raise RuntimeError(f"Rust {class_name} classfile and TASTy UUIDs do not match")
        for path in (class_path, tasty_path):
            other = second / path.relative_to(first)
            if digest(path) != digest(other):
                raise RuntimeError(f"fresh Rust artifact bytes differ: {path.relative_to(first)}")
        hashes[class_name] = {
            "class_sha256": digest(class_path),
            "tasty_sha256": digest(tasty_path),
        }
        links[class_name] = class_link.hex()
    if links["Counter"] == links["CounterFactory"]:
        raise RuntimeError("Counter and CounterFactory unexpectedly share one TASTy UUID")

    compiler_jars = [Path(path) for path in toolchain.classpath.split(os.pathsep)]
    for jar in compiler_jars:
        with zipfile.ZipFile(jar) as archive:
            names = set(archive.namelist())
        for class_name in classes:
            if f"example/{class_name}.class" in names:
                raise RuntimeError(f"Scala compiler/runtime dependency contains {class_name}.class: {jar}")

    consumer_source = ROOT / "tests/fixtures/jvm_tasty_cross_class_factories/FactoryConsumer.scala"
    probes = {
        "Counter": (
            """package consumer
object CounterFactoryTastyProbe:
  def read(counter: example.Counter): Int = counter.get()
""",
            "get",
        ),
        "CounterFactory": (
            """package consumer
object FactoryTastyProbe:
  def read(factory: example.CounterFactory): Int = factory.create(seed = 7).get()
""",
            "create",
        ),
    }
    runtime_outputs: list[str] = []
    compile_commands: list[list[str]] = []
    runtime_commands: list[list[str]] = []
    negative_diagnostics: dict[str, str] = {}
    artifact_hashes_before = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }

    with tempfile.TemporaryDirectory(prefix="dotty-scala-factory-consumer-") as temporary:
        work = Path(temporary)
        client_source = work / "src/FactoryConsumer.scala"
        client_source.parent.mkdir()
        shutil.copyfile(consumer_source, client_source)
        for index, pair_root in enumerate((first, second)):
            client = work / f"client-{index}"
            client.mkdir()
            toolchain.compiler(
                ["-d", client, client_source],
                classpath=os.pathsep.join((str(pair_root), toolchain.runtime_classpath)),
            )
            compile_commands.append([
                str(toolchain.java), "-XX:-UsePerfData", "-cp", toolchain.classpath,
                "dotty.tools.dotc.Main", "-classpath",
                os.pathsep.join((str(pair_root), toolchain.runtime_classpath)), "-color:never",
                "-d", str(client), str(client_source),
            ])
            classfiles = sorted(path.relative_to(client).as_posix() for path in client.rglob("*.class"))
            expected_client = ["consumer/Consumer$.class", "consumer/Consumer.class"]
            if classfiles != expected_client:
                raise RuntimeError(f"Scala consumer emitted unexpected classfiles: {classfiles}")
            if any(client.rglob("Counter.class")) or any(client.rglob("CounterFactory.class")):
                raise RuntimeError("Scala compiler emitted or copied a Rust dependency class")

            javap = execute([
                toolchain.javap,
                "-classpath",
                os.pathsep.join((str(client), str(pair_root))),
                "-verbose",
                "consumer.Consumer$",
            ])
            methodrefs = (
                'example/CounterFactory."<init>":()V',
                "example/CounterFactory.create:(I)Lexample/Counter;",
                "example/Counter.get:()I",
                "example/Counter.set:(I)V",
            )
            for reference in methodrefs:
                if reference not in javap:
                    raise RuntimeError(f"Scala consumer bytecode lacks Methodref {reference}: {javap}")
            if "example/CounterFactory.create:(I)Ljava/lang/Object;" in javap:
                raise RuntimeError("Scala compiler widened CounterFactory.create result to Object")

            runtime = execute([
                toolchain.java,
                "-Xverify:all",
                "-XX:-UsePerfData",
                "-classpath",
                os.pathsep.join((str(pair_root), str(client), toolchain.runtime_classpath)),
                "consumer.Consumer",
            ])
            runtime_commands.append([
                str(toolchain.java), "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                os.pathsep.join((str(pair_root), str(client), toolchain.runtime_classpath)),
                "consumer.Consumer",
            ])
            output = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if output != ["42"]:
                raise RuntimeError(f"Scala consumer returned unexpected output: {runtime}")
            runtime_outputs.extend(output)

        def compile_probe(class_name: str, label: str, classpath_root: Path,
                          source: Path, *, werror: bool = False) -> None:
            output = work / f"{label}-{class_name}-output"
            output.mkdir()
            args: list[str | Path] = []
            if werror:
                args.append("-Werror")
            args.extend(("-d", output, source))
            toolchain.compiler(
                args,
                classpath=os.pathsep.join((str(classpath_root), toolchain.runtime_classpath)),
            )

        for class_name, (probe_text, member) in probes.items():
            probe = work / f"{class_name}TastyProbe.scala"
            probe.write_text(probe_text)
            tasty_only = batch_root / f"tasty-only-{class_name}"
            try:
                compile_probe(class_name, "tasty-only", tasty_only, probe)
            except CommandFailure as error:
                raise RuntimeError(f"{class_name} TASTy-only lookup failed: {error.output}") from error

            classfile_only = batch_root / f"classfile-only-{class_name}"
            try:
                compile_probe(class_name, "classfile-only", classfile_only, probe)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"{class_name} classfile-only control resolved {member}")
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(f"{class_name} classfile-only failed for another reason: {failure}")
            negative_diagnostics[f"{class_name}.classfile_only"] = failure.strip()

            missing_classfile = batch_root / f"missing-classfile-{class_name}"
            try:
                compile_probe(class_name, "missing-classfile-tasty-only", missing_classfile, probe)
            except CommandFailure as error:
                raise RuntimeError(
                    f"{class_name} missing-classfile TASTy lookup failed: {error.output}"
                ) from error
            missing_client = work / f"missing-classfile-{class_name}-consumer-output"
            missing_client.mkdir()
            toolchain.compiler(
                ["-d", missing_client, client_source],
                classpath=os.pathsep.join((str(missing_classfile), toolchain.runtime_classpath)),
            )
            runtime = subprocess.run(
                [toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                 os.pathsep.join((str(missing_classfile), str(missing_client),
                                  toolchain.runtime_classpath)), "consumer.Consumer"],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                timeout=180,
            )
            missing_binary = f"example/{class_name}"
            failure = runtime.stdout
            if runtime.returncode == 0 or f"NoClassDefFoundError: {missing_binary}" not in failure:
                raise RuntimeError(
                    f"{class_name} missing-classfile runtime did not fail at class loading: {failure}"
                )
            negative_diagnostics[f"{class_name}.missing_classfile_runtime"] = failure.strip()

            for control, pattern in (
                ("uuid-mismatch", r"(?i)out of sync with its TASTy file"),
                ("missing-sidecar", r"(?i)this `?\.tasty`? file is missing"),
                ("wrong-path", r"(?i)this `?\.tasty`? file is missing"),
                ("corrupt-sidecar", r"(?is)could not be read.*not a TASTy file"),
            ):
                try:
                    compile_probe(
                        class_name,
                        control,
                        batch_root / f"{control}-{class_name}",
                        probe,
                        werror=True,
                    )
                except CommandFailure as error:
                    failure = error.output
                else:
                    raise RuntimeError(f"{class_name} {control} control unexpectedly resolved {member}")
                if not re.search(pattern, failure):
                    raise RuntimeError(
                        f"{class_name} {control} did not report its expected first-stage diagnostic: {failure}"
                    )
                negative_diagnostics[f"{class_name}.{control}"] = failure.strip()

    artifact_hashes_after = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }
    if artifact_hashes_after != artifact_hashes_before:
        raise RuntimeError("Scala compile/runtime changed a staged Rust artifact")

    return {
        "scala_consumer": True,
        "scala_revision": REVISION,
        "scala_source_sha256": digest(consumer_source),
        "fresh_batches_byte_identical": True,
        "fresh_batch_hashes": hashes,
        "tasty_link_uuids": links,
        "runtime_outputs": runtime_outputs,
        "counter_tasty_only_get_resolved": True,
        "factory_tasty_only_create_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "factory_classfile_only_control_rejected": True,
        "counter_missing_classfile_tasty_compile_succeeded": True,
        "factory_missing_classfile_tasty_compile_succeeded": True,
        "counter_missing_classfile_runtime_rejected": True,
        "factory_missing_classfile_runtime_rejected": True,
        "counter_uuid_mismatch_control_rejected": True,
        "factory_uuid_mismatch_control_rejected": True,
        "counter_missing_sidecar_control_rejected": True,
        "factory_missing_sidecar_control_rejected": True,
        "counter_wrong_path_control_rejected": True,
        "factory_wrong_path_control_rejected": True,
        "counter_corrupt_sidecar_control_rejected": True,
        "factory_corrupt_sidecar_control_rejected": True,
        "negative_control_diagnostics": negative_diagnostics,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "consumer_classes": ["consumer/Consumer$.class", "consumer/Consumer.class"],
        "consumer_methodrefs": [
            'example/CounterFactory."<init>":()V',
            "example/CounterFactory.create:(I)Lexample/Counter;",
            "example/Counter.get:()I",
            "example/Counter.set:(I)V",
        ],
        "compile_classpath_order": ["rust-batch-root", *[str(path) for path in toolchain.runtime_jars]],
        "compiler_process_classpath_order": [str(path) for path in compiler_jars],
        "runtime_classpath_order": ["rust-batch-root", "consumer-output", *[str(path) for path in toolchain.runtime_jars]],
        "compile_commands": compile_commands,
        "runtime_commands": runtime_commands,
        "compiler_process_classpath_order": [str(path) for path in compiler_jars],
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "compiler_artifacts_sha256": toolchain.artifacts,
    }


def consume_object_composition_batch(toolchain: Toolchain, batch_root: Path) -> dict:
    """Prove pinned Scala consumes Rust Counter/CounterOwner TASTy and JVM output."""
    relative_paths = {
        "example/Counter.class",
        "example/Counter.tasty",
        "example/CounterOwner.class",
        "example/CounterOwner.tasty",
    }
    roots: dict[str, set[str]] = {"fresh-a": relative_paths, "fresh-b": relative_paths}
    for class_name in ("Counter", "CounterOwner"):
        tasty_path = f"example/{class_name}.tasty"
        roots[f"tasty-only-{class_name}"] = relative_paths
        roots[f"uuid-mismatch-{class_name}"] = relative_paths
        roots[f"renamed-link-{class_name}"] = relative_paths - {tasty_path}
        roots[f"classfile-only-{class_name}"] = relative_paths - {tasty_path}
        roots[f"missing-classfile-{class_name}"] = relative_paths - {f"example/{class_name}.class"}
        roots[f"missing-sidecar-{class_name}"] = relative_paths - {tasty_path}
        roots[f"wrong-path-{class_name}"] = (
            relative_paths - {tasty_path} | {f"misplaced/{class_name}.tasty"}
        )
        roots[f"corrupt-sidecar-{class_name}"] = relative_paths

    for name, expected in roots.items():
        root = batch_root / name
        actual = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
        if actual != expected:
            raise RuntimeError(f"{name} artifact inventory differs: {sorted(actual)}")

    first = batch_root / "fresh-a"
    second = batch_root / "fresh-b"
    classes = ("Counter", "CounterOwner")
    hashes: dict[str, dict[str, str]] = {}
    links: dict[str, str] = {}
    for class_name in classes:
        class_path = first / f"example/{class_name}.class"
        tasty_path = first / f"example/{class_name}.tasty"
        class_bytes = class_path.read_bytes()
        tasty_bytes = tasty_path.read_bytes()
        if class_bytes[6:8] != (65).to_bytes(2, "big"):
            raise RuntimeError(f"Rust {class_name}.class is not classfile major 65")
        class_link = _class_tasty_link(class_bytes)
        tasty_link = _tasty_header_uuid(tasty_bytes)
        if class_link is None or class_link != tasty_link:
            raise RuntimeError(f"Rust {class_name} classfile and TASTy UUIDs do not match")
        for path in (class_path, tasty_path):
            other = second / path.relative_to(first)
            if digest(path) != digest(other):
                raise RuntimeError(f"fresh Rust artifact bytes differ: {path.relative_to(first)}")
        hashes[class_name] = {
            "class_sha256": digest(class_path),
            "tasty_sha256": digest(tasty_path),
        }
        links[class_name] = class_link.hex()
    if links["Counter"] == links["CounterOwner"]:
        raise RuntimeError("Counter and CounterOwner unexpectedly share one TASTy UUID")

    compiler_jars = [Path(path) for path in toolchain.classpath.split(os.pathsep)]
    for jar in compiler_jars:
        with zipfile.ZipFile(jar) as archive:
            names = set(archive.namelist())
        for class_name in classes:
            if f"example/{class_name}.class" in names:
                raise RuntimeError(f"Scala compiler/runtime dependency contains {class_name}.class: {jar}")

    consumer_source = ROOT / "tests/fixtures/jvm_tasty_object_composition/Consumer.scala"
    source_hash = digest(consumer_source)
    owner_probe_text = """package consumer
object OwnerTastyProbe:
  def read(owner: example.CounterOwner): Int = owner.read()
"""
    counter_probe_text = """package consumer
object CounterTastyProbe:
  def get(counter: example.Counter): Int = counter.get()
"""
    runtime_outputs: list[str] = []
    compile_commands: list[list[str]] = []
    runtime_commands: list[list[str]] = []
    negative_diagnostics: dict[str, str] = {}
    artifact_hashes_before = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }

    with tempfile.TemporaryDirectory(prefix="dotty-scala-object-composition-") as temporary:
        work = Path(temporary)
        client_source = work / "src/Consumer.scala"
        client_source.parent.mkdir()
        shutil.copyfile(consumer_source, client_source)
        for index, pair_root in enumerate((first, second)):
            client = work / f"client-{index}"
            client.mkdir()
            client_classpath = os.pathsep.join((str(pair_root), toolchain.runtime_classpath))
            toolchain.compiler(["-d", client, client_source], classpath=client_classpath)
            compile_commands.append([
                str(toolchain.java), "-XX:-UsePerfData", "-cp", toolchain.classpath,
                "dotty.tools.dotc.Main", "-classpath", client_classpath, "-color:never",
                "-d", str(client), str(client_source),
            ])
            classfiles = sorted(path.relative_to(client).as_posix() for path in client.rglob("*.class"))
            expected_client = ["consumer/Consumer$.class", "consumer/Consumer.class"]
            if classfiles != expected_client:
                raise RuntimeError(f"Scala consumer emitted unexpected classfiles: {classfiles}")
            if any(client.rglob("Counter.class")) or any(client.rglob("CounterOwner.class")):
                raise RuntimeError("Scala compiler emitted or copied a Rust dependency class")

            javap = execute([
                toolchain.javap, "-classpath", os.pathsep.join((str(client), str(pair_root))),
                "-verbose", "consumer.Consumer$",
            ])
            for reference in (
                'example/CounterOwner."<init>":(I)V',
                "example/CounterOwner.read:()I",
            ):
                if reference not in javap:
                    raise RuntimeError(f"Scala consumer bytecode lacks Methodref {reference}: {javap}")

            driver_source = work / "Runner.java"
            driver_source.write_text(
                "public final class Runner { public static void main(String[] args) { "
                "System.out.println(consumer.Consumer$.MODULE$.run()); } }\n"
            )
            driver = work / f"driver-{index}"
            driver.mkdir()
            driver_classpath = os.pathsep.join((str(pair_root), str(client), toolchain.runtime_classpath))
            execute([toolchain.javac, "-classpath", driver_classpath, "-d", driver, driver_source])
            if sorted(path.relative_to(driver).as_posix() for path in driver.rglob("*.class")) != ["Runner.class"]:
                raise RuntimeError("Java runtime driver output contains unexpected classes")

            runtime_classpath = os.pathsep.join((str(pair_root), str(client), str(driver), toolchain.runtime_classpath))
            runtime = execute([
                toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                runtime_classpath, "Runner",
            ])
            runtime_commands.append([
                str(toolchain.java), "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                runtime_classpath, "Runner",
            ])
            output = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if output != ["742"]:
                raise RuntimeError(f"Scala consumer returned unexpected output: {runtime}")
            runtime_outputs.extend(output)

        def compile_source(label: str, classpath_root: Path, source: Path, *, werror: bool = False) -> str:
            output = work / f"{label}-output"
            output.mkdir()
            args: list[str | Path] = []
            if werror:
                args.append("-Werror")
            args.extend(("-d", output, source))
            return str(toolchain.compiler(
                args,
                classpath=os.pathsep.join((str(classpath_root), toolchain.runtime_classpath)),
            ))

        for class_name, source_text, member in (
            ("CounterOwner", owner_probe_text, "read"),
            ("Counter", counter_probe_text, "get"),
        ):
            probe = work / f"{class_name}TastyProbe.scala"
            probe.write_text(source_text)
            try:
                compile_source(f"tasty-only-{class_name}", batch_root / f"tasty-only-{class_name}", probe)
            except CommandFailure as error:
                raise RuntimeError(f"{class_name} TASTy-only lookup failed: {error.output}") from error

            classfile_only = batch_root / f"classfile-only-{class_name}"
            try:
                compile_source(f"classfile-only-{class_name}", classfile_only, probe)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"{class_name} classfile-only control resolved {member}")
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(f"{class_name} classfile-only failed for another reason: {failure}")
            negative_diagnostics[f"{class_name}.classfile_only"] = failure.strip()

            missing_classfile = batch_root / f"missing-classfile-{class_name}"
            output = work / f"missing-classfile-{class_name}-client"
            output.mkdir()
            toolchain.compiler(
                ["-d", output, client_source],
                classpath=os.pathsep.join((str(missing_classfile), toolchain.runtime_classpath)),
            )
            driver = work / "driver-0"
            runtime = subprocess.run(
                [toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                 os.pathsep.join((str(missing_classfile), str(output), str(driver),
                                  toolchain.runtime_classpath)), "Runner"],
                cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180,
            )
            failure = runtime.stdout
            if runtime.returncode == 0 or f"NoClassDefFoundError: example/{class_name}" not in failure:
                raise RuntimeError(
                    f"{class_name} missing-classfile runtime did not fail at class loading: {failure}"
                )
            negative_diagnostics[f"{class_name}.missing_classfile_runtime"] = failure.strip()

            renamed_link = batch_root / f"renamed-link-{class_name}"
            try:
                compile_source(f"renamed-link-{class_name}", renamed_link, probe)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(
                    f"{class_name} non-TASTY classfile marker still exposed TASTy-only {member}"
                )
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(
                    f"{class_name} non-TASTY marker failed for another reason: {failure}"
                )
            negative_diagnostics[f"{class_name}.renamed_link"] = failure.strip()

            for control, pattern in (
                ("uuid-mismatch", r"(?i)out of sync with its TASTy file"),
                ("missing-sidecar", r"(?i)this `?\.tasty`? file is missing"),
                ("wrong-path", r"(?i)this `?\.tasty`? file is missing"),
                ("corrupt-sidecar", r"(?is)could not be read.*not a TASTy file"),
            ):
                try:
                    compile_source(
                        f"{control}-{class_name}", batch_root / f"{control}-{class_name}",
                        probe, werror=True,
                    )
                except CommandFailure as error:
                    failure = error.output
                else:
                    raise RuntimeError(f"{class_name} {control} control unexpectedly resolved {member}")
                if not re.search(pattern, failure):
                    raise RuntimeError(
                        f"{class_name} {control} did not report its expected first-stage diagnostic: {failure}"
                    )
                negative_diagnostics[f"{class_name}.{control}"] = failure.strip()

        sidecar_root = work / "from-tasty-input"
        sidecar_package = sidecar_root / "example"
        sidecar_package.mkdir(parents=True)
        for class_name in classes:
            shutil.copyfile(first / f"example/{class_name}.tasty", sidecar_package / f"{class_name}.tasty")
        owner_tasty = sidecar_package / "CounterOwner.tasty"
        decompiled = toolchain.compiler([owner_tasty], decompile=True)
        expected_semantics = (
            "private[this] val inner: example.Counter = new example.Counter(seed)",
            "def read(): scala.Int = CounterOwner.this.inner.get()",
        )
        if not all(member in decompiled for member in expected_semantics):
            raise RuntimeError(f"Rust CounterOwner.tasty lost source field/initializer/read semantics: {decompiled}")
        counter_decompiled = toolchain.compiler([sidecar_package / "Counter.tasty"], decompile=True)
        expected_counter_semantics = "def get(): scala.Int = Counter.this.current"
        if expected_counter_semantics not in counter_decompiled:
            raise RuntimeError(f"Rust Counter.tasty lost source get semantics: {counter_decompiled}")

        rebuilt = work / "from-tasty-output"
        rebuilt.mkdir()
        sidecar_classpath = os.pathsep.join((str(sidecar_root), toolchain.runtime_classpath))
        toolchain.compiler(["-from-tasty", "-d", rebuilt, owner_tasty], classpath=sidecar_classpath)
        rebuilt_owner = rebuilt / "example/CounterOwner.class"
        if not rebuilt_owner.is_file():
            raise RuntimeError("Scala -from-tasty did not reconstruct CounterOwner.class")
        rebuilt_classes = sorted(path.relative_to(rebuilt).as_posix()
                                 for path in rebuilt.rglob("*.class"))
        if rebuilt_classes != ["example/CounterOwner.class"]:
            raise RuntimeError(f"Scala -from-tasty emitted unexpected dependency classes: {rebuilt_classes}")
        rebuilt_javap = execute([
            toolchain.javap, "-classpath", os.pathsep.join((str(rebuilt), str(sidecar_root))),
            "-private", "-c", "example.CounterOwner",
        ])
        rebuilt_shape = (
            "private final example.Counter inner;",
            "invokespecial",
            'example/Counter."<init>":(I)V',
            "putfield",
            "Field inner:Lexample/Counter;",
            "getfield",
            "invokevirtual",
            "example/Counter.get:()I",
        )
        if not all(reference in rebuilt_javap for reference in rebuilt_shape):
            raise RuntimeError(
                f"Scala -from-tasty output has unexpected field or method owners: {rebuilt_javap}"
            )

        rebuilt_runtime_dependencies = work / "from-tasty-runtime-dependencies"
        counter_dependency = rebuilt_runtime_dependencies / "example/Counter.class"
        counter_dependency.parent.mkdir(parents=True)
        shutil.copyfile(first / "example/Counter.class", counter_dependency)
        from_tasty_driver_source = work / "FromTastyRunner.java"
        from_tasty_driver_source.write_text(
            "public final class FromTastyRunner { public static void main(String[] args) { "
            "example.CounterOwner a = new example.CounterOwner(7); "
            "example.CounterOwner b = new example.CounterOwner(42); "
            "if (a.read() != 7 || b.read() != 42) throw new AssertionError(\"bad TASTy rebuild\"); "
            "System.out.println(a.read() * 100 + b.read()); } }\n"
        )
        from_tasty_driver = work / "from-tasty-driver"
        from_tasty_driver.mkdir()
        from_tasty_classpath = os.pathsep.join((
            str(rebuilt), str(rebuilt_runtime_dependencies), toolchain.runtime_classpath,
        ))
        execute([
            toolchain.javac, "-classpath", from_tasty_classpath,
            "-d", from_tasty_driver, from_tasty_driver_source,
        ])
        from_tasty_runtime_classpath = os.pathsep.join((
            str(rebuilt), str(rebuilt_runtime_dependencies), str(from_tasty_driver),
            toolchain.runtime_classpath,
        ))
        from_tasty_runtime = execute([
            toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
            from_tasty_runtime_classpath, "FromTastyRunner",
        ])
        from_tasty_output = [line for line in from_tasty_runtime.strip().splitlines()
                             if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
        if from_tasty_output != ["742"]:
            raise RuntimeError(f"isolated Scala -from-tasty runtime returned: {from_tasty_runtime}")

    artifact_hashes_after = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in (first, second)
        for relative in sorted(relative_paths)
    }
    if artifact_hashes_after != artifact_hashes_before:
        raise RuntimeError("Scala compile/runtime changed a staged Rust artifact")

    return {
        "scala_consumer": True,
        "scala_revision": REVISION,
        "scala_source_sha256": source_hash,
        "fresh_batches_byte_identical": True,
        "fresh_batch_hashes": hashes,
        "tasty_link_uuids": links,
        "runtime_outputs": runtime_outputs,
        "counter_tasty_only_get_resolved": True,
        "owner_tasty_only_read_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "owner_classfile_only_control_rejected": True,
        "counter_missing_classfile_runtime_rejected": True,
        "owner_missing_classfile_runtime_rejected": True,
        "counter_uuid_mismatch_control_rejected": True,
        "owner_uuid_mismatch_control_rejected": True,
        "counter_renamed_link_control_rejected": True,
        "owner_renamed_link_control_rejected": True,
        "counter_missing_sidecar_control_rejected": True,
        "owner_missing_sidecar_control_rejected": True,
        "counter_wrong_path_control_rejected": True,
        "owner_wrong_path_control_rejected": True,
        "counter_corrupt_sidecar_control_rejected": True,
        "owner_corrupt_sidecar_control_rejected": True,
        "owner_tasty_initializer_and_read_decompiled": True,
        "counter_tasty_get_decompiled": True,
        "owner_from_tasty_semantics_verified": True,
        "isolated_from_tasty_runtime_output": from_tasty_output,
        "negative_control_diagnostics": negative_diagnostics,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "consumer_methodrefs": [
            'example/CounterOwner."<init>":(I)V',
            "example/CounterOwner.read:()I",
        ],
        "compile_classpath_order": ["rust-batch-root", *[str(path) for path in toolchain.runtime_jars]],
        "runtime_classpath_order": [
            "rust-batch-root", "consumer-output", "java-driver-output",
            *[str(path) for path in toolchain.runtime_jars],
        ],
        "from_tasty_runtime_classpath_order": [
            "scala-from-tasty-output", "original-rust-Counter.class-only-root",
            "java-driver-output", *[str(path) for path in toolchain.runtime_jars],
        ],
        "compile_commands": compile_commands,
        "runtime_commands": runtime_commands,
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "compiler_artifacts_sha256": toolchain.artifacts,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    generation = subcommands.add_parser("generate", help="explicitly record/check independent fixtures")
    generation.add_argument("--check", action="store_true", help="compare regenerated structure without writing fixtures")
    verify = subcommands.add_parser("verify", help="validate and independently compile a candidate")
    verify.add_argument("candidate", type=Path)
    verify.add_argument("--runtime", action="store_true",
                        help="inspect, decompile, compile, and execute a supported fresh fixture class")
    consume = subcommands.add_parser("consume-pair", help="compile and run a Scala client against a Rust linked pair")
    consume.add_argument("pair_root", type=Path)
    consume.add_argument("--tasty-link-uuid", required=True)
    batch = subcommands.add_parser(
        "consume-batch", help="compile and run a Scala client against a Rust linked batch"
    )
    batch.add_argument("batch_root", type=Path)
    cross_class = subcommands.add_parser(
        "consume-cross-class",
        help="compile and run a Scala client against a Rust Counter/Reader cross-class batch",
    )
    cross_class.add_argument("batch_root", type=Path)
    factory_consumer = subcommands.add_parser(
        "consume-factory",
        help="compile and run a Scala client against a Rust Counter/CounterFactory batch",
    )
    factory_consumer.add_argument("batch_root", type=Path)
    object_composition = subcommands.add_parser(
        "consume-object-composition",
        help="prove Scala 3.9 consumes Rust Counter/CounterOwner object-composition TASTy",
    )
    object_composition.add_argument("batch_root", type=Path)
    subcommands.add_parser("check", help="regenerate/check fixtures, compile originals/reduced Answer, run negative controls")
    args = parser.parse_args()
    if args.command == "consume-pair":
        toolchain = Toolchain()
        print(json.dumps(consume_rust_pair(toolchain, args.pair_root, args.tasty_link_uuid),
                         indent=2, sort_keys=True))
        return
    if args.command == "consume-batch":
        toolchain = Toolchain()
        print(json.dumps(consume_rust_batch(toolchain, args.batch_root), indent=2, sort_keys=True))
        return
    if args.command == "consume-cross-class":
        toolchain = Toolchain()
        print(json.dumps(consume_cross_class_batch(toolchain, args.batch_root), indent=2, sort_keys=True))
        return
    if args.command == "consume-factory":
        toolchain = Toolchain()
        print(json.dumps(consume_factory_batch(toolchain, args.batch_root), indent=2, sort_keys=True))
        return
    if args.command == "consume-object-composition":
        toolchain = Toolchain()
        print(json.dumps(consume_object_composition_batch(toolchain, args.batch_root), indent=2, sort_keys=True))
        return
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
