#!/usr/bin/env python3
"""Pinned, offline Scala reference and independent primitive-value-discard consumers."""
import argparse
import hashlib
import json
from pathlib import Path
import runpy
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
ORACLE = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))
CLASS = "PrimitiveValueDiscard"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def consume(toolchain, candidate, output):
    tasty = candidate / f"example/{CLASS}.tasty"
    originals = [tasty, candidate / f"example/{CLASS}.class"]
    before = {p: digest(p) for p in originals}
    printed = toolchain.compiler(["-decompile", tasty], decompile=True)
    for shape in ["double_formal", "long_while_body", "float_if_unit",
                  "double_return_value", "divide_unit", "divide_after_return"]:
        assert shape in printed, f"Scala semantic reader lost {shape}"
    rebuilt = output / "from-tasty"
    rebuilt.mkdir()
    toolchain.compiler(["-release", "21", "-from-tasty", "-d", rebuilt, tasty],
                       classpath=f"{candidate}:{toolchain.runtime_classpath}")
    java = output / "java"
    java.mkdir()
    execute = ORACLE["execute"]
    execute([toolchain.javac, "--release", "21", "-cp", candidate,
             "-d", java, HERE / "JavaConsumer.java"])
    results = {}
    for name, producer in [("original", candidate), ("from_tasty", rebuilt)]:
        cp = f"{producer}:{toolchain.runtime_classpath}"
        result = execute([toolchain.java, "-Xverify:all", "-ea", "-cp",
                          f"{java}:{cp}", "JavaConsumer"])
        assert result.strip() == "OK 1928 checks", result
        results[f"java_{name}"] = result.strip()
        scala = output / f"scala-{name}"
        scala.mkdir()
        toolchain.compiler(["-release", "21", "-d", scala, HERE / "ScalaConsumer.scala"],
                           classpath=cp)
        result = execute([toolchain.java, "-Xverify:all", "-ea", "-cp",
                          f"{scala}:{cp}", "ScalaConsumer"])
        assert result.strip() == "OK 1928 checks", result
        results[f"scala_{name}"] = result.strip()
    assert all(digest(p) == sha for p, sha in before.items())
    results["original_sha256"] = {str(p.relative_to(candidate)): sha for p, sha in before.items()}
    print(json.dumps(results, sort_keys=True))
    return printed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate", nargs="?", type=Path)
    args = parser.parse_args()
    toolchain = ORACLE["Toolchain"]()
    with tempfile.TemporaryDirectory(prefix="primitive-value-discard-") as scratch:
        output = Path(scratch)
        if args.candidate:
            provenance = json.loads((HERE / "provenance.json").read_text())
            assert provenance["scala_source_revision"] == ORACLE["REVISION"]
            for name, key in [(f"{CLASS}.scala", "source_sha256"), ("oracle.txt", "oracle_sha256"),
                              ("JavaConsumer.java", "java_consumer_sha256"),
                              ("ScalaConsumer.scala", "scala_consumer_sha256")]:
                assert digest(HERE / name) == provenance[key]
            consume(toolchain, args.candidate, output)
            print("Original Rust primitive-value-discard artifacts accepted by pinned Scala 3.9")
            return
        candidate = output / "reference"
        candidate.mkdir()
        toolchain.compiler(["-release", "21", "-sourceroot", HERE, "-d", candidate,
                            HERE / f"{CLASS}.scala"])
        # The pinned raw printer mishandles large signed payloads; use the semantic reader.
        printed = consume(toolchain, candidate, output).replace(str(output), "<oracle>")
        (HERE / "oracle.txt").write_text(printed)
        receipt = {
            "scala_source_revision": ORACLE["REVISION"],
            "compiler_artifacts_sha256": toolchain.artifacts,
            "jdk_release": toolchain.jdk_release,
            "source_sha256": digest(HERE / f"{CLASS}.scala"),
            "tasty_sha256": digest(candidate / f"example/{CLASS}.tasty"),
            "oracle_sha256": digest(HERE / "oracle.txt"),
            "java_consumer_sha256": digest(HERE / "JavaConsumer.java"),
            "scala_consumer_sha256": digest(HERE / "ScalaConsumer.scala"),
        }
        (HERE / "provenance.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
        print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
