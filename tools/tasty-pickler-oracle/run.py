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
    command = [str(a) for a in args]
    task_tmp = os.environ.get("TMPDIR")
    if task_tmp and command:
        executable = Path(command[0]).name
        if executable == "java":
            command.insert(1, f"-Djava.io.tmpdir={task_tmp}")
        elif executable in ("javac", "javap"):
            command.insert(1, f"-J-Djava.io.tmpdir={task_tmp}")
            command.insert(1, "-J-XX:-UsePerfData")
    try:
        result = subprocess.run(command, cwd=cwd, text=True,
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


def _classfile_without_method(class_bytes: bytes, method_name: str, descriptor: str) -> bytes:
    """Remove one method without changing its linked TASTY metadata."""
    offset = 8
    cp_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    utf8: dict[int, str] = {}
    index = 1
    while index < cp_count:
        tag = class_bytes[offset]
        offset += 1
        if tag == 1:
            length = int.from_bytes(class_bytes[offset:offset + 2], "big")
            offset += 2
            utf8[index] = class_bytes[offset:offset + length].decode("utf-8")
            offset += length
        elif tag in (3, 4):
            offset += 4
        elif tag in (5, 6):
            offset += 8
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            offset += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            offset += 4
        elif tag == 15:
            offset += 3
        else:
            raise RuntimeError(f"unknown classfile constant-pool tag {tag}")
        index += 1
    offset += 6
    interfaces = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2 + 2 * interfaces

    def skip_members(position: int, count: int) -> int:
        for _ in range(count):
            position += 6
            attributes = int.from_bytes(class_bytes[position:position + 2], "big")
            position += 2
            for _ in range(attributes):
                length = int.from_bytes(class_bytes[position + 2:position + 6], "big")
                position += 6 + length
        return position

    fields = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    offset = skip_members(offset, fields)
    count_position = offset
    method_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    kept: list[bytes] = []
    removed = 0
    for _ in range(method_count):
        start = offset
        name_index = int.from_bytes(class_bytes[offset + 2:offset + 4], "big")
        descriptor_index = int.from_bytes(class_bytes[offset + 4:offset + 6], "big")
        offset += 6
        attributes = int.from_bytes(class_bytes[offset:offset + 2], "big")
        offset += 2
        for _ in range(attributes):
            length = int.from_bytes(class_bytes[offset + 2:offset + 6], "big")
            offset += 6 + length
        if utf8.get(name_index) == method_name and utf8.get(descriptor_index) == descriptor:
            removed += 1
        else:
            kept.append(class_bytes[start:offset])
    if removed != 1:
        raise RuntimeError(f"expected one {method_name}{descriptor} method, removed {removed}")
    return (class_bytes[:count_position] + len(kept).to_bytes(2, "big")
            + b"".join(kept) + class_bytes[offset:])


def _flip_tasty_link(class_bytes: bytes, *, rename_attribute: bool = False) -> bytes:
    result = bytearray(class_bytes)
    if rename_attribute:
        marker = b"TASTY"
        matches = [match.start() for match in re.finditer(marker, bytes(result))]
        if len(matches) != 1:
            raise RuntimeError(f"expected one TASTY attribute-name string, found {len(matches)}")
        result[matches[0] + len(marker) - 1] = ord("X")
    else:
        uuid = _class_tasty_link(class_bytes)
        if uuid is None:
            raise RuntimeError("classfile has no TASTY link to corrupt")
        matches = [match.start() for match in re.finditer(re.escape(uuid), bytes(result))]
        if len(matches) != 1:
            raise RuntimeError(f"expected one TASTY link UUID, found {len(matches)}")
        result[matches[0]] ^= 0x80
    return bytes(result)


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
        self.compiler_version = version.strip()
        self.javac_version = execute([self.javac, "-version"]).strip()
        self.javap_version = execute([self.javap, "-version"]).strip()

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


def consume_object_composition_batch(
    toolchain: Toolchain, batch_root: Path, *, constructor_injection: bool = False
) -> dict:
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
        if constructor_injection:
            roots[f"wrong-class-path-{class_name}"] = (
                relative_paths - {f"example/{class_name}.class"}
                | {f"misplaced/{class_name}.class"}
            )
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

    consumer_source = ROOT / (
        "tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala"
        if constructor_injection
        else "tests/fixtures/jvm_tasty_object_composition/Consumer.scala"
    )
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

    temporary_prefix = (
        "dotty-scala-constructor-injection-consumer-"
        if constructor_injection
        else "dotty-scala-object-composition-consumer-"
    )
    with tempfile.TemporaryDirectory(prefix=temporary_prefix) as temporary:
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
            consumer_methodrefs = [
                'example/CounterOwner."<init>":(Lexample/Counter;)V'
                if constructor_injection
                else 'example/CounterOwner."<init>":(I)V',
                "example/CounterOwner.read:()I",
            ]
            if constructor_injection:
                consumer_methodrefs.extend((
                    'example/Counter."<init>":(I)V',
                    "example/Counter.set:(I)V",
                ))
            for reference in consumer_methodrefs:
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
            expected_output = "-9" if constructor_injection else "742"
            if output != [expected_output]:
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

            if constructor_injection:
                wrong_class_path = batch_root / f"wrong-class-path-{class_name}"
                wrong_client = work / f"wrong-class-path-{class_name}-client"
                wrong_client.mkdir()
                toolchain.compiler(
                    ["-d", wrong_client, client_source],
                    classpath=os.pathsep.join((str(wrong_class_path), toolchain.runtime_classpath)),
                )
                wrong_driver_source = work / f"wrong-class-path-{class_name}-src/Runner.java"
                wrong_driver_source.parent.mkdir()
                wrong_driver_source.write_text(
                    "public final class Runner { public static void main(String[] args) { "
                    "System.out.println(consumer.Consumer$.MODULE$.run()); } }\n"
                )
                wrong_driver = work / f"wrong-class-path-{class_name}-driver"
                wrong_driver.mkdir()
                wrong_driver_classpath = os.pathsep.join((
                    str(wrong_class_path), str(wrong_client), toolchain.runtime_classpath,
                ))
                execute([
                    toolchain.javac, "-classpath", wrong_driver_classpath,
                    "-d", wrong_driver, wrong_driver_source,
                ])
                wrong_runtime = subprocess.run(
                    [toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath",
                     os.pathsep.join((str(wrong_class_path), str(wrong_client), str(wrong_driver),
                                      toolchain.runtime_classpath)), "Runner"],
                    cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180,
                )
                failure = wrong_runtime.stdout
                if wrong_runtime.returncode == 0 or f"NoClassDefFoundError: example/{class_name}" not in failure:
                    raise RuntimeError(
                        f"{class_name} misplaced classfile runtime did not fail at class loading: {failure}"
                    )
                negative_diagnostics[f"{class_name}.wrong_classfile_path_runtime"] = failure.strip()

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

        if constructor_injection:
            wrong_formal = work / "WrongConstructorFormal.scala"
            wrong_formal.write_text(
                "package consumer\n"
                "class Other\n"
                "object WrongConstructorFormal:\n"
                "  def build(other: Other): example.CounterOwner =\n"
                "    new example.CounterOwner(counter = other)\n"
            )
            try:
                compile_source("wrong-constructor-formal", first, wrong_formal)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError("CounterOwner accepted an unrelated constructor argument type")
            if "Other" not in failure or "Counter" not in failure:
                raise RuntimeError(f"wrong constructor formal failed for an unexpected reason: {failure}")
            negative_diagnostics["CounterOwner.wrong_constructor_formal"] = failure.strip()

        sidecar_root = work / "from-tasty-input"
        sidecar_package = sidecar_root / "example"
        sidecar_package.mkdir(parents=True)
        for class_name in classes:
            shutil.copyfile(first / f"example/{class_name}.tasty", sidecar_package / f"{class_name}.tasty")
        owner_tasty = sidecar_package / "CounterOwner.tasty"
        decompiled = toolchain.compiler([owner_tasty], decompile=True)
        expected_semantics = (
            (
                "class CounterOwner(counter: example.Counter)",
                "private[this] val inner: example.Counter = CounterOwner.this.counter",
                "def read(): scala.Int = CounterOwner.this.inner.get()",
            )
            if constructor_injection
            else (
                "private[this] val inner: example.Counter = new example.Counter(seed)",
                "def read(): scala.Int = CounterOwner.this.inner.get()",
            )
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
            "-verbose", "-private", "-c", "example.CounterOwner",
        ])
        rebuilt_shape = (
            (
                "private final example.Counter inner;",
                "descriptor: (Lexample/Counter;)V",
                "aload_1",
                "putfield",
                "Field inner:Lexample/Counter;",
                "getfield",
                "invokevirtual",
                "example/Counter.get:()I",
            )
            if constructor_injection
            else (
                "private final example.Counter inner;",
                "invokespecial",
                'example/Counter."<init>":(I)V',
                "putfield",
                "Field inner:Lexample/Counter;",
                "getfield",
                "invokevirtual",
                "example/Counter.get:()I",
            )
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
        if constructor_injection:
            driver_body = (
                "example.Counter counter = new example.Counter(7); "
                "example.CounterOwner owner = new example.CounterOwner(counter); "
                "if (owner.read() != 7) throw new AssertionError(\"initial alias\"); "
                "counter.set(42); "
                "if (owner.read() != 42) throw new AssertionError(\"lost alias\"); "
                "counter.set(-9); "
                "if (owner.read() != -9) throw new AssertionError(\"lost mutation\"); "
                "System.out.println(owner.read());"
            )
        else:
            driver_body = (
                "example.CounterOwner a = new example.CounterOwner(7); "
                "example.CounterOwner b = new example.CounterOwner(42); "
                "if (a.read() != 7 || b.read() != 42) "
                "throw new AssertionError(\"bad TASTy rebuild\"); "
                "System.out.println(a.read() * 100 + b.read());"
            )
        from_tasty_driver_source.write_text(
            "public final class FromTastyRunner { public static void main(String[] args) { "
            + driver_body + " } }\n"
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
        expected_from_tasty_output = "-9" if constructor_injection else "742"
        if from_tasty_output != [expected_from_tasty_output]:
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
        "counter_wrong_classfile_path_runtime_rejected": constructor_injection,
        "owner_wrong_classfile_path_runtime_rejected": constructor_injection,
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
        "constructor_parameter_alias_decompiled": constructor_injection,
        "wrong_constructor_formal_type_rejected": constructor_injection,
        "owner_from_tasty_semantics_verified": True,
        "isolated_from_tasty_runtime_output": from_tasty_output,
        "negative_control_diagnostics": negative_diagnostics,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "consumer_methodrefs": [
            'example/CounterOwner."<init>":(Lexample/Counter;)V'
            if constructor_injection
            else 'example/CounterOwner."<init>":(I)V',
            "example/CounterOwner.read:()I",
            *(
                ['example/Counter."<init>":(I)V', "example/Counter.set:(I)V"]
                if constructor_injection else []
            ),
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
        "jdk_implementor": toolchain.jdk_release["IMPLEMENTOR"],
        "jdk_implementor_version": toolchain.jdk_release["IMPLEMENTOR_VERSION"],
        "javac_version": toolchain.javac_version,
        "javap_version": toolchain.javap_version,
        "scala_compiler_version": toolchain.compiler_version,
        "compiler_artifacts_sha256": toolchain.artifacts,
    }



def consume_reference_constructor_batch(toolchain: Toolchain, batch_root: Path) -> dict:
    """Consume a Rust Counter -> Owner -> Factory graph with a separate Scala client."""
    class_methods = {
        "Counter": ("get", "()I"),
        "CounterOwner": ("read", "()I"),
        "CounterOwnerFactory": ("wrap", "(Lexample/Counter;)Lexample/CounterOwner;"),
    }
    relative_paths = {
        f"example/{name}.{suffix}"
        for name in class_methods
        for suffix in ("class", "tasty")
    }
    roots = (batch_root / "fresh-a", batch_root / "fresh-b")
    for root in roots:
        actual = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
        if actual != relative_paths:
            raise RuntimeError(f"{root.name} must contain exactly the six Rust artifacts: {sorted(actual)}")

    hashes: dict[str, dict[str, str]] = {}
    links: dict[str, str] = {}
    for name in class_methods:
        class_path = f"example/{name}.class"
        tasty_path = f"example/{name}.tasty"
        first_class = roots[0] / class_path
        first_tasty = roots[0] / tasty_path
        class_bytes = first_class.read_bytes()
        tasty_bytes = first_tasty.read_bytes()
        if class_bytes[6:8] != (65).to_bytes(2, "big"):
            raise RuntimeError(f"Rust {class_path} is not classfile major 65")
        class_uuid = _class_tasty_link(class_bytes)
        tasty_uuid = _tasty_header_uuid(tasty_bytes)
        if class_uuid is None or class_uuid != tasty_uuid:
            raise RuntimeError(f"Rust {name} TASTY attribute does not match its TASTy header UUID")
        for path in (first_class, first_tasty):
            relative = path.relative_to(roots[0])
            if path.read_bytes() != (roots[1] / relative).read_bytes():
                raise RuntimeError(f"fresh Rust output differs for {relative}")
        hashes[name] = {"class_sha256": digest(first_class), "tasty_sha256": digest(first_tasty)}
        links[name] = class_uuid.hex()
    if len(set(links.values())) != 3:
        raise RuntimeError(f"each producer class must have its own TASTy link UUID: {links}")

    compiler_jars = [Path(path) for path in toolchain.classpath.split(os.pathsep)]
    for jar in compiler_jars:
        with zipfile.ZipFile(jar) as archive:
            names = set(archive.namelist())
        for name in class_methods:
            if f"example/{name}.class" in names:
                raise RuntimeError(f"Scala compiler dependency shadows Rust {name}: {jar}")

    client_source = ROOT / "tests/fixtures/jvm_tasty_reference_constructor_calls/ScalaConsumer.scala"
    source_hash = digest(client_source)
    artifact_hashes_before = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in roots for relative in sorted(relative_paths)
    }
    reports: dict[str, str] = {}
    runtime_outputs: list[str] = []
    methodrefs: list[str] = []
    with tempfile.TemporaryDirectory(prefix="dotty-scala-reference-constructor-consumer-") as temporary:
        work = Path(temporary)
        client_input = work / "Consumer.scala"
        shutil.copyfile(client_source, client_input)
        for index, root in enumerate(roots):
            client_out = work / f"client-{index}"
            client_out.mkdir()
            compile_cp = os.pathsep.join((str(root), toolchain.runtime_classpath))
            toolchain.compiler(["-d", client_out, client_input], classpath=compile_cp)
            output_classes = sorted(p.relative_to(client_out).as_posix()
                                    for p in client_out.rglob("*.class"))
            if output_classes != ["consumer/Consumer$.class", "consumer/Consumer.class"]:
                raise RuntimeError(f"Scala consumer emitted unexpected files: {output_classes}")
            javap = execute([
                toolchain.javap, "-classpath", os.pathsep.join((str(client_out), str(root))),
                "-verbose", "consumer.Consumer$",
            ])
            expected_refs = (
                'example/Counter."<init>":(I)V',
                'example/CounterOwnerFactory."<init>":()V',
                "example/CounterOwnerFactory.wrap:(Lexample/Counter;)Lexample/CounterOwner;",
                "example/CounterOwner.read:()I",
                "example/Counter.set:(I)V",
            )
            for reference in expected_refs:
                if reference not in javap:
                    raise RuntimeError(f"Scala consumer lacks Methodref {reference}: {javap}")
                methodrefs.append(reference)
            if 'example/CounterOwner."<init>"' in javap:
                raise RuntimeError("positive Scala consumer directly constructs CounterOwner")
            driver_source = work / "Runner.java"
            driver_source.write_text(
                "public final class Runner { public static void main(String[] args) { "
                "System.out.println(consumer.Consumer$.MODULE$.run()); } }\n"
            )
            driver = work / f"driver-{index}"
            driver.mkdir()
            runtime_cp = os.pathsep.join((str(root), str(client_out), toolchain.runtime_classpath))
            execute([toolchain.javac, "-classpath", runtime_cp, "-d", driver, driver_source])
            java_cp = os.pathsep.join((str(root), str(client_out), str(driver), toolchain.runtime_classpath))
            runtime = execute([
                toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath", java_cp, "Runner",
            ])
            output = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if output != ["-9"]:
                raise RuntimeError(f"Rust object graph returned {output}, expected -9")
            runtime_outputs.extend(output)

        renamed_roots = (batch_root / "renamed-a", batch_root / "renamed-b")
        renamed_paths = {
            f"renamed/{name}.{suffix}"
            for name in ("Widget", "WidgetHolder", "WidgetBuilder")
            for suffix in ("class", "tasty")
        }
        renamed_hashes_before: dict[str, str] = {}
        renamed_consumer_source = client_source.read_text()
        for before, after in (
            ("example.CounterOwnerFactory", "renamed.WidgetBuilder"),
            ("example.CounterOwner", "renamed.WidgetHolder"),
            ("example.Counter", "renamed.Widget"),
            (".wrap(", ".build("),
        ):
            renamed_consumer_source = renamed_consumer_source.replace(before, after)
        renamed_input = work / "RenamedConsumer.scala"
        renamed_input.write_text(renamed_consumer_source)
        renamed_outputs: list[str] = []
        for index, root in enumerate(renamed_roots):
            actual = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
            if actual != renamed_paths:
                raise RuntimeError(f"{root.name} renamed source artifacts differ: {sorted(actual)}")
            for relative in sorted(renamed_paths):
                renamed_hashes_before[f"{root.name}/{relative}"] = digest(root / relative)
            for name in ("Widget", "WidgetHolder", "WidgetBuilder"):
                class_path = root / f"renamed/{name}.class"
                tasty_path = root / f"renamed/{name}.tasty"
                if class_path.read_bytes()[6:8] != (65).to_bytes(2, "big"):
                    raise RuntimeError(f"renamed Rust {name} classfile is not major 65")
                if _class_tasty_link(class_path.read_bytes()) != _tasty_header_uuid(tasty_path.read_bytes()):
                    raise RuntimeError(f"renamed Rust {name} classfile/TASTy UUID mismatch")
            client_out = work / f"renamed-client-{index}"
            client_out.mkdir()
            compile_cp = os.pathsep.join((str(root), toolchain.runtime_classpath))
            toolchain.compiler(["-d", client_out, renamed_input], classpath=compile_cp)
            javap = execute([
                toolchain.javap, "-classpath", os.pathsep.join((str(client_out), str(root))),
                "-verbose", "consumer.Consumer$",
            ])
            renamed_refs = (
                'renamed/Widget."<init>":(I)V',
                'renamed/WidgetBuilder."<init>":()V',
                "renamed/WidgetBuilder.build:(Lrenamed/Widget;)Lrenamed/WidgetHolder;",
                "renamed/WidgetHolder.read:()I",
                "renamed/Widget.set:(I)V",
            )
            for reference in renamed_refs:
                if reference not in javap:
                    raise RuntimeError(f"renamed Scala client lacks {reference}: {javap}")
            if 'renamed/WidgetHolder."<init>"' in javap:
                raise RuntimeError("renamed Scala client directly constructs WidgetHolder")
            driver = work / f"renamed-driver-{index}"
            driver.mkdir()
            driver_source = work / "Runner.java"
            runtime_cp = os.pathsep.join((str(root), str(client_out), toolchain.runtime_classpath))
            execute([toolchain.javac, "-classpath", runtime_cp, "-d", driver, driver_source])
            java_cp = os.pathsep.join((str(root), str(client_out), str(driver), toolchain.runtime_classpath))
            runtime = execute([
                toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath", java_cp, "Runner",
            ])
            output = [line for line in runtime.strip().splitlines()
                      if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if output != ["-9"]:
                raise RuntimeError(f"renamed Rust object graph returned {output}, expected -9")
            renamed_outputs.extend(output)

        def compile_probe(label: str, root: Path, source: str, *, werror: bool = False) -> str:
            source_path = work / f"{label}.scala"
            source_path.write_text(source)
            destination = work / f"{label}-classes"
            destination.mkdir()
            args: list[str | Path] = []
            if werror:
                args.append("-Werror")
            args.extend(("-d", destination, source_path))
            return str(toolchain.compiler(
                args, classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)),
            ))

        probes = {
            "Counter": ("def get(value: example.Counter): Int = value.get()", "get"),
            "CounterOwner": ("def read(value: example.CounterOwner): Int = value.read()", "read"),
            "CounterOwnerFactory": (
                "def wrap(value: example.CounterOwnerFactory, counter: example.Counter): "
                "example.CounterOwner = value.wrap(counter)", "wrap",
            ),
        }
        for name, (body, member) in probes.items():
            method, descriptor = class_methods[name]
            probe = f"package consumer\nobject Probe{ name }:\n  {body}\n"
            root = work / f"lookup-{name}"
            shutil.copytree(roots[0], root)
            target = root / f"example/{name}.class"
            target.write_bytes(_classfile_without_method(target.read_bytes(), method, descriptor))
            compile_probe(f"tasty-only-{name}", root, probe)
            reports[f"{name}.tasty_only"] = "passed"

            (root / f"example/{name}.tasty").unlink()
            try:
                compile_probe(f"classfile-only-{name}", root, probe)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"{name} classfile-only probe unexpectedly found {member}")
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(f"{name} classfile-only control failed for an unrelated reason: {failure}")
            reports[f"{name}.classfile_only"] = failure.strip()

            for control in ("missing", "misplaced", "corrupt", "uuid_mismatch"):
                root = work / f"{control}-{name}"
                shutil.copytree(roots[0], root)
                sidecar = root / f"example/{name}.tasty"
                if control == "missing":
                    sidecar.unlink()
                elif control == "misplaced":
                    moved = root / f"misplaced/{name}.tasty"
                    moved.parent.mkdir(parents=True)
                    sidecar.rename(moved)
                elif control == "corrupt":
                    sidecar.write_bytes(b"not a TASTy sidecar")
                else:
                    class_path = root / f"example/{name}.class"
                    class_path.write_bytes(_flip_tasty_link(class_path.read_bytes()))
                try:
                    compile_probe(f"{control}-{name}", root, probe, werror=True)
                except CommandFailure as error:
                    failure = error.output
                else:
                    raise RuntimeError(f"{name} {control} sidecar control unexpectedly compiled")
                expected_failure = {
                    "missing": r"(?i)this `?\.tasty`? file is missing",
                    "misplaced": r"(?i)this `?\.tasty`? file is missing",
                    "corrupt": r"(?is)could not be read.*not a TASTy file",
                    "uuid_mismatch": r"(?i)out of sync with its TASTy file",
                }[control]
                if not re.search(expected_failure, failure):
                    raise RuntimeError(
                        f"{name} {control} was rejected at an unexpected first layer: {failure}"
                    )
                reports[f"{name}.{control}"] = failure.strip()

            renamed_root = work / f"renamed-link-{name}"
            shutil.copytree(roots[0], renamed_root)
            renamed_class = renamed_root / f"example/{name}.class"
            renamed_class.write_bytes(_flip_tasty_link(
                _classfile_without_method(renamed_class.read_bytes(), method, descriptor),
                rename_attribute=True,
            ))
            (renamed_root / f"example/{name}.tasty").unlink()
            try:
                compile_probe(f"renamed-link-{name}", renamed_root, probe, werror=True)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"{name} renamed-link control exposed {member} from classfile")
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(f"{name} renamed-link failed for another reason: {failure}")
            reports[f"{name}.renamed_link"] = failure.strip()

        wrong_formal = work / "WrongOwnerFormal.scala"
        wrong_formal.write_text(
            "package consumer\nclass Other\nobject WrongOwnerFormal:\n"
            "  def build(other: Other): example.CounterOwner =\n"
            "    new example.CounterOwner(counter = other)\n"
        )
        wrong_formal_out = work / "wrong-owner-formal-output"
        wrong_formal_out.mkdir()
        try:
            toolchain.compiler(["-d", wrong_formal_out, wrong_formal],
                               classpath=os.pathsep.join((str(roots[0]), toolchain.runtime_classpath)))
        except CommandFailure as error:
            if "Other" not in error.output or "Counter" not in error.output:
                raise RuntimeError(f"wrong Owner formal failed unexpectedly: {error.output}") from error
            reports["wrong_owner_formal"] = error.output.strip()
        else:
            raise RuntimeError("Rust Owner TASTy accepted an unrelated constructor formal")

        wrong_return = work / "WrongFactoryReturn.scala"
        wrong_return.write_text(
            "package consumer\nobject WrongFactoryReturn:\n"
            "  def build(factory: example.CounterOwnerFactory, counter: example.Counter): example.Counter =\n"
            "    factory.wrap(counter)\n"
        )
        wrong_return_out = work / "wrong-factory-return-output"
        wrong_return_out.mkdir()
        try:
            toolchain.compiler(["-d", wrong_return_out, wrong_return],
                               classpath=os.pathsep.join((str(roots[0]), toolchain.runtime_classpath)))
        except CommandFailure as error:
            if "CounterOwner" not in error.output or "Counter" not in error.output:
                raise RuntimeError(f"wrong Factory return failed unexpectedly: {error.output}") from error
            reports["wrong_factory_return"] = error.output.strip()
        else:
            raise RuntimeError("Factory.wrap TASTy did not retain its distinct CounterOwner result")

        wrong_package = work / "WrongPackage.scala"
        wrong_package.write_text(
            "package consumer\nobject WrongPackage:\n"
            "  def read(value: wrong.CounterOwner): Int = value.read()\n"
        )
        wrong_package_out = work / "wrong-package-output"
        wrong_package_out.mkdir()
        try:
            toolchain.compiler(["-d", wrong_package_out, wrong_package],
                               classpath=os.pathsep.join((str(roots[0]), toolchain.runtime_classpath)))
        except CommandFailure as error:
            if "wrong" not in error.output or "CounterOwner" not in error.output:
                raise RuntimeError(f"wrong-package reference failed unexpectedly: {error.output}") from error
            reports["wrong_package"] = error.output.strip()
        else:
            raise RuntimeError("Rust CounterOwner TASTy resolved from the wrong package")

        driver_source = work / "NegativeRunner.java"
        driver_source.write_text(
            "public final class NegativeRunner { public static void main(String[] args) { "
            "System.out.println(consumer.Consumer$.MODULE$.run()); } }\n"
        )

        def compile_and_run(label: str, root: Path, expected: str) -> str:
            client = work / f"{label}-client"
            client.mkdir()
            toolchain.compiler(["-d", client, client_input],
                               classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))
            driver = work / f"{label}-driver"
            driver.mkdir()
            cp = os.pathsep.join((str(root), str(client), toolchain.runtime_classpath))
            execute([toolchain.javac, "-classpath", cp, "-d", driver, driver_source])
            java_cp = os.pathsep.join((str(root), str(client), str(driver), toolchain.runtime_classpath))
            result = subprocess.run(
                [toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath", java_cp,
                 "NegativeRunner"],
                cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180,
            )
            if result.returncode == 0 or expected not in result.stdout:
                raise RuntimeError(f"{label} did not fail first at {expected}: {result.stdout}")
            return result.stdout.strip()

        missing_dependency = work / "missing-owner-classfile"
        shutil.copytree(roots[0], missing_dependency)
        (missing_dependency / "example/CounterOwner.class").unlink()
        reports["missing_owner_classfile_runtime"] = compile_and_run(
            "missing-owner", missing_dependency, "NoClassDefFoundError: example/CounterOwner",
        )

        missing_constructor = work / "missing-owner-constructor"
        shutil.copytree(roots[0], missing_constructor)
        owner_class = missing_constructor / "example/CounterOwner.class"
        owner_class.write_bytes(_classfile_without_method(
            owner_class.read_bytes(), "<init>", "(Lexample/Counter;)V",
        ))
        reports["missing_owner_constructor_runtime"] = compile_and_run(
            "missing-owner-constructor", missing_constructor, "NoSuchMethodError",
        )

        sidecars = work / "from-tasty-input/example"
        sidecars.mkdir(parents=True)
        for name in class_methods:
            shutil.copyfile(roots[0] / f"example/{name}.tasty", sidecars / f"{name}.tasty")
        decompiled: dict[str, str] = {}
        for name in class_methods:
            output = toolchain.compiler([sidecars / f"{name}.tasty"], decompile=True)
            decompiled[name] = output
        factory_text = decompiled["CounterOwnerFactory"]
        for expected in (
            "def wrap(counter: example.Counter): example.CounterOwner",
            "new example.CounterOwner(counter)",
        ):
            if expected not in factory_text:
                raise RuntimeError(f"Factory TASTy decompile lacks {expected!r}: {factory_text}")
        owner_text = decompiled["CounterOwner"]
        for expected in (
            "class CounterOwner(counter: example.Counter)",
            "private[this] val inner: example.Counter = CounterOwner.this.counter",
            "CounterOwner.this.counter",
        ):
            if expected not in owner_text:
                raise RuntimeError(f"Owner TASTy decompile lacks {expected!r}: {owner_text}")
        counter_text = decompiled["Counter"]
        counter_body_evidence = (
            "private[this] var current: scala.Int = start",
            "def get(): scala.Int = Counter.this.current",
            "def set(value: scala.Int): scala.Unit = Counter.this.current = value",
        )
        for expected in counter_body_evidence:
            if expected not in counter_text:
                raise RuntimeError(
                    f"Counter TASTy decompile lacks source-body evidence {expected!r}: {counter_text}"
                )
        counter_bad_body_controls = (
            counter_text.replace(
                "def get(): scala.Int = Counter.this.current",
                "def get(): scala.Int = 0",
                1,
            ),
            counter_text.replace(
                "def set(value: scala.Int): scala.Unit = Counter.this.current = value",
                "def set(value: scala.Int): scala.Unit = ()",
                1,
            ),
        )
        if any(all(evidence in mutant for evidence in counter_body_evidence)
               for mutant in counter_bad_body_controls):
            raise RuntimeError("Counter body evidence accepted a constant getter or no-op setter")

        rebuilt = work / "from-tasty-output"
        rebuilt.mkdir()
        sidecar_root = sidecars.parent
        toolchain.compiler([
            "-from-tasty", "-d", rebuilt,
            *(sidecars / f"{name}.tasty" for name in class_methods),
        ], classpath=os.pathsep.join((str(sidecar_root), toolchain.runtime_classpath)))
        rebuilt_classes = sorted(path.relative_to(rebuilt).as_posix()
                                 for path in rebuilt.rglob("*.class"))
        if rebuilt_classes != [f"example/{name}.class" for name in class_methods]:
            raise RuntimeError(f"-from-tasty emitted unexpected class set: {rebuilt_classes}")

    artifact_hashes_after = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in roots for relative in sorted(relative_paths)
    }
    if artifact_hashes_after != artifact_hashes_before:
        raise RuntimeError("Scala consumer changed original Rust dependency artifacts")
    renamed_hashes_after = {
        f"{root.name}/{relative}": digest(root / relative)
        for root in renamed_roots for relative in sorted(renamed_paths)
    }
    if renamed_hashes_after != renamed_hashes_before:
        raise RuntimeError("renamed Scala consumer changed original Rust artifacts")
    return {
        "scala_consumer": True,
        "scala_revision": REVISION,
        "scala_source_sha256": source_hash,
        "fresh_batches_byte_identical": True,
        "fresh_batch_hashes": hashes,
        "tasty_link_uuids": links,
        "runtime_outputs": runtime_outputs,
        "renamed_widget_graph_scala_consumer": True,
        "renamed_widget_runtime_outputs": renamed_outputs,
        "consumer_methodrefs": sorted(set(methodrefs)),
        "client_factory_wrap_methodref": True,
        "client_does_not_construct_owner": True,
        "counter_tasty_only_get_resolved": True,
        "owner_tasty_only_read_resolved": True,
        "factory_tasty_only_wrap_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "owner_classfile_only_control_rejected": True,
        "factory_classfile_only_control_rejected": True,
        "counter_tasty_get_set_decompiled": True,
        "counter_tasty_source_bodies_verified": True,
        "owner_tasty_parameter_alias_decompiled": True,
        "factory_tasty_constructor_call_decompiled": True,
        "isolated_from_tasty_semantics_verified": True,
        "negative_controls": reports,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "compile_classpath_order": ["rust-six-artifact-root", *[str(p) for p in toolchain.runtime_jars]],
        "runtime_outputs_are_original_rust_classes": True,
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "scala_compiler_version": toolchain.compiler_version,
        "compiler_artifacts_sha256": toolchain.artifacts,
    }


