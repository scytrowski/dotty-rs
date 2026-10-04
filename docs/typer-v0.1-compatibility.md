# Typer v0.1 source-to-TASTy semantic gate

This document records the supported boundary for the first source signature
Typer milestone. The parity tests compare structurally normalized semantic
snapshots from the source parser/namer/Typer and the TASTy unpickler. They do
not compare arena IDs, scope IDs, TASTy addresses, source positions, or
allocation order.

## Reproduction and oracle

Run the deterministic parity suite with:

```text
cargo test -p dotty-typer --test semantic_parity
```

The checked-in semantic `.tasty` fixtures are real compiler output from Scala
3.9.0. Their source files are adjacent in
[`crates/dotty-tasty-unpickler/tests/fixtures/semantic`](../crates/dotty-tasty-unpickler/tests/fixtures/semantic),
and `generate.sh` documents how to regenerate them using the Scala 3.9.0
compiler artifacts. The Scala compiler source checkout used by this repository
is pinned to revision
`777528f19a58e794c9954a2f433373472ec57f8` (branch `3.9.0`); the fixture
compiler version is pinned to the exact Maven artifact version `3.9.0`.

The suite currently compares plain and generic classes, traits, class
parameters/accessors, explicit superclass and applied generic parents, explicit
self types, nested classes, aliases, and explicit method signatures. Method
cases cover polymorphic binders and `ParamRef`, empty and multiple clauses,
contextual clauses, overloads, and an extension method. Module classes include
nested modules; the constructor fixture compares a completed primary-
constructor signature. An ID-independence test constructs the same class graph
with different arena and scope allocation, and negative tests prove that parent
and parameter-reference changes are detected.

A mismatch reports the normalized semantic path and the first differing
normalized component from each frontend. Builtin package-prefix normalization
is deliberately limited to named Scala and `java.lang` builtin targets.

## Expression typing layout

`SourceTyper::type_expression_inner` is the expression dispatcher. It handles
typed-tree cache lookups, raw generic-constructor preparation, dispatch, and
source-to-typed index updates. Expression-specific behavior lives under
`crates/dotty-typer/src/typer/expression/`: references and selections,
control flow, assignments, construction, and blocks/local definitions each
have dedicated modules. Block-local symbol and source-tree metadata is owned
by the block module.

Handlers share the same `SourceTyper` state and pass the existing info journal
and pending source-tree mappings through recursive typing. The enclosing
typing transaction remains responsible for publishing those mappings on
success and rolling back semantic mutations and typed-tree cache entries on
failure. Keep cache insertion and rollback behavior in that orchestration
layer when adding expression handlers.

Source `Parens` wrappers are transparent expression nodes: they are typed in
the same `ExpressionContext` as their enclosed tree and map to that exact
typed-tree identity. The Typed AST keeps the enclosed expression's own type
and source position; it has no synthetic parentheses node. The checked
Scala 3.9.0 `-Vprint:typer` fixture in
[`parenthesized-expressions`](../crates/dotty-typer/tests/fixtures/parenthesized-expressions/ParenthesizedExpressions.typed-tree.txt)
pins nested `(x)` as `x` and `(1)` as `1`. Tuple syntax remains a separate
`UntypedNode::Tuple` form.

## Pattern typing foundation

Pattern typing has its own recursive-ready entry point under
`typer/expression/patterns.rs`; it consumes the selector prototype type and
the current expression context, and records source-to-typed identity in
`SourceTypedIndex`. The supported roots are an ordinary unquoted wildcard,
lowercase variable identifiers, literal constants, stable term identifiers and
selections, explicit bindings over wildcard, literal, or stable-value
patterns, plus a bounded subset of typed patterns (`_: T`, `x: T`, and
`x @ (_: T)`), and non-binding alternatives of supported patterns.
Wildcards become typed `Ident(_)` nodes carrying the selector prototype.
Variable identifiers lower to typed `Bind(name, _)` nodes and introduce one
typer-owned local symbol in the case scope. Explicit `name @ _` uses the same
symbol creation path and maps both source Bind and source wildcard nodes.
Typed wildcard patterns project and reify `T` as a typed `TypeTree`; their
typed `Typed` node carries `T`. Typed variable patterns lower to one `Bind`
whose body retains the typed test, and the case-local binder has value info
`T`, so guards and bodies see the narrowed type. An explicit binder around a
typed wildcard uses the same narrowing when the typed pattern is the complete
bound body.
Pattern-bound identifiers in the case body resolve through ordinary local
scope lookup, shadow outer names for that case, and remain isolated from sibling
cases. Backquoted identifiers and uppercase-leading identifiers are resolved
as stable term references rather than classified as variables. Stable patterns
use ordinary expression lookup and preserve the resolved symbol; mutable
values and method references are rejected as unstable. Other pattern roots
remain explicitly deferred.

Variable classification mirrors the parser's source convention: unquoted
names beginning with `_` or a lowercase Unicode character are variable
patterns, except `_`, `true`, `false`, and `null`. This keeps the Typer
independent of parser-private helpers while pinning the same rule with focused
regressions. A variable's symbol info is the selector prototype, and the typed
Bind's own type is a `TermRef` to that exact symbol. An implicit variable root
maps to the typed Bind; the synthetic wildcard child does not get a fabricated
source mapping.

