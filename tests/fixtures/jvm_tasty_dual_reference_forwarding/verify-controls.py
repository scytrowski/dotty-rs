#!/usr/bin/env python3
"""Compile the issue's source-language controls with its pinned scalac."""
import json
import pathlib
import subprocess
import sys

source_path = pathlib.Path(sys.argv[1])
scalac = sys.argv[2]
out_root = pathlib.Path(sys.argv[3])
base = source_path.read_text()
renamed = (base.replace("package example", "package renamed")
           .replace("PairFactory", "Assembler").replace("Pair", "Envelope")
           .replace("Counter", "Meter").replace("wrap", "forward")
           .replace("first", "sourceLeft").replace("second", "sourceRight"))
reordered = base.replace(
    "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\n", "")
reordered = reordered.replace(
    "class Pair(first: Counter, second: Counter):\n  private val left: Counter = first\n  private val right: Counter = second\n  def leftValue(): Int = left.get()\n  def rightValue(): Int = right.get()\n\n", "")
reordered += "\nclass Pair(first: Counter, second: Counter):\n  private val left: Counter = first\n  private val right: Counter = second\n  def leftValue(): Int = left.get()\n  def rightValue(): Int = right.get()\n\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n"

cases = [
    ("renamed package/classes/method/formals", renamed, True, []),
    ("reordered declarations", reordered, True, []),
    ("intentional first/first alias", base.replace("new Pair(first, second)", "new Pair(first, first)"), True, []),
    ("argument reversal", base.replace("new Pair(first, second)", "new Pair(second, first)"), True, []),
    ("formal reversal", base.replace("def wrap(first: Counter, second: Counter)", "def wrap(second: Counter, first: Counter)"), True, []),
    ("third method formal", base.replace("second: Counter): Pair", "second: Counter, unused: Int): Pair"), True, []),
    ("contextual formal", base.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter)(using second: Counter)"), True, []),
    ("default formal", base.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, second: Counter = new Counter(0))"), True, []),
    ("generic method", base.replace("def wrap(", "def wrap[A]("), True, []),
    ("overload", base.replace("new Pair(first, second)\n", "new Pair(first, second)\n  def wrap(value: Int, other: Int): Pair = new Pair(new Counter(value), new Counter(other))\n"), True, []),
    ("mixed direct/new", base.replace("new Pair(first, second)", "new Pair(first, new Counter(second.get()))"), True, []),
    ("alternate Pair subclass constructor", base.replace("class PairFactory:", "class PairChild(first: Counter, second: Counter) extends Pair(first, second):\n  def marker(): Int = 0\n\nclass PairFactory:").replace("new Pair(first, second)", "new PairChild(first, second)"), True, []),
    ("unused erased formal", base.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, erased second: Counter)").replace("new Pair(first, second)", "new Pair(first, first)"), True, ["-language:experimental.erasedDefinitions"]),
    ("third Pair constructor argument", base.replace("new Pair(first, second)", "new Pair(first, second, first)"), False, []),
    ("wrong argument type", base.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Int, second: Counter)"), False, []),
    ("wrong Pair arity", base.replace("new Pair(first, second)", "new Pair(first)"), False, []),
    ("duplicate formal name", base.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, first: Counter)"), False, []),
    ("foreign same-spelled Counter", base.replace("package example\n", "package example\n\nobject foreign:\n  class Counter(start: Int):\n    def get(): Int = start\n\n").replace("def wrap(first: Counter, second: Counter)", "def wrap(first: foreign.Counter, second: Counter)"), False, []),
]

out_root.mkdir(parents=True, exist_ok=True)
results = []
for ordinal, (label, text, expected_valid, flags) in enumerate(cases):
    case_dir = out_root / f"case-{ordinal:02}"
    classes = case_dir / "classes"
    classes.mkdir(parents=True, exist_ok=True)
    variant = case_dir / "Variant.scala"
    variant.write_text(text)
    process = subprocess.run([scalac, "-release", "21", *flags, "-d", str(classes), str(variant)], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    actual_valid = process.returncode == 0
    results.append({"case": label, "scala_3_9": "valid" if actual_valid else "invalid", "expected": "valid" if expected_valid else "invalid"})
    if actual_valid != expected_valid:
        print(f"{label}: expected {expected_valid}, got {actual_valid}\n{process.stdout}", file=sys.stderr)
        raise SystemExit(1)
(out_root / "results.json").write_text(json.dumps(results, indent=2) + "\n")
for result in results:
    print(f"{result['case']}: {result['scala_3_9']}")
