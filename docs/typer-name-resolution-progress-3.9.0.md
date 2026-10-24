# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

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