Pattern selector adaptation preserves an exact `ConstantType`. Other selector
types use the existing expression type-widening rules. Literal pattern types
remain constant types and are checked against the selector prototype; when the
selector prototype is itself a constant, the literal must have the same
constant value. Otherwise, the existing bounded conformance relation must
prove compatibility;
unresolved type relations are reported as deferred rather than guessed. This follows the pinned
Scala 3.9.0 typer: [`typedIdent`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Typer.scala#L622-L637)
returns the wildcard tree with the pattern prototype, while
[`typedMatch`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Typer.scala#L2210-L2225)
keeps a constant selector type and widens other selector types before typing
the cases. The fixture in
[`wildcard-patterns`](../crates/dotty-typer/tests/fixtures/wildcard-patterns)
records the source shape and the normalized expected wildcard tree.
The `T` in a typed pattern must currently be a non-generic nominal class or
trait reference (or a builtin with a supported runtime-test representation).
The existing bounded
relation must prove compatibility in at least one direction between `T` and
the selector; proven unrelated types produce a focused mismatch, while
unsupported relations and runtime-test shapes such as generic applications,
aliases, intersections, type parameters, and non-reifiable `TypeRef` prefixes
are deferred. Package prefixes are permitted; path-dependent and applied
prefixes are not. This does not
synthesize `TypeTest`/`ClassTag` evidence or implement GADT refinement.
[`typed-patterns`](../crates/dotty-typer/tests/fixtures/typed-patterns)
records the normalized source and typed shape for the supported subset.
The normalized variable and explicit Bind shapes are pinned in
[`variable-patterns`](../crates/dotty-typer/tests/fixtures/variable-patterns).
Literal and stable-value pattern shapes are pinned in
[`value-patterns`](../crates/dotty-typer/tests/fixtures/value-patterns).
Alternative branches use the same selector prototype and retain source order.
Their widened pattern types are folded through the existing bounded branch
join; unsupported relations produce a focused alternative-join error. Binding
patterns in any branch are rejected before they can enter the case scope. The
literal and stable-value source shapes are pinned against Scala 3.9.0 in
[`pattern-alternatives`](../crates/dotty-typer/tests/fixtures/pattern-alternatives).

Extractor applications with a simple identifier, stable selected-value, or
package-qualified object qualifier resolve through ordinary
expression/stable-selection typing, look up exactly one `unapply`, validate
its plain unary method shape, and check
`selector <: input` with the bounded relation. Boolean-result extractors
accept zero nested patterns and lower to `UnApply` with empty pattern and
implicit lists. For the supported Option-like result protocol, the result must
expose parameterless `isEmpty` with Boolean type and parameterless value `get`;
the `get` type is passed unchanged as the nested pattern prototype. The typed
`UnApply` retains the selector/input prototype as its own type and the exact
selected `unapply` symbol. Nested patterns reuse the existing case-local
binder machinery and may naturally contain another supported unary extractor.
The pinned Scala 3.9.0 oracles and normalized shapes are recorded in
[`unary-option-extractors`](../crates/dotty-typer/tests/fixtures/unary-option-extractors),
[`boolean-extractors`](../crates/dotty-typer/tests/fixtures/boolean-extractors),
[`selected-extractors`](../crates/dotty-typer/tests/fixtures/selected-extractors),
and [`binary-product-extractors`](../crates/dotty-typer/tests/fixtures/binary-product-extractors).

Product extraction accepts N source arguments when it can discover the
consecutive parameterless value selectors `_1` through `_N`. A direct
`unapply` result must conform to `scala.Product`; selectors are discovered
through ordinary inherited member lookup. If the direct result is not a
product or its selectors do not match the source arity, the Option-like path
checks `isEmpty` and reads selectors from the type returned by `get`, without
using selectors declared by the wrapper. When that fallback is unavailable,
the direct selector-arity error is preserved. The typer probes `_N+1` so a
wider product is rejected instead of silently truncated; a methodic or
otherwise invalid next selector is treated as absent.
Each component is passed to recursive pattern typing in source order and
remains in the existing CaseDef-local binding scope. Missing, ambiguous, or
otherwise unsupported required selectors return a focused extractor error.
The normalized pinned fixture records both direct product and `get` result
shapes.

Zero or multiple nested patterns for scalar Option-like extractors, nonzero
nested patterns for Boolean extractors, missing/ambiguous selected qualifier members,
missing/overloaded Option-like protocol members, and non-Boolean `isEmpty`
return focused extractor errors. Products with missing or nonconsecutive
selectors and source argument counts that do not match the discovered product
arity are rejected explicitly. Tuple source patterns lower through the
canonical `scala.TupleN` companion's generic `unapply`, retaining the selected
extractor identity and recursively typing each component. Empty tuple syntax
is the Unit literal pattern; one-element parentheses remain transparent. A
missing canonical tuple class, companion, or supported `unapply` shape is
reported through `TuplePatternResolutionDeferred`, without synthesizing tuple
symbols. The selector must be compatible with the instantiated tuple input;
disjoint types fail, and relations outside the bounded checker are deferred.
Generic or overloaded extractor APIs beyond the canonical tuple
`unapply`, contextual, erased, repeated, or by-name parameters, `unapplySeq`,
and pattern constraint inference remain unsupported. The
foundation resolution shape is recorded in
[`extractor-foundation`](../crates/dotty-typer/tests/fixtures/extractor-foundation).
Infix pattern syntax remains a separate deferred form and returns
`InfixPatternDeferred`; it is counted independently from ordinary extractor
applications in the pinned corpus audit.

The dedicated `type_case_def` helper in `expression/match_expr.rs` types
supported `CaseDef` nodes independently; `Match` expression typing
is supported for matches whose cases use wildcards, variable patterns, typed
patterns in the bounded subset above, literal patterns, stable-value patterns,
the unary Option-like and N-ary product extractors above, tuple patterns, non-binding alternatives, or explicit
bindings over wildcard, literal, stable-value, and typed wildcard patterns. An explicit binder's info is normally the selector prototype; when
its complete nested pattern is a typed wildcard, its info is narrowed to that
pattern type. The nested pattern retains its own type. It types the
selector once, preserves the selector's own type, and computes the shared
pattern prototype by preserving constants and widening other expression
types. It delegates each case to `type_case_def` in source order and builds a
typed Match with its ordered typed cases. The Match result folds widened case
body types through the same bounded branch join used by `if`: equivalent and
subtype-related values retain the supported branch behavior, while unrelated
values use the current `Type::Or` fallback. This intentionally does not
implement Scala 3.9's general `TypeComparer.lub`; unions are not normalized
beyond the existing relation rules. Nested wildcard matches are supported.

For supported patterns, including unary Option-like extractors, guards are typed after pattern bindings are entered
and before the body, in the same case-local scope. They use canonical Boolean
as the expected type and retain their own typed expression type. Guard types do
not participate in Match result joining. Pattern, guard, and body failures
roll back the case and enclosing Match transaction. Unsupported patterns,
including unsupported tuple resolution and extractor protocols, fail
before guard or body typing. Empty Match nodes and failed case typing return
focused errors, and the enclosing expression transaction rolls back all case
and selector state on failure. The normalized
[`wildcard-case-def` fixture](../crates/dotty-typer/tests/fixtures/wildcard-case-def)
records this typed shape. In pinned Scala 3.9.0,
[`typedCase`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Typer.scala#L2383-L2412)
types a pattern with the selector prototype, and
[`assignType(CaseDef, ...)`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L446-L464)
uses the typed body type for ordinary term cases.
The related [`wildcard-match` fixture](../crates/dotty-typer/tests/fixtures/wildcard-match)
records simple, multi-case, and nested Match shapes and calls out the bounded
join divergence.
The [`guarded-patterns` fixture](../crates/dotty-typer/tests/fixtures/guarded-patterns)
records variable, explicit-bind, literal, stable-value, and wildcard guard
shapes.

## Real-source local-definition audit

Run the pinned Scala 3.9.0 source audit with:

```text
SCALA39_ROOT=/path/to/pinned/scala3 \
  cargo test -p dotty-typer --test local_definition_audit \
  pinned_scala39_local_definition_audit -- --ignored --nocapture
```

The report retains local declaration counters and adds a deterministic
`expression_forms` histogram for the required term-expression shapes in named
method bodies. The histogram follows AST term children before naming or typing
and excludes parameter and type trees, so an early import/classpath failure
cannot hide later expression forms. It prints every supported histogram key,
including zero counts.

`parser_diagnostics` counts parser diagnostic kinds separately. Failed local
methods are attributed to the nearest enclosing root-method failure and
reported as stable buckets with a top-level family: resolution/classpath
environment, parser/namer, unsupported expression syntax/semantics, local
declaration deferral, type relation/inference/completion, or other. For
example, unsupported forms use `UnsupportedExpression::InfixOp` and deferred
block declarations retain their subkind, such as
`LocalBlockDeclarationDeferred::import`. Each failure bucket includes up to
five lexicographically ordered representative source paths. Bucket and
histogram rows sort by count descending, then name.

Resolution/classpath failures remain visible in the report and are grouped
separately from unsupported expression forms. A high resolution-failure count
is evidence about the audit environment or missing semantic inputs; it should
not be read as a count of typer features that need implementation. No corpus
counts are recorded here unless the pinned full audit is run for that change.

## Classpath-backed source typing

Run the integration harness with:

```text
cargo test --test classpath_source_typing
```

The harness exercises the public parser/namer/typer pipeline with a
`ClasspathSymbolResolver` in the root facade test target. This keeps the
production dependency direction unchanged: `dotty-typer` depends on
`dotty-core`, while the facade test target can use both the typer and
classloader. It checks an imported external package/class, classfile-backed
member selection and application, canonical package/class/member ownership,
missing and malformed symbols, ambiguity propagation, and rollback followed
by a successful retry. A companion case hands a package registry produced by
the TASTy unpickler to the classpath resolver and checks that an already
entered class keeps its `SymbolId`.

The session lifecycle is:

1. Create one `SemanticStore` and call `Definitions::bootstrap` once.
2. Parse and name source using one `Packages` registry.
3. Move that registry into `LoadingSession::with_packages` and construct the
   classpath resolver with the same store's `Definitions`.
4. Construct `SourceTyper` with the named source and install the resolver with
   `with_resolver`.
5. After typing, hand the resolver's session to the next adapter with
   `into_session`; use `into_packages` only when that adapter accepts
   `Packages` directly.

The generic Java fixture confirms that `GenericSample<T>` and its `first`
member load into the shared store. Typing the member through a source receiver
of type `GenericSample[A]` is currently reported as
`ExternalGenericInstantiationDeferred`, because external generic argument
order is not modeled by the typer. The integration test pins this typed
limitation rather than treating it as successful adaptation.

## Supported source contract

For the tested subset, source Typer completion currently supports:

- declared simple, qualified, and applied type references;
- source declarations entered by the namer, including classes, traits,
  parameters, fields, methods, and type members;
- explicit class parents and explicit self types;
- class parameters with explicit types, including `val` accessors;
- method signatures with explicit parameter types and inferred result types,
  polymorphic binders, multiple term clauses, empty clauses, and `using`
  clauses. Inferred results come from the typed method body after expression
  widening, and the typed body is retained for later consumers. Recursive
  inferred results are rejected explicitly, including self and mutual cycles;
- extension-method signatures whose receiver and declared signature are
  supported by the source Typer;
- aliases and bounds represented by the current semantic type model;
- source union and intersection trees using exactly `|` and `&`, lowered to
  `Type::Or` and `Type::And` while preserving parser grouping. The pinned
  Scala 3.9.0 fixture is
  [`union-intersection-types`](../crates/dotty-typer/tests/fixtures/union-intersection-types).
  Arbitrary infix type operators remain explicitly unsupported;
- wildcard arguments in applied source types, projected as
  `Type::Wildcard { bounds }` with canonical `Nothing`/`Any` defaults;
  wildcard capture, variance checks, and inference through wildcard bounds
  remain deferred. Declaration bounds continue to project as `Type::Bounds`;
- class header completion as `ClassInfo`, including its parent list, declared
  members, prefix, and explicit self type where supplied.
- primary constructor signature normalization for a plain constructor.

Expression typing currently includes typed identifiers, stable term selections,
literal expressions, plain method applications, `New` nodes, and blocks with
expressions, local `val`/`var` definitions with or without explicit types, and
local methods with explicit result types.
Ordinary left-associative term infix calls lower through selected-member
application, including overload selection and generic argument inference. Their
typed shape is a `Select` followed by an `Apply`; the Scala 3.9.0 oracle fixture
is `crates/dotty-typer/tests/fixtures/infix-expressions/InfixExpressions.scala`.
Right-associative infix calls (operators ending in `:`) remain explicitly
deferred because they require Scala's operand rewrite.
It also supports expected-type conformance checks, source type ascriptions, direct
assignments to mutable locals and fields, ordinary `if` expressions over the
supported expression subset, condition-bearing `while` expressions, and local
`return` expressions in methods with an explicit result type.
Expected typing widens the expression type for the existing conformance
relation without changing the child tree's own type; a typed ascription node
carries the projected source type. Assignment requires an exact mutable symbol
reference, checks the right-hand side through expected typing, and produces
`Unit`. Selected field types are adapted to the receiver.

An ordinary source `New` node projects and retains its exact instance type,
including applied type arguments, and reifies its source type tree as a typed
`TypeTree`. Only concrete ordinary classes can be instantiated: traits,
abstract classes, module classes, packages, unresolved/non-class references,
and type parameters are rejected explicitly. Simple aliases and the existing
transparent wrappers are normalized only to discover the class symbol; the
typed `New` keeps the unnormalized projected type. Anonymous-template `new`
is deferred. Constructor discovery reads only the target class's declaration
scope, returns every `<init>` constructor in insertion order, and never walks
parent classes. A primary constructor application preserves its constructor
symbol in the typed selection and reuses ordinary method argument checking,
including nested curried clauses. For a generic primary constructor, explicit
type arguments are read from the typed `New` instance type and instantiate the
constructor's exact `Poly` binder by index. Ordinary lower and upper bounds
use the same relation as explicit method type applications, and the
instantiated final result must agree with the instance class and type
arguments. When class type arguments are omitted, a generic primary
constructor can infer them from widened term argument types. Inference uses the
constructor Poly binder and parameter indices, supports matching applied type
constructors recursively, and combines constraints from curried clauses.
Repeated equivalent constraints succeed; conflicts and unconstrained
parameters are errors, without a `Nothing`/`Any` fallback or LUB. The inferred
arguments pass through the same bounds checker and Poly instantiator as other
generic calls. The typed `New` is finalized only after inference succeeds, so
it and the final constructor application retain the same applied instance
type. No synthetic source `TypeApply` is introduced. Primary and secondary
constructors participate in overload resolution. This includes competition
between an inferable generic primary constructor and monomorphic secondary
constructors. With explicit owner type arguments, the instantiated signature
of a generic primary constructor competes with monomorphic secondary
constructors. Candidates are filtered by arity and argument conformance, then
the unique most-specific applicable signature is selected. Ambiguity and
no-applicable errors retain candidate identities and rejection reasons.
Inference for raw generic `new` is supported when a generic primary constructor
wins. If a raw generic `new` selects a secondary constructor, typing is
explicitly rejected because its signature has not been adapted using inferred
owner type arguments. Expected-type inference, automatic contextual constructor
argument insertion, and implicit search remain explicit deferrals; explicit
constructor `using` clauses are checked and can contribute inference constraints.

A secondary constructor in a generic class keeps the enclosing class's type
parameters in its result, for example `C[A]`; those parameters remain owned by
the class and are not rebound as constructor parameters. Scala 3.9.0's pinned
[syntax grammar](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/docs/_docs/reference/syntax.md#L442-L443)
does not allow a secondary constructor to declare its own type-parameter
clause, so such recovered source is reported as unsupported instead of being
typed as a generic constructor.

An `if` condition is checked against canonical `Boolean`. Both branches are
typed in the same lexical context and retain their own types in the typed AST.
The join widens branch value types, chooses an equivalent/supertype branch
when the supported nominal relation can prove it, and otherwise produces
`Type::Or`. Union subtyping implements only the two recursive rules required by
that join and is bounded by depth and comparison limits. Intersections and
other advanced relations remain explicit unsupported cases. Scala 3.9.0's
`TypeAssigner.assignType(If, ...)` combines the typed branch types with `|`
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L428-L429));
the focused tests pin this subset's literal widening, nominal supertype, and
unrelated-class union behavior. An omitted source `else` uses the parser's
synthetic Unit tree and follows the same branch typing and join path.

A `while` condition is checked against canonical `Boolean`; its body is typed
as an ordinary expression in the same lexical context, so a block creates its
scope through the existing block path. The body's value is discarded without
a Unit conformance check, and the typed loop has canonical `Unit` type. This
result type matches Scala 3.9.0's `TypeAssigner.assignType(WhileDo)`
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L476-L477)).
The parser currently represents ordinary loops with a condition; conditionless
loops and `Nothing` results are not part of this source typing contract.

