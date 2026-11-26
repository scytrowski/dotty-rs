## Project canonical builtin references

Projection can be used separately from full class emission. This example
produces a structural type fragment for the current session's Int type.

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
let fragment = context.finish(&[int])?;
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

Complete plans can preserve Int/Boolean/Unit ascriptions as TYPED nodes and
Int-selector matches as MATCH with ordered CASEDEF nodes. Matches require a
final unguarded wildcard and support primitive results; binders, guards and
extractors remain unsupported. Int `&` and equality/inequality between Int
operands use Scala 3.9 signed primitive selections. Source Any equality symbols
remain canonical; only their validated Int application is specialized to the
Scala Int wire signature. Ordinary Int locals use exact source/typed/symbol/scope
bindings, including declarations whose Int type was inferred by the typer.

Guarded Int matches preserve optional guards as the final CASEDEF child, after
pattern and body. Pattern bindings arrive through
`SourceTypedBindings::pattern_value_bindings`, using the exact source/typed,
symbol and case-scope tuple from the frontend. The producer emits BIND labels
and direct symbol references, validates visibility independently per case, and
rejects missing, duplicate, stale or cross-case binding evidence without store
mutation. The subset includes variable roots and explicit bindings over Int
literals or wildcards, with a final unguarded irrefutable case.

Local returns are supported in explicitly typed ordinary Int/Boolean/Unit
methods. RETURN references the exact current method declaration. A bounded
read-only flow validator separates normal completion from a proven local exit;
it does not interpret arbitrary Nothing-typed expressions as supported returns.
Blocks, If, While and Int matches retain their source/typed bindings, including
the typer's checked Unit adaptation for an If without else. Bare return uses the
actual synthetic Unit value already supplied by the typer. Non-local targets,
returns in lambdas/local definitions and reference/category-two results remain
outside this producer subset.
