# Package prefix equivalence for `Option.map` (#2197)

This note records the bounded typer change and the next outcome for the
original Scala 3.9.0 `Option.map` method. It does not adapt `Some[A]` to
`Option[B]`, complete missing source class information, or change branch-join
policy.

## Reproduction input

- dotty-rs base: `69f3404c5d371b326573a765876cec957dab1ce1`.
- Scala source: `library/src/scala/Option.scala#tree=214` from revision
  `777528f19a58e794c9954a42f433373472ec57f8`.
- The focused replay uses the original complete `DefDef` and captures live
  relation operands in the transaction failure observer before rollback.

## Prefix graph captured before the change

At the original branch join (tree 213), the left type was the canonical source
`scala.None$` module class:

```text
TypeRef(prefix = TypeRef(prefix = NoPrefix, target = scala), target = None$)
```

The right type was canonical source `scala.Some` applied to method parameter
`B`:

```text
Applied(
  TypeRef(prefix = ThisType(scala package), target = Some),
  B
)
```

These are the original method's source symbols and type nodes, observed before
the typing transaction rolls them back. The earlier failure was
`IfBranchJoinUnsupported` with nested
`NonClassLikeSymbol { kind: Package }` for the `scala` package `ThisType` used
as a class-reference prefix.

## Result after package prefix handling

Replaying the same source tree succeeds through package-prefix validation and
the branch join. The unchanged whole method next reports:

```text
ExpectedExpressionConformanceUnsupported {
  source: SourceId(0),
  tree_index: 213,
  actual: Or(None$, Some[B]),
  expected: Option[B],
  error: ClassInfoUnavailable { symbol: None$, state: Missing }
}
```

The captured `actual` remains the `Or` of the canonical `None$` branch and
`Some[B]`; `expected` is the method's `Option[B]` result. This records the
exact next relation boundary: `None$` class information is still missing.
Generic parent adaptation and source class completion remain outside this
increment.

Both protected ordinary full-method acceptance cohorts (25 main and 25
analysis roots) passed their required-identity and determinism gates. The main
report retains `Option.scala#tree=214` as a valid blocked attempt with this
same `ExpectedExpressionConformanceUnsupported` outcome; it receives no whole
method typed credit.

## Validation command

With the pinned source checkout, JDK 21, and the eight verified Scala 3.9.0
artifact JARs on `SCALA39_CLASSPATH`, run:

```sh
cargo test -p dotty-typer --lib \
  captures_acceptance_iv_relation_operands_before_transaction_rollback \
  -- --ignored --nocapture
```

The test asserts the captured pre-rollback graph, canonical source identities,
the exact next `Option.map` relation error, and the unchanged `Tuple.toArray`
control outcome.
