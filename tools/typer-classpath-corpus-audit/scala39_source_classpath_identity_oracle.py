#!/usr/bin/env python3
"""Check Scala 3.9 source/classpath identity and companion precedence."""

import os
import re
import tempfile
from pathlib import Path

from scala39_import_oracle import compile_sources


def main() -> int:
    java_home = Path(os.environ["JAVA_HOME"])
    classpath = os.environ["SCALA39_CLASSPATH"]
    with tempfile.TemporaryDirectory(prefix="scala39-source-classpath-oracle-") as temporary:
        root = Path(temporary)
        source = root / "source"
        binary = root / "binary"
        output = root / "output"
        source.mkdir()
        binary.mkdir()
        output.mkdir()

        library = source / "Library.scala"
        library.write_text(
            "package fixture1807\n"
            "class Base\n"
            "class Child extends Base\n"
            "case class Product(value: Int)\n"
            "object Product { def classpathOnly: String = \"binary\" }\n",
            encoding="utf-8",
        )
        result = compile_sources(java_home, classpath, binary, [library])
        if result.returncode:
            raise SystemExit("Scala 3.9 oracle library fixture failed:\n" + result.stderr)

        shadow = source / "SourceShadow.scala"
        shadow.write_text(
            "package fixture1807\n"
            "class Base { def sourceOnly: String = \"source\" }\n"
            "case class Product(value: String)\n"
            "object SourceUse {\n"
            "  val child: Base = new Child\n"
            "  val selected: String = child.sourceOnly\n"
            "  val constructed: Product = Product(\"source\")\n"
            "}\n",
            encoding="utf-8",
        )
        result = compile_sources(java_home, classpath + os.pathsep + str(binary), output, [shadow])
        if result.returncode:
            raise SystemExit(
                "Scala 3.9 source shadow / external parent controls failed:\n"
                + result.stdout
                + result.stderr
            )

        companion = source / "SourceCompanionDoesNotMerge.scala"
        companion.write_text(
            "package fixture1807\n"
            "case class Product(value: String)\n"
            "object SourceCompanionDoesNotMerge { val value = Product.classpathOnly }\n",
            encoding="utf-8",
        )
        result = compile_sources(
            java_home, classpath + os.pathsep + str(binary), root / "companion", [companion]
        )
        diagnostic = re.sub(r"\x1b\[[0-9;]*m", "", result.stdout + result.stderr)
        if result.returncode == 0 or not all(
            fragment in diagnostic for fragment in ("classpathOnly", "not a member of object")
        ):
            raise SystemExit(
                "Scala 3.9 reused the classpath companion for a source case class:\n" + diagnostic
            )

        unrelated = source / "UnrelatedSourceClassDoesNotMerge.scala"
        unrelated.write_text(
            "package fixture1807\n"
            "class Product\n"
            "object UnrelatedSourceClassDoesNotMerge { val value = Product.classpathOnly }\n",
            encoding="utf-8",
        )
        result = compile_sources(
            java_home, classpath + os.pathsep + str(binary), root / "unrelated", [unrelated]
        )
        diagnostic = re.sub(r"\x1b\[[0-9;]*m", "", result.stdout + result.stderr)
        if result.returncode == 0 or not all(
            fragment in diagnostic for fragment in ("classpathOnly", "not a member of object")
        ):
            raise SystemExit(
                "Scala 3.9 reused a classpath companion for an unrelated source class:\n"
                + diagnostic
            )

    print(
        "scala=3.9.0 source_classpath=source_class_shadows_binary_class "
        "external_child_parent=source_class source_case_companion=source_synthesized "
        "classpath_companion_reuse=rejected unrelated_source_class_companion_reuse=rejected"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
