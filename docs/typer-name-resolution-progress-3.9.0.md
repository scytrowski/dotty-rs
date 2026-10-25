# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

## Latest increment: select a module's unique `apply` method

This increment starts from merged `main` at
`7d6c3ed3502d80efb235f994e551792fd2b4bdbb`, after #1600. In the pinned Scala
3.9.0 source, `InlineInfoAttribute.scala:115` constructs `Some(name + desc)`.
`Some` resolves to the canonical `scala.Some` companion module, whose generated
polymorphic `apply[A](value: A): Some[A]` method makes the module callable.
The source typer now selects that method when a module-class callee has exactly
one callable `apply`, then uses the existing polymorphic application inference
and conformance path. It preserves the selected method's symbol identity. A
module without `apply` still reports `ApplicationCalleeNotMethod`; ambiguous
module `apply` candidates remain explicitly deferred.

The focused regression exercises generic `Factory.apply[A]`, checks the
selected source method identity and inferred `Int` result, accepts plain
`private apply` from its declaring module, rejects unrelated private/protected
apply methods, and confirms that a non-callable module is rejected. Other
non-public visibility forms remain unsupported by this implicit selection.
The deterministic `package-term` audit used
the same 60 files and 369 methods in both runs. The normalized reports and
369-row cohort TSVs matched exactly. Both runs had no parser or namer failures
and typed zero complete methods. Eight frozen cohort rows advanced, all from
`ApplicationCalleeNotMethod`:

| Frozen row(s) | New first blocker | Evidence reached |
| --- | --- | --- |
| `InlineInfoAttribute.scala#tree=212,226,239` (`nextByte`, `nextUTF8`, `nextShort`) | `MemberNotFound` | `Some(...)` selects its companion `apply`; the enclosing `read#tree=420` attempt next fails while typing `name + desc`. |
| `ScalaVersion.scala#tree=234,254,264,335` | `UnsupportedExpression::InterpolatedString` | `Failure(...)` in `parse#tree=394` proceeds past module `apply` and reaches the interpolated failure message. |
| `MacroClassLoader.scala#tree=115` | `OverloadedSelectionDeferred` | `Option(...)` in `makeMacroClassLoader#tree=165` proceeds past module `apply`; the enclosing attempt next defers an overloaded selection. |

Aggregate first-blocker counts changed from `ApplicationCalleeNotMethod` 8 to
0, `MemberNotFound` 3 to 6, `UnsupportedExpression::InterpolatedString` 0 to
4, and `OverloadedSelectionDeferred` 1 to 2. All other first blockers were
unchanged. These are measured blocker transitions, not fully typed real
methods. The eight changed rows are recorded in
[`typer-name-resolution-1566-module-apply-3.9.0.tsv`](typer-name-resolution-1566-module-apply-3.9.0.tsv).

The deterministic 3-file / 31-method smoke audit also remained byte-for-byte
stable across its two runs. Outside the frozen cohort,
`StringParsers.parseInt#tree=482` advances from `ApplicationCalleeNotMethod` to
`ApplicationArgumentConformanceUnsupported` after resolving module `apply`;
the pinned smoke baseline records this intentional later blocker.

Reproduce the deterministic cohort comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-some-apply-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-some-apply.md
```

The next bounded investigation is the `MemberNotFound` reached from
`name + desc` in `InlineInfoAttribute.read#tree=420`; identify the exact
receiver and canonical `+` member before choosing a fix.

## Latest increment: adapt `if` expressions without `else` to `Unit`

This increment starts from merged `main` at
`91304853b7ffdc382107841414710de48e31e01f`, after #1598. The pinned source
`InlineInfoAttribute.scala:110` contains `if (hasSelf) nextUTF8()` without an
`else`. Before this change, the parser's synthetic zero-width `Unit` else was
joined with the `String` then branch. The typer tried to compare those nominal
types and stopped at `IfBranchJoinUnsupported` because `String` class info was
unavailable. Scala 3.9.0's typer oracle confirms that a no-else conditional
discards a non-`Unit` branch value, has type `Unit`, and warns about the
discarded value. An explicit `else ()` remains a two-branch join.