A local `return` follows the semantic owner chain to its enclosing source
method and projects that method's explicit result type through normal method
completion. Its expression is checked against that type, and the typed Return
node has canonical `Nothing` type, matching Scala 3.9.0's
`TypeAssigner.assignType(Return)`
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L473-L474)).
For a bare source `return`, the parser preserves an absent operand and Scala
3.9.0's `typedReturn` supplies a synthetic Unit literal before expected-type
checking
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/Typer.scala#L2445-L2473)); the source typer mirrors that behavior. Returns in methods with inferred results and valid explicit `Return.from` targets remain deferred; malformed targets return a focused error. Constructors are outside this contract. Since `Nothing` conforms as the bottom type, a return in one `if` branch leaves the other branch's ordinary result type intact. If the existing bounded conformance relation cannot decide whether the return expression matches the declared result, typing reports a focused unsupported-relation error.

An inferred local
uses the already typed RHS and `widen_expression_type`; it does not repeat
identifier, member, or application lookup. The pinned Scala 3.9.0 source, TASTy,
and normalized typed-tree oracle are in
[`local-value-inference`](../crates/dotty-typer/tests/fixtures/local-value-inference).
They show `val n = 1` as `val n: Int = 1`, while the RHS remains a literal.
Inferred declarations publish the widened type and `var` retains its mutable
flag. The synthetic missing annotation's source `TypeTree` maps to the typed
`TypeTree` that carries the inferred type.
Blocks type their statements in order, preserve the typed Block shape and
source position, and use the final expression's own type without widening it.
Each block allocates an empty scope in `SemanticStore` owned by the current
semantic expression owner; nested blocks push nested scopes. A block-local
import is retained as a typed `Import` statement, then becomes active for later
statements. Nested blocks inherit active outer imports, but imports declared in
them do not leak outward. Local import state is typer-owned and rolls back with
failed expression transactions. Explicit, wildcard, renamed, and hidden
selectors reuse source-import resolution for term and type names. Before typing
statements, each block pre-enters its direct local method headers in source
order. A local method receives a typer-owned `Method` symbol and a distinct
method-owned scope; the local method index retains its source tree, scope, and
enclosing block expression context without modifying `SourceSemanticIndex`.
This makes forward references and same-name overload buckets visible during
name lookup. Local methods with explicit result types complete generic,
curried, ordinary, implicit, and contextual parameter clauses through the
shared method-signature builder. Type parameters and term parameters receive
typer-owned symbols in the method's distinct scope, with source-tree identity
and the declaration site's lexical type context retained without changing
`SourceSemanticIndex`. Forward calls can complete and use these signatures on
demand; overload selection uses the shared application resolver. The typed
`DefDef` retains its type parameters and each term clause. Local methods can
infer plain, generic, and curried result types from their typed bodies. Generic
results are rebound to the final `Poly` binder, and the synthetic source result
`TypeTree` maps to a typed node carrying that result. Forward calls can trigger
inference before the definition statement. Self and mutual inferred-result
cycles fail deterministically and roll back completion state; an explicit
result type can break a mixed recursive cycle, as confirmed against the pinned
Scala 3.9.0 compiler by
[`explicit-breaks-inference-cycle.scala`](../crates/dotty-typer/tests/fixtures/local-method-results/explicit-breaks-inference-cycle.scala).
Parameter-dependent result
types, erased, by-name, or repeated parameters, higher-kinded and aliased
type-parameter bounds, and unsupported parameter modifiers remain explicitly
deferred. A method body is typed in a method
context that composes the declaration-site lexical context with the
method-owned parameter scope. Its RHS is checked against the explicit result
type, and the typed `DefDef`, typed type and value parameter definitions,
result `TypeTree`, and RHS are retained with source mappings. The definition
tree type is a `TermRef` to the exact local `Method` symbol. Explicit
recursion, forward calls, and reads of enclosing locals are supported. Nested
method headers are indexed only by their own block. The block scope, local
method and parameter symbols/scopes and metadata, typed nodes, local symbol
mappings, and source mappings are rolled back when any part of the enclosing
block fails. Failed signature completion also leaves the local method's
pre-indexed scope and missing info intact. A local `val` or
`var` with a source-written type
shadows outer bindings throughout the statement sequence, including its own
initializer; a reference to the local while it is being initialized reports a
recursive initializer error. Explicitly typed initializers are checked against
their declared types. Inferred initializers are typed before the new local is
entered, so they resolve an outer binding of the same name or report ordinary
name lookup failure. Unsupported methodic, bounds, incomplete, and other
non-value inferred infos return a focused error. Both typed explicit and
inferred locals are entered with one stable symbol identity; a later failure in
the block rolls back local symbols, source mappings, and typed nodes.

