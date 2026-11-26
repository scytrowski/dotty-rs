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

Planned methods also lower Int/Boolean/Unit ascriptions without runtime
conversion, canonical Int `&`, and canonical Any `==`/`!=` specialized for two
Int operands. Int-selector matches accept ordered Int literals and exactly one
final unguarded irrefutable case, with Int/Boolean/Unit or Long results. A Nothing-typed
match is accepted only when every case proves a supported local return. A temporary local
preserves one selector evaluation; branch initialization facts are intersected
across normal exits. Match lowering currently requires a planned local layout.
Guards and immutable Int binders are supported; extractors, type tests and missing unconditional fallback cases return typed errors.
The semantic TASTy producer retains these source constructs separately; a
successful JVM emission alone does not certify a paired artifact.

`JvmLocalBinding::pattern` records the exact typed Bind, canonical immutable
symbol, case scope and typed CaseDef. Validation checks the declaration edge
and scope membership. JVM lowering initializes the slot only on pattern success
and removes case-local initialization facts before the next case and match join.

## Local primitive method returns

Ordinary planned methods with Int, Boolean or Unit results can return directly
or from blocks, If branches, While bodies and supported Int matches. A local
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
a checked method parameter/local TermRef. This does not add Long selectors,
arithmetic, conversions or category-two discard operations.