The source typer recognizes a parser-synthetic zero-width `Unit` else, including
when it is reached recursively through an `else if` chain. Each non-`Unit` then
value in that incomplete chain is wrapped as a typed block statement followed
by a canonical `Unit` literal, so the whole chain has type `Unit`. An explicit
`else ()` remains an ordinary two-branch join. The focused typer regression
checks direct and nested chains as well as the explicit-else type, and the
pinned oracle fixture is
[`IfWithoutElse.oracle.md`](../crates/dotty-typer/tests/fixtures/if-without-else/IfWithoutElse.oracle.md).
The initial implementation commit was
`f7f0ac67b9df1a272883c8648f84ca99bea460ba`; the review correction covering
incomplete `else if` chains is `aade2d4de2`.

The deterministic `package-term` comparison used the same 60 files and 369
methods before and after this increment and was repeated after the review
correction. Both final runs matched exactly, typed zero complete methods, and
had no parser or namer failures. Exactly the three `InlineInfoAttribute` rows
advance, with no other first-blocker changes:

| First blocker | Before (`913048`) | After (`f7f0ac`) |
| --- | ---: | ---: |
| `IfBranchJoinUnsupported` | 3 | 0 |
| `ApplicationCalleeNotMethod` | 5 | 8 |
| `MemberNotFound` | 3 | 3 |
| All other first blockers | unchanged | unchanged |
| Fully typed methods | 0 | 0 |

`InlineInfoAttribute.scala#tree=212` (`nextByte`), `#tree=226`
(`nextUTF8`), and `#tree=239` (`nextShort`) now pass the no-else branch join.
Their enclosing `read#tree=420` attempt next stops at
`ApplicationCalleeNotMethod`. This is measured blocker progress, not a fully
typed real method. The per-method rows and failing source branch are in
[`typer-name-resolution-1566-if-without-else-unit-3.9.0.tsv`](typer-name-resolution-1566-if-without-else-unit-3.9.0.tsv).
The deterministic focused 3-file / 31-method smoke audit also matches its
checked-in baseline exactly. The Scala 3.9.0 oracle fixture includes
`if (flag) "outer" else if (innerFlag) "inner"`; the typer output assigns the
whole chain `Unit` and warns for both discarded strings, while explicit
`else ()` remains `String | Unit`. After the review correction, all 884
`dotty-typer` unit tests and integration targets passed, as did Clippy,
rustdoc, formatting, and the frontend boundary guard.

Reproduce the cohort comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-if-unit-chain-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-if-unit-chain.md
```

The deterministic run above was executed after `aade2d4de2`; the two normalized
369-row cohort TSV outputs matched exactly and had no blocker-count changes
relative to the pre-correction result.

The next bounded investigation is `ApplicationCalleeNotMethod` in the
`InlineInfoAttribute.read#tree=420` attempt. Identify the exact application and
its canonical callee before choosing a fix; do not expand into general
application or overload support from this result alone.

## Latest increment: bounded `Int.&`

This increment starts from merged `main` at
`a92dec87aaee0b0aa24098e01c18d811dfd0cbb8`, after #1595. The pinned Scala
3.9.0 library declares `Int.&(x: Int): Int` in `library/src/scala/Int.scala`.
The source typer now registers that one exact canonical method identity beside
the existing bounded `Int` operators. It accepts `flags & 1` and `Int & Int`;
the real typer fixture asserts the selected `SymbolId`, result types, and
rejection of an unrelated source class. This increment does not add the
adjacent Byte, Short, Char, Long, OR, XOR, or shift overloads. The positive and
negative compiler oracle cases and reproduction command are recorded in
[`IntBitwiseAnd.oracle.md`](../crates/dotty-typer/tests/fixtures/int-bitwise-and/IntBitwiseAnd.oracle.md).
The implementation and focused regression fixtures were validated at
`2fe24ca588f3d9b4093082a35a6f22b95ccbe89e`.

