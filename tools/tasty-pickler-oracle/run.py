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
            if relative.as_posix() == "example/Locals.tasty":
                signatures = (
                    r"public int copy\(int\)", r"public int literal\(\)",
                    r"public int call\(int\)", r"public int multiple\(int\)",
                    r"public int identity\(int\)",
                )
                if not all(re.search(pattern, javap) for pattern in signatures):
                    raise RuntimeError(f"fresh Locals class has unexpected signatures: {javap}")
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


def check_oracle(toolchain: Toolchain, binary: Path, directory: Path) -> list[dict]:
    verify_provenance()
    generated = directory / "regenerated"
    generated.mkdir()
    generate(toolchain, binary, generated, check=True)
    results = []
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
    int chooseTrue = decisions.choose(true, 17, 42);
    int chooseFalse = decisions.choose(false, 17, 42);
    int nestedTrueTrue = decisions.nested(true, true);
    int nestedTrueFalse = decisions.nested(true, false);
    int nestedFalse = decisions.nested(false, true);
    int storedTrue = decisions.stored(true, 17);
    int storedFalse = decisions.stored(false, 17);
    if (!enabled || disabled || chooseTrue != 17 || chooseFalse != 42 ||
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
    expected_source_output = "Decisions source = true, false, 17, 42, 1, 2, 3, 17, 0"
    if expected_source_output not in source_run:
        raise RuntimeError(f"pinned Scala source reference returned unexpected values: {source_run}")
    results.append({"source_reference": "example.Decisions", "scala_source_compile": True,
                    "source_runtime_output": expected_source_output})
    for candidate in sorted((FIXTURES / "tasty").rglob("*.tasty")):
        results.append(verify_candidate(toolchain, binary, candidate,
                                        directory / f"verify-{candidate.stem}"))
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
                        help="inspect and execute the fresh Answer class; requires example/Answer")
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
