# Minimal Scala 3.9 standard-library method cohort

This closes the bounded measurement requested by #1705. The candidate scan
uses the repository's pinned Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8` and only `library/src/scala`.
The scan visited 649 files, parsed and named all of them without diagnostics
or file-level naming failures, and found 9,388 eligible canonical ordinary
methods with bodies. The scan output is intentionally not checked in; its
byte-identical repeated output had SHA-256
`b6a43b7f2dd2d6c1bec64143e4adca42cde17a29271039ed7ad303c07728d325`.

## Ranking and frozen cohort

Ranking is a deterministic syntax-based estimate of dependency/support cost,
not a semantic complexity claim:

```text
RHS positioned AST nodes
+ 4 * maximum positioned-node nesting depth
+ 8 * type parameters
+ 2 * value parameters
+ 3 * calls
+ 3 * selections
+ 10 * nested methods
+ 4 * (branches + 2 * matches + 2 * tries + 2 * closures
       + 2 * while loops + 2 * for comprehensions)
+ 12 * default parameters
+ 4 * inferred result type
+ min(source-file import count, 5)
+ 8 * same-owner same-name overloads
+ 100 * placeholder RHS (??? or NotImplementedError)
```

A0 is score 0–24, A1 is 25–55, and `edge` is above 55. Lambdas/functions,
parsed `try`, and `for` syntax are counted from their actual untyped phase
nodes. Contextual parameter status comes from source parameter modifiers;
method-level `implicit`/`given` modifiers are not treated as contextual
parameter clauses. The all-source inventory has 1,814 A0, 3,298 A1, and 4,276
edge candidates. Import count remains a file-level proxy, and the score does
not attempt to predict classpath resolution.

The selected set is frozen at 25 exact AST identities across eight files:
18 A0, three A1, and four edge cases. IDs were not swapped after fixing the
feature profile. Candidates are ordered by score, then source path and tree ID.
The compact manifest at
[`typer-stdlib-minimal-method-cohort-3.9.0.tsv`](typer-stdlib-minimal-method-cohort-3.9.0.tsv)
records source line, score/tier/features, baseline outcome, first blocker,
and the required typed subset. It is the reviewed cohort, not a raw corpus
dump.

## Oracle and baseline audit

All eight selected real Scala source files were compiled together by the pinned
Scala 3.9.0 compiler on JDK `21.0.8+9`, using the Scala 3.9.0 runtime/compiler
artifacts and `-Yexplicit-nulls -Ystop-after:typer`. The pinned compiler's
`-Xprint:typer` output confirmed the result shapes recorded for the ten
baseline-typed methods in the manifest. Generic results are alpha-normalized:
method/class type parameters use `typeparam#0`; `scala.util.Success.T` is the
owner-qualified type member printed by the compiler as `Try.this.T` /
`Success.this.T`. A positive generic identity control compiled. A negative
`String = 1` control failed with compiler diagnostic E007, confirming the
oracle rejects a mismatched result type. This is direct source-compiler
validation of the selected files and their typed trees; it does not claim
independent module-build validation or prove that dotty-rs and Scala accepted
the same individual blocked methods.

The dotty-rs whole-method audit ran serially twice with determinism verification;
the normalized reports matched in 1.579 seconds total (SHA-256
`ab9ada945d2df5e3e80f2545c19755c585cb3665144e527e4a873ff9e80fb9f7`). The
measured base is `bbd93722f66ce177788cdb8e63a147c510ddf74c`; no production typer
code changed in this increment. Toolchain inputs were Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala compiler/library 3.9.0,
Temurin JDK `21.0.8+9`, and dotty-rs base commit above. Compiler artifacts
were `scala3-compiler_3`, `scala3-library_3`, `scala-library`, `tasty-core_3`,
`scala3-interfaces`, `scala-asm`, `compiler-interface`, and `util-interface` at
the pinned versions listed in `tools/typer-classpath-corpus-audit/run`.

| Measure | Result |
| --- | ---: |
| Selected / attempted | 25 / 25 |
| Fully typed at baseline | 10 |
| Blocked | 15 |
| Excluded / not attempted | 0 / 0 |
| Newly typed by #1705 | 0 |
| Target: at least one newly typed method | **UNMET** |

“Fully typed” requires the canonical method symbol and completed signature,
typed method subtree, parameter/result mappings, and body mapping. Partial body
success does not count. The ten passing methods are pre-existing baseline
coverage, not issue gains. Their frozen result shapes are guarded by the
ordinary cohort gate. The remaining blockers span member lookup, parent
adaptation, unannotated function literals, unstable selections, classpath
loading, ambiguous type names, conformance, and term-name resolution. They are
recorded rather than expanded into adjacent typer work.

## Reproduction

Set `SCALA39_ROOT` to a checkout at the pin, `JAVA_HOME` to JDK 21,
`SCALA39_JDK_RELEASE=21`, and `COURSIER_MAVEN_ROOT` to the cache with Scala
3.9.0 artifacts. Generate the full inventory twice and compare output:

```sh
DOTTY_ORDINARY_CANDIDATE_INVENTORY=/tmp/ordinary-inventory.tsv \
  cargo test -p dotty-typer --test local_definition_audit \
  pinned_scala39_ordinary_member_candidate_inventory --locked -- --ignored
```

Compile the selected real source files with the pinned compiler:

```sh
SCALA39_CLASSPATH="$(printf '%s:' \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar")"
mkdir -p /tmp/scala39-ordinary-oracle
SCALA="$SCALA39_ROOT/library/src/scala"
java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Yexplicit-nulls -Ystop-after:typer \
  -Vprint:typer -d /tmp/scala39-ordinary-oracle \
  "$SCALA/Option.scala" "$SCALA/util/Either.scala" \
  "$SCALA/util/Try.scala" "$SCALA/Tuple.scala" \
  "$SCALA/Enumeration.scala" "$SCALA/Predef.scala" \
  "$SCALA/Function1.scala" "$SCALA/collection/Iterator.scala"
```

Run the deterministic dotty-rs audit and enforce the ten frozen complete results:

```sh
tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism /tmp/ordinary-cohort.tsv
```

The gate fails if a required identity is absent, blocked/partial, or has a
changed normalized result shape. Its behavior is covered for pass, regression,
missing method, and result-shape mismatch cases by
`tools/typer-classpath-corpus-audit/test_verify_ordinary_cohort.py`. No hosted
full-corpus audit was run.
