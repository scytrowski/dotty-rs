# Scala constructor accessor ABI audit

This document records the bounded audit and implementation status for ordinary
primary-constructor `val`/`var` parameters in Scala 3.9.0, tracked by
[#1553](https://github.com/scytrowski/dotty-rs/issues/1553). Checked backing
field metadata and constructor initialization are implemented, and the
backend emits audited public getter methods plus setters for mutable
constructor accessors.
The contract keeps source identity, source selection typing, and emitted JVM
members separate.

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
The oracle stores these constructor accessors before `Object.<init>()V`. The
bounded backend implementation follows #1556's stricter initialized-`this`
contract: it invokes the direct parent first, then stores accessor formals,
then runs class-body field initializers. This preserves exact fields, values,
and source order within the supported subset, but intentionally does not
match Scala's constructor instruction order byte for byte. The oracle class
files use major version 61 by default; this does not change the backend's
independent Java 21 class-file target (major 65).

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
field symbol. JVM getter emission is tested independently through decoded
class files and Java 21 invocation; source call-site lowering remains outside
this issue.

`JvmPrimaryConstructorPlan::from_source` accepts the audited parameter roles
and produces an immutable source binding. The backend emits private backing
field_info entries, checked constructor stores, public getters, and public
setters for mutable accessors from those exact bindings. Source call-site
integration remains deferred.
The plan accepts a plain
parameter only with `ParamAccessor` plus `PrivateLocal`, an immutable public
accessor only with `ParamAccessor`, and a mutable public accessor only with
`ParamAccessor` plus `Var`. Defaulted, contextual, annotated, qualified
private, or otherwise modified parameters remain rejected.

## Frozen ownership and initial backend contract

| Concern | Owning layer | Contract for a future implementation |
| --- | --- | --- |
| Constructor parameter, source field, and source selection identity | `dotty-core` / namer / typer | Preserve canonical IDs and source mappings. Never invent source methods for generated JVM accessors or reconstruct identity from spelling. |
| Source field to derived constructor formal binding | `dotty-jvm` constructor plan | Preserve exact class/constructor IDs, source `TreeId`, class-owned `Field`, distinct constructor-owned `Parameter`, source ordinal, and mutability. Revalidate symbol kinds/owners/origins, class-scope membership, uniqueness, and completed type agreement before exposing the checked binding through `JvmClassMetadata`. |
| Getter/setter JVM names, descriptors, flags, backing storage, and slot layout | JVM-only ABI projection in `dotty-jvm` | Derive from the pinned compiler oracle and canonical source metadata. Keep JVM names out of frontend contracts. `JvmAccessorMetadata` records checked backing-field and getter / optional setter signatures without source symbols. |
| Constructor accessor stores | `dotty-jvm` constructor initializer and class emitter | Copy the exact constructor-owned formal into private storage after direct-super initialization and before ordinary class-body field initializers. Keep accessor stores separate from typed source RHS plans. |
| Encoding and verification of fields and bytecode | `dotty-classfile` | Receive validated field and instruction plans; keep byte-level encoding separate from source semantics. |
| Source call-site support where current Typer behavior changes | Separate frontend/Typer issue | This audit records current successes and does not authorize Typer changes or accessor-specific call lowering. |

Before publishing any future class bytes, the projection must validate the
owner, source-to-derived parameter binding, field/parameter type agreement,
mutability, visibility, and explicit source order. A malformed or stale
identity must be a typed rejection. Generated JVM signatures must be checked
for collisions against user-declared and other generated members; the policy
must reject ambiguity before artifact publication rather than silently
shadowing either member. Accessor metadata performs these checks. Emitted
classes contain the checked backing fields, constructor stores, generated
getters, and setters for mutable accessors.

## Scope matrix

| Shape | Audit result / disposition |
| --- | --- |
| `Measurement(val id: Int, var value: Double)` | Emits private `id:I` and `value:D` storage, exactly one initialized store from slots 1 and 2–3 after `Object.<init>`, public `id()I` / `value()D` getters, and public `value_$eq(D)V`; `val id` has no setter. |
| `Mixed(val enabled: Boolean, var count: Long)` | Emits private `enabled:Z` and `count:J` storage, public `enabled()Z` / `count()J` getters and public `count_$eq(J)V`; the setter uses two local slots for its Long parameter. |
| `Ref(val item: Measurement)` | Emits private-final planned reference storage, copies the exact reference formal after direct-super initialization, and emits `item()LMeasurement;`. |
| Plain constructor parameter | Existing distinct class-owned and constructor-owned parameter identities remain; it does not gain an accessor by implication. |
| Immutable `val` | Oracle confirms private-final storage and public getter with no setter. |
| `private[this]`, class-body public fields, `lazy`, visibility variants | Deferred; no ABI claim is made by this audit. |
| Secondary, defaulted, multiple-clause, or `using` constructors | Deferred. |
| Trait, case, generic, or externally compiled classes; inheritance bridges | Deferred. Accessor metadata is rejected on non-Object subclasses and across planned accessor-inheritance edges until generated signatures participate in override/bridge validation. |
| Accessor-bearing class with a planned non-Object parent | Rejected with `UnsupportedJvmAccessorInheritance`, before accessor metadata is returned. This prevents generated getter/setter signatures from bypassing inherited final-method and override checks. |
| Malformed owner/derived IDs, stale typed IDs, generated signature collisions | Metadata validation rejects invalid bindings and binary collisions before bytes are published; bytecode-builder and classfile errors remain typed and publish no generated class artifact. |
| Unsupported source calls or assignments | Keep the actual Typer/backend error observable; do not guess or bypass it. |

## Backend acceptance coverage

Getter and setter emission is covered for `Int`, `Boolean`, `Long`, `Float`,
`Double`, and validated planned-class references. Tests assert accessor names,
descriptors, flags, field references, typed return/load opcodes, deterministic
output, and accessor order. Setters load the exact JVM parameter category,
write the validated private field once, and return `void`; immutable `val`s and
plain constructor parameters have no setter. A Java harness invokes getters
and mutates `var`s under `java -Xverify:all`. These JVM-only tests do not depend
on Scala source call-site lowering. Rust-produced output passing the JVM
verifier alone is not evidence of Scala ABI compatibility.
