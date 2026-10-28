#!/usr/bin/env python3
"""Check top-level case-class companion precedence with pinned Scala 3.9.0."""

import os
import re
import subprocess
import tempfile
from pathlib import Path

from scala39_import_oracle import compile_sources


REVISION = "777528f19a58e794c9954a42f433373472ec57f8"


def main() -> int:
    scala_root = Path(os.environ["SCALA39_ROOT"])
    if subprocess.check_output(
        ["git", "-C", str(scala_root), "rev-parse", "HEAD"], text=True
    ).strip() != REVISION:
        raise SystemExit(f"SCALA39_ROOT must be pinned at {REVISION}")
    java_home = Path(os.environ["JAVA_HOME"])
    if not (java_home / "jmods").is_dir():
        raise SystemExit("JAVA_HOME must be a JDK 21 installation")
    release = (java_home / "release").read_text(encoding="utf-8")
    java_version = re.search(r'^JAVA_VERSION="([^"]+)"', release, re.MULTILINE)
    if java_version is None or java_version.group(1).split(".", 1)[0] != "21":
        raise SystemExit("JAVA_HOME must be a JDK 21 installation")
    classpath = os.environ.get("SCALA39_CLASSPATH")
    if classpath is None:
        maven_root = Path(os.environ["COURSIER_MAVEN_ROOT"])
        artifacts = (
            "org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar",
            "org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar",
            "org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar",
            "org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar",
            "org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar",
            "org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar",
            "org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar",
            "org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar",
        )
        jars = [maven_root / artifact for artifact in artifacts]
        missing = [str(jar) for jar in jars if not jar.is_file()]
        if missing:
            raise SystemExit("missing Scala compiler artifacts:\n" + "\n".join(missing))
        classpath = os.pathsep.join(map(str, jars))

    with tempfile.TemporaryDirectory(prefix="scala39-top-level-case-companion-") as temporary:
        root = Path(temporary)
        source = root / "source"
        source.mkdir()
        binary = root / "binary"
        binary_source = source / "Binary.scala"
        binary_source.write_text(
            "package issue1825\n"
            "case class Shadow[A](first: A, second: Int)\n"
            "object Shadow { def classpathOnly: String = \"binary\" }\n",
            encoding="utf-8",
        )
        result = compile_sources(java_home, classpath, binary, [binary_source])
        if result.returncode:
            raise SystemExit("Scala 3.9 binary control failed:\n" + result.stdout + result.stderr)

        positive = source / "Positive.scala"
        positive.write_text(
            "package issue1825\n"
            "case class Shadow[A](first: A, second: Int)\n"
            "object Shadow { def sourceOnly: String = \"source\" }\n"
            "case class Explicit[A](value: A)\n"
            "object Explicit { def sourceOnly: String = \"explicit\" }\n"
            "case class CustomApply(value: Int)\n"
            "object CustomApply { def apply(value: Int): CustomApply = new CustomApply(value) }\n"
            "case class Box[A](value: A)\n"
            "object Box { def apply[B](value: B): Box[B] = new Box[B](value) }\n"
            "object Positive {\n"
            "  val shadow: Shadow[Int] = Shadow(1, 2)\n"
            "  val shadowMember: String = Shadow.sourceOnly\n"
            "  val explicit: Explicit[Int] = Explicit(3)\n"
            "  val explicitMember: String = Explicit.sourceOnly\n"
            "  val customApply: CustomApply = CustomApply(4)\n"
            "  val box: Box[Int] = Box(5)\n"
            "}\n",
            encoding="utf-8",
        )
        result = compile_sources(
            java_home, classpath + os.pathsep + str(binary), root / "positive", [positive]
        )
        if result.returncode:
            raise SystemExit(
                "Scala 3.9 top-level/explicit source companion controls failed:\n"
                + result.stdout
                + result.stderr
            )

        source_case_leak = source / "SourceCaseClassCannotUseBinaryCompanion.scala"
        source_case_leak.write_text(
            "package issue1825\n"
            "case class Shadow[A](first: A, second: Int)\n"
            "object SourceCaseClassCannotUseBinaryCompanion {\n"
            "  val leak = Shadow.classpathOnly\n"
            "}\n",
            encoding="utf-8",
        )
        result = compile_sources(
            java_home,
            classpath + os.pathsep + str(binary),
            root / "source-case-leak",
            [source_case_leak],
        )
        diagnostic = re.sub(r"\x1b\[[0-9;]*m", "", result.stdout + result.stderr)
        if result.returncode == 0 or not all(
            fragment in diagnostic for fragment in ("classpathOnly", "not a member of object")
        ):
            raise SystemExit(
                "Scala 3.9 exposed a binary-only member through the source case class:\n"
                + diagnostic
            )

        unrelated = source / "UnrelatedSourceClassCannotUseBinaryCompanion.scala"
        unrelated.write_text(
            "package issue1825\n"
            "class Shadow\n"
            "object UnrelatedSourceClassCannotUseBinaryCompanion {\n"
            "  val leak = Shadow.classpathOnly\n"
            "}\n",
            encoding="utf-8",
        )
        result = compile_sources(
            java_home,
            classpath + os.pathsep + str(binary),
            root / "unrelated-source-leak",
            [unrelated],
        )
        diagnostic = re.sub(r"\x1b\[[0-9;]*m", "", result.stdout + result.stderr)
        if result.returncode == 0 or not all(
            fragment in diagnostic for fragment in ("classpathOnly", "not a member of object")
        ):
            raise SystemExit(
                "Scala 3.9 exposed a binary companion member through an unrelated source class:\n"
                + diagnostic
            )

    print(
        "scala=3.9.0 jdk=21 top_level_case_class=apply_and_signature_ok "
        "explicit_source_companion=preserved matching_custom_apply=one_candidate "
        "renamed_generic_custom_apply=one_candidate "
        "source_classpath_shadow=source_wins "
        "binary_only_companion_member=rejected unrelated_source_class_merge=rejected"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
