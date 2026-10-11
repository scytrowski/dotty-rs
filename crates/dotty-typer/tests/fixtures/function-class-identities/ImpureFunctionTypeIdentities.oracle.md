# Impure function type identity oracle

This reference pins Scala 3.9.0 at compiler revision
`777528f19a58e794c9954a42f433373472ec57f8`. It records type identity and
representation from the pinned compiler source. The Typer audit profile is in
[`typer-classpath-corpus-audit-3.9.0.md`](../../../../../docs/typer-classpath-corpus-audit-3.9.0.md).

## Arrow selection under capture checking

The pinned parser's `Feature.pureFunsEnabled` path marks `=>` with `Impure`
before it distinguishes ordinary from contextual arrows. The parser adds
`Given` for `?=>`. Pure arrows do not receive `Impure`; capture sets are kept
in `CapturesAndResult` metadata.

| Source spelling | Parser shape when capture checking is enabled | Semantic identity |
| --- | --- | --- |
| `A => B` | `FunctionWithMods [Impure]` | `scala.ImpureFunction1[A, B]` |
| `A ?=> B` | `FunctionWithMods [Given, Impure]` | `scala.ImpureContextFunction1[A, B]` |
| `A -> B` | `Function` (with capture/result metadata as applicable) | `scala.Function1[A, B]` |
| `A ?-> B` | `FunctionWithMods [Given]` (with capture/result metadata as applicable) | `scala.ContextFunction1[A, B]` |

Without capture checking / `pureFunsEnabled`, `=>` and `?=>` select the
ordinary pure `FunctionN` and contextual pure `ContextFunctionN` families.
Thus the spelling `A => B` alone does not imply `ImpureFunctionN`; it is the
active capture-checking language feature that changes the parser metadata.

## Scala 3.9 semantic declarations

`Definitions.FunctionSymbol(arity, isContextual, isImpure)` selects four
families as the product of contextuality and impurity. `FunctionN` and
`ContextFunctionN` are class symbols. `ImpureFunctionN` and
`ImpureContextFunctionN` are not classes: `newFunctionNType` synthesizes
permanent `TypeAlias` symbols in package `scala` on demand. The alias name
prefix is removed to find its target family (`FunctionN` or
`ContextFunctionN`).

For arity *N*, the alias has *N* contravariant input parameters followed by a
covariant result parameter; every parameter has `TypeBounds.empty`. Its exact
semantic shape is:

```text
SymbolKind::TypeAlias
info = AliasingBounds(
  TypeLambda([-T1, ..., -TN, +R],
    Annotated(
      Applied(TypeRef(scala.FunctionN or scala.ContextFunctionN), [T1, ..., TN, R]),
      RetainsCap
    )
  )
)
```

The annotation is Dotty's `RetainingAnnotation(defn.RetainsCapAnnot)` (printed
in the type syntax as `^{any}`). The pure function class has no such impure
alias wrapper. `scala`'s function-name synthesizer creates the alias when
looked up; the alias is not a library class-file artifact. The target
`FunctionN` class is an ordinary function class; `ContextFunctionN` is also
synthesized by the Scala package for the pinned compiler, rather than loaded
from a Scala 3.9 class file. Function class symbols are synthesized at each
nonnegative arity; the implementation-relevant class arities are 0 through
22, with higher-arity runtime erasure using `scala.runtime.FunctionXXL`.
The 13 current audit observations all have arity 1.

Before alias expansion, a semantic type can retain an applied reference to the
`Impure*FunctionN` alias symbol. After expansion, it is an annotated applied
reference to the pure underlying function class. TASTy type references
distinguish the `Impure*FunctionN` alias name from a pure class reference;
when opened, the compiler's Scala package synthesizer reconstructs the alias
info. The alias is not a library class-file artifact. This is distinct from
representing an impure alias as a class symbol.

## Current audit evidence

The pinned corpus audit finds 13 first-blocker observations but only 3
distinct `FunctionWithMods` source trees, across 2 files. They are at
`StringParsers.scala:185` (`Char => Boolean`) and
`JavaCollectionWrappers.scala:399,510` (`Option[V] => Option[V]`). All three
trees parse as one-argument `FunctionWithMods [Impure]`, with
`erased_params=[false]`; none has `Given`, capture-set syntax, a pure arrow,
annotations, or a visibility modifier. The first tree is observed from 11
local methods: its `forAllBetween` declaration is the one direct observation
and ten other local methods inherit the blocker. Each wrapper tree is observed
from a local `remap` method, while its direct source owner is the enclosing
member `updateWith`; both are sibling/inherited observations. In total, 1
observation is direct and 12 are sibling/inherited. The outer first-failure
methods are `checkFloatFormat` for the first tree and `updateWith` for the two
wrappers.
This confirms that the current 13 / 2 bucket is the capture-checking rewrite
of otherwise ordinary `=>` function types, not a mixture of erased or pure
function syntax.

