# `Option.map` generic parent metadata observation (#2180)

This note records the classloader boundary needed before the `Option.map`
relation failure can be addressed. It does not change the typer's subtype or
branch-join behavior.

## Reproduction inputs

- Scala source checkout: revision
  `777528f19a58e794c9954a42f433373472ec57f8`.
- Scala binary artifacts: Scala 3.9.0 artifacts from the pinned Maven
  repository used by `SCALA39_CLASSPATH`, including
  `org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar`.
- JDK release: 21.
- dotty-rs base: `0c2e6108bfe9dec05b6850bf198390cfdcd6b30d`.

The ignored `captures_acceptance_iv_relation_operands_before_transaction_rollback`
test parses the original `library/src/scala/Option.scala` `Option.map`
definition and reproduces the current `IfBranchJoinUnsupported` error. Its
left operand is the `None$` package-prefixed reference and its right operand
is `Some[B]`. It also observes the source declarations for `Option` and
`Some`: each resolves to one canonical `Source` symbol, and the original
`Some` parent AST is `Option[A]` with one type argument. The existing failure
is downstream in relation handling; this issue does not change it.

## Original artifact and cause

The focused classloader test opens the original Scala 3.9.0 library JAR,
locates and decodes its `scala/Some.tasty`, then loads both `scala.Some` and
`scala.Option` through the normal classloader. `Some` and `Option` each retain
their one declared type parameter in declaration order. Their parameter
symbols are `Tasty`-origin symbols owned by the corresponding canonical class.
The loaded `Some` `ClassInfo` contains an applied `Option` parent whose sole
argument references that exact `Some.A` symbol. The same class and parameter
IDs remain available after transferring the `LoadingSession` to another
loader.

In the original `Some.tasty`, the parent is represented as a zero-term-argument
`Apply` around `TypeApply(SelectIn(... Option ...), one type argument)`. The
parent projection previously handled the inner type application but did not
unwrap the outer zero-argument term application, so it reduced the parent to
bare `Option` and discarded `A`. `decode_tasty_parent_type` now recursively
unwraps only an `Apply` with no term arguments. Nonempty applications retain
the existing fallback behavior. An out-of-range class-parameter reference
continues to lower as an explicit `Type::Error`.

## Scope boundary

This establishes the metadata needed for follow-up typer work. It does not
make `Option.map` type successfully: after this correction, the original
source replay still reports the same package-symbol `IfBranchJoinUnsupported`
error. The follow-up must address that relation behavior separately (issue
#2181); no broader parent decoder or source identity change is implied here.
