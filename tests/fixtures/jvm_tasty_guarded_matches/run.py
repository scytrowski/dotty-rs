#!/usr/bin/env python3
"""Offline Scala 3.9 oracle for the bounded JVM/TASTy guarded-match increment."""

import argparse
import hashlib
import json
from pathlib import Path
import runpy
import tempfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
ORACLE = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate", nargs="?", type=Path)
    args = parser.parse_args()
    toolchain = ORACLE["Toolchain"]()
    if args.candidate:
        provenance = json.loads((HERE / "provenance.json").read_text())
        assert provenance["scala_source_revision"] == ORACLE["REVISION"]
        assert hashlib.sha256((HERE / "GuardedMatches.scala").read_bytes()).hexdigest() == provenance["source_sha256"]
        assert hashlib.sha256((HERE / "oracle.txt").read_bytes()).hexdigest() == provenance["oracle_sha256"]
        candidate = args.candidate / "example/GuardedMatches.tasty"
        classfile = args.candidate / "example/GuardedMatches.class"
        originals = {path: path.read_bytes() for path in [candidate, classfile]}
        with tempfile.TemporaryDirectory(prefix="guarded-match-consumer-") as scratch:
            output = Path(scratch)
            rebuilt = output / "from-tasty"
            java_out = output / "java"
            scala_out = output / "scala"
            for directory in [rebuilt, java_out, scala_out]:
                directory.mkdir()
            printed = toolchain.compiler(["-print-tasty", candidate], decompile=True)
            for shape in ["match", "case 0 if", "case n if", "scala.Int", "guardCode"]:
                assert shape in printed, f"official Scala reader lost {shape}"
            toolchain.compiler(["-from-tasty", "-d", rebuilt, candidate],
                               classpath=str(args.candidate) + ":" + toolchain.runtime_classpath)
            assert (rebuilt / "example/GuardedMatches.class").is_file()
            execute = ORACLE["execute"]
            execute([toolchain.javac, "--release", "21", "-cp", args.candidate,
                     "-d", java_out, HERE / "JavaConsumer.java"])
            for producer in [args.candidate, rebuilt]:
                result = execute([toolchain.java, "-Xverify:all", "-ea", "-cp",
                                  f"{java_out}:{producer}:{toolchain.runtime_classpath}", "JavaConsumer"])
                assert result.strip() == "guarded match oracle ok", result
            toolchain.compiler(["-d", scala_out, HERE / "ScalaConsumer.scala"],
                               classpath=f"{args.candidate}:{toolchain.runtime_classpath}")
            result = execute([toolchain.java, "-Xverify:all", "-ea", "-cp",
                              f"{scala_out}:{args.candidate}:{toolchain.runtime_classpath}", "ScalaConsumer"])
            assert result.strip() == "guarded match oracle ok", result
            print(json.dumps({
                "java_original": "guarded match oracle ok",
                "scala_original": "guarded match oracle ok",
                "java_from_tasty": "guarded match oracle ok",
                "original_sha256": {str(p.relative_to(args.candidate)): hashlib.sha256(data).hexdigest()
                                    for p, data in originals.items()},
            }, sort_keys=True))
        assert all(path.read_bytes() == data for path, data in originals.items())
        print("Rust guarded-match TASTy accepted and compiled by pinned Scala 3.9")
        return
    with tempfile.TemporaryDirectory(prefix="guarded-match-oracle-") as scratch:
        output = Path(scratch)
        wire = toolchain.compiler([
            "-release", "21", "-Yprint-tasty", "-sourceroot", HERE, "-d", output,
            HERE / "GuardedMatches.scala",
        ])
        candidate = output / "example/GuardedMatches.tasty"
        printed = wire + "\n" + toolchain.compiler(["-print-tasty", candidate], decompile=True)
        printed = printed.replace(str(output), "<oracle>")
        (HERE / "oracle.txt").write_text(printed)
        receipt = {
            "scala_source_revision": ORACLE["REVISION"],
            "compiler_artifacts_sha256": toolchain.artifacts,
            "jdk_release": toolchain.jdk_release,
            "source_sha256": hashlib.sha256((HERE / "GuardedMatches.scala").read_bytes()).hexdigest(),
            "tasty_sha256": hashlib.sha256(candidate.read_bytes()).hexdigest(),
            "oracle_sha256": hashlib.sha256(printed.encode()).hexdigest(),
        }
        (HERE / "provenance.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
        print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
