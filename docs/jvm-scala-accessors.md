# Scala constructor accessor ABI audit

This document records the bounded audit for ordinary primary-constructor
`val`/`var` parameters in Scala 3.9.0, tracked by
[#1553](https://github.com/scytrowski/dotty-rs/issues/1553). It does not enable
accessor code generation. It keeps three questions separate: source identity,
source selection typing, and the JVM members Scala emits.

## Evidence and oracle status

The compatibility target is Scala 3.9.0 at revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
At this revision, [`Getters.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/transform/Getters.scala)
rewrites ordinary class `val`/`var` declarations into getter definitions;
mutable declarations also get setters. [`Memoize.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/transform/Memoize.scala)
later introduces private storage for these methods. This source-level phase
description is useful for locating responsibilities, but it does **not** prove
the final class-file names, flags, descriptors, or instruction layout.

The pinned compiler was built from the exact checkout revision above and
reported `3.9.0-bin-SNAPSHOT-git-777528f`. The three declarations below were
compiled with that compiler and inspected with `javap -c -v -p`. This freezes
the bounded ABI used by the metadata projection; it does not claim support for
other Scala releases or accessor bytecode in `dotty-jvm`.

```scala
class Measurement(val id: Int, var value: Double)
class Mixed(val enabled: Boolean, var count: Long)
class Ref(val item: Measurement)
```

Observed class-file members:

| Class | Backing storage | Accessor methods | Constructor evidence |
| --- | --- | --- | --- |
| `Measurement` | `id:I` private final; `value:D` private | `id()I` public; `value()D` public; `value_$eq(D)V` public | `<init>(ID)V`; `id` loads from slot 1 and `value` from slots 2–3; both stores precede `Object.<init>()V`. |
| `Mixed` | `enabled:Z` private final; `count:J` private | `enabled()Z` public; `count()J` public; `count_$eq(J)V` public | `<init>(ZJ)V`; `enabled` loads from slot 1 and `count` from slots 2–3; both stores precede `Object.<init>()V`. |
| `Ref` | `item:LMeasurement;` private final | `item()LMeasurement;` public | `<init>(LMeasurement;)V`; `item` loads from slot 1 and its store precedes `Object.<init>()V`. |

No bridge methods were present in these shapes. A `val` has no setter; a `var`
setter returns `void`. Scala source `value_=` encodes to JVM name `value_$eq`.
Constructor store ordering is recorded as compiler evidence for follow-up work;
this issue only plans metadata and does not lower constructors. The oracle
class files use major version 61 by default; this observation does not change
the backend's independent Java 21 class-file target (major 65).

## Source identities and current Typer behavior

The real source pipeline test in
`tests/jvm_stateful_class_contract.rs` runs the requested `Measurement`,
`Mixed`, and `Ref` declarations through scanner, parser, namer, and Typer.
For each constructor parameter accessor it verifies:

- the source parameter tree maps to a class-owned `Field`;
- `derived_symbol_at(constructor, source, tree)` yields a distinct,
  constructor-owned `Parameter`;
- mutability belongs to the `Field`, while the derived parameter remains
  immutable;
- field and parameter types preserve the canonical primitive or class type;
- parameter order is the source constructor-clause order.

The frontend does not synthesize source `Method` symbols for JVM accessors.
The JVM getter/setter entrypoints are an ABI projection and must not be
inserted into source name lookup or substituted for the field's `SymbolId`.

The current Typer successfully types all of the following in the same source
session: external `m.id` and `m.value`, `this.id` and `this.value`, the
assignment `m.value = 42.0`, and passing `m.id` as an argument to another
method. The typed references for each selected member designate the canonical
field symbol. These tests characterize source typing only; they do not prove
that the current backend emits a compatible accessor ABI or can compile those
classes.

The first current backend rejection is constructor-plan source-shape
validation for emission: `JvmPrimaryConstructorPlan::from_source` accepts the
audited parameter roles and produces an immutable source binding, but the
backend still has no accessor field/method emission. The plan accepts a plain
parameter only with `ParamAccessor` plus `PrivateLocal`, an immutable public
accessor only with `ParamAccessor`, and a mutable public accessor only with
`ParamAccessor` plus `Var`. Defaulted, contextual, annotated, qualified
private, or otherwise modified parameters remain rejected.

## Frozen ownership and initial backend contract

| Concern | Owning layer | Contract for a future implementation |
| --- | --- | --- |
| Constructor parameter, source field, and source selection identity | `dotty-core` / namer / typer | Preserve canonical IDs and source mappings. Never invent source methods for generated JVM accessors or reconstruct identity from spelling. |
| Source field to derived constructor formal binding | `dotty-jvm` constructor plan | Preserve exact class/constructor IDs, source `TreeId`, class-owned `Field`, distinct constructor-owned `Parameter`, source ordinal, and mutability. Revalidate symbol kinds/owners/origins, class-scope membership, uniqueness, and completed type agreement before exposing the checked binding through `JvmClassMetadata`. |
| Getter/setter JVM names, descriptors, flags, backing storage, and slot layout | JVM-only ABI projection in `dotty-jvm` | Derive from the pinned compiler oracle and canonical source metadata. Keep JVM names out of frontend contracts. `JvmAccessorMetadata` records checked backing-field and getter / optional setter signatures without source symbols or bytecode. |
| Encoding and verification of generated members | `dotty-classfile` | Receive already validated field/method plans; keep byte-level encoding separate from source semantics. |
| Source call-site support where current Typer behavior changes | Separate frontend/Typer issue | This audit records current successes and does not authorize Typer changes or accessor-specific call lowering. |

Before publishing any future class bytes, the projection must validate the
owner, source-to-derived parameter binding, field/parameter type agreement,
mutability, visibility, and explicit source order. A malformed or stale
identity must be a typed rejection. Generated JVM signatures must be checked
for collisions against user-declared and other generated members; the policy
must reject ambiguity before artifact publication rather than silently
shadowing either member. Accessor metadata now performs these checks; no
accessor classfile is an accepted backend output yet.

## Scope matrix

| Shape | Audit result / disposition |
| --- | --- |
| `Measurement(val id: Int, var value: Double)` | Exact backing field/getter/setter names, descriptors, flags, and `D` slot width recorded above and projected in metadata. No accessor bytecode. |
| `Mixed(val enabled: Boolean, var count: Long)` | Exact `Z`/`J` field and accessor descriptors recorded above; `Long` uses two constructor local slots. No accessor bytecode. |
| `Ref(val item: Measurement)` | Exact planned reference descriptor and private-final storage recorded above. No accessor bytecode. |
| Plain constructor parameter | Existing distinct class-owned and constructor-owned parameter identities remain; it does not gain an accessor by implication. |
| Immutable `val` | Oracle confirms private-final storage and public getter with no setter. |
| `private[this]`, class-body public fields, `lazy`, visibility variants | Deferred; no ABI claim is made by this audit. |
| Secondary, defaulted, multiple-clause, or `using` constructors | Deferred. |
| Trait, case, generic, or externally compiled classes; inheritance bridges | Deferred. |
| Malformed owner/derived IDs, stale typed IDs, generated signature collisions | Metadata validation rejects invalid bindings and binary collisions before bytes are published; codegen is not implemented here. |
| Unsupported source calls or assignments | Keep the actual Typer/backend error observable; do not guess or bypass it. |

## Future acceptance gate

Accessor code generation remains disabled until an implementation change adds
focused tests for canonical source/derived bindings, exact descriptors and
flags from the pinned Scala oracle, setter `void` return behavior, constructor
parameter and category-two slot layout, source-order initialization, collision
rejection, deterministic bytes, and rejection without partial artifacts. The
generated public getter/setter entrypoints must be callable from Java, and
generated classes must pass `java -Xverify:all` plus state/constructor behavior
checks under the repository's mandatory JDK 21 gate. These JVM-only tests must
not depend on Scala source call-site lowering. Rust-produced output passing the
JVM verifier alone is not evidence of Scala ABI compatibility.