The deterministic `package-term` comparison used the same 60 files and 369
methods before and after this change. Both runs typed zero complete methods and
had no parser or namer failures. Aggregate first-blocker counts changed only
for the three selected rows: `MemberNotFound` moved from 6 to 3 and
`IfBranchJoinUnsupported` moved from 0 to 3; all other counts stayed the same.
`InlineInfoAttribute.scala#tree=212` (`nextByte`), `#tree=226` (`nextUTF8`),
and `#tree=239` (`nextShort`) now type their `flags & 1` expressions and stop
later in the enclosing `read#tree=420` attempt at `IfBranchJoinUnsupported`.
This is verified first-blocker progress, not a fully typed real method. The
remaining three-row movement is recorded in
[`typer-name-resolution-1566-int-bitwise-and-3.9.0.tsv`](typer-name-resolution-1566-int-bitwise-and-3.9.0.tsv).

The focused 3-file / 31-method smoke audit remained deterministic and matched
its checked-in baseline exactly. Reproduce the `package-term` comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-bit-and-final-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-bit-and-final.md
```

The smoke comparison uses the same environment with
`--focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv`, then runs
`tools/typer-classpath-corpus-audit/verify_focused_gate.py` against the pinned
baseline.

The `IfBranchJoinUnsupported` follow-up from this historical increment was
investigated and resolved by the newer no-else `Unit` adaptation section above.

## Latest increment: universal equality

This increment starts from `main` at
`ed3325e57395a315b5e8b78cd9a9c0ca0bbc379d`, after #1593. The pinned Scala
3.9.0 compiler synthesizes final `Any.==` and `Any.!=` methods. The source typer
now registers these canonical builtin symbols, resolves universal equality
alongside narrower source overloads, and rejects statically incompatible
primitive/reference operand pairs after bounded alias normalization. An
applicable source overload keeps its symbol identity; otherwise compatible
operands use the canonical universal method.

The Scala 3.9 oracle accepts equality on `Any`, Boolean, numerics within the
numeric group, and references, and rejects `Int == String` with “Values of
types Int and String cannot be compared with == or !=”. Focused tests cover
both operators, literal and widened numeric operands, `Any` with `Int`,
compatible aliases, inherited universal equality alongside narrower class
overloads, and incompatible primitive/reference pairs. Oracle fixture details
are recorded in
[`UniversalEquality.oracle.md`](../crates/dotty-typer/tests/fixtures/universal-equality/UniversalEquality.oracle.md).

The deterministic `package-term` comparison uses the same 60 files and 369
methods before and after the equality change. Both runs typed zero complete
methods and had no parser or namer failures. Aggregate first-blocker counts
were unchanged (`ApplicationArgumentConformanceUnsupported=1`,
`MemberNotFound=6`, `SymbolResolution=91`, all other blockers=271). However,
the three `InlineInfoAttribute.read` attempts progress past `version == 1` and
now stop at `flags & 1`; the blocker remains `MemberNotFound`. No full method
typing is claimed. Per-row movement is recorded in
[`typer-name-resolution-1566-universal-equality-3.9.0.tsv`](typer-name-resolution-1566-universal-equality-3.9.0.tsv).

## Previous increment: Scala and JVM array conformance

This increment starts from `main` at `844c8f4965992504cfa21598309d6650c90d2b9d`,
after #1591. It allows the canonical `scala.Array[T]` and `Type::JavaArray`
representations to conform when their element types are equivalent. Element
types remain invariant. The matcher verifies the canonical `scala.Array` class
by its type namespace and package owner chain, including classpath-loaded
symbols whose `scala` package is absent from the source package registry.
Unrelated classes named `Array` do not receive this relation.

Focused unit controls cover `Array[Char]` against `char[]`, reject mismatched
element types and `custom.Array[Char]`, preserve `Array[Char] <: Object`, and
reject a `scala.Array` symbol with a noncanonical package prefix. A Scala 3.9.0
compiler oracle accepts passing `Array[Char]` to a Java `char[]` parameter and
rejects `Array[Byte]` at that call site.

The deterministic `package-term` comparison uses the same 60 files and 369
methods before and after this change. Both runs typed zero complete methods and
had no parser or namer failures. Three rows move forward without regressions:

| First blocker | Before (`844c8f`) | After this increment |
| --- | ---: | ---: |
| `ApplicationArgumentConformanceUnsupported` | 4 | 1 |
| `MemberNotFound` | 3 | 6 |
| `SymbolResolution` | 91 | 91 |
| All other first blockers | 271 | 271 |
| Fully typed methods | 0 | 0 |

`InlineInfoAttribute.scala#tree=212` (`nextByte`), `#tree=226` (`nextUTF8`),
and `#tree=239` (`nextShort`) now pass the array argument check. Their enclosing
`read` attempt next stops at `MemberNotFound` for `==` in `version == 1`.
This increment does not claim a fully typed real method. Per-row results are in
[`typer-name-resolution-1566-array-conformance-3.9.0.tsv`](typer-name-resolution-1566-array-conformance-3.9.0.tsv).