### Local-definition source audit v1 (historical)

The first-generation audit scanned the pinned Scala 3.9.0 `library/src` and
`compiler/src` trees and tried source typing for named method bodies that
contain local definitions, without a real classpath resolver. Its command was:

```text
SCALA39_ROOT=/tmp/scala3-3.9.0 \
  cargo test -p dotty-typer --test local_definition_audit --locked \
  -- --ignored --nocapture
```

The corpus contains 1,236 Scala files at source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The audit identifies local
declarations from block statement lists, so members of a local class are not
misclassified as enclosing-method locals. At this typer revision, it found
23,218 local declaration nodes: 19,020 local values, 3,778 local methods, 36
local type definitions, 74 local classes, 58 local objects, 228 local imports,
and 24 pattern bindings found by traversing local pattern definitions. No local
method body typed successfully in this source-corpus run (0/3,778). This
conservative corpus number measures methods for which an
enclosing named method body can be typed as a whole without the classpath/session
loader; it does not contradict the focused local-method regressions, which type
supported examples against their complete in-memory source context.

The five largest local-method failure categories were `ImportQualifierNotFound`
(2,046; `BCodeBodyBuilder.scala`, `BCodeHelpers.scala`, `BCodeSkelBuilder.scala`,
`BCodeUtils.scala`, `BTypeLoader.scala`), `UnsupportedExpression` (572;
`BCodeBodyBuilder.scala`, `BCodeSkelBuilder.scala`, `BCodeSyncAndTry.scala`,
`BTypes.scala`, `GenBCode.scala`), `NoSuccessfulEnclosingMethodTyping` (483;
`BackendUtils.scala`, `GenericSignatureVisitor.scala`, `BoxUnbox.scala`,
`Desugar.scala`, `TreeInfo.scala`), `LocalBlockDeclarationDeferred` (272;
`ScalaPrimitives.scala`, `BCodeBodyBuilder.scala`, `BCodeHelpers.scala`,
`BackendUtils.scala`, `ClosureOptimizer.scala`), and
`NamerError::InvalidVisibilityQualifier` (127; `TypeComparer.scala`,
`ProtoTypes.scala`). The audit reports stable sorted buckets and representative
paths. Its fixture tests repeated-run determinism and excludes local-class
members from the enclosing method's local-definition counts.

