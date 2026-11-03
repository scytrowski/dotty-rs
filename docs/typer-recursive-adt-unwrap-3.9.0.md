# Recursive ADT `Evaluator.unwrap` proof

This note records the complete source-typing proof for the original
`Evaluator.unwrap(expr: Expr): Expr` method in the frozen Scala 3.9.0 fixture.
The source and pinned oracle evidence are documented in
[`typer-recursive-adt-baseline-3.9.0.md`](typer-recursive-adt-baseline-3.9.0.md).

The earlier authentic root attempt stopped at `Succ.inner` with
`UnsupportedProduct1FieldType`, before typing the method body. After merged
generated-`unapply` and Product1 projection support, the same source root
`Expr.scala#tree36` produces a complete typed `DefDef`. The compact
before/after receipt is checked in as
[`rust-unwrap-before-after.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-unwrap-before-after.tsv).

The regression runs the original source through the real Lexer, Parser, Namer,
and `SourceTyper::type_method_definition`, with the authentic Scala `Product`
TASTy and JDK classpath. It types the root in two independent sessions and
checks the same normalized receipt in both. The method signature and typed
Match are `(Expr): Expr` and `Expr`, with the two original ordered cases.
`Num(_)` keeps its wildcard and returns the method's original `expr` parameter.
`Succ(inner)` selects the canonical source-derived `Succ$.unapply`, narrows
only its pattern to `Succ`, creates one case-local `inner` whose canonical
TypeRef targets the original source `Expr` trait, and returns that binder.

The test also checks source-to-typed mappings and unchanged spans for the
method, result type, Match, selector, cases, extractor patterns and selected
qualifiers, wildcard/binder, and both branch expressions. The selector and
joined Match remain `Expr`. Existing class evidence verifies the distinct
source fields, Product1 selectors, generated unapply methods and companion
owners for `Num` and `Succ`.

This result required no additional production typer behavior. `Evaluator.eval`
and its recursion contract remain outside this proof.