The report below records earlier #1566 increments and their historical
baselines; its “current” labels refer to those recorded runs, not this latest
array-conformance result.

## Baselines and increments

The cumulative comparison starts from `main` at
`e06c4eeea02471f674c6b4c0263eaf69f3bc1f60`, before #1577 was merged. PR #1577
added the Java static-qualifier increment, handling an imported Java class
used as a qualifier for static members. Its historical Scala 3.9 oracle check
accepted `Paths.get("input.txt")` after importing `java.nio.file.Paths`, and rejected
`Paths` as a value and instance-method selection through the class. Those
results describe that earlier increment, not this PR.

The historical pre-increment baseline for the mutable-local `+=` change is
`main` at
`bfc4759ea7d4eca10767ef5021ffc8386178c4a5`; it includes #1586, #1577, #1571,
and #1574. That increment added a bounded fallback for `+=` on a mutable local `Int`:
if direct `+=` member lookup misses and no visible extension candidate takes
precedence, it types `value + rhs` and lowers that result to assignment while
preserving the local symbol identity. The pinned inputs are Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and JDK
21.0.8.

## Frozen-cohort result

The pinned `package-term` cohort covers 60 files and 369 real methods. The
fresh pre-increment baseline and latest run each attempted all 369 methods;
both typed zero methods. That earlier increment advanced three rows with no
first-blocker regressions. The normalized reports and resolver sidecars for that run
matched byte-for-byte across two runs; neither run had parser or namer
failures.

### Historical `+=` comparison: `bfc4759` to that increment

| First blocker | Pre-increment rows | Current rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 1 | 1 |
| `MemberNotFound` | 6 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

