# Scala 3.9 `Function1.compose` conformance

This note records the diagnostic and bounded typer change for
`scytrowski/dotty-rs#1891`. The source oracle is the Scala 3.9.0 checkout at
revision `777528f19a58e794c9954a42f433373472ec57f8`; JDK-backed controls use
JDK 21. The source root is `library/src/scala/Function1.scala#tree=86`.

## Diagnosis

The original whole-method audit failed while typing the *inner* `g(x)` at
source `Apply#tree=81`, argument 0. The argument's `Select` is at source range
`2710..2711`; the Apply's argument tree is 80. The call is the application of
`Function1.apply` to the result of `g(x)`, so the outer `apply(g(x))` had not
been reached when the error occurred. This distinguishes the failure from the
outer application.

The diagnostic was:

```text
ApplicationArgumentConformanceUnsupported {
  source: SourceId(0), tree_index: 81, argument_index: 0,
  actual: TypeId(57), expected: TypeId(42),
  error: UnsupportedType { found: TypeId(57), expected: TypeId(57) }
}
```

At this point `TypeId(57)` is `ParamRef { binder: TypeId(53), index: 0 }`.
The formal `TypeId(42)` is `TypeRef` to canonical symbol `SymbolId(63)`, the
method type parameter `A` owned by `SymbolId(59)` (`compose`). The canonical
method signature is `TypeId(53)`, a one-parameter `Poly` binder. The class
parameters `T1` and `R` are canonical symbols `SymbolId(53)` and
`SymbolId(54)` respectively (symbol and type IDs are separate ID domains).

The selected method is canonical `SymbolId(58)` (`Function1.apply`), owned by
`SymbolId(42)` (`Function1`, a trait). Its method parameter/result are
`TypeId(42)`/`TypeId(44)`. The receiver is canonical parameter
`SymbolId(64)` (`g`), owned by `compose`; its signature is
`Function1[TypeId(42), TypeId(44)]`, i.e. `Function1[A, T1]`. The complete
error chain has one relation leaf: `UnsupportedType { found: TypeId(57),
expected: TypeId(57) }`.

This established a specific identity mismatch: a completed source type
parameter is represented by a `TypeRef` to its canonical symbol in method
signatures, while the active polymorphic method body uses the corresponding
`ParamRef`. The two refer to the same method binder by canonical symbol and
index. No name-based inference or broad type relation relaxation is needed.

## Bounded rule

When checking an application argument, the typer now examines the actual
argument type for `ParamRef`s belonging to currently active local method
binders. For those binders only, it substitutes the corresponding canonical
source type-parameter symbols in the expected type with `ParamRef`s carrying
the same binder and parameter index. It then runs the existing conformance
relation unchanged. Ordinary type references are untouched when the actual
type does not mention an active binder.

The rule is independent of method spelling and `Function1`. It does not widen
to `Any`, reinterpret failed relations as success, or alter unrelated type
IDs. `TypeRebinding` remains an explicit error path. Application and method
typing continue to use their existing transactional rollback behavior.

## Evidence

The focused regression in
`crates/dotty-typer/src/typer/tests/compose_conformance.rs` exercises a direct
generic call and a complete generic closure method. It checks the owned typed
`DefDef`, closure captures (`this` and the canonical `g` parameter), inner and
outer application source-to-typed mappings and their distinct canonical `apply`
owners, inferred lambda input binder, method binder identity, and final `A => R`
result. Negative methods cover a wrong
argument, mismatched function owner/input, wrong function variance, wrong
output, wrong arity, and an unrelated `apply`. A separate empty-classpath case
checks that missing `Function1` produces `SourceFunctionClassNotFound` and rolls
back the failed method. Each failure is repeated and checked for deterministic
errors, semantic and typed-store rollback, absent failed-root mappings,
preserved previous successes, and a valid same-session retry afterward.

The pinned Scala oracle controls in
`crates/dotty-typer/tests/fixtures/function1-compose-conformance/` compile
`g(x): T1`, `apply(g(x)): R`, a full generic `compose[A]`, and a real generic
function value. Negative controls are rejected for wrong input, wrong owner,
wrong variance, wrong arity, and unrelated `apply`. The runner verifies the
source checkout revision, JDK 21, and SHA256 values for its compiler artifacts
before invoking Scala.

The Rust regression also exercises two active, same-spelled method binders:
outer `binderFailure[A](g: A => T1)` contains `inner[A](x: A) = g(x)`. It
asserts that the canonical symbols and owners for the two `A`s differ, then
checks the exact `ApplicationArgumentConformanceUnsupported` at the inner
`g(x)` application: the actual type refers to inner `A`, the expected type to
outer `A`, and the relation error identifies inner `A` as a non-class-like
type parameter. The failed outer method leaves semantic and typed stores and
all relevant source mappings rolled back, repeats with the same error, then
types a valid generic closure in the same session. A literal nested closure
that captures outer `g` currently fails earlier with
`FunctionLiteralCaptureUnsupported`; this is an existing capture boundary, so
the binder identity control uses a nested method body and does not extend
capture support.

The deterministic two-root whole-method audit contains `andThen#tree=111` and
`compose#tree=86`: both type successfully after the change, including the
complete compose method body and its `A => R` result. The exact before/after
root statuses and root provenance are recorded in
`typer-function1-compose-roots-3.9.0.tsv`.

The fresh 25-method baseline accepted 15 roots. The post-change whole-method
audit typed 16 of 25 and preserved every one of the prior accepted 15; the
additional typed root is `compose#tree=86`. The cohort audit was run with
determinism verification and `--require-typed-cohort` using the preserved
pre-change typed-root manifest. This is scoped evidence for the checked-in
cohort, not a full Scala typer corpus result.

Reproduction for the Scala oracle requires the pinned Scala checkout, JDK 21,
and cached artifacts. Set `SCALA39_ROOT`, `JAVA_HOME`,
`SCALA39_JDK_RELEASE=21`, `COURSIER_MAVEN_ROOT`, and a scratch path in
`DOTTY_COMPOSE_ORACLE_TMP_ROOT`, then run the fixture's `run` script. The
classpath corpus audit's whole-method mode is selected with
`DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1`.
