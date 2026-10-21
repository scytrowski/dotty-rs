Warning: truncated output (original token count: 28860)
Total output lines: 1798

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

## Singleton source types

Source singleton type trees (`x.type`, stable selections, and `this.type`) are
projected to the exact stable `TermRef` or `ThisType` represented by their
reference path. Stable identifier and selection resolution preserves the
resolved symbol and selection prefix. Mutable values, methods, overloaded
selections, and references outside identifier, selection, parenthesized, and
`this` paths remain errors; arbitrary expressions are not typed as a side
effect of projecting a type. Failed projection uses the enclosing type
projection transaction so symbol completion and type-cache mutations roll back.

Literal singleton syntax parses as `SingletonTypeTree` around a `Literal`
node. Scala 3.9.0 accepts Boolean, Char, Int, Long, Float, Double, and String
literal singleton annotations; its typed-tree oracle covers `true`, `false`,
`1`, `2`, `'a'`, `1L`, `1.0f`, `1.0d`, and `"foo"`. Source projection maps
those forms directly to `Type::Constant`, preserving their exact payload.
Projection caches the singleton type on the source type tree, repeated
projection reuses that `TypeId`, and a failed enclosing transaction rolls the
cache and type allocation back. Constant types pass the existing singleton
stability check without creating term symbols.

The source subset is explicit. Scala has no Byte or Short literal-type token;
numeric literals retain their lexical Int, Long, Float, or Double constant
kind. `null` is not a type-reference literal, `Unit` and `Null` are ordinary
class types, and Class / UTF-16-only constants do not arise from source literal
syntax. These unsupported semantic constant kinds return
`UnsupportedSingletonLiteralKind` with a stable kind label.

The #880 pinned audit identified 22 local-method first blockers from the one
`true` singleton declaration `actionable` in `CheckUnused.scala`. The #881
rerun moved all 22 past source type projection. #882 added exact constant
relations, #883 preserves an expression's exact constant when the expected
type contains a singleton alternative, and #884 pins source singleton result
annotations through projection and expected expression typing for `true`,
`false`, and `1`. The rerun still stops these 22 corpus methods at
`ImportQualifierNotFound`; it does not count them as fully typed. This supports
local value initializers, local method results, and method arguments while
ordinary expected types continue to use the underlying widened type. Literal
singleton annotations also reify into typed type trees. The audit measures
first-blocker movement and does not claim full method typing.

The semantic model already has `Type::Constant`, and source literal
expressions retain exact constant types. The bounded relation validates and
compares Unit, Boolean, Byte, Short, Char, Int, Long, Float, and Double
constants by their semantic payload. Separately allocated constants with equal payloads
are equivalent; unequal values are unrelated. These constants conform to the
canonical builtin type already held by `Definitions`, while the reverse
relation remains false. Constant alternatives also participate in the existing
bounded union rules. Expression widening uses the same constant-to-builtin
mapping as the relation.

String constants support exact equality, but the current `Definitions` has no
canonical String type, so string-to-String conformance remains unsupported.
UTF-16-only strings, Null, and Class constants remain outside the supported
relation subset and return `TypeRelationError::UnsupportedType`. `Class`
constants are not treated as nominal class views. `require_stable_selection_prefix`
continues to accept supported constant types as stable prefixes. TASTy decoding
maps wire constant types to the same `Type::Constant` representation. Expected
expression adaptation preserves an exact constant only when the normalized
expected type contains a constant singleton, including a union alternative;
all other expectations use normal expression widening before conformance.
This is intentionally limited to literal constants and does not add general
bidirectional typing.

`null`, class literals, and general constant folding remain unsupported until
their own source and relation contracts are specified.

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

