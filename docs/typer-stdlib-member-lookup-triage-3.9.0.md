# Scala 3.9 stdlib MemberLookup triage

Issue #1808 classifies eight exact ordinary-method roots from fresh sessions,
groups their causes, and records only complete canonical `DefDef` results.
A method is typed only when its signature, parameters, entire RHS, result
conformance, owned typed tree, and exact source-to-typed mapping are present.

## Revisions and frozen cohorts

- Fresh current-main baseline: `b62fbfda7421e2afac423ccc1251d31886e784c7`.
- Scala source and library revision:
  `777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0).
- Oracle runtime: Temurin JDK `21.0.13+9`.
- Main Tier A, unchanged manifest: **14/25**, with zero regressions.
- Former holdout, now analysis cohort, unchanged manifest: **15/25**, up from
  its frozen 9/25. Six newly complete methods are `App.scala#tree=36`,
  `Byte.scala#tree=516`, `Char.scala#tree=510`, `Short.scala#tree=504`,
  `ArrayBuilder.scala#tree=48`, and `HashMap.scala#tree=125`.
- Two-pass normalized audit SHA-256: main
  `2be03443d031397c61fa32049fa7b3c47ed635656aef98379b0834957538a847`;
  analysis
  `3730ce339eafabcb4d71c0a0913fc90f2998bd61b22fc8408a49e3fcdfcac701`.
  Both used `--verify-determinism`; the separate 50-method union audit also
  passed its two-pass comparison.

The per-root before/after ledger is
[`typer-stdlib-member-lookup-triage-3.9.0.tsv`](typer-stdlib-member-lookup-triage-3.9.0.tsv).
It keeps the exact eight identities and first blockers. The 25 analysis
identities were not replaced or re-ranked.

## Verified causes and changes

The eight nested `MemberLookup` errors resolve into four causes:

1. **Unqualified package type lookup** missed `ReusableBuilder`, `AbstractMap`,
   and `Prop`. Type resolution now asks the configured resolver for the exact
   type member in the current or enclosing package after lexical source lookup
   and imports. It accepts only symbols owned by that package chain, ignores
   wider resolver results, preserves canonical builtin identity, and retains
   resolver ambiguity/errors. `ArrayBuilder.knownSize` and
   `HashMap.unimproveHash` now type completely. `PropImpl.clear` advances past
   parent completion but remains blocked by a separate `Unit` RHS conformance
   error; it is not counted as a gain.
2. **The compiler-defined `scala.Matchable` parent was absent from builtins.**
   Scala 3.9's `AnyVal` source class extends `Matchable`, which has no ordinary
   library class to load. `Definitions` now owns the canonical `Matchable`
   trait and its `Any` parent; class loading and type alias resolution reuse
   that identity. This completes `Short.short2float`, `Byte.byte2double`, and
   `Char.char2double` without synthesizing conversion methods.
3. **MapOps higher-kinded type parameter completion** remains deferred while
   typing `clone` (`C`, type tree 111). No broader type parameter change was
   justified.
4. **Canonical Double member class info** remains unavailable for
   `DoubleIsFractional.negate`'s unary minus. No class-info or member fallback
   was added.

These changes do not alter class/member identity reconciliation from #1812 or
Namer identity work from #1826. They add a compiler builtin and resolve a type
in package scope; they do not merge names, manufacture members, or turn lookup
failures into subtype success.

## Scala 3.9 oracle evidence

Oracle fixtures are in
[`crates/dotty-typer/tests/fixtures/member-lookup-oracle-3.9.0`](../crates/dotty-typer/tests/fixtures/member-lookup-oracle-3.9.0).
With pinned compiler/library jars and JDK 21, Scala 3.9.0 compiled:

- `Short.toFloat`: `(S)F`, bytecode `i2f`;
- `Byte.toDouble`: `(B)D`, bytecode `i2d`;
- `Char.toDouble`: `(C)D`, bytecode `i2d`;
- `Double` unary minus: `(D)D`, bytecode `dneg`;
- `Numeric.DoubleIsFractional.negate`: call descriptor `(D)D`;
- source-owned `unary_-`: call to that source declaration.

The negative fixtures separately fail with `toFloat is not a member of
Array[Int]` and `Short.toFloat does not take parameters`, checking wrong
receiver and wrong arity. The compiler jar SHA-1 is
`f0c9c4f2ad340ce7408064ad6b0eb7fa9a9b8733`.

Recheck the positive oracle with:

```sh
ORACLE_CP="$(find "$COURSIER_MAVEN_ROOT" -type f -name '*.jar' | sort | paste -sd:)"
java -cp "$ORACLE_CP" dotty.tools.dotc.Main \
  -classpath "$ORACLE_CP" -d target/member-lookup-oracle/classes \
  crates/dotty-typer/tests/fixtures/member-lookup-oracle-3.9.0/MemberLookupOracle.scala
javap -classpath target/member-lookup-oracle/classes -p -s -c 'MemberLookupOracle$'
```

Compile each `Negative*.scala` separately and expect a nonzero exit. The
project prefix-operator regression asserts the selected identity for a
source-owned unary operator.

## Reproduction and validation

Use the pinned Scala checkout, JDK 21, and Maven cache. Keep audit and Cargo
scratch under `target/` when `/tmp` is constrained.

```sh
cargo test -p dotty-core --lib matchable_is_a_canonical_compiler_defined_trait_with_any_parent --locked
cargo test -p dotty-classloader compiler_builtin_class_names_reuse_definitions_and_ignore_int_classpath_entry --locked
cargo test -p dotty-classloader compiler_builtin_aliases_are_scala_type_members_without_classpath_probes --locked
cargo test -p dotty-typer --lib unqualified_external_type --locked

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism target/member-lookup-main-25.tsv
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism target/member-lookup-analysis-25.tsv
```

Lookup regressions cover current and enclosing package identity, reject a
sibling-package and a term-namespace result, preserve resolver ambiguity, and
assert checkpoint/source-index rollback on failures and retry. Matchable tests
assert canonical trait identity, parent, package membership, and behavior when
a same-named classpath entry is present. Historical local-method denominators
and the 1,236-file corpus are not part of these results.
