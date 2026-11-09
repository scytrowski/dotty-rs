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
`RuntimeProbe.java` is the frozen #2187 oracle input; its manifest SHA-256 is
`207e7a5bca59ace13abe384655b9ea825ac3e9f4023e298156c9de7095ebe0ad`. The
expanded #2192 Java consumer is stored separately as `RuntimeProbe2192.java`.

## Reproduce the Scala oracle

Use Scala 3.9.0 at source revision
`777528f19a58e794c9954a42f433373472ec57f8`, the published 3.9.0 Scala runtime
jars listed in `oracle.json`, and Eclipse Temurin JDK 21. The captured run used
JDK `21.0.8+9`; tool and input hashes are in the manifest. Point `SCALAC` to
the pinned compiler's `scalac`, and direct all temporary and output files to a
managed scratch directory.

```sh
set -eu
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR=/path/to/managed-scratch
export TMP="$TMPDIR" TEMP="$TMPDIR"
export JAVA_TOOL_OPTIONS="-XX:-UsePerfData -Djava.io.tmpdir=$TMPDIR"
SCALAC=/path/to/scala-3.9.0/bin/scalac
OUT="$TMPDIR/ordered-local-unit-calls"
mkdir -p "$OUT/classes" "$OUT/typed"

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

At the original #2187 audit baseline, the paired planner rejected after
frontend typing at `cross-class constructor-call identity proof`:

```text
SourceShape("factory body must be a direct Apply or a one- or two-local Block")
```

That historical observation predates the merged #2189–#2191 producer work.
The integrated acceptance run below verifies the current two-call path and
must be used for the final-gate result.

## Control matrix

| Input | Scala/source validity | Original #2187 paired-path result |
| --- | --- | --- |
| Existing no-call two-local fixture | Accepted | Accepted |
| Existing one-call fixture | Accepted | Accepted |
| Two-call target | Accepted and runtime verified | Constructor-call identity proof: factory body shape |
| Reversed local declarations | Accepted source shape | Constructor-call identity proof: factory body shape |
| Swapped call order | Accepted | Constructor-call identity proof: factory body shape |
| Both calls on the same local | Accepted | Constructor-call identity proof: factory body shape |
| Duplicate identical call (`first.set(secondSeed)` twice) | Accepted | Constructor-call identity proof: factory body shape |
| Modified argument | Accepted | Constructor-call identity proof: factory body shape |
| Third call | Deferred by issue scope | Constructor-call identity proof: factory body shape |
| Wrong arity | Rejected by Rust typer | Method body typing: `ApplicationArityMismatch` |
| Boolean argument | Rejected by Rust typer | Method body typing: `ApplicationArgumentTypeMismatch` |

The zero-call and one-call paths remain covered by their existing tests. The
controls keep source-shape rejection distinct from invalid typed syntax. A
third call remains outside this audit. No producer, pickler, JVM, or public API
behavior was changed by this final evidence gate.

## Integrated two-call gate on merged prerequisites

The final-gate baseline is `d71d912b1803efc0b8346b57846f1233b4d13734`,
which includes the reviewed merges for issues #2187–#2193. This integrated
run used Rust 1.98.1, Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8` (TASTy 28.9), and Temurin JDK
21.0.13+9. The compiler reported
`3.9.0-bin-SNAPSHOT-git-777528f`; the selected JDK binary SHA-256 was
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`.
Temporary files and generated outputs were kept in the registered task scratch
directory.

| Issue | Reviewed PR head | Published review | Merge commit |
| --- | --- | --- | --- |
| #2187 | `dcf8ebaf01101b0b64d9f323d816bf05ca193c6a` | [PR #2196 review](https://github.com/scytrowski/dotty-rs/pull/2196#pullrequestreview-5818058666) | `69f3404c5d371b326573a765876cec957dab1ce1` |
| #2188 | `b5f2984fda543b064b7a86d6b6591641cd59caa3` | [PR #2199 review](https://github.com/scytrowski/dotty-rs/pull/2199#pullrequestreview-5818645168) | `3161f47445873deff36085f3aabeaea56489d921` |
| #2189 | `2a38a58d2057c55d938e4cb10ec1f6322216a662` | [PR #2200 review](https://github.com/scytrowski/dotty-rs/pull/2200#pullrequestreview-5819184225) | `7d568478c4e7d8c9e44b100447ab11de9eb9bde0` |
| #2190 | `1d0a29c9738d7501c5572a287fe0e31539e2672f` | [PR #2209 review](https://github.com/scytrowski/dotty-rs/pull/2209#pullrequestreview-5819905091) | `93e1ec9a6252f1995ff83f9cfe76488a3598b5a8` |
| #2191 | `4966977cd39de186e13299c5608727f1672b577f` | [PR #2214 review](https://github.com/scytrowski/dotty-rs/pull/2214#pullrequestreview-5820316026) | `34b802eb451c0fc9dd2914e031e69313cc29e1ec` |
| #2192 | `9857709ce0f7a0e7d11d20a9ebd8fb2436cd8705` | [PR #2217 review](https://github.com/scytrowski/dotty-rs/pull/2217#pullrequestreview-5820616099) | `b26e5a4286a2af1278316d7ea6e48392131f628b` |
| #2193 | `626b9ce1df954879f76105dabb2078358ff812a5` | [PR #2219 review](https://github.com/scytrowski/dotty-rs/pull/2219#pullrequestreview-5823122087) | `d71d912b1803efc0b8346b57846f1233b4d13734` |

The required test command was run with
`DOTTY_REQUIRE_SCALA_TASTY_TWO_LOCAL_COUNTER_SET_CONSUMER=1`,
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`, and
`DOTTY_CLASSFILE_REQUIRE_JDK=1`:

