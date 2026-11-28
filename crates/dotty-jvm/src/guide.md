## Inspect a canonical typed expression without emission

The intrinsic resolvers read an already-typed AST and the same session's
builtin definitions. Ordinary expressions and user-defined calls are not
recognized merely because their spelling resembles an operator.

```rust
use dotty_core::{AstArena, Constant, Definitions, SemanticStore, Typed, TypedAstBuilder};
use dotty_jvm::{resolve_int_intrinsic, JvmType};

let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let mut typed = AstArena::<Typed>::new();
let literal = TypedAstBuilder::new(&mut typed, &store.types)
    .literal(Constant::Int(42), definitions.int, None);
assert_eq!(resolve_int_intrinsic(&store, &typed, literal, &definitions), Ok(None));
assert_eq!(JvmType::Int.descriptor_char(), Some('I'));
assert_eq!(JvmType::Boolean.descriptor_char(), Some('Z'));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Whole-class lowering starts with a [`JvmCompilationUnit`] borrowing the
canonical store and exact typed arena. Supply validated class, constructor,
member, parameter, and source-binding evidence; this crate does not parse,
name, or retype source. Preserve source order and canonical symbol identities.

Validation and lowering reject unsupported constructs through
[`JvmCodegenError`] before publishing artifacts. Generated class files use
major version 65 (Java 21). Metadata projection, bytecode construction, class
emission, and external JVM verification have distinct guarantees. Run a JDK
verifier for emitted programs; the in-memory example above needs no JDK.
TASTy emission and combined Scala ABI validation are separate output paths.

## Bounded primitive expression parity

Planned methods also lower Int/Boolean/Unit/Long/Float/Double ascriptions without runtime
conversion, canonical Int `&`, and canonical Any `==`/`!=` specialized for two
Int operands. Int/Boolean/Long/Float/Double-selector matches accept literals of the selector type and
literal alternatives, with Int/Boolean/Unit/Long/Float/Double results. Int, Long, Float and Double matches
require a final unguarded irrefutable case; Boolean matches may instead prove full
unguarded coverage of both values with an unguarded final case. A Nothing-typed
match is accepted only when every case proves a supported local return. A temporary local
preserves one selector evaluation; branch initialization facts are intersected
across normal exits. Match lowering currently requires a planned local layout.
Guards and immutable Int/Boolean/Long/Float/Double binders are supported. Same-type primitive
patterns `_: Int`, `n: Int`, `n @ (_: Int)` and Boolean/Long/Float/Double counterparts are proved
irrefutable using exact canonical wrapper, type-tree and wildcard types. An
unguarded irrefutable case must be final; a guard still requires a fallback.
This proof emits no runtime type test. Typed alternatives, reference type tests,
extractors and incomplete coverage without a fallback remain unsupported.
The semantic TASTy producer retains these source constructs separately; a
successful JVM emission alone does not certify a paired artifact.

`JvmLocalBinding::pattern` records the exact typed Bind, canonical immutable
symbol, case scope and typed CaseDef. Validation checks the declaration edge
and scope membership. JVM lowering initializes the slot only on pattern success
and removes case-local initialization facts before the next case and match join.

## Local primitive method returns

Ordinary planned methods with Int, Boolean, Unit, Long, Float or Double results can return directly
or from blocks, If branches, While bodies and supported Int/Boolean/Long/Float/Double matches. A local
return emits the method's exact primitive return instruction. All-returning
branches contribute no join value or trailing epilogue. A block may retain
the primitive type of an unreachable tail; lowering still validates that tail
and proves the actual return flow before omitting its code. A Nothing type
by itself does not establish a supported exit. Non-local returns and returns
inside operands, conditions or guards remain unsupported.


Long-valued Int matches preserve two-slot stack values at joins and use one
Long verification entry per value. Guarded cases and normal/returning paths
share the existing selector-once and case-scope rules. A proven returning Block,
If or Match can retain the canonical Long type of its unreachable tail, including
a checked method parameter/local TermRef. Long selectors use two-slot temporaries and LCmp; this does not add
arithmetic or conversions.

Literal alternatives such as `case 0 | 1` can be nested or parenthesized and
retain source case order. Each case has one shared guard/body: even duplicate
matching alternatives evaluate the guard once, and a false guard skips the case.
Alternative leaves must be literals of the selector type; bindings over/inside alternatives,
wildcards inside alternatives, and selectors other than Int/Boolean/Long/Float/Double remain unsupported.
The existing expression depth/node budgets apply to every alternative descendant.

## Floating method values

Float/Double-valued Int matches support the existing literal alternatives,
guards, case binders and fallback. Normal joins retain exact Float/Double stack
kinds; all-returning branches have no placeholder or epilogue. Returning Block,
If and Match nodes may retain a floating constant/type/formal/local tail type
only after lowering proves the exit and authenticates planned binding kinds.
Float uses one physical slot and verifier tag 2; Double uses two slots and one
verifier tag 3 entry. These are distinct from Integer/tag 1 and Long/tag 4.
Floating arithmetic and conversions remain unsupported. Paired TASTy support covers method values and control
flow, with narrower field/call support than this independent JVM backend.

Float/Double selectors support same-family literals and alternatives, wildcard
and canonical same-type typed patterns, with their original case binders and
Boolean guards. FCmpL/DCmpL implement numeric pattern equality: signed zeros
compare equal and NaN reaches an irrefutable fallback. Binders copy the original
selector value, preserving its zero sign. Raw literal/type validation remains
bit-exact. A final unguarded irrefutable case is required, including singleton
selectors. Float selector temporaries occupy one slot; Double temporaries occupy
two. This does not add floating arithmetic, widening or source comparison calls.

## Boolean-selector matches

Boolean matches reuse the ordered Int matcher, while validating the semantic
selector family separately from its Integer JVM verification category. `true`
and `false` patterns use exact Boolean singleton types; wildcard and immutable
case bindings retain canonical Boolean types and case scopes. Nested Boolean/Int
matches, short-circuit selectors/guards and all existing primitive results are
supported. A final unconditional wildcard or variable case is optional when
validated unguarded cases cover both Boolean values and the last case has no guard.
Guards guarantee no coverage; duplicate alternatives add no values. The JVM closes
the final mismatch edge using this proof while retaining binder initialization and
case order. Partial matches and general MatchError lowering remain unsupported.
Paired TASTy production also supports ordinary Boolean locals and
bounded Boolean value blocks; arbitrary Boolean-returning calls remain outside
that paired subset.

Ordinary Boolean `val`/`var` declarations keep exact planned symbols and local
slots, and can drive branches, loops, selectors and guards. Proven returning
branches authenticate Boolean TermRef tails, including singleton case bindings,
without confusing Boolean with Int or manufacturing a Nothing type.

## Primitive statement values

Supported Int/Boolean/Long/Float/Double expressions can be discarded in block
statements and While bodies. Evaluation preserves source order, effects,
exceptions and validated local returns. POP consumes Int/Boolean/Float; POP2
consumes Long/Double. Unit and proven returning paths require no pop. The final
block expression keeps its exact result type. Expected-Unit method bodies borrow
the frontend's actual adapted root; lowering does not fabricate source bindings.
This support does not add arithmetic, conversions or call/field signatures.
