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


def execute(args: list[str | Path], *, cwd: Path = ROOT) -> str:
    result = subprocess.run([str(a) for a in args], cwd=cwd, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {args}\n{result.stdout}")
    return result.stdout


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


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
        if not self.java.is_file():
            raise RuntimeError(f"missing compiler Java executable: {self.java}")
        base = Path(os.environ["COURSIER_MAVEN_ROOT"])
        jars = [base / group / artifact / version / f"{artifact}-{version}.jar"
                for group, artifact, version in ARTIFACTS]
        for jar in jars:
            if not jar.is_file():
                raise RuntimeError(f"missing cached compiler dependency: {jar}")
        self.classpath = os.pathsep.join(str(p) for p in jars)
        self.artifacts = {p.name: digest(p) for p in jars}
        release = dict(re.findall(r'^([A-Z_]+)="(.*)"$',
                                  (java_home / "release").read_text(), re.MULTILINE))
        self.jdk_release = {key: release[key] for key in
                            ("JAVA_VERSION", "IMPLEMENTOR", "IMPLEMENTOR_VERSION", "OS_ARCH")}
        version = self.compiler(["-version"])
        if not re.search(r"Scala compiler version 3\.9\.0 --", version):
            raise RuntimeError(f"expected Scala 3.9.0 compiler, got: {version}")
        if (FIXTURES / "provenance.json").exists():
            pinned = json.loads((FIXTURES / "provenance.json").read_text())
            if self.artifacts != pinned["compiler_artifacts_sha256"]:
                raise RuntimeError("compiler artifacts differ from pinned SHA-256 provenance")

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
        metadata = json.loads(execute(["cargo", "metadata", "--format-version", "1", "--no-deps"]))
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    generation = subcommands.add_parser("generate", help="explicitly record/check independent fixtures")
    generation.add_argument("--check", action="store_true", help="compare regenerated structure without writing fixtures")
    args = parser.parse_args()
    toolchain = Toolchain()
    binary = inspector()
    with tempfile.TemporaryDirectory(prefix="tasty-pickler-oracle-") as temporary:
        generate(toolchain, binary, Path(temporary), args.check)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError) as error:
        print(f"tasty-pickler-oracle: {error}", file=sys.stderr)
        sys.exit(1)