This result is retained for historical comparison. The classpath-backed
second-generation run below supersedes its missing-classpath limitation and
is the current audit reference.

### Classpath-backed Scala 3.9.0 source audit

The second-generation audit uses the same pinned `library/src` and
`compiler/src` roots and runs the real `ClasspathSymbolResolver` through the
integration path validated by #580. Each Scala source file is one isolated
audit unit with one `SemanticStore`, one `Definitions`, source packages seeded
into one `LoadingSession`, and (when the file has target methods) one
`SourceTyper`; the resolver session is shared throughout that unit. A single
read-only classpath index is shared by the units. Per-file read, parser, namer,
and typing failures are captured without stopping the corpus run. Per-file
stores prevent unrelated source declarations from affecting each other's
IDs. Audit-only instrumentation counts resolver requests and outcomes without
changing production APIs. Session cache reuse across files is not counted
because stores are intentionally independent.

Reproduce the two-run normalized determinism check and regenerate the checked
report with:

```sh
SCALA39_ROOT=/path/to/scala3-3.9.0 \
JAVA_HOME=/path/to/jdk-21 \
SCALA39_JDK_RELEASE=21 \
tools/typer-classpath-corpus-audit/run
```

The script rejects a Scala checkout whose `HEAD` is not
`777528f19a58e794c9954a42f433373472ec57f8`. It constructs
`SCALA39_CLASSPATH` itself from these pinned Maven artifacts under
`COURSIER_MAVEN_ROOT` (default
`$HOME/.cache/coursier/v1/https/repo1.maven.org/maven2`):