## dotty-rs representation and reuse boundary

The existing core model directly supports the canonical alias shape:

```text
SymbolKind::TypeAlias
SymbolInfo::Complete(Type::AliasingBounds(Type::TypeLambda(...)))
  -> Type::Annotated(Type::Applied(TypeRef(FunctionN or ContextFunctionN), ...), RetainsCap)
```

No new core type variant is needed. The type lambda must retain its binder and
`ParamRef` identities; its arguments must not be substituted by position or
type name outside that binder.

Keep `SourceFunctionKind::{Ordinary, Contextual}` as the contextuality
dimension for class identities. Model purity as an orthogonal dimension for
the source function family, and use a separate alias-identity lookup for the
impure cases. The existing `source_function_class` validates that a lookup
returns a class or trait and therefore must remain the pure-class path.
`source_function_type_constructor` returns a `TypeRef` to those class symbols;
`project_function_type` projects only ordinary and `[Given]` non-erased trees.
Its source type cache and transaction journal should be reused for either
future path, while the impure path records a reference to the alias symbol
instead of synthesizing a class. `ContextFunctionN` materialization remains
the pure contextual class case and must not be reused to fake an impure alias.

## Generic alias normalization prerequisites

`TypeNormalizer::dealias_top_with_budget` returns
`TypeNormalizeError::AliasRequiresSubstitution` when an `Applied` type has a
generic alias as its constructor. No other applied-alias instantiation path
currently exists. `dotty-core` has reusable type graph rebinding and symbol
substitution; `TypeLambda` construction abstracts symbol references into
`ParamRef`s, while `instantiate_poly` handles the related but distinct `Poly`
case. There is no type-lambda application helper yet.

The smallest safe addition for later work is a bounded helper that validates
the applied alias's type-lambda arity, substitutes its binder parameters with
the supplied type arguments using exact binder identity, and returns the
instantiated body. It should reuse the rebinder's memoized graph walk, preserve
annotations and nested binder identities, and use a semantic-store checkpoint
so every failure rolls back allocations. It should return typed errors for
arity mismatch, malformed or unfilled references, cycles, and depth overflow.
Alias-chain cycle detection and the existing normalization fuel bound should
remain in `TypeNormalizer`; the helper should only instantiate one lambda.

## Shapes outside this audit slice

The typer currently rejects every `erased_params=true` vector before alias
selection. Pure arrows with capture sets are carried separately through
`CapturesAndResult`; they are not impure aliases. The projector rejects
function-type visibility, annotations, and modifier lists other than exactly
`[Given]`; unsupported combinations stay typed deferrals. The current 13 rows
contain none of those shapes. This issue records these boundaries and does not
project impure aliases, normalize generic aliases, or type erased/capture-set
function types.

The parser regression tests for ordinary/contextual arrows, pure arrows,
capture-set preservation, named and nested parameters, erased parameters, and
modifier ordering include `capture_checking_marks_the_ordinary_function_arrow_as_impure`,
`capture_checked_context_function_arrows_keep_distinct_modifiers`,
`pure_function_type_keeps_its_capture_set_on_the_function_tree`,
`pure_context_function_type_keeps_context_metadata_and_capture_set`,
`parses_an_erased_named_function_parameter_when_enabled`, and
`nested_named_function_types_are_parsed_recursively`. The profiler regression
checks named and unnamed parameter source shapes; the pinned audit asserts that
13 observations correspond to exactly 3 source trees.

## Pinned source references

- `compiler/src/dotty/tools/dotc/parsing/Parsers.scala`,
  `functionRest` and `Feature.pureFunsEnabled` arrow handling.
- `compiler/src/dotty/tools/dotc/typer/Typer.scala`,
  `typedFunctionType` selection of contextual and impure identities.
- `compiler/src/dotty/tools/dotc/core/Definitions.scala`, `newFunctionNType`,
  `FunType`, `FunctionSymbol`, and `ScalaPackageClass`'s function synthesizer.
- `compiler/src/dotty/tools/dotc/core/NameOps.scala`, synthetic function-name
  parsing and arity.
