# Ordered local `Unit` calls: Scala 3.9 and paired-path audit

This report records the exact two-call source delta from issue #2187 against
merged main `dbec2acb7fe9ce8dd43078b55e52f9dba56f05cc`. That main commit
descends from the one-call audit merge `bc190f52e78a313fe747ad7e1e9a800741361908`
(#2170 / PR #2185). GitHub's recent commit listing confirmed `dbec2acb` was
main when the audit ran; local Git fetch could not resolve GitHub in the
restricted network environment. The commit object was present locally and its
required ancestry was verified before work.

The fixture at
`tests/fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala`
retains the `Counter`, `Pair`, and `PairFactory` declarations from the existing
one-call fixture. Its `create` method contains exactly two immutable
`Counter` locals, then `first.set(secondSeed)`, `second.set(firstSeed)`, and the
final `Pair` construction. The Java runtime probe checks `(7, 3) -> (3, 7)` and
`(-9, 42) -> (42, -9)`, and verifies that the two stored counters are distinct.

## Reproduce the Scala oracle

Use Scala 3.9.0 at source revision
`777528f19a58e794c9954a42f433373472ec57f8`, the published 3.9.0 Scala runtime
jars listed in `oracle.json`, and Eclipse Temurin JDK 21. The captured run used
JDK `21.0.8+9`; tool and input hashes are in the manifest. Point `SCALAC` to
the pinned compiler's `scalac`, and direct all temporary and output files to a
managed scratch directory.

```sh
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR=/path/to/managed-scratch
export TMP="$TMPDIR" TEMP="$TMPDIR"
export JAVA_TOOL_OPTIONS="-XX:-UsePerfData -Djava.io.tmpdir=$TMPDIR"
SCALAC=/path/to/scala-3.9.0/bin/scalac
OUT="$TMPDIR/ordered-local-unit-calls"
mkdir -p "$OUT/classes"

"$SCALAC" -Werror -release 21 -d "$OUT/classes" \
  tests/fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala
javac --release 21 -cp "$OUT/classes" -d "$OUT/classes" \
  tests/fixtures/jvm_tasty_ordered_local_unit_calls/RuntimeProbe.java
java -Xverify:all -cp "$OUT/classes" example.RuntimeProbe
javap -v -c -p -classpath "$OUT/classes" example.PairFactory \
  > "$OUT/PairFactory.javap.txt"
"$SCALAC" -color:never -print-tasty "$OUT/classes/example/PairFactory.tasty" \
  > "$OUT/PairFactory.print-tasty.txt"
"$SCALAC" -Werror -release 21 -Vprint:typer -d "$OUT/typed" \
  tests/fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala \
  > "$OUT/typer.txt" 2>&1
```

The runtime output is `3/7 distinct=true` and `42/-9 distinct=true`. The
captured method has descriptor `(II)Lexample/Pair;`, `stack=4`, `locals=5`, two
`Counter.set(I)V` invocations in source order, and then
`Pair.<init>(Counter,Counter)V`. The stored `print-tasty.txt` reports TASTy
28.9.0 and `BLOCK@58` with ordered `VALDEF@76`, `VALDEF@99`, `APPLY@120`,
`APPLY@131`; its result is `APPLY@60`. The TASTy method selection references
`set[Signed Signature(List(scala.Int),scala.Unit) @set]`. The binary artifacts,
full `javap` output, compiler typer output, and hashes are checked in alongside
the source.

## Rust frontend identity and first paired rejection

The focused integration test uses one lexer → parser → namer → typer session
with source ID 21870. It observes the `create` formals as `SymbolId(61)` and
`SymbolId(62)`, and the two typed local bindings as `SymbolId(63)` and
`SymbolId(64)`, both owned by `create`. The two original source statements map
to typed calls `TreeId(42)` and `TreeId(46)`; both select the same canonical
`Counter.set` symbol `SymbolId(48)`. Call one binds receiver `SymbolId(63)` to
formal `SymbolId(62)`, and call two binds receiver `SymbolId(64)` to formal
`SymbolId(61)`. Both local declaration trees map to their typed `ValDef`
declarations. The final source expression maps to its typed `Pair` constructor
application, whose selected constructor is owned by the canonical `Pair`
symbol and whose ordered arguments retain local identities `SymbolId(63)` and
`SymbolId(64)`. These identities come from the session indexes and typed
references, not spelling or statement position.

The real paired planner rejects only after frontend typing, at
`cross-class constructor-call identity proof`:

```text
SourceShape("factory body must be a direct Apply or a one- or two-local Block")
```

The two calls are typed and ordered in the source and typed blocks, but the
paired source-shape proof expects the final constructor application after no
calls or one supported local receiver call. The paired artifact API returns an
error and publishes no accepted artifact batch.

## Control matrix

| Input | Scala/source validity | Current paired-path first result |
| --- | --- | --- |
| Existing no-call two-local fixture | Accepted | Accepted |
| Existing one-call fixture | Accepted | Accepted |
| Two-call target | Accepted and runtime verified | Constructor-call identity proof: factory body shape |
| Reversed local declarations | Accepted source shape | Constructor-call identity proof: factory body shape |
| Swapped call order | Accepted | Constructor-call identity proof: factory body shape |
| Both calls on the same local | Accepted | Constructor-call identity proof: factory body shape |
| Duplicate call / modified argument | Accepted | Constructor-call identity proof: factory body shape |
| Third call | Deferred by issue scope | Constructor-call identity proof: factory body shape |
| Wrong arity | Rejected by Rust typer | Method body typing: `ApplicationArityMismatch` |
| Boolean argument | Rejected by Rust typer | Method body typing: `ApplicationArgumentTypeMismatch` |

The zero-call and one-call paths remain covered by their existing tests. The
controls keep source-shape rejection distinct from invalid typed syntax; they
do not claim that the Rust compiler accepts the two-call target. A third call
remains outside this audit. No producer, pickler, JVM, or public API behavior
was changed.