- `org.scala-lang:scala3-library_3:3.9.0`;
- `org.scala-lang:scala-library:3.9.0`;
- `org.scala-lang:scala3-compiler_3:3.9.0`;
- `org.scala-lang:tasty-core_3:3.9.0`;
- `org.scala-lang:scala3-interfaces:3.9.0`;
- `org.scala-lang.modules:scala-asm:9.9.0-scala-1`;
- `org.scala-sbt:compiler-interface:1.12.0`;
- `org.scala-sbt:util-interface:1.11.5`.

The explicit `JAVA_HOME/jmods` plus those jars are the complete classpath
inputs; ambient `CLASSPATH` is ignored. The corpus checkout's `out/` is empty,
so the pinned published library/compiler jars provide compiled Scala symbols.
The script runs the ignored corpus test twice, extracts only its normalized
report section, compares the sections byte-for-byte, and then writes
[`typer-classpath-corpus-audit-3.9.0.md`](typer-classpath-corpus-audit-3.9.0.md).
In the recorded environment (JDK 21), one run took about five seconds; peak
memory was not measured. The normal workspace test suite skips this audit.

The report keeps v1 counts and shows deltas, exact
`UnsupportedExpression::<AST kind>` and
`LocalBlockDeclarationDeferred::<kind>` buckets, method-body structural AST
counts, parser/namer/recovery separation, up to five deterministic representative
paths per bucket, and a top-ten list of Typer-owned first blockers with
per-bucket scope notes for the top five. `NoSuccessfulEnclosingMethodTyping`
is reported as a downstream count and excluded from semantic-gap ranking.
Classpath-resolution buckets are also reported separately and excluded from
the Typer-semantic ranking so missing external symbols cannot masquerade as
Typer features.

The #581 baseline resolved 1,965 packages from the classpath, reused 5,025
source packages, and materialized no external classes or members (3,884
unresolved member requests and 91 resolver errors). The #610 rerun resolves
2,530 external packages and reuses 6,406 source packages. It materializes one
external class, but no non-class members: the resolver records one successful
member request because that result is the class symbol itself. It records
5,070 unresolved member requests, 116 errors, 25 distinct packages, one class,
and zero members. This does not meet #610's real external class/member
materialization gate.

The previous `java/lang/Object` / `java/lang/Class` circular-load failures are
gone. Remaining resolver errors are dominated by JDK classfile signatures
whose generic references cannot be resolved (`K` in
`java/lang/invoke/ClassSpecializer$SpeciesData`), which cascade through
`Object` and reflection types (80 errors), plus 34 published Scala TASTy files
whose supertype references cannot be resolved to names. Thus the measured
classloader limitation is now unresolved JDK generic-signature variables and
Scala library TASTy supertypes. The updated audit records this as a blocker;
it does not claim that the #605/#606 resolver integration gate passed.

For the #610 feature comparison against #581, first-blocker counts moved from
146 to 0 for `UnsupportedExpression::Parens`, 146 to 0 for
`UnsupportedExpression::InfixOp`, and 139 to 0 for
`LocalBlockDeclarationDeferred::import`. The complete rankings and the next
first errors are recorded in the linked report. These are reachability and
first-error movements, not by themselves proof of semantic completion. Two
`RightAssociativeInfixDeferred` sites remain; ordinary left-associative infix
blockers are gone. The latest pinned audit after #682 records 7,437 structural
Match nodes and 21,736 cases (2,111 guarded). Its root histogram includes
4,648 wildcards, 1,145 variable identifiers, 1,735 stable identifiers, 623
stable selections, 1,365 literals, 5,384 typed variables, 380 typed
wildcards, 443 alternatives, 479 tuples, 4,282 extractor-looking Apply
roots, and 468 infix roots. These are source-corpus counts, not counts of
expressions that the Typer supports.

