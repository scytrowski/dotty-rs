#!/usr/bin/env python3
"""Pinned, offline Scala reference and independent boolean-operators consumers."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import runpy
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
ORACLE = runpy.run_path(str(ROOT / "tools/tasty-pickler-oracle/run.py"))
CLASS = "BooleanOperators"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def reference_excerpt(raw):
    """Keep the seven formal-operand trees, with original wire addresses/names."""
    trees = raw.split("\nTrees (", 1)[1]
    scala_package = re.search(r"(?m)^ *\d+: +TERMREFpkg \d+ \[scala\]$", trees)[0]
    methods = list(re.finditer(
        r"(?m)^ *\d+:       DEFDEF\(\d+\) \d+ \[([^\]]+)\]$", trees))
    excerpts = []
    for current, following in zip(methods, methods[1:]):
        if current[1].endswith("_formals"):
            excerpts.append(trees[current.start():following.start()].rstrip())
    assert len(excerpts) == 7, "missing Boolean operator reference tree"
    return ("Scala 3.9.0 / TASTy 28.9.0: formal-operand DEFDEF excerpts\n"
            "Extracted from -Yprint-tasty; original addresses and shared references retained.\n\n"
            + scala_package + "\n\n" + "\n\n".join(excerpts) + "\n")


def consume(toolchain, candidate, output):
    tasty = candidate / f"example/{CLASS}.tasty"
    originals = [tasty, candidate / f"example/{CLASS}.class"]
    before = {p: digest(p) for p in originals}
    printed = toolchain.compiler(["-decompile", tasty], decompile=True)
    for shape in ["and_formals", "or_order", "xor_throw",
                  "eq_guard", "ne_discard", "short_and_order"]:
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
        assert result.strip() == "OK 448 checks", result
        results[f"java_{name}"] = result.strip()
        scala = output / f"scala-{name}"
        scala.mkdir()
        toolchain.compiler(["-release", "21", "-d", scala, HERE / "ScalaConsumer.scala"],
                           classpath=cp)
        result = execute([toolchain.java, "-Xverify:all", "-ea", "-cp",
                          f"{scala}:{cp}", "ScalaConsumer"])
        assert result.strip() == "OK 448 checks", result
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
    with tempfile.TemporaryDirectory(prefix="boolean-operators-") as scratch:
        output = Path(scratch)
        if args.candidate:
            provenance = json.loads((HERE / "provenance.json").read_text())
            assert provenance["scala_source_revision"] == ORACLE["REVISION"]
            for name, key in [(f"{CLASS}.scala", "source_sha256"), ("oracle.txt", "oracle_sha256"),
                              ("JavaConsumer.java", "java_consumer_sha256"),
                              ("ScalaConsumer.scala", "scala_consumer_sha256")]:
                assert digest(HERE / name) == provenance[key]
            consume(toolchain, args.candidate, output)
            print("Original Rust boolean-operators artifacts accepted by pinned Scala 3.9")
            return
        candidate = output / "reference"
        candidate.mkdir()
        raw = toolchain.compiler(["-release", "21", "-Yprint-tasty", "-sourceroot", HERE, "-d", candidate,
                            HERE / f"{CLASS}.scala"])
        consume(toolchain, candidate, output)
        (HERE / "oracle.txt").write_text(reference_excerpt(raw))
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
