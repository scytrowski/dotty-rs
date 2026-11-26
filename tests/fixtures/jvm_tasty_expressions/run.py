#!/usr/bin/env python3
"""Offline Scala 3.9 oracle for the bounded JVM/TASTy expression increment."""

import hashlib
import json
from pathlib import Path
import runpy
import tempfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
ORACLE = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))


def main():
    toolchain = ORACLE["Toolchain"]()
    with tempfile.TemporaryDirectory(prefix="expression-oracle-") as scratch:
        output = Path(scratch)
        wire = toolchain.compiler([
            "-release", "21", "-Yprint-tasty", "-sourceroot", HERE, "-d", output,
            HERE / "Expressions.scala",
        ])
        candidate = output / "example/Expressions.tasty"
        printed = wire + "\n" + toolchain.compiler(["-print-tasty", candidate], decompile=True)
        printed = printed.replace(str(output), "<oracle>")
        (HERE / "oracle.txt").write_text(printed)
        receipt = {
            "scala_source_revision": ORACLE["REVISION"],
            "compiler_artifacts_sha256": toolchain.artifacts,
            "jdk_release": toolchain.jdk_release,
            "source_sha256": hashlib.sha256((HERE / "Expressions.scala").read_bytes()).hexdigest(),
            "tasty_sha256": hashlib.sha256(candidate.read_bytes()).hexdigest(),
            "oracle_sha256": hashlib.sha256(printed.encode()).hexdigest(),
        }
        (HERE / "provenance.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
        print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