The expanded isolated case probes type 212 wildcard and 30 variable/bind
cases. No other case family or extractor protocol succeeds under this
classpath and enclosing-method context; the audit still materializes zero
external class/member symbols. This is an environment/coverage limit, not
evidence against the focused extractor, alternative, and tuple tests. The
probes record first error variants, tuple-resolution subcategories,
successful protocols, and representative files. Infix patterns are counted
as explicitly deferred. The report keeps method-level Match blockers separate
from isolated-case failures and the global semantic-gap ranking. See the
[normalized audit report](typer-classpath-corpus-audit-3.9.0.md) for current
counts and the next-increment recommendation.

The previous wildcard-only report recorded 1,666 Match first-blocker methods,
2,151 Match nodes, and 6,615 cases. It stopped at `Match`, so downstream
pattern counters below were not reached in that snapshot. The current
collector includes the typed-pattern relation/runtime-test errors added by
later increments; its 284 first-blocker methods are therefore a broader error
inventory, not a strictly like-for-like total.

| First blocker | Previous report | Current report | Movement |
| --- | ---: | ---: | --- |
| `UnsupportedExpression::Match` | 191 | 0 | -191 |
| Unsupported variable identifier pattern | 0 observed | 0 | remains absent |
| Binding-body unsupported pattern | 0 observed | 6 | now exposed downstream |
| Literal pattern type mismatch/relation | 0 observed | 0 | none observed |
| Stable-identifier pattern relation deferred | 0 observed | 9 | now exposed downstream |
| Guard-specific deferred/error bucket | 0 observed | 0 | none observed |
| Typed-pattern runtime-test deferral | 0 observed | 155 method blockers; 9 local-def failures | now exposed downstream |
| Typed-pattern relation deferral | 0 observed | 31 method blockers | now exposed downstream |
| Typed-pattern mismatch | 0 observed | 0 | none observed |
| Match-case result widening/join errors | 0 observed | 0 | none observed |

The current values count first errors, not every occurrence in every method;
the isolated case probes separately find 450 runtime-test, 43 relation, and
258 type-tree projection deferrals. `UnsupportedExpression::Match` and
unsupported variable identifier patterns have ceased to appear as first
blockers; the remaining errors point to concrete pattern families and
selector adaptation.

### Extractor-pattern support and remaining audit scope

The normalized extractor profile contains 4,282 extractor-looking root
patterns. It sees 5,970 extractor roots: 5,534 simple identifiers and 436
selected extractors. Argument arities remain concentrated at one (2,409) and
two (2,823), followed by three (467); the profile also sees arities through
22. It found no type-applied extractor, sequence wildcard, or named pattern
argument. There are 468 infix roots and 780 infix forms including nested
ones. Representative files include
`compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala`,
`compiler/src/dotty/tools/dotc/ast/Trees.scala`, and
`compiler/src/dotty/tools/dotc/cc/SepCheck.scala`.

