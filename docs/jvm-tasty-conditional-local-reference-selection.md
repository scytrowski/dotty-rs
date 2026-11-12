# Conditional immutable local reference receipt

This proof-only increment authenticates the source and typed identity of one
immutable local initialized by a Boolean `if` that selects between two
previously initialized `Counter` locals. The receipt borrows the canonical
`SemanticStore`, source and typed ASTs, semantic/source indexes, and JVM unit
from one frontend session. It retains the source and typed roots for the three
local declarations and initializers, IF, condition, both ordered arms, setter
receiver/argument, final Pair result/arguments, the method owner, local scope,
formal, local symbols, and declared Counter type.

Validation requires the condition to resolve to the original Boolean formal;
both arms to resolve to distinct symbols from the two initialized locals; the
IF and selected declaration type to be the canonical Counter class; the
selected binding to be immutable and method-owned in the same block scope; and
the later setter and Pair construction to retain their original typed roots.
It does not infer identity from local names, source order, or JVM slots. A
valid source may reverse the declarations or swap the two IF branches; the
receipt records the resolved branch symbols in branch order. Nested IFs,
branch allocations, mutable selected locals, subtype joins, and extra block
statements are outside this contract.

The private paired-plan `SourceTypedBindings` carrier has a dedicated
conditional-selection binding. A future plan consumer can read that binding
without receiving serialization authorization. Current emitters, public
source acceptance, and accepted paired artifacts are unchanged.

## Frozen producer rejection

The original #2265 source and measured session identities are recorded in
[audit PR #2275](https://github.com/scytrowski/dotty-rs/pull/2275). In that one real session, `create` has four formals and the
bound IF condition and branches resolve to the original formal and two
initialized local symbols. The existing candidate-method proof returns its
rejection before its paired body proof runs:

```text
phase: cross-class constructor-call identity proof
SourceShape("candidate method must have one plain formal clause and a concrete public body")
```

The callback is not reached and no plan or artifact batch is returned. The new
receipt authenticates the body independently so it can travel through the
typed-binding seam if a later plan admits it; it does not alter the existing
candidate-method error or callback/artifact result.

## First failure stages for invalid receipts

| Bad input | First failure |
| --- | --- |
| Wrong source node shape, mutable selected local, nested IF, or non-reference arm | `SourceShape` or `SourceIdentity` while authenticating the source roots |
| Missing source-to-typed mapping for IF, condition, arm, call, or Pair argument | `TypedBinding` at the first missing edge |
| Stale/swapped typed arm mapping, wrong formal, receiver, or Pair argument target | `TypedBinding` when the typed node or symbol target disagrees with its original source edge |
| Non-Boolean condition, incompatible branch/result type, or uninitialized source reference | Frontend typer error before receipt validation |

The focused regression exercises the original, renamed, reordered, and
legitimately swapped-branch source controls; missing and falsified mappings;
wrong typed targets; invalid frontend controls; and the preserved paired
producer failure before its callback.
