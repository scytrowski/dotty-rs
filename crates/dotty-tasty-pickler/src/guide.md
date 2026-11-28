## Project canonical builtin references

Projection can be used separately from full class emission. This example
produces a structural type fragment for the current session's Int, Long, Float and Double types.

```rust
use dotty_core::{Definitions, SemanticStore, SourceId, SourceSemanticIndex};
use dotty_tasty_pickler::{EncodingContext, ProjectionLimits};

let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let index = SourceSemanticIndex::new();
let mut context = EncodingContext::new(
    &store, &definitions, &index, SourceId::from_index(0), ProjectionLimits::default(),
)?;
let int = context.lower_type(definitions.int)?;
let long = context.lower_type(definitions.long)?;
let float = context.lower_type(definitions.float)?;
let double = context.lower_type(definitions.double)?;
let fragment = context.finish(&[int, long, float, double])?;
assert!(fragment.ast.address(float).is_some());
assert!(fragment.ast.address(double).is_some());
assert!(fragment.ast.address(long).is_some());
assert!(fragment.ast.address(int).is_some());
assert!(!fragment.ast.bytes().is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

All IDs, source indexes, definitions, and source-to-typed bindings must belong
to the same canonical session. A mutating projection failure poisons its
[`EncodingContext`]; discard it and start a fresh context. Validation borrows
semantic state and does not modify caller stores, arenas, or files. Work
limits reject the operation rather than truncating output.

For complete supported classes, validate a [`PicklePlan`] or
[`ClassStatePicklePlan`], then call the corresponding receipt's `pickle`
method. Preserve class-member and parameter order and exact typed RHS roots.
[`PickledTasty`] owns complete bytes for its supported subset.
[`DeclarationSkeleton`] and the mutable-local fragments intentionally omit
parts of bodies and must not be published as completed artifacts. Structural
validation and successful Rust decoding alone do not prove Scala ABI or
consumer compatibility; the pinned Scala oracle validates supported producer
shapes independently. This crate performs no filesystem publication.

## Bounded primitive expressions

Complete plans can preserve Int/Boolean/Unit/Long/Float/Double ascriptions as TYPED nodes and
Int/Boolean/Long/Float/Double-selector matches as MATCH with ordered CASEDEF nodes. Matches support
Int/Boolean/Unit/Long/Float/Double results and require a final unguarded irrefutable
case unless unguarded Boolean cases cover both values with an unguarded final case;
guards and canonical Int/Boolean/Long binders follow the contract below. Extractors remain
unsupported. Int `&` and equality/inequality between Int
operands use Scala 3.9 signed primitive selections. Source Any equality symbols
remain canonical; only their validated Int application is specialized to the
Scala Int wire signature. Ordinary Int locals use exact source/typed/symbol/scope
bindings, including declarations whose Int type was inferred by the typer.

Guarded Int/Boolean/Long/Float/Double matches preserve optional guards as the final CASEDEF child, after
pattern and body. Pattern bindings arrive through
`SourceTypedBindings::pattern_value_bindings`, using the exact source/typed,
symbol and case-scope tuple from the frontend. The producer emits BIND labels
and direct symbol references, validates visibility independently per case, and
rejects missing, duplicate, stale or cross-case binding evidence without store
mutation. The subset includes variable roots and explicit bindings over Int/Boolean/Long
literals or wildcards, with a final unguarded irrefutable case or complete
unguarded Boolean coverage as described below.

Same-type primitive patterns retain TYPED nodes, explicit unqualified Int/Boolean/Long/Float/Double
type syntax and any outer BIND. A source `n: Int` maps to a typed Bind around a
synthetic `Typed(_, Int)`; validation authenticates the identifier-to-Bind and
type-tree edges without inventing source mappings for synthetic nodes. Explicit
`n @ (_: Int)` has its own genuine inner mappings. Boolean, Long, Float and Double forms follow the same
rules. These patterns are irrefutable only without a guard and must then be the
final case. Typed alternatives, reference tests and selectors beyond
Int/Boolean/Long/Float/Double remain unsupported.

Local returns are supported in explicitly typed ordinary Int/Boolean/Unit/Long/Float/Double
methods. RETURN references the exact current method declaration. A bounded
read-only flow validator separates normal completion from a proven local exit;
it does not interpret arbitrary Nothing-typed expressions as supported returns.
Blocks, If, While and Int/Boolean/Long/Float/Double matches retain their source/typed bindings, including
the typer's checked Unit adaptation for an If without else. Bare return uses the
actual synthetic Unit value already supplied by the typer. Non-local targets,
returns in lambdas/local definitions and reference results remain
outside this producer subset.


## Bounded Long method values

Ordinary single-clause methods may use canonical Long parameters/results and
full signed 64-bit constants, including mixed Int/Boolean/Long formal order.
Long locals may be inferred or explicit, immutable or mutable. Supported If,
While, Int/Boolean/Long/Float/Double-selector Match and local Return paths preserve canonical source/typed
identity, exact values and case scope. A Long return targets the same current
DEFDEF as other local primitive returns. No scalar value is supplied for a
proven returning path. Long singleton case binders retain LONGCONST types and
registered direct references; reads authenticate ownership and case visibility.

Calls with Long arguments or results, Long fields/constructor parameters, arithmetic,
conversions and discarded Long expressions remain outside
this producer slice. Interoperability requires consuming original Rust artifacts
with Scala 3.9 and executing both original JVM output and from-tasty output; the
Scala numeric raw printer is not used as the value oracle.

## Literal alternative patterns

Int/Boolean/Long/Float/Double matches also preserve literal alternatives as nested ALTERNATIVE nodes.
Validation checks every original source/typed child edge, literal payload,
constant type, source order and span, with the existing expression budgets.
Parentheses retain their real source bindings. Guards remain the final CASEDEF
child after its one pattern and body. Nested alternatives and duplicates are
supported; wildcards or bindings inside alternatives, bindings around them and
selectors other than Int/Boolean/Long/Float/Double are rejected. No semantic-store mutation or partial complete
artifact escapes validation failure.

## Bounded floating method values

Single-clause ordinary methods support canonical Float/Double formals/results,
mixed Int/Boolean/Long/Float/Double parameter order, explicit or inferred locals,
assignment and existing If/While/Int-match/local-return flow. Builtins are
validated on use without imposing unused floating identities on older plans.
FLOATconst/DOUBLEconst payloads use exact raw bits, including signed zero.
Unsuffixed decimal/exponent source nodes retain their original Number binding;
validation bounds the spelling before conversion and checks the same finite f64
bits produced by the typer. Suffixed literals retain their exact Constant bits.

Fields, constructor parameters, calls taking/returning floating values, arithmetic,
comparison, widening, boxing and discarded floating statements remain unsupported
in this paired producer slice. Same-type Long/Float/Double expression ascriptions
preserve the original TYPED nodes without runtime conversion. Independent JVM
support does not expand this contract. Runtime NaN checks establish classification;
raw codec bit preservation does not promise payload fidelity through arbitrary
JVM execution. Failed validation is read-only and emits no partial complete file.

Floating selectors preserve same-family FLOATCONST/DOUBLECONST patterns,
ordered ALTERNATIVE children and canonical typed wildcard/binder wrappers.
Literal singleton binders retain raw-bit constant types and registered direct
references. Validation distinguishes positive and negative zero even though
runtime matching treats them as equal. Every floating match requires a final
unguarded irrefutable case; no NaN/infinity stable patterns, widening or general
exhaustivity analysis is introduced. Source singleton zero cases rejected by the
frontend remain rejected. The pinned Scala oracle checks original Rust output
and separately rebuilt from-TASTy output; a Rust round trip alone is insufficient.

## Boolean-selector matches

Boolean selectors reuse the existing MATCH/CASEDEF structure with literal
TRUEconst/FALSEconst patterns, BIND declarations and nested ALTERNATIVE nodes.
Selectors may be literals, formals, visible canonical pattern bindings or the
already supported Boolean condition expressions. Case bindings are visible only
within their guard/body, including inner matches; repeated spelling never merges
symbols or scopes. Constant-selector binder types retain their exact Boolean
singleton payload. A final unguarded irrefutable case may be omitted when validated
unguarded cases cover both Boolean values and the last case has no guard. Coverage
is collected during the same bounded source/typed pattern validation; guard-bearing
cases contribute no guaranteed values. Original cases, alternatives and BIND targets
are retained in TASTy, with no synthetic fallback. Partial matches, singleton-only
coverage, arbitrary Boolean-returning calls and typed/extractor patterns remain
unsupported. Complete paired output checks declaration/symbol/case/scope agreement
with the JVM local plan.

## Ordinary Boolean locals and values

Initialized method-local `val`/`var Boolean` use exact source-typer declaration,
symbol and scope proofs with explicit or inferred canonical type syntax. Visible
reads use TERMREFdirect targets; mutable declarations carry MUTABLE and their
assignments retain ASSIGN with the exact local target. Boolean If/Block values,
Int comparisons, negation, short-circuit effects and existing Int/Boolean matches
compose with ordinary locals, loop conditions, guards and primitive method
results. Lexical visibility is restored after blocks and branches. Repeated names
in sibling scopes and parameter shadowing preserve distinct declaration identity;
general nested same-name ordinary-local shadowing remains excluded.

Local returns retain their existing supported statement/result positions.
Proven all-returning Boolean matches can retain the typer's singleton type for an
unreachable binder tail. Returns inside initializer/assignment operands,
conditions, guards and short-circuit operands remain unsupported. Complete output
requires read-only body validation and exact ordinary-local agreement with the
JVM plan; failure exposes no partial artifact. The pinned Scala gate consumes
original Rust TASTy and executes independently rebuilt from-TASTy classes.
