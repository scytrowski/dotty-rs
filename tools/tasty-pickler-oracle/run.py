Warning: truncated output (original token count: 40821)
Total output lines: 3222

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
 …20821 tokens truncated…mber}")
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
        for expected in ("def get(): scala.Int", "def set(value: scala.Int): scala.Unit"):
            if expected not in counter_text:
                raise RuntimeError(f"Counter TASTy decompile lacks {expected!r}: {counter_text}")

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