Product extraction accepts N source arguments when it can disc…21860 tokens truncated…ing parameter types, parameter modifiers, annotations, and duplicate
parameter names remain explicit errors. Inferred one- and multi-parameter
lambdas report `UnsupportedFunctionLiteralParameter`; this remains outside
the increment even when the audit moves those inputs past the generic
`UnsupportedExpression::Function` blocker. Contextual function types project
to `ContextFunctionN`, but expression application, contextual argument search,
and contextual lambda expressions are still unsupported. Captures requiring
an environment are rejected explicitly; references to shared object-owned
values do not count as closure captures. Failed lambda typing rolls back its
synthetic method, parameter symbols, scopes, typed nodes, and source mappings. The pinned
Scala 3.9.0 typer and `lambdaLift` shapes, including source positions, are
recorded in the [function-literal fixture](../crates/dotty-typer/tests/fixtures/function-literals/FunctionLiterals.oracle.md).
The #829 rollback regression forces a late failure by omitting `scala.Function1`;
after synthetic method and parameter setup, both the first attempt and retry
return `SourceFunctionClassNotFound` with the store, typed AST, type cache, and
source mappings restored to their checkpoints.

The #829 comparison reran the pre-#826 baseline's same 25 local methods,
matched by source path, line, and method name, against the post-#829 typer.
For the 13 old `UnsupportedExpression::Function` cases, 9 now stop at
`UnsupportedFunctionLiteralParameter` in `Synthesizer.scala`; the other 4
stop at `ImportQualifierNotFound` in `Scala2Unpickler.scala`. None fully typed.
For the 12 old `UnsupportedTypeTree::Function` cases, all now stop at
`SymbolResolution` across the same 8 files; none fully typed. The frozen
`UnsupportedTypeTree::FunctionWithMods` cohort was 13 in 2 files and now has
zero current first blockers; all 13 rows are retained individually. The
comparison records each method and its current first blocker in the [audit
report](typer-classpath-corpus-audit-3.9.0.md#826-829-source-function-support).
No local method fully typed in the full corpus run, and external member
materialization remains zero. The refreshed pre-field-inference audit ranks
`MissingDeclaredType` (34 in 15 files) first and recommends the immutable
inferred `Class` field slice scoped in
[#825](typer-classpath-corpus-audit-3.9.0.md#recommended-next-sprint). That
slice is implemented in #856; the audit counts remain the baseline taken
before field inference was added.


### #1023 deferred classpath hardening gate

The [hardening report](typer-deferred-classpath-hardening-3.9.0.md) documents
lifecycle/regression policy, before/after dependency metrics, frozen cohort
accounting, and exactly one current next-sprint recommendation. The unchanged
[#976 baseline](typer-classpath-corpus-audit-976-3.9.0.md) is retained separately
from the [current audit](typer-classpath-corpus-audit-3.9.0.md).

For a directly comparable run use `JAVA_HOME` pointing to Temurin OpenJDK
21.0.8, the pinned Scala checkout/artifacts above, and `--verify-determinism`.
The runner compares two normalized outputs byte-for-byte and emits the report
plus `*-deferred-cohorts.tsv`. The comparison tool rejects missing frozen rows, lost successful class target
keys, a reduced class-success count, a changed typed-method denominator, p95
depth above 70, and full-completion fanout above 524. Shell identities touched are reported separately.

### #1051 package-term and Scala module audit

The pinned Scala 3.9.0 audit retains the immutable 2,749-request / 80-key
package-term profile and 369-row #1046 first-blocker cohort. The resolver now
reports canonical Object successes and distinct verified object identities
separately from attempted target keys, and distinguishes successful module
members from member requests that miss or fail. Across two deterministic runs,
1,791 requests resolved to canonical Objects across 46 verified identities;
zero corpus requests reached module-member lookup. The focused real-artifact
source fixture nevertheless verifies `scala.io.Source.stdin` end to end.
See the [#1051 checkpoint](typer-classpath-corpus-audit-3.9.0.md#1051-package-term-hardening-checkpoint)
for frozen-row movement, blocker reasons, performance limits, and the single
next-sprint recommendation. No default-import behavior is implied.

### #1130 intermediate classpath packages

The classpath resolver can reuse or materialize canonical package symbols for
intermediate paths when a usable class/TASTy resource exists anywhere below
the complete requested path. Direct package evidence and descendant-only
evidence remain distinguishable in resolver audit traces. Missing next
segments remain unresolved, source-owned packages retain their identity and
members, and a non-package term collision is reported as an ambiguity. Package
prefix recognition does not change source import precedence or establish a
class/object member at the package name. Package entry and rollback use the
shared `Packages` registry, preserving its existing origin and ownership
rules.

### #1131 qualified source imports through descendant packages

Source imports have end-to-end regressions for classes and real Scala 3.9
TASTy Objects when the classpath contains only a deeper package descendant.
Coverage includes explicit, renamed, wildcard, and block-local Object imports;
`_root_` qualification when a local term shadows the first package segment;
source-owned package identity and declarations; and coexistence of a TASTy
class and Object with the same name. A failure after resolving an Object import
must roll back package and symbol state, typed trees, and source indexes, while
a subsequent valid expression must resolve the canonical Object again. These
checks exercise the shared typer import path without adding default imports or
a second resolver.

### #1174 stacked package precedence and root qualification

After local lexical and import bindings miss, unrooted import qualifiers
search the actual current source package, then only the enclosing package
contexts represented by stacked package clauses, and finally the canonical
root package. Dotted package declarations do not create synthetic lexical
parents. An explicit import binding keeps its existing precedence over package
candidates. Once a qualifier resolves to a package, a missing later member does
not restart lookup from a lower-priority package. Explicit `_root_`
qualification begins at the canonical root symbol and never asks the classpath
resolver to resolve `_root_` as a package segment. Regression coverage includes
stacked-versus-dotted declarations, current-versus-enclosing TASTy packages,
imported-name shadowing, root qualification under a relative-name collision,
and failure after selecting a higher-priority package.

### #1072 post-parity Object module reprofile

The current audit also emits
`typer-classpath-corpus-audit-3.9.0-object-module-reprofile.tsv`, joined against
the immutable 576-row #1051 checkpoint in
`typer-classpath-corpus-audit-1051-object-module-blockers.tsv`. Stable keys use
source paths and AST tree indexes, never arena IDs. The frozen rows include the
method and failed-attempt trees, direct/inherited attribution, and any exact
package-term requests/module identities observed at the baseline. Current rows
separate Object origin, ModuleClass lookup outcome and state, completion,
member lookup, rollback, and the enclosing method's current first blocker.

The ignored audit test reports progress every 25 files; these progress messages
are outside the normalized report. `--verify-determinism` still compares the
normalized report itself byte-for-byte. A completion or member result marked
`rolled_back` is an attempted operation and is not counted as committed
semantic state.

### #1073 corrected resolver-parity checkpoint

Source-owned objects obtain their `ModuleClass` from the source semantic index;
external objects require the classpath resolver's authoritative module
identity. A `$`-suffixed class name alone never establishes an Object. The
audit-wrapper fixture checks exact parity with the direct classpath resolver
for the pinned Scala 3.9.0 `scala.io.Source.stdin`, including successful
ModuleClass completion, a resolved `stdin` member, and a missing-member result.
Rollback regressions also verify that observations remain visible as attempts
while rolled-back symbols do not count as committed state, and that the same
module member can be resolved again after rollback.

The corrected corpus contains 17 external ModuleClass completions and 58
module-member successes across nine distinct members. It retains all 576
#1051 module observations and all 369 #1046 module/import rows; 197 rows moved
past their baseline first blocker and none of the 3,297 local methods typed.
The largest attributable residual is first-segment/package import resolution:
2,394 observations across 209 files and 30 targets. The 31 remaining
`ObjectModuleClassUnavailable` rows have no authoritative Object target
identity, so they do not establish a new broad module-completion gap.

See the [#1073 corrected audit checkpoint](typer-classpath-corpus-audit-3.9.0.md#1073-post-auditresolver-parity-gate-and-next-sprint)
and [append-only classpath comparison](typer-deferred-classpath-hardening-3.9.0.md#1073-post-parity-comparison-and-sprint-recommendation)
for the independent metric definitions, limits, and single recommended next
sprint. The two normalized pinned reports compared byte-for-byte.


### #1180 lexical import resolution checkpoint

Pinned Scala 3.9.0 regressions cover package candidate identity and precedence,
stacked versus single-dotted package clauses, `_root_` lookup, final imported
member selection and rollback/retry. In the real `Resident.scala` fixture, the
relative qualifier `core` selects package `dotty.tools.dotc.core`; the final
`Contexts` selection is the canonical TASTy Object at
`dotty.tools.dotc.core.Contexts`. The method still has a later
`TypeNameNotFound` blocker, so this does not count as a fully typed method.
The full weighted reconciliation and deterministic profile are recorded in
the [#1180 checkpoint](typer-classpath-import-prefix-profile-1180-3.9.0.md).

The later audit-cost follow-up bypasses discarded resolver-observation
collection for ordinary match-case probes and uses rollback-aware reverse
indexes for classpath module identities. A targeted `tpd.scala` method profile
improved from 6,214 ms to 3,546 ms (512 member requests: 4,939 ms to 2,797 ms
inside lookup). The full-corpus rerun stopped at 250 / 1,236 files after about
20 minutes at that checkpoint. A subsequent pair of full pinned runs completed
all 1,236 files with identical normalized reports; typed methods remain
0 / 3,297, and deferred-loading limits passed at p95 depth 0 / 6 and completed
class fanout 9 / 16. Current blocker
counts and direct origins are summarized in the [#1180 blocker sidecar](typer-classpath-corpus-audit-1180-current-blockers.tsv).
See the completion report in
[`typer-classpath-corpus-audit-3.9.0.md`](typer-classpath-corpus-audit-3.9.0.md).

### #1339 package-owned Object import qualifiers

After existing lexical bindings and source package-scope declarations miss,
an unrooted import qualifier checks for both a corresponding child package
and a unique Term member from the candidate package. A classpath result
is accepted only when it is a canonical Object owned by that package and the
resolver confirms its ModuleClass identity. Missing evidence remains a miss;
malformed or ambiguous evidence is reported and does not continue at a lower
package candidate. If the same classpath candidate has both Package and Object
evidence, the qualifier reports explicit ambiguity. Both providers are probed
before choosing so the result is stable whether or not a prior lookup warmed
the Object in the shared package scope. Explicit `_root_` continues to start
at the canonical root.

The real Scala 3.9.0 compiler `Contexts.tasty` regression resolves
`import Contexts.*` to its package-owned Object, then resolves
`withFreshTyperState` through the Object's ModuleClass. The focused resolver
profile records one package-term request / Object success and one later
module-member success; repeated qualifier lookup is satisfied by the entered
package scope. This counts qualifier and selected-member operations
separately, and does not count a fully typed method. A source-owned `Contexts`
in the same package keeps its identity when the classpath also contains the
compiler TASTy Object. Missing lookup and malformed non-Object resolver
results leave store and resolver checkpoints unchanged. A package and Object
both entered under the same term name in one source package scope produce an
explicit ambiguity rather than an allocation-order choice. A separate
classpath fixture reports both a real compiler Object and the matching
package-prefix evidence; fresh lookup, Object-warmed lookup, and retry all
report the same ambiguity with no semantic-store mutation.

## Post-compiler-module audit checkpoint (#1342)

The latest pinned full-corpus audit and stable #1180 import reconciliation are
summarized in the [post-compiler-module audit checkpoint](typer-classpath-corpus-audit-post-compiler-modules-3.9.0.md).
These measurements preserve the 3,297-method denominator and distinguish
Object qualification, ModuleClass attempts, selector outcomes, and fully typed
methods.

### #1342 final audit closeout

The post-compiler-module checkpoint records the two serial, byte-identical
Scala 3.9.0 corpus runs, current blocker ranking, #1180 reconciliation, and a
single evidence-based next-sprint recommendation. Its final closeout evidence
maps the eight required rollback/atomicity cases to the focused production and
`AuditResolver` regressions and records the successful workspace CI run. See
[`the #1342 audit checkpoint`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0.md#final-closeout-evidence).