| Source method | Pre-increment blocker | Current blocker | Attempt |
| --- | --- | --- | --- |
| `InlineInfoAttribute.scala#tree=212` (`nextByte`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |
| `InlineInfoAttribute.scala#tree=226` (`nextUTF8`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |
| `InlineInfoAttribute.scala#tree=239` (`nextShort`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |

The rewrite types the mutable local integer update in each method. At that
point, their next failure was loading `scala.Array`, whose TASTy parent
`java.io.Serializable` remained unresolved. The latest increment above moves
past that boundary but still does not fully type these methods.

### Historical cumulative snapshot: `e06c4ee` to the `bfc4759` increment

Four rows advanced across the earlier increments captured in the compact TSV:
one in merged #1577 and three in the mutable-local `+=` change. This table
compares the historical baseline with the result at that earlier increment;
it is not the `bfc4759` pre-increment comparison above.

| First blocker | Historical baseline rows | Current rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 2 | 1 |
| `MemberNotFound` | 5 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

The complete per-method before/after reconciliation for that historical
snapshot is in
[`typer-name-resolution-1566-cohort-3.9.0.tsv`](typer-name-resolution-1566-cohort-3.9.0.tsv).
Its `before_*` columns refer to the historical `e06c4ee` baseline; its
`after_*` columns refer to the current implementation.
The earlier default Scala package-term increment remains recorded in
[`typer-name-resolution-progress-3.9.0.tsv`](typer-name-resolution-progress-3.9.0.tsv).

## Previously recorded bounded blockers

The next failure in `expandArg` is Scala's `String.stripPrefix` extension
method. The pinned Scala source defines `Predef.augmentString` with the alias
`Predef.String` as its parameter (`library/src/scala/Predef.scala:171`). The
classpath resolver finds the canonical `scala.Predef$.augmentString` method,
but the current TASTy member-type lowering treats its parameter name
`scala/Predef$String` as a JVM class. No such class exists, so the parameter is
represented as `Type::Error` with the loader diagnostic `class not found:
scala/Predef$String`. Replacing that error with `java.lang.String` by spelling
would manufacture a type instead of resolving the alias, so no implicit-view
call is synthesized. Correct support needs alias-aware TASTy member-type
resolution; general implicit search remains outside this bounded increment.
At that time, the three advanced methods encountered a separate classpath boundary:
decoding `scala.Array` fails on its selected-qualified `java.io.Serializable`
supertype reference. The loader leaves semantic references unresolved when it
cannot prove their owner path; this increment does not add a spelling-based
package guess.

The fresh pre-increment baseline at `bfc4759ea7d4eca10767ef5021ffc8386178c4a5`
and the latest implementation each audited all 60 files and 369 methods twice
with deterministic normalized reports and sidecars. In the latest report,
`CommandLineParser.scala#tree=467` still stops at `MemberNotFound` for
`String.stripPrefix`; the three `InlineInfoAttribute` rows stop at
`SymbolResolution`. The compact TSV records the cumulative before/after rows.
No full-corpus result is claimed.

## Reproduction

The Scala oracle command below validates the earlier Java static-qualifier
increment, not this PR. Run it with the pinned artifacts by setting `JAVA_HOME`
and `SCALA39_CLASSPATH`, then:

```sh
python3 tools/typer-classpath-corpus-audit/scala39_import_oracle.py
cargo test -p dotty-typer --test refactor_contract java_class_import_qualifier --locked
```

The deterministic pre-increment baseline audit used:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-latest-baseline-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-latest-baseline.md
```

The deterministic audit for the array-conformance increment used the same
command with `target/issue-1566-array-audit-final.md` as the output and
`target/issue-1566-array-audit-final-runs` as `DOTTY_AUDIT_TMP_ROOT`. The
universal-equality increment used `target/issue-1566-equality-final.md` and
`target/issue-1566-equality-final-runs`; both normalized reports and resolver
sidecars matched across runs. The equality audit was run after removing
temporary diagnostics. Its result shows the same first-blocker counts while
the three noted expressions advance from `version == 1` to `flags & 1`.

The CI focused smoke manifest was also rerun twice after the equality
increment. Its normalized output is deterministic and is recorded in
[`typer-focused-smoke-baseline-3.9.0.txt`](typer-focused-smoke-baseline-3.9.0.txt).
In the selected 31 methods, no method becomes fully typed. Three
`StringParsers.scala` rows move from `MemberNotFound` to `MemberLookup`,
`ApplicationCalleeNotMethod`, and `MemberLookup`; `scala.Some` is now
materialized while resolving the affected branch. The aggregate selected
blockers move from 11 to 12 `ApplicationCalleeNotMethod`, from 3 to 0
`MemberNotFound`, and add 2 `MemberLookup` rows. This is audit progress, not a
claim that the surrounding methods now type completely.