Scala 3.9.0 source inspection and `-Vprint:typer` on a small pinned compiler
fixture show that a source `Apply` extractor pattern is not typed as an
ordinary method call. [`Applications.typedUnApply`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Applications.scala#L1718-L1975)
resolves the extractor object/member, invokes
`unapply` first and falls back to `unapplySeq`, applies the selector as the
extractor input, and constructs a typed `UnApply` whose children are the typed
argument patterns. Explicit type arguments are handled on the extractor
function. Overloaded resolution, aliases, and trailing type parameters need
their own constraints; Scala rejects `unapply` signatures with type parameters
after the last explicit term clause because `UnApply` cannot encode them.

The typer now supports Boolean, unary Option-like, and N-ary direct/product-
inside-`get` extractors, plus tuple syntax lowered through canonical
`scala.TupleN.unapply`. Components are recursively typed in source order, so
alternatives inside product components use the existing branch checks. The
classpath-backed corpus probes report no successful extractor protocols
because external members still do not materialize; focused tests and the
pinned tuple fixture cover the supported semantics independently. Remain
deferred: `unapplySeq`, arbitrary generic or overloaded extractors, explicit
extractor type applications, contextual clauses, named pattern arguments,
and general pattern-constraint inference. Structural counts cannot establish
the prevalence of implicit/contextual extractor parameters.

Pinned Scala 3.9.0 `Applications.typedUnApply` checks the selector against the
extractor's first parameter. If the selector conforms, that selector type is
the `UnApply` type; otherwise Scala performs pattern-type constraint
inference and types nested arguments from the extractor result. For
`unapply`, Scala handles Boolean results (zero arguments), `Option`-like
results through `get`, and product results through component selectors.
`unapplySeq` additionally expands sequence/product tails.
[`TypeAssigner.assignType(UnApply, ...)`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L543-L544)
gives `UnApply` its prototype type. `typedUnApply` supplies the selector type when it
conforms to the extractor input, and the extractor input type when Scala must
constrain the pattern; neither case uses the extractor's `Option` or product
return type as `Typed UnApply.ty`. The existing case-scope transaction,
explicit binder entry, and recursive pattern typing can be reused for each
extracted component; component types should be passed as child prototypes and
all bindings should remain inside the current case scope.

The audit counts 443 alternative roots and 479 tuple roots. Alternatives
remain supported when branches introduce no bindings; incompatible branch
joins are explicit errors. Tuple patterns use the same `UnApply` and
product-selector lowering as extractors. The pinned fixture confirms that
`case (left, right)` becomes `Tuple2.unapply[Any, Any]`, nested tuple
components lower recursively, `(single)` remains one binder, and `()` is the
Unit literal pattern. Of the 479 tuple-shaped source patterns in the corpus,
8 are empty `()` patterns and lower as Unit literals.

The post-sprint typed-pattern inventory has 155 first-blocker runtime-test
deferrals, 31 unsupported type-relation deferrals, and 258 type-tree projection
deferrals in the isolated corpus probes (the first two categories also appear
as 450 and 43 probe failures across all candidate cases). The runtime-test
error currently groups generic/erased and other unsupported runtime shapes;
it does not expose separate counts for those subtypes. No GADT-refinement or
`TypeTest`/`ClassTag` synthesis error is reported because neither operation is
implemented as a distinct typer path yet. The evidence does not make that work
urgent: retain these boundaries while the extractor and type-projection
increments are developed, and split the runtime categories only when a
focused implementation requires that distinction.

The ranked list remains a deterministic inventory of Typer-owned first errors
observed after the resolver calls that succeeded; unresolved external symbols
remain separate from Typer semantic gaps, and later failures in those bodies
are not known.

Application sites can resolve lexical, imported, and selected overload buckets
for supported monomorphic methods and `Poly -> Method` candidates. Generic
candidates infer type arguments from the already typed and widened arguments
using the same direct `ParamRef` and nested `Applied` rules as a single generic
callee. Inferred arguments are checked against ordinary bounds before generic
and monomorphic candidates enter the same applicability and specificity
selection. Only a unique method whose instantiated formal types are strictly
more specific than every other applicable candidate is selected. Selected
members use receiver-adapted signatures, including inherited generic methods.
Arguments are typed once per overload set. Candidate-local inference conflicts,
argument mismatches, and bound violations reject that candidate; unsupported
inference or conformance shapes remain conservative when they could compete.
Standalone overloaded identifiers and selections remain deferred.

Application syntax is checked against the current method clause: regular
applications consume plain clauses, while explicit `using` applications consume
contextual or legacy implicit clauses. Explicit arguments use the ordinary
typing, widening, arity, and conformance checks, and the typed `Apply` retains
its `ApplyKind`. Overload applicability applies the same clause-kind filter, so
a mismatched candidate cannot win solely because its argument types match.
Generic inference uses the current explicit clause, including contextual or
legacy implicit clauses passed with `using`, and the supported direct-parameter
and matching applied-type constraints also participate in overload selection.
Curried plain-then-contextual calls continue from the exact result callable of
the first clause, including after generic inference. Automatic contextual or
implicit argument insertion and implicit search remain deferred. Erased and
by-name parameters remain unsupported for explicit application. The Scala
grammar rejects repeated parameters in contextual clauses; ordinary repeated
parameter application remains deferred.

`SourceTyper::expression_context_for` builds a method or constructor body
context from its indexed source declaration context and owned scope. Term
lookup checks typer-local scopes from innermost to outermost before source
scopes and their import rules, preserving each matching overload bucket. The
method scope is reused as indexed, so parameters are not entered a second time.
Constructor contexts expose constructor-owned parameters for lookup, but
constructor-body typing is not otherwise implemented. `push_local_scope` adds
empty block scopes without changing the source-context graph.

Explicit positional type applications are supported for one resolved `Poly`
callee. Type arguments use the expression's lexical context, require exact
arity, and are checked against ordinary lower and upper bounds before the Poly
binder is instantiated. Typed `TypeApply` nodes retain the exact function
reference and carry projected type arguments as typed `TypeTree` nodes.
Overloaded type applications, unsupported bounds, and unsupported source type
argument forms return typed deferrals/errors.

Plain applications of one resolved `Poly` whose result is one immediate plain
`Method` infer type arguments from the method's term arguments. Inference is
binder/index based and supports direct `ParamRef` formals and matching nested
`Applied` type constructors. Repeated equivalent constraints are accepted;
conflicts do not compute a least upper bound. Concrete formal fragments still
require conformance, and inferred arguments are checked against instantiated
ordinary bounds. Unconstrained parameters and unsupported shapes return typed
errors. Expected-result inference, variance solving, inherited-constructor
matching, inference through unions/intersections, relation solving for those
types, wildcard capture, match-type reduction, type-lambda unification,
implicit search, and numeric weak conformance remain deferred.

Source and TASTy snapshots intentionally normalize adapter-specific identity
and provenance. TASTy's package prefixes on the built-in Scala and
`java.lang.Object` references normalize to the source Typer's canonical
no-prefix form. The TASTy class scope also contains a synthetic `writeReplace`
method for serializable compiler output; that exact synthetic method is omitted
from class member equality. These are named, narrow normalizations; other
members and type components remain part of equality.

## Deferred work

The parity gate does not claim support for arbitrary Scala 3.9 source. These
parts remain deferred until their source semantic implementation and fixtures
are ready:

- expression forms outside the supported identifier, selection, literal,
  application, `New`, block, ordinary `if`, condition-bearing `while`, and local
  `return` subset, including `match`, `try`, lambdas, and other forms that are
  not currently handled by the expression typer;
- other local `def` signature/body shapes, class, type, and pattern
  declarations;
- generic overload inference outside the structural candidate-local subset,
  contextual/implicit argument insertion and search, dependent result
  application, and right-associative extension normalization;
- expected-type-driven constructor inference, selecting constructor overloads, and
  anonymous-class lowering;
- enum semantics, case-class synthetic APIs, and `derives`;
- context-bound evidence synthesis;
- default imports and general standard-library member lookup;
- annotations not represented by the current source typer;
- advanced parent feasibility and compiler-generated wrapper members.

Scala 3.9 TASTy records inferred/default module self references that source
`ClassInfo` completion does not yet synthesize. For module classes only, the
normalizer treats a missing self type and the matching compiler-generated
module self reference as the same default. Explicit non-default self types are
still compared structurally. Do not treat this gate as evidence of general
expression typing or full Scala source compatibility.