def consume_nested_constructor_batch(toolchain: Toolchain, batch_root: Path) -> dict:
    """Consume the exact Rust nested-allocation graph with pinned Scala 3.9."""
    class_methods = {
        "Counter": ("get", "()I"),
        "CounterOwner": ("read", "()I"),
        "CounterOwnerFactory": ("create", "(I)Lexample/CounterOwner;"),
    }
    relative_paths = {
        f"example/{name}.{suffix}"
        for name in class_methods for suffix in ("class", "tasty")
    }
    roots = (batch_root / "fresh-a", batch_root / "fresh-b")
    before: dict[str, str] = {}
    hashes: dict[str, dict[str, str]] = {}
    links: dict[str, str] = {}
    for root in roots:
        actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
        if actual != relative_paths:
            raise RuntimeError(f"{root.name} must contain exactly the six Rust artifacts: {sorted(actual)}")
    for name in class_methods:
        class_path = f"example/{name}.class"
        tasty_path = f"example/{name}.tasty"
        class_bytes = (roots[0] / class_path).read_bytes()
        tasty_bytes = (roots[0] / tasty_path).read_bytes()
        if class_bytes[6:8] != (65).to_bytes(2, "big"):
            raise RuntimeError(f"Rust {class_path} is not classfile major 65")
        link = _class_tasty_link(class_bytes)
        if link is None or link != _tasty_header_uuid(tasty_bytes):
            raise RuntimeError(f"Rust {name} TASTY attribute does not match its TASTy header UUID")
        for root in roots:
            relative = Path(class_path)
            if (roots[0] / relative).read_bytes() != (root / relative).read_bytes():
                raise RuntimeError(f"fresh Rust output differs for {class_path}")
            relative = Path(tasty_path)
            if (roots[0] / relative).read_bytes() != (root / relative).read_bytes():
                raise RuntimeError(f"fresh Rust output differs for {tasty_path}")
        hashes[name] = {
            "class_sha256": digest(roots[0] / class_path),
            "tasty_sha256": digest(roots[0] / tasty_path),
        }
        links[name] = link.hex()
    if len(set(links.values())) != 3:
        raise RuntimeError(f"each Rust producer class must have its own UUID: {links}")

    for jar in (Path(path) for path in toolchain.classpath.split(os.pathsep)):
        with zipfile.ZipFile(jar) as archive:
            entries = set(archive.namelist())
        if any(f"example/{name}.class" in entries for name in class_methods):
            raise RuntimeError(f"pinned Scala compiler jar shadows a Rust producer class: {jar}")

    client_source = ROOT / "tests/fixtures/jvm_tasty_nested_constructor_calls/ScalaConsumer.scala"
    source_hash = digest(client_source)
    before = {f"{root.name}/{relative}": digest(root / relative)
              for root in roots for relative in sorted(relative_paths)}
    reports: dict[str, str] = {}
    outputs: list[str] = []
    with tempfile.TemporaryDirectory(prefix="dotty-scala-nested-constructor-consumer-") as temporary:
        work = Path(temporary)
        client_input = work / "ScalaConsumer.scala"
        shutil.copyfile(client_source, client_input)

        def compile_probe(label: str, root: Path, source: str, *, werror: bool = False) -> str:
            source_path = work / f"{label}.scala"
            source_path.write_text(source)
            output = work / f"{label}-classes"
            output.mkdir()
            args: list[str | Path] = []
            if werror:
                args.append("-Werror")
            args.extend(("-release", "21", "-d", output, source_path))
            return toolchain.compiler(args, classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))

        for index, root in enumerate(roots):
            client_out = work / f"client-{index}"
            client_out.mkdir()
            toolchain.compiler(["-release", "21", "-d", client_out, client_input],
                               classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))
            java_source = work / "Runner.java"
            java_source.write_text(
                "public final class Runner { public static void main(String[] args) { "
                "System.out.println(consumer.ScalaConsumer$.MODULE$.run()); } }\n"
            )
            driver = work / f"driver-{index}"
            driver.mkdir()
            cp = os.pathsep.join((str(root), str(client_out), toolchain.runtime_classpath))
            execute([toolchain.javac, "--release", "21", "-classpath", cp, "-d", driver, java_source])
            java_cp = os.pathsep.join((str(root), str(client_out), str(driver), toolchain.runtime_classpath))
            runtime = execute([toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath", java_cp, "Runner"])
            lines = [line for line in runtime.strip().splitlines()
                     if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if lines != ["7,-9,3"]:
                raise RuntimeError(f"Rust nested object graph returned {lines}, expected ['7,-9,3']")
            outputs.extend(lines)

        renamed_roots = (batch_root / "renamed-a", batch_root / "renamed-b")
        renamed_methods = {
            "Widget": ("get", "()I"),
            "WidgetHolder": ("read", "()I"),
            "WidgetBuilder": ("create", "(I)Lrenamed/WidgetHolder;"),
        }
        renamed_paths = {
            f"renamed/{name}.{suffix}"
            for name in renamed_methods for suffix in ("class", "tasty")
        }
        renamed_source = client_source.read_text()
        for before_name, after_name in (
            ("example.CounterOwnerFactory", "renamed.WidgetBuilder"),
            ("example.CounterOwner", "renamed.WidgetHolder"),
            ("example.Counter", "renamed.Widget"),
            ("CounterOwnerFactory", "WidgetBuilder"),
            ("CounterOwner", "WidgetHolder"),
            ("Counter", "Widget"),
        ):
            renamed_source = renamed_source.replace(before_name, after_name)
        renamed_input = work / "RenamedScalaConsumer.scala"
        renamed_input.write_text(renamed_source)
        renamed_hashes: dict[str, str] = {}
        for index, root in enumerate(renamed_roots):
            actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
            if actual != renamed_paths:
                raise RuntimeError(f"{root.name} must contain its renamed six artifacts: {sorted(actual)}")
            for name in renamed_methods:
                class_file = root / f"renamed/{name}.class"
                tasty_file = root / f"renamed/{name}.tasty"
                if _class_tasty_link(class_file.read_bytes()) != _tasty_header_uuid(tasty_file.read_bytes()):
                    raise RuntimeError(f"renamed {name} classfile/TASTy UUID mismatch")
            for relative in sorted(renamed_paths):
                left = renamed_roots[0] / relative
                right = root / relative
                if left.read_bytes() != right.read_bytes():
                    raise RuntimeError(f"renamed fresh sessions differ for {relative}")
                renamed_hashes[f"{root.name}/{relative}"] = digest(right)
            output = work / f"renamed-client-{index}"
            output.mkdir()
            toolchain.compiler(["-release", "21", "-d", output, renamed_input],
                               classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))
            driver = work / f"renamed-driver-{index}"
            driver.mkdir()
            cp = os.pathsep.join((str(root), str(output), toolchain.runtime_classpath))
            execute([toolchain.javac, "--release", "21", "-classpath", cp, "-d", driver, java_source])
            runtime_cp = os.pathsep.join((str(root), str(output), str(driver), toolchain.runtime_classpath))
            runtime = execute([toolchain.java, "-Xverify:all", "-XX:-UsePerfData", "-classpath", runtime_cp, "Runner"])
            lines = [line for line in runtime.strip().splitlines()
                     if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if lines != ["7,-9,3"]:
                raise RuntimeError(f"renamed/reordered graph returned {lines}, expected ['7,-9,3']")
        reports["renamed_reordered"] = "Scala compile and -Xverify:all runtime passed"

        renamed_classfile_only = work / "renamed-classfile-only"
        shutil.copytree(renamed_roots[0], renamed_classfile_only)
        renamed_builder_class = renamed_classfile_only / "renamed/WidgetBuilder.class"
        renamed_builder_class.write_bytes(_classfile_without_method(
            renamed_builder_class.read_bytes(), "create", "(I)Lrenamed/WidgetHolder;",
        ))
        (renamed_classfile_only / "renamed/WidgetBuilder.tasty").unlink()
        renamed_probe = (
            "package consumer\nobject RenamedProbe:\n"
            "  def create(value: renamed.WidgetBuilder): renamed.WidgetHolder = value.create(1)\n"
        )
        try:
            compile_probe("renamed-classfile-only", renamed_classfile_only, renamed_probe)
        except CommandFailure as error:
            if "create is not a member of renamed.WidgetBuilder" not in error.output:
                raise RuntimeError(f"renamed classfile-only lookup failed unexpectedly: {error.output}") from error
            reports["renamed_classfile_only"] = error.output.strip()
        else:
            raise RuntimeError("renamed classfile-only control resolved create without its TASTy")

        # The valid alternate batch changes the Factory result to Counter.
        # A client requiring CounterOwner must reject the mismatch at typing.
        wrong_result_root = batch_root / "wrong-result"
        wrong_result_paths = {
            f"example/{name}.{suffix}"
            for name in class_methods for suffix in ("class", "tasty")
        }
        actual_wrong_result_paths = {
            path.relative_to(wrong_result_root).as_posix()
            for path in wrong_result_root.rglob("*") if path.is_file()
        }
        if actual_wrong_result_paths != wrong_result_paths:
            raise RuntimeError(
                "wrong-result batch must contain exactly the six class/TASTy artifacts: "
                f"{sorted(actual_wrong_result_paths)}"
            )
        wrong_result_uuids: set[bytes] = set()
        for name in class_methods:
            class_bytes = (wrong_result_root / f"example/{name}.class").read_bytes()
            tasty_bytes = (wrong_result_root / f"example/{name}.tasty").read_bytes()
            link = _class_tasty_link(class_bytes)
            if class_bytes[6:8] != (65).to_bytes(2, "big"):
                raise RuntimeError(f"wrong-result {name} classfile is not major version 65")
            if link is None or link != _tasty_header_uuid(tasty_bytes):
                raise RuntimeError(f"wrong-result {name} classfile/TASTy UUID mismatch")
            wrong_result_uuids.add(link)
        if len(wrong_result_uuids) != len(class_methods):
            raise RuntimeError("wrong-result batch does not have one distinct TASTY UUID per class")

        wrong_result_probe = (
            "package consumer\nobject WrongResult:\n"
            "  def create(value: example.CounterOwnerFactory): example.CounterOwner = value.create(1)\n"
        )

        def is_expected_wrong_result_diagnostic(output: str) -> bool:
            return (
                re.search(r"(?m)^-- \[E007\] Type Mismatch Error:", output) is not None
                and re.search(r"(?m)^\s*\|\s+Found:\s+example\.Counter\s*$", output) is not None
                and re.search(r"(?m)^\s*\|\s+Required:\s+example\.CounterOwner\s*$", output) is not None
                and re.search(r"(?m)^1 error found$", output) is not None
            )

        try:
            compile_probe("wrong-factory-result", wrong_result_root, wrong_result_probe)
        except CommandFailure as error:
            failure = error.output
        else:
            raise RuntimeError("altered Factory result unexpectedly typed as CounterOwner")
        if not is_expected_wrong_result_diagnostic(failure):
            raise RuntimeError(f"altered Factory result failed for an unexpected reason: {failure}")
        reports["wrong_factory_result"] = failure.strip()

        missing_wrong_result_root = work / "wrong-result-missing-classpath"
        missing_wrong_result_root.mkdir()
        try:
            compile_probe(
                "wrong-factory-result-missing-classpath",
                missing_wrong_result_root,
                wrong_result_probe,
            )
        except CommandFailure as error:
            missing_classpath_failure = error.output
        else:
            raise RuntimeError("wrong-result control unexpectedly compiled without its classpath")
        if is_expected_wrong_result_diagnostic(missing_classpath_failure):
            raise RuntimeError("missing-classpath E006 was accepted as the altered-result E007")
        if not re.search(r"(?m)^-- \[E006\] Not Found Error:", missing_classpath_failure) or \
                "Not found: example" not in missing_classpath_failure or \
                not re.search(r"(?m)^2 errors found$", missing_classpath_failure):
            raise RuntimeError(
                "missing-classpath regression did not reach the expected E006 failure: "
                f"{missing_classpath_failure}"
            )
        reports["wrong_factory_result_missing_classpath"] = missing_classpath_failure.strip()

        # Compilation-only ABI control: hide the Owner sidecar and remove its
        # reference constructor from the classfile. No modified producer
        # classfile is ever placed on a runtime classpath.
        wrong_ctor = work / "wrong-owner-constructor"
        shutil.copytree(roots[0], wrong_ctor)
        owner_class = wrong_ctor / "example/CounterOwner.class"
        owner_class.write_bytes(_classfile_without_method(
            owner_class.read_bytes(), "<init>", "(Lexample/Counter;)V",
        ))
        (wrong_ctor / "example/CounterOwner.tasty").unlink()
        wrong_ctor_probe = work / "WrongOwnerConstructor.scala"
        wrong_ctor_probe.write_text(
            "package consumer\nobject WrongOwnerConstructor:\n"
            "  def create(): example.CounterOwner =\n"
            "    new example.CounterOwner(new example.Counter(1))\n"
        )
        wrong_ctor_out = work / "wrong-owner-classes"
        wrong_ctor_out.mkdir()
        try:
            toolchain.compiler(["-release", "21", "-d", wrong_ctor_out, wrong_ctor_probe],
                               classpath=os.pathsep.join((str(wrong_ctor), toolchain.runtime_classpath)))
        except CommandFailure as error:
            if "-- [E173] Reference Error" not in error.output or \
                    "constructor CounterOwner cannot be accessed as a member" not in error.output:
                raise RuntimeError(f"wrong Owner constructor failed unexpectedly: {error.output}") from error
            reports["wrong_factory_constructor_signature"] = error.output.strip()
        else:
            raise RuntimeError("wrong Owner constructor signature unexpectedly compiled from classfile only")

        probes = {
            "Counter": ("def get(value: example.Counter): Int = value.get()", "get"),
            "CounterOwner": ("def read(value: example.CounterOwner): Int = value.read()", "read"),
            "CounterOwnerFactory": (
                "def create(value: example.CounterOwnerFactory): example.CounterOwner = value.create(1)",
                "create",
            ),
        }
        for name, (body, member) in probes.items():
            method, descriptor = class_methods[name]
            source = f"package consumer\nobject Probe:\n  {body}\n"
            root = work / f"tasty-only-{name}"
            shutil.copytree(roots[0], root)
            class_path = root / f"example/{name}.class"
            class_path.write_bytes(_classfile_without_method(class_path.read_bytes(), method, descriptor))
            compile_probe(f"tasty-only-{name}", root, source)
            reports[f"{name}.tasty_only"] = "passed"
            (root / f"example/{name}.tasty").unlink()
            try:
                compile_probe(f"classfile-only-{name}", root, source)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"{name} classfile-only control unexpectedly resolved {member}")
            if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", failure):
                raise RuntimeError(f"{name} classfile-only control failed for another reason: {failure}")
            reports[f"{name}.classfile_only"] = failure.strip()

        # The official reader must recover the original nested expression from the
        # unmodified Rust sidecars, not from a Scala-produced oracle fixture.
        sidecars = work / "from-tasty-input/example"
        sidecars.mkdir(parents=True)
        for name in class_methods:
            shutil.copyfile(roots[0] / f"example/{name}.tasty", sidecars / f"{name}.tasty")
        decompiled = {
            name: toolchain.compiler([sidecars / f"{name}.tasty"], decompile=True)
            for name in class_methods
        }
        factory = decompiled["CounterOwnerFactory"]
        for expected in (
            "def create(seed: scala.Int): example.CounterOwner",
            "new example.CounterOwner(new example.Counter(seed))",
        ):
            if expected not in factory:
                raise RuntimeError(f"Factory TASTy decompile lacks {expected!r}: {factory}")
        owner = decompiled["CounterOwner"]
        if "private[this] val inner: example.Counter = CounterOwner.this.counter" not in owner:
            raise RuntimeError(f"Owner TASTy lost its original inner parameter alias: {owner}")
        counter = decompiled["Counter"]
        for expected in (
            "def get(): scala.Int = Counter.this.current",
            "def set(value: scala.Int): scala.Unit = Counter.this.current = value",
        ):
            if expected not in counter:
                raise RuntimeError(f"Counter TASTy lost source method {expected!r}: {counter}")
        rebuilt = work / "from-tasty-output"
        rebuilt.mkdir()
        toolchain.compiler([
            "-release", "21", "-from-tasty", "-d", rebuilt,
            *(sidecars / f"{name}.tasty" for name in class_methods),
        ], classpath=os.pathsep.join((str(sidecars.parent), toolchain.runtime_classpath)))
        rebuilt_classes = sorted(p.relative_to(rebuilt).as_posix() for p in rebuilt.rglob("*.class"))
        if rebuilt_classes != [f"example/{name}.class" for name in class_methods]:
            raise RuntimeError(f"-from-tasty emitted unexpected classes: {rebuilt_classes}")

        # Fail-closed sidecar controls on the third artifact exercise missing,
        # misplaced, corrupt and stale UUID links through the pinned reader.
        for control in ("missing", "misplaced", "corrupt", "uuid_mismatch"):
            root = work / f"{control}-factory"
            shutil.copytree(roots[0], root)
            tasty_path = root / "example/CounterOwnerFactory.tasty"
            class_path = root / "example/CounterOwnerFactory.class"
            if control == "missing":
                tasty_path.unlink()
            elif control == "misplaced":
                moved = root / "elsewhere/CounterOwnerFactory.tasty"
                moved.parent.mkdir(parents=True)
                tasty_path.rename(moved)
            elif control == "corrupt":
                tasty_path.write_bytes(b"not a TASTy sidecar")
            else:
                class_path.write_bytes(_flip_tasty_link(class_path.read_bytes()))
            source = "package consumer\nobject Probe:\n  def create(value: example.CounterOwnerFactory): example.CounterOwner = value.create(1)\n"
            try:
                compile_probe(control, root, source, werror=True)
            except CommandFailure as error:
                failure = error.output
            else:
                raise RuntimeError(f"Factory {control} sidecar control unexpectedly compiled")
            pattern = {
                "missing": r"(?i)this `?\.tasty`? file is missing",
                "misplaced": r"(?i)this `?\.tasty`? file is missing",
                "corrupt": r"(?is)could not be read.*not a TASTy file",
                "uuid_mismatch": r"(?i)out of sync with its TASTy file",
            }[control]
            if not re.search(pattern, failure):
                raise RuntimeError(f"Factory {control} failed at an unexpected stage: {failure}")
            reports[f"factory_{control}"] = failure.strip()

        partial = work / "partial-six-artifact-batch"
        shutil.copytree(roots[0], partial)
        (partial / "example/CounterOwnerFactory.tasty").unlink()
        actual = {p.relative_to(partial).as_posix() for p in partial.rglob("*") if p.is_file()}
        if actual == relative_paths:
            raise RuntimeError("partial six-artifact control was accepted")
        partial_probe = (
            "package consumer\nobject PartialBatch:\n"
            "  def create(value: example.CounterOwnerFactory): example.CounterOwner = value.create(1)\n"
        )
        try:
            compile_probe("partial-batch", partial, partial_probe, werror=True)
        except CommandFailure as error:
            if "This `.tasty` file is missing" not in error.output or "1 error found" not in error.output:
                raise RuntimeError(f"partial batch failed for an unexpected reason: {error.output}") from error
            reports["partial_batch"] = error.output.strip()
        else:
            raise RuntimeError("partial six-artifact batch unexpectedly compiled")

    after = {f"{root.name}/{relative}": digest(root / relative)
             for root in roots for relative in sorted(relative_paths)}
    if before != after:
        raise RuntimeError("Scala consumer changed original Rust classfile/TASTy bytes")
    renamed_after = {f"{root.name}/{relative}": digest(root / relative)
                     for root in renamed_roots for relative in sorted(renamed_paths)}
    if renamed_after != renamed_hashes:
        raise RuntimeError("renamed Scala consumer changed Rust artifacts")
    return {
        "scala_consumer": True,
        "scala_revision": REVISION,
        "scala_compiler_version": toolchain.compiler_version,
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "scala_source_sha256": digest(ROOT / "tests/fixtures/jvm_tasty_nested_constructor_calls/NestedConstructorCalls.scala"),
        "client_source_sha256": source_hash,
        "rust_artifact_hashes": hashes,
        "tasty_link_uuids": links,
        "runtime_outputs": outputs,
        "counter_tasty_only_get_resolved": True,
        "owner_tasty_only_read_resolved": True,
        "factory_tasty_only_create_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "owner_classfile_only_control_rejected": True,
        "factory_classfile_only_control_rejected": True,
        "factory_tasty_nested_constructor_call_decompiled": True,
        "owner_tasty_parameter_alias_decompiled": True,
        "counter_tasty_get_set_decompiled": True,
        "from_tasty_classes": rebuilt_classes,
        "negative_controls": reports,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "runtime_outputs_are_original_rust_classes": True,
        "compile_classpath_order": ["rust-six-artifact-root", *[str(p) for p in toolchain.runtime_jars]],
        "compiler_artifacts_sha256": toolchain.artifacts,
        "renamed_reordered_scala_consumer": True,
        "renamed_classfile_only_control_rejected": True,
        "wrong_factory_result_rejected": True,
        "wrong_factory_result_missing_classpath_rejected": True,
        "wrong_factory_constructor_signature_rejected_at_compile": True,
    }


def _classfile_with_method_descriptor(class_bytes: bytes, method_name: str,
                                      old_descriptor: str, new_descriptor: str) -> bytes:
    """Change one method's descriptor index, preserving the rest of a valid classfile."""
    offset = 8
    cp_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    utf8: dict[int, str] = {}
    index = 1
    while index < cp_count:
        tag = class_bytes[offset]
        offset += 1
        if tag == 1:
            length = int.from_bytes(class_bytes[offset:offset + 2], "big")
            offset += 2
            utf8[index] = class_bytes[offset:offset + length].decode("utf-8")
            offset += length
        elif tag in (3, 4):
            offset += 4
        elif tag in (5, 6):
            offset += 8
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            offset += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            offset += 4
        elif tag == 15:
            offset += 3
        else:
            raise RuntimeError(f"unknown classfile constant-pool tag {tag}")
        index += 1
    new_index = next((i for i, value in utf8.items() if value == new_descriptor), None)
    if new_index is None:
        raise RuntimeError(f"classfile lacks descriptor constant {new_descriptor}")
    offset += 6
    interfaces = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2 + 2 * interfaces

    def skip_members(position: int, count: int) -> int:
        for _ in range(count):
            position += 6
            attributes = int.from_bytes(class_bytes[position:position + 2], "big")
            position += 2
            for _ in range(attributes):
                length = int.from_bytes(class_bytes[position + 2:position + 6], "big")
                position += 6 + length
        return position

    field_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset = skip_members(offset + 2, field_count)
    method_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    result = bytearray(class_bytes)
    changed = 0
    for _ in range(method_count):
        name_index = int.from_bytes(class_bytes[offset + 2:offset + 4], "big")
        descriptor_offset = offset + 4
        descriptor_index = int.from_bytes(class_bytes[descriptor_offset:descriptor_offset + 2], "big")
        attributes = int.from_bytes(class_bytes[offset + 6:offset + 8], "big")
        position = offset + 8
        for _ in range(attributes):
            length = int.from_bytes(class_bytes[position + 2:position + 6], "big")
            position += 6 + length
        if utf8.get(name_index) == method_name and utf8.get(descriptor_index) == old_descriptor:
            result[descriptor_offset:descriptor_offset + 2] = new_index.to_bytes(2, "big")
            changed += 1
        offset = position
    if changed != 1:
        raise RuntimeError(f"expected one {method_name}{old_descriptor}, changed {changed}")
    return bytes(result)


def _classfile_rename_method(class_bytes: bytes, method_name: str, descriptor: str,
                             replacement_name: str) -> bytes:
    """Rename a method entry without stripping its body or changing its ABI descriptor."""
    offset = 8
    cp_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    utf8: dict[int, str] = {}
    index = 1
    while index < cp_count:
        tag = class_bytes[offset]
        offset += 1
        if tag == 1:
            length = int.from_bytes(class_bytes[offset:offset + 2], "big")
            offset += 2
            utf8[index] = class_bytes[offset:offset + length].decode("utf-8")
            offset += length
        elif tag in (3, 4):
            offset += 4
        elif tag in (5, 6):
            offset += 8
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            offset += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            offset += 4
        elif tag == 15:
            offset += 3
        else:
            raise RuntimeError(f"unknown classfile constant-pool tag {tag}")
        index += 1
    replacement_index = next((i for i, value in utf8.items() if value == replacement_name), None)
    if replacement_index is None:
        raise RuntimeError(f"classfile lacks method name constant {replacement_name}")
    offset += 6
    interfaces = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2 + 2 * interfaces

    def skip_members(position: int, count: int) -> int:
        for _ in range(count):
            position += 6
            attributes = int.from_bytes(class_bytes[position:position + 2], "big")
            position += 2
            for _ in range(attributes):
                length = int.from_bytes(class_bytes[position + 2:position + 6], "big")
                position += 6 + length
        return position

    fields = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset = skip_members(offset + 2, fields)
    method_count = int.from_bytes(class_bytes[offset:offset + 2], "big")
    offset += 2
    result = bytearray(class_bytes)
    changed = 0
    for _ in range(method_count):
        name_index = int.from_bytes(class_bytes[offset + 2:offset + 4], "big")
        descriptor_index = int.from_bytes(class_bytes[offset + 4:offset + 6], "big")
        attributes = int.from_bytes(class_bytes[offset + 6:offset + 8], "big")
        position = offset + 8
        for _ in range(attributes):
            length = int.from_bytes(class_bytes[position + 2:position + 6], "big")
            position += 6 + length
        if utf8.get(name_index) == method_name and utf8.get(descriptor_index) == descriptor:
            result[offset + 2:offset + 4] = replacement_index.to_bytes(2, "big")
            changed += 1
        offset = position
    if changed != 1:
        raise RuntimeError(f"expected one {method_name}{descriptor}, renamed {changed}")
    return bytes(result)


def _classfile_change_internal_owner(class_bytes: bytes, expected: str, replacement: str) -> bytes:
    """Change a classfile's self/internal references with an equal-length name."""
    old, new = expected.encode("utf-8"), replacement.encode("utf-8")
    if len(old) != len(new):
        raise ValueError("owner replacement must preserve UTF-8 length")
    result = bytearray(class_bytes)
    offset = 8
    cp_count = int.from_bytes(result[offset:offset + 2], "big")
    offset += 2
    changed = 0
    index = 1
    while index < cp_count:
        tag = result[offset]
        offset += 1
        if tag == 1:
            length = int.from_bytes(result[offset:offset + 2], "big")
            start = offset + 2
            end = start + length
            if result[start:end] == old:
                result[start:end] = new
                changed += 1
            offset = end
        elif tag in (3, 4):
            offset += 4
        elif tag in (5, 6):
            offset += 8
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            offset += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            offset += 4
        elif tag == 15:
            offset += 3
        else:
            raise RuntimeError(f"unknown classfile constant-pool tag {tag}")
        index += 1
    if changed == 0:
        raise RuntimeError(f"classfile contains no internal owner {expected}")
    return bytes(result)


def consume_factory_result_batch(toolchain: Toolchain, batch_root: Path,
                                 *, reference_local: bool = False) -> dict:
    """Prove independent Scala 3.9 source lookup and runtime against Rust artifacts."""
    methods = {
        "Counter": {"get": "()I"},
        "CounterOwner": {"read": "()I"},
        "CounterOwnerFactory": {
            "create": "(I)Lexample/CounterOwner;",
            "makeCounter": "(I)Lexample/Counter;",
        },
    }
    relative_paths = {
        f"example/{name}.{suffix}"
        for name in methods for suffix in ("class", "tasty")
    }
    roots = (batch_root / "fresh-a", batch_root / "fresh-b")
    renamed_roots = (batch_root / "renamed-a", batch_root / "renamed-b")
    renamed_paths = {
        f"renamed/{name}.{suffix}"
        for name in ("Widget", "WidgetHolder", "WidgetBuilder")
        for suffix in ("class", "tasty")
    }
    hashes: dict[str, dict[str, str]] = {}
    uuids: dict[str, str] = {}
    for root in roots:
        actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
        if actual != relative_paths:
            raise RuntimeError(f"{root.name} must contain exactly the six Rust artifacts: {sorted(actual)}")
    for name in methods:
        class_path, tasty_path = f"example/{name}.class", f"example/{name}.tasty"
        class_bytes, tasty_bytes = (roots[0] / class_path).read_bytes(), (roots[0] / tasty_path).read_bytes()
        link = _class_tasty_link(class_bytes)
        if class_bytes[6:8] != (65).to_bytes(2, "big") or link is None or link != _tasty_header_uuid(tasty_bytes):
            raise RuntimeError(f"Rust {name} classfile version or TASTy UUID link is invalid")
        if any((root / class_path).read_bytes() != class_bytes or
               (root / tasty_path).read_bytes() != tasty_bytes for root in roots[1:]):
            raise RuntimeError(f"fresh Rust producer batches differ for {name}")
        hashes[name] = {"class_sha256": digest(roots[0] / class_path),
                        "tasty_sha256": digest(roots[0] / tasty_path)}
        uuids[name] = link.hex()
    if reference_local:
        expected_hashes = {
            "Counter": {
                "class_sha256": "b38143cee8571a6602865ff6062eca92dd771582f539b156100ffc9647af94ae",
                "tasty_sha256": "a0e78329d60aba318de825eaddb7668d34838197158653c1b7384188143099c7",
            },
            "CounterOwner": {
                "class_sha256": "d914c2dfc5b8d22079be304fc7c463be171394a528b71603de43342a79db91b2",
                "tasty_sha256": "9bcfbbb9d12f71a50122c6b2af960fe76ffd7f629a57738f61f4d384821f7f13",
            },
            "CounterOwnerFactory": {
                "class_sha256": "e8e786041ad543af5cd7278c460c2d4f99a70261d41a8f611297c25a51d49c7d",
                "tasty_sha256": "dda2d294a5eaa72c5060bb7089e760ba2576a8c350fcb6e93815c1653bbe52f2",
            },
        }
        if hashes != expected_hashes:
            raise RuntimeError(f"frozen original Rust artifact hashes changed: {hashes}")
    if len(set(uuids.values())) != 3:
        raise RuntimeError(f"expected a distinct TASTy UUID for each producer class: {uuids}")
    for jar in (Path(path) for path in toolchain.classpath.split(os.pathsep)):
        with zipfile.ZipFile(jar) as archive:
            if any(f"example/{name}.class" in archive.namelist() for name in methods):
                raise RuntimeError(f"Scala compiler jar shadows original Rust classes: {jar}")

    renamed_hashes: dict[str, str] = {}
    renamed_uuids: dict[str, str] = {}
    for root in renamed_roots:
        actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
        if actual != renamed_paths:
            raise RuntimeError(f"{root.name} must contain the renamed six artifacts: {sorted(actual)}")
    for name in ("Widget", "WidgetHolder", "WidgetBuilder"):
        class_path, tasty_path = f"renamed/{name}.class", f"renamed/{name}.tasty"
        class_bytes, tasty_bytes = (renamed_roots[0] / class_path).read_bytes(), \
            (renamed_roots[0] / tasty_path).read_bytes()
        link = _class_tasty_link(class_bytes)
        if class_bytes[6:8] != (65).to_bytes(2, "big") or link is None or link != _tasty_header_uuid(tasty_bytes):
            raise RuntimeError(f"renamed Rust {name} classfile/TASTy UUID link is invalid")
        if any((root / class_path).read_bytes() != class_bytes or
               (root / tasty_path).read_bytes() != tasty_bytes for root in renamed_roots[1:]):
            raise RuntimeError(f"renamed fresh Rust producer batches differ for {name}")
        renamed_uuids[name] = link.hex()
    if len(set(renamed_uuids.values())) != 3:
        raise RuntimeError(f"renamed producer classes must have distinct TASTy UUIDs: {renamed_uuids}")
    for relative in sorted(renamed_paths):
        renamed_hashes[relative] = digest(renamed_roots[0] / relative)

    client = ROOT / (
        "tests/fixtures/jvm_tasty_reference_local_constructor_flows/Consumer.scala"
        if reference_local else
        "tests/fixtures/jvm_tasty_factory_result_arguments/FactoryResultConsumer.scala"
    )
    client_hash = digest(client)
    before = {f"{root.name}/{relative}": digest(root / relative)
              for root in (*roots, *renamed_roots) for relative in
              sorted(relative_paths if root in roots else renamed_paths)}
    reports: dict[str, str] = {}
    runtime_outputs: list[str] = []
    with tempfile.TemporaryDirectory(prefix="dotty-scala-factory-result-", dir=os.environ.get("TMPDIR")) as temporary:
        work = Path(temporary)
        client_input = work / ("Consumer.scala" if reference_local else "FactoryResultConsumer.scala")
        shutil.copyfile(client, client_input)

        def compile_probe(label: str, root: Path, source: str, *, werror: bool = False) -> str:
            source_path = work / f"{label}.scala"
            source_path.write_text(source)
            output = work / f"{label}-classes"
            output.mkdir()
            args: list[str | Path] = []
            if werror:
                args.append("-Werror")
            args.extend(("-release", "21", "-d", output, source_path))
            return toolchain.compiler(args, classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))

        for index, root in enumerate(roots):
            output = work / f"client-{index}"
            output.mkdir()
            toolchain.compiler(["-release", "21", "-d", output, client_input],
                               classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))
            driver_source = work / f"Runner{index}.java"
            if reference_local:
                driver_source.write_text(
                    "public final class Runner%d { public static void main(String[] args) { "
                    "example.FactoryConsumer$.MODULE$.main(new String[0]); } }\n" % index
                )
            else:
                driver_source.write_text(
                    "public final class Runner%d { public static void main(String[] args) { "
                    "System.out.println(consumer.FactoryResultConsumer$.MODULE$.run()); } }\n" % index
                )
            driver = work / f"driver-{index}"
            driver.mkdir()
            cp = os.pathsep.join((str(root), str(output), toolchain.runtime_classpath))
            execute([toolchain.javac, "--release", "21", "-classpath", cp, "-d", driver, driver_source])
            runtime_cp = os.pathsep.join((str(root), str(output), str(driver), toolchain.runtime_classpath))
            runtime = execute([toolchain.java, "-Xverify:all", "-ea", "-XX:-UsePerfData",
                               "-classpath", runtime_cp, f"Runner{index}"])
            lines = [line for line in runtime.strip().splitlines()
                     if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            expected_output = (
                ["scala39-reference-local-constructor-flow=ok"]
                if reference_local else ["7,-9,3"]
            )
            if lines != expected_output:
                raise RuntimeError(f"original Rust classfiles returned {lines}, expected {expected_output}")
            runtime_outputs.extend(lines)

        probes = {
            ("Counter", "get"): "def get(value: example.Counter): Int = value.get()",
            ("CounterOwner", "read"): "def read(value: example.CounterOwner): Int = value.read()",
            ("CounterOwnerFactory", "create"): "def create(value: example.CounterOwnerFactory): example.CounterOwner = value.create(1)",
            ("CounterOwnerFactory", "makeCounter"): "def makeCounter(value: example.CounterOwnerFactory): example.Counter = value.makeCounter(1)",
        }
        for (name, member), body in probes.items():
            descriptor = methods[name][member]
            source = f"package witness\nobject Probe:\n  {body}\n"
            root = work / f"tasty-only-{name}-{member}"
            shutil.copytree(roots[0], root)
            class_path = root / f"example/{name}.class"
            class_path.write_bytes(_classfile_without_method(class_path.read_bytes(), member, descriptor))
            compile_probe(f"tasty-only-{name}-{member}", root, source)
            reports[f"{name}.{member}.tasty_only"] = "resolved from TASTy"
            (root / f"example/{name}.tasty").unlink()
            try:
                compile_probe(f"classfile-only-{name}-{member}", root, source)
            except CommandFailure as error:
                if not re.search(rf"(?i){member}.*(not found|not a member)|no member.*{member}", error.output):
                    raise RuntimeError(f"{name}.{member} classfile-only probe failed unexpectedly: {error.output}") from error
                reports[f"{name}.{member}.classfile_only"] = error.output.strip()
            else:
                raise RuntimeError(f"{name}.{member} resolved from the classfile without TASTy")

        sidecars = work / "from-tasty-input/example"
        sidecars.mkdir(parents=True)
        for name in methods:
            shutil.copyfile(roots[0] / f"example/{name}.tasty", sidecars / f"{name}.tasty")
        decompiled = {name: toolchain.compiler([sidecars / f"{name}.tasty"], decompile=True)
                      for name in methods}
        factory = decompiled["CounterOwnerFactory"]
        for expected in (
            "def makeCounter(seed: scala.Int): example.Counter",
            "new example.Counter(seed)",
        ):
            if expected not in factory:
                raise RuntimeError(f"Rust Factory TASTy decompile lacks {expected!r}: {factory}")
        if reference_local:
            if not re.search(
                r"def create\(`?seed(?:₂)?`?: scala\.Int\): example\.CounterOwner =\s*"
                r"\{\s*val counter: example\.Counter = "
                r"CounterOwnerFactory\.this\.makeCounter\(`?seed(?:₂)?`?\)\s*"
                r"new example\.CounterOwner\(counter\)\s*\}",
                factory,
            ):
                raise RuntimeError(
                    f"Rust Factory TASTy did not reconstruct create's BLOCK, immutable typed local, "
                    f"original helper call, local use and CounterOwner result: {factory}"
                )
            local_proof = {
                "factory_create_decompile_has_block_valdef_helper_and_local_use": True,
                "local_type": "example.Counter",
                "local_declaration": "counter",
                "helper_call": "CounterOwnerFactory.makeCounter(seed)",
                "owner_constructor_argument": "counter",
                "factory_create_result": "example.CounterOwner",
            }
        else:
            if not re.search(
                r"def create\(`?seed(?:₂)?`?: scala\.Int\): example\.CounterOwner = "
                r"new example\.CounterOwner\(CounterOwnerFactory\.this\.makeCounter\(`?seed(?:₂)?`?\)\)",
                factory,
            ):
                raise RuntimeError(f"Rust Factory TASTy lost create's selected helper call or CounterOwner result: {factory}")
            local_proof = {}
        if "class Counter(start: scala.Int)" not in decompiled["Counter"] or \
                "class CounterOwner(counter: example.Counter)" not in decompiled["CounterOwner"]:
            raise RuntimeError("Rust TASTy did not retain separate Counter and Owner declarations")
        rebuilt = work / "from-tasty-output"
        rebuilt.mkdir()
        toolchain.compiler(["-release", "21", "-from-tasty", "-d", rebuilt,
                            *(sidecars / f"{name}.tasty" for name in methods)],
                           classpath=os.pathsep.join((str(sidecars.parent), toolchain.runtime_classpath)))
        rebuilt_classes = sorted(p.relative_to(rebuilt).as_posix() for p in rebuilt.rglob("*.class"))
        if rebuilt_classes != [f"example/{name}.class" for name in methods]:
            raise RuntimeError(f"-from-tasty emitted unexpected class set: {rebuilt_classes}")

        # Each class has its own missing/misplaced/corrupt/stale UUID sidecar matrix.
        sidecar_probes = {"Counter": probes[("Counter", "get")],
                          "CounterOwner": probes[("CounterOwner", "read")],
                          "CounterOwnerFactory": probes[("CounterOwnerFactory", "create")]}
        for name, body in sidecar_probes.items():
            method = next(iter(methods[name]))
            descriptor = methods[name][method]
            source = f"package witness\nobject SidecarProbe:\n  {body}\n"
            for control in ("missing", "misplaced", "corrupt", "uuid_mismatch"):
                root = work / f"{control}-{name}"
                shutil.copytree(roots[0], root)
                tasty_path, class_path = root / f"example/{name}.tasty", root / f"example/{name}.class"
                if control == "missing":
                    tasty_path.unlink()
                elif control == "misplaced":
                    moved = root / f"elsewhere/{name}.tasty"
                    moved.parent.mkdir()
                    tasty_path.rename(moved)
                elif control == "corrupt":
                    tasty_path.write_bytes(b"not a TASTy sidecar")
                else:
                    class_path.write_bytes(_flip_tasty_link(class_path.read_bytes()))
                try:
                    compile_probe(f"{control}-{name}", root, source, werror=True)
                except CommandFailure as error:
                    patterns = {
                        "missing": r"(?i)this `?\.tasty`? file is missing",
                        "misplaced": r"(?i)this `?\.tasty`? file is missing",
                        "corrupt": r"(?is)could not be read.*not a TASTy file",
                        "uuid_mismatch": r"(?i)out of sync with its TASTy file",
                    }
                    if not re.search(patterns[control], error.output):
                        raise RuntimeError(f"{name} {control} sidecar failed at wrong stage: {error.output}") from error
                    reports[f"{name}.{control}"] = error.output.strip()
                else:
                    raise RuntimeError(f"{name} {control} sidecar was unexpectedly accepted")

        # ABI controls hide the classfile's TASTY attribute and remove its
        # sidecar. This makes Scala consume the classfile ABI directly so a
        # missing-sidecar reader error cannot precede the mutation.
        helper_abi_baseline = work / "helper-result-descriptor-baseline"
        shutil.copytree(roots[0], helper_abi_baseline)
        helper_class = helper_abi_baseline / "example/CounterOwnerFactory.class"
        helper_class.write_bytes(_flip_tasty_link(helper_class.read_bytes(), rename_attribute=True))
        (helper_abi_baseline / "example/CounterOwnerFactory.tasty").unlink()
        helper_probe = (
            "package witness\nobject HelperMismatch:\n"
            "  def use(f: example.CounterOwnerFactory): example.Counter = f.makeCounter(1)\n"
        )
        compile_probe("helper-result-descriptor-baseline", helper_abi_baseline, helper_probe)
        reports["helper_result_descriptor_baseline"] = "classfile-only ABI baseline compiled"

        helper_mismatch = work / "helper-result-descriptor-mismatch"
        shutil.copytree(roots[0], helper_mismatch)
        helper_class = helper_mismatch / "example/CounterOwnerFactory.class"
        helper_class.write_bytes(_classfile_with_method_descriptor(
            helper_class.read_bytes(), "makeCounter", "(I)Lexample/Counter;",
            "(I)Lexample/CounterOwner;"))
        helper_class.write_bytes(_flip_tasty_link(helper_class.read_bytes(), rename_attribute=True))
        (helper_mismatch / "example/CounterOwnerFactory.tasty").unlink()
        try:
            compile_probe("helper-result-descriptor-mismatch", helper_mismatch, helper_probe)
        except CommandFailure as error:
            if "This `.tasty` file is missing" in error.output or \
                    not re.search(r"(?m)^-- \[E007\] Type Mismatch Error:", error.output) or \
                    not re.search(r"(?m)^\s*\|\s+Found:\s+example\.CounterOwner\s*$", error.output) or \
                    not re.search(r"(?m)^\s*\|\s+Required:\s+example\.Counter\s*$", error.output) or \
                    not re.search(r"(?m)^1 error found$", error.output):
                raise RuntimeError(f"helper result descriptor control failed unexpectedly: {error.output}") from error
            reports["helper_result_descriptor_mismatch"] = error.output.strip()
        else:
            raise RuntimeError("mismatched makeCounter result descriptor unexpectedly compiled")

        missing_helper = work / "missing-helper-declaration"
        shutil.copytree(roots[0], missing_helper)
        helper_class = missing_helper / "example/CounterOwnerFactory.class"
        helper_class.write_bytes(_classfile_without_method(helper_class.read_bytes(), "makeCounter", "(I)Lexample/Counter;"))
        (missing_helper / "example/CounterOwnerFactory.tasty").unlink()
        try:
            compile_probe("missing-helper-declaration", missing_helper,
                          "package witness\nobject MissingHelper:\n  def use(f: example.CounterOwnerFactory): example.Counter = f.makeCounter(1)\n")
        except CommandFailure as error:
            if "makeCounter" not in error.output:
                raise RuntimeError(f"missing helper declaration failed unexpectedly: {error.output}") from error
            reports["missing_helper_declaration"] = error.output.strip()
        else:
            raise RuntimeError("missing makeCounter declaration unexpectedly compiled")

        wrong_owner_baseline = work / "wrong-owner-constructor-baseline"
        shutil.copytree(roots[0], wrong_owner_baseline)
        owner_class = wrong_owner_baseline / "example/CounterOwner.class"
        owner_class.write_bytes(_flip_tasty_link(owner_class.read_bytes(), rename_attribute=True))
        (wrong_owner_baseline / "example/CounterOwner.tasty").unlink()
        wrong_owner_probe = (
            "package witness\nobject WrongOwner:\n"
            "  def create(): example.CounterOwner = new example.CounterOwner(new example.Counter(1))\n"
        )
        compile_probe("wrong-owner-constructor-baseline", wrong_owner_baseline, wrong_owner_probe)
        reports["wrong_owner_constructor_baseline"] = "classfile-only ABI baseline compiled"

        wrong_owner_ctor = work / "wrong-owner-constructor"
        shutil.copytree(roots[0], wrong_owner_ctor)
        owner_class = wrong_owner_ctor / "example/CounterOwner.class"
        owner_class.write_bytes(_classfile_without_method(owner_class.read_bytes(), "<init>", "(Lexample/Counter;)V"))
        owner_class.write_bytes(_flip_tasty_link(owner_class.read_bytes(), rename_attribute=True))
        (wrong_owner_ctor / "example/CounterOwner.tasty").unlink()
        try:
            compile_probe("wrong-owner-constructor", wrong_owner_ctor, wrong_owner_probe)
        except CommandFailure as error:
            if "This `.tasty` file is missing" in error.output or \
                    "-- [E173] Reference Error:" not in error.output or \
                    "constructor CounterOwner cannot be accessed as a member" not in error.output or \
                    not re.search(r"(?m)^1 error found$", error.output):
                raise RuntimeError(f"wrong Owner constructor control failed unexpectedly: {error.output}") from error
            reports["wrong_owner_constructor_signature"] = error.output.strip()
        else:
            raise RuntimeError("missing Owner primary constructor unexpectedly compiled")

        missing_class = work / "missing-required-artifact"
        shutil.copytree(roots[0], missing_class)
        (missing_class / "example/Counter.class").unlink()
        (missing_class / "example/Counter.tasty").unlink()
        try:
            compile_probe("missing-required-artifact", missing_class,
                          "package witness\nobject MissingArtifact:\n  def read(c: example.Counter): Int = c.get()\n")
        except CommandFailure as error:
            if "Counter" not in error.output or "Not Found" not in error.output:
                raise RuntimeError(f"missing required artifact failed unexpectedly: {error.output}") from error
            reports["missing_required_artifact"] = error.output.strip()
        else:
            raise RuntimeError("missing Counter classfile unexpectedly compiled")

        partial_batch = work / "partial-six-file-batch"
        shutil.copytree(roots[0], partial_batch)
        (partial_batch / "example/CounterOwnerFactory.tasty").unlink()
        try:
            compile_probe("partial-six-file-batch", partial_batch,
                          "package witness\nobject PartialBatch:\n"
                          "  def create(f: example.CounterOwnerFactory): example.CounterOwner = f.create(7)\n",
                          werror=True)
        except CommandFailure as error:
            if "This `.tasty` file is missing" not in error.output:
                raise RuntimeError(f"incomplete six-file batch failed unexpectedly: {error.output}") from error
            reports["partial_six_file_batch"] = error.output.strip()
        else:
            raise RuntimeError("incomplete six-file Rust batch unexpectedly compiled")

        wrong_package = work / "wrong-package-owner"
        shutil.copytree(roots[0], wrong_package)
        misplaced_class_dir = wrong_package / "relocated"
        misplaced_class_dir.mkdir()
        for suffix in ("class", "tasty"):
            (wrong_package / f"example/Counter.{suffix}").rename(
                misplaced_class_dir / f"Counter.{suffix}")
        try:
            compile_probe("wrong-package-owner", wrong_package,
                          "package witness\nobject WrongPackage:\n  def read(c: example.Counter): Int = c.get()\n")
        except CommandFailure as error:
            if "Counter" not in error.output or "Not Found" not in error.output:
                raise RuntimeError(f"wrong package/owner control failed unexpectedly: {error.output}") from error
            reports["wrong_package_owner"] = error.output.strip()
        else:
            raise RuntimeError("wrong package/owner path unexpectedly resolved example.Counter")

        wrong_internal_owner = work / "wrong-internal-owner"
        shutil.copytree(roots[0], wrong_internal_owner)
        owner_path = wrong_internal_owner / "example/Counter.class"
        owner_path.write_bytes(_classfile_change_internal_owner(
            owner_path.read_bytes(), "example/Counter", "example/Coupler"))
        (wrong_internal_owner / "example/Counter.tasty").unlink()
        try:
            compile_probe("wrong-internal-owner", wrong_internal_owner,
                          "package witness\nobject WrongInternalOwner:\n  def read(c: example.Counter): Int = c.get()\n")
        except CommandFailure as error:
            if not re.search(r"(?i)(wrong class|class file.*Counter|example\.Coupler)", error.output):
                raise RuntimeError(f"wrong internal owner failed unexpectedly: {error.output}") from error
            reports["wrong_internal_owner"] = error.output.strip()
        else:
            raise RuntimeError("classfile with a mismatched internal owner unexpectedly compiled")

        # Runtime linkage controls use a precompiled consumer and never execute a
        # member-stripped classfile. The wrong descriptor and absent class are isolated.
        linkage_source = work / "LinkageClient.scala"
        linkage_source.write_text("package witness\nobject LinkageClient:\n  def run(): Int = new example.CounterOwnerFactory().create(5).read()\n")
        linkage_classes = work / "linkage-client"
        linkage_classes.mkdir()
        toolchain.compiler(["-release", "21", "-d", linkage_classes, linkage_source],
                           classpath=os.pathsep.join((str(roots[0]), toolchain.runtime_classpath)))
        no_such = work / "no-such-method-runtime"
        shutil.copytree(roots[0], no_such)
        factory_class = no_such / "example/CounterOwnerFactory.class"
        factory_class.write_bytes(_classfile_rename_method(
            factory_class.read_bytes(), "create", "(I)Lexample/CounterOwner;", "makeCounter"))
        driver = work / "LinkageDriver.java"
        driver.write_text("public final class LinkageDriver { public static void main(String[] a) { "
                          "try { System.out.println(witness.LinkageClient$.MODULE$.run()); } "
                          "catch (NoSuchMethodError e) { System.out.println(\"NoSuchMethodError\"); } "
                          "catch (NoClassDefFoundError e) { System.out.println(\"NoClassDefFoundError\"); } } }\n")
        for label, root, missing in (("no-such-method", no_such, None),
                                     ("no-class-def", roots[0], "Counter.class")):
            driver_out = work / f"{label}-driver"
            driver_out.mkdir()
            cp = os.pathsep.join((str(root), str(linkage_classes), toolchain.runtime_classpath))
            execute([toolchain.javac, "--release", "21", "-classpath", cp, "-d", driver_out, driver])
            run_root = work / f"{label}-runtime"
            if missing:
                shutil.copytree(root, run_root)
                (run_root / "example" / missing).unlink()
            else:
                run_root = root
            runtime_cp = os.pathsep.join((str(run_root), str(linkage_classes), str(driver_out),
                                          toolchain.runtime_classpath))
            runtime = execute([toolchain.java, "-Xverify:all", "-ea", "-XX:-UsePerfData",
                               "-classpath", runtime_cp, "LinkageDriver"])
            expected = "NoSuchMethodError" if label == "no-such-method" else "NoClassDefFoundError"
            if expected not in runtime:
                raise RuntimeError(f"{label} control did not reach {expected}: {runtime}")
            reports[label] = runtime.strip()

        renamed_client = client.read_text()
        if reference_local:
            renamed_client = renamed_client.replace("package example", "package renamed")
        for before_name, after_name in (("example.", "renamed."), ("CounterOwnerFactory", "WidgetBuilder"),
                                        ("CounterOwner", "WidgetHolder"), ("Counter", "Widget")):
            renamed_client = renamed_client.replace(before_name, after_name)
        renamed_input = work / "RenamedFactoryConsumer.scala"
        renamed_input.write_text(renamed_client)
        renamed_outputs: list[str] = []
        for index, root in enumerate(renamed_roots):
            actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
            if actual != renamed_paths:
                raise RuntimeError(f"{root.name} must contain renamed six artifacts: {sorted(actual)}")
            output = work / f"renamed-client-{index}"
            output.mkdir()
            toolchain.compiler(["-release", "21", "-d", output, renamed_input],
                               classpath=os.pathsep.join((str(root), toolchain.runtime_classpath)))
            driver_path = work / f"RenamedRunner{index}.java"
            if reference_local:
                driver_path.write_text(
                    "public final class RenamedRunner%d { public static void main(String[] a) { "
                    "renamed.FactoryConsumer$.MODULE$.main(new String[0]); } }\n" % index)
            else:
                driver_path.write_text(
                    "public final class RenamedRunner%d { public static void main(String[] a) { "
                    "System.out.println(consumer.FactoryResultConsumer$.MODULE$.run()); } }\n" % index)
            driver_out = work / f"renamed-driver-{index}"
            driver_out.mkdir()
            cp = os.pathsep.join((str(root), str(output), toolchain.runtime_classpath))
            execute([toolchain.javac, "--release", "21", "-classpath", cp, "-d", driver_out, driver_path])
            runtime_cp = os.pathsep.join((str(root), str(output), str(driver_out), toolchain.runtime_classpath))
            runtime = execute([toolchain.java, "-Xverify:all", "-ea", "-XX:-UsePerfData",
                               "-classpath", runtime_cp, f"RenamedRunner{index}"])
            lines = [line for line in runtime.strip().splitlines()
                     if not line.startswith("Picked up JAVA_TOOL_OPTIONS:")]
            if lines != expected_output:
                raise RuntimeError(f"renamed/reordered Rust batch returned {lines}")
            renamed_outputs.extend(lines)

    after = {f"{root.name}/{relative}": digest(root / relative)
             for root in (*roots, *renamed_roots) for relative in
             sorted(relative_paths if root in roots else renamed_paths)}
    if before != after:
        raise RuntimeError("Scala compile/runtime or isolated negative controls changed original Rust artifacts")
    return {
        "scala_consumer": True,
        "scala_revision": REVISION,
        "scala_compiler_version": toolchain.compiler_version,
        "java_version": toolchain.jdk_release["JAVA_VERSION"],
        "javac_version": toolchain.javac_version,
        "javap_version": toolchain.javap_version,
        "jdk_release": toolchain.jdk_release,
        "scala_source_sha256": digest(ROOT / (
            "tests/fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala"
            if reference_local else
            "tests/fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala"
        )),
        "client_source_sha256": client_hash,
        "rust_artifact_hashes": hashes,
        "tasty_link_uuids": uuids,
        "renamed_artifact_hashes": renamed_hashes,
        "renamed_tasty_link_uuids": renamed_uuids,
        "runtime_outputs": runtime_outputs,
        "counter_tasty_only_get_resolved": True,
        "owner_tasty_only_read_resolved": True,
        "factory_tasty_only_create_resolved": True,
        "factory_tasty_only_make_counter_resolved": True,
        "counter_classfile_only_control_rejected": True,
        "owner_classfile_only_control_rejected": True,
        "factory_classfile_only_create_control_rejected": True,
        "factory_classfile_only_make_counter_control_rejected": True,
        "factory_tasty_methods_decompiled": True,
        "from_tasty_classes": rebuilt_classes,
        "per_class_sidecar_matrix_rejected": True,
        "negative_controls": reports,
        "renamed_reordered_scala_consumer": True,
        "renamed_runtime_outputs": renamed_outputs,
        "rust_artifacts_unchanged_after_compile_and_run": True,
        "compile_classpath_order": ["rust-six-artifact-root", *[str(p) for p in toolchain.runtime_jars]],
        "runtime_outputs_are_original_rust_classes": True,
        "compiler_artifacts_sha256": toolchain.artifacts,
        **local_proof,
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
    constructor_injection = subcommands.add_parser(
        "consume-constructor-injection",
        help="prove pinned Scala consumes Rust Counter/CounterOwner constructor-alias TASTy",
    )
    constructor_injection.add_argument("batch_root", type=Path)
    reference_constructor = subcommands.add_parser(
        "consume-reference-constructor",
        help="prove pinned Scala consumes all three Rust reference-constructor TASTy sidecars",
    )
    reference_constructor.add_argument("batch_root", type=Path)
    nested_constructor = subcommands.add_parser(
        "consume-nested-constructor",
        help="prove pinned Scala consumes and executes the Rust nested-constructor artifact batch",
    )
    nested_constructor.add_argument("batch_root", type=Path)
    factory_result = subcommands.add_parser(
        "consume-factory-result",
        help="prove pinned Scala consumes factory-call Rust TASTy and executes original classes",
    )
    factory_result.add_argument("batch_root", type=Path)
    factory_result.add_argument("--reference-local", action="store_true")
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
    if args.command == "consume-constructor-injection":
        toolchain = Toolchain()
        print(json.dumps(consume_object_composition_batch(
            toolchain, args.batch_root, constructor_injection=True,
        ), indent=2, sort_keys=True))
        return
    if args.command == "consume-reference-constructor":
        toolchain = Toolchain()
        print(json.dumps(consume_reference_constructor_batch(toolchain, args.batch_root),
                         indent=2, sort_keys=True))
        return
    if args.command == "consume-nested-constructor":
        toolchain = Toolchain()
        print(json.dumps(consume_nested_constructor_batch(toolchain, args.batch_root),
                         indent=2, sort_keys=True))
        return
    if args.command == "consume-factory-result":
        toolchain = Toolchain()
        print(json.dumps(consume_factory_result_batch(
            toolchain, args.batch_root, reference_local=args.reference_local,
        ),
                         indent=2, sort_keys=True))
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