```sh
cargo +1.98.1 test -p dotty \
  --test jvm_tasty_two_local_counter_set_scala_consumer \
  --locked -- --nocapture --test-threads=1
```

All nine tests passed. The explicitly labelled `two-call-original` input is
`tests/fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala`
(SHA-256
`2e60a40b35587559589b431c9cc191f5d752ea3b4219aa2fc71b99edd2da3b8b`). Its
Scala client is
`tests/fixtures/jvm_tasty_two_local_counter_set/TwoCallConsumer.scala` (SHA-256
`977c8742af2c6175bbe57f651d74845ddb50444cc107eb692190436cd7c6f7a0`), and the
expanded Java 21 client is
`tests/fixtures/jvm_tasty_ordered_local_unit_calls/RuntimeProbe2192.java`
(SHA-256
`1e4704b49a79591c80f60ae97ed26850a75bea5d25fa247e68b6db423e0d8269`). Its six
original Rust artifacts were emitted and linked as follows:

| Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | linked by Counter class |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | linked by Pair class |
| `example/PairFactory.class` | `5b9b6633c58972cb795e32beafa3e4b8d38eb3d942b9246063aefe6eb35c430e` | `004181d6451a3bc2000000000000819d` |
| `example/PairFactory.tasty` | `9e1a96a9670922573b2f3ba782070df738f9ff90fdba5838139ecae2088c4632` | linked by PairFactory class |

The `two-call-original` Rust graph check observed TASTy block `57` with direct
local addresses `[75, 100]`, used those same addresses as final `Pair`
arguments, and retained the source order of both calls. The pinned Scala reader
observed the same block and addresses, calls `[(0, 1), (1, 0)]`, and final Pair
arguments `[75, 100]`. Its `PairFactory.class` is classfile major `65`, method
descriptor `(II)Lexample/Pair;`, `max_stack=4`, and `max_locals=5`. `javap -c`
confirmed `aload_3; iload_2; invokevirtual Counter.set(I)V`, then
`aload 4; iload_1; invokevirtual Counter.set(I)V`, followed by the Pair
constructor with locals 3 and 4. The original Rust artifacts remained
unchanged after Scala compilation and runtime execution. The test also
exercised reversed local declarations, reversed calls, same-receiver calls,
malformed and missing-call controls, source-contract rejection, Scala lookup
and TASTy negatives, and
Java verifier descriptor negatives; observed first failures are recorded by
the test output without treating them as acceptance failures.

The test verifies two distinct counter instances and the expected results for
`create(7, 3)` and `create(-9, 42)`. The strict target manifest validates 25
required targets over four shards and includes this Scala consumer. The
existing `jvm-tasty-factory-scala-consumer` workflow also runs when this
receipt changes, so documentation-only edits cannot bypass the integrated
gate. The focused Java 21 test
`ordered_two_local_unit_calls_execute_under_strict_jdk21` passed with
`DOTTY_CLASSFILE_REQUIRE_JDK=1` (1 passed, 8 filtered). The integrated
acceptance and full local workspace test head was
`e3ef99e6b62ec33120d99b1e117cd0861cc3cfc1`; all source, client, graph, artifact,
bytecode, Scala, Java, and negative-control evidence above was captured from
that checkout. The receipt appendix itself is a documentation-only successor.

### Hosted workflow evidence for the integrated test head

Both required pull-request workflows completed successfully on the integrated
test head `e3ef99e6b62ec33120d99b1e117cd0861cc3cfc1`:

| Workflow/job | Outcome | Run / job |
| --- | --- | --- |
| Strict Scala/TASTy shard 1/4 | success | [job 129090838306](https://github.com/scytrowski/dotty-rs/actions/runs/42596602664/jobs/129090838306) |
| Strict Scala/TASTy shard 2/4 | success | [job 129090837832](https://github.com/scytrowski/dotty-rs/actions/runs/42596602664/jobs/129090837832) |
| Strict Scala/TASTy shard 3/4 | success | [job 129090838099](https://github.com/scytrowski/dotty-rs/actions/runs/42596602664/jobs/129090838099) |
| Strict Scala/TASTy shard 4/4 | success | [job 129090838307](https://github.com/scytrowski/dotty-rs/actions/runs/42596602664/jobs/129090838307) |
| Pinned Scala 3.9 factory TASTy aggregate consumer | success | [job 129098549506](https://github.com/scytrowski/dotty-rs/actions/runs/42596602664/jobs/129098549506) |
| Rust checks (format, frontend boundaries, Clippy, rustdoc, workspace tests, diff) | success | [job 129090752270](https://github.com/scytrowski/dotty-rs/actions/runs/42596602689/jobs/129090752270) |
| Classfile JVM encoder gate | success | [job 129090752790](https://github.com/scytrowski/dotty-rs/actions/runs/42596602689/jobs/129090752790) |

The full local locked workspace test also passed on the integrated test head
with every required strict consumer flag, the pinned Scala source/compiler and
runtime jars, and JDK 21.0.13+9. The current PR head is available from
[PR #2222](https://github.com/scytrowski/dotty-rs/pull/2222); its receipt-only
successor changes this evidence document, not the tested producer or consumer
code.
