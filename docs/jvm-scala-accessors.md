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

The current development environment has no `scalac`, `scala`, `java`,
`javac`, or `javap` on `PATH`, and no cached Scala 3.9.0 compiler artifact was
found in the checked local compiler caches. Consequently this audit does not
claim observed `javap` output and deliberately leaves exact ABI fields below
unfrozen. The pinned compiler source is evidence about the intended
transformation, not a substitute for the required compiler-generated class.

To close this evidence gap, compile this source with the pinned compiler and
record the output of `javap -c -v -p` for all three classes:

```scala
class Measurement(val id: Int, var value: Double)
class Mixed(val enabled: Boolean, var count: Long)
class Ref(val item: Measurement)
```

```sh
scalac -d out AccessorAudit.scala
javap -classpath out -c -v -p Measurement Mixed Ref
```

The captured output must identify backing fields and their exact access flags,
getter and setter names/flags/descriptors, setter return type, constructor
descriptor and initialization order, category-two local slots for `Double`
and `Long`, and any bridges. In particular, the setter name encoding (often
written in Scala source notation with `_=`) must be read from the compiled
class; this document does not assume its JVM spelling.

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
| Getter/setter JVM names, descriptors, flags, backing storage, and slot layout | JVM-only ABI projection in `dotty-jvm` | Derive from the pinned compiler oracle and canonical source metadata. Keep JVM names out of frontend contracts. The exact projection is pending the oracle output above. |
| Encoding and verification of generated members | `dotty-classfile` | Receive already validated field/method plans; keep byte-level encoding separate from source semantics. |
| Source call-site support where current Typer behavior changes | Separate frontend/Typer issue | This audit records current successes and does not authorize Typer changes or accessor-specific call lowering. |

Before publishing any future class bytes, the projection must validate the
owner, source-to-derived parameter binding, field/parameter type agreement,
mutability, visibility, and explicit source order. A malformed or stale
identity must be a typed rejection. Generated JVM signatures must be checked
for collisions against user-declared and other generated members; the policy
must reject ambiguity before artifact publication rather than silently
shadowing either member. Until the oracle fixes the exact ABI, no accessor
classfile is an accepted backend output.

## Scope matrix

| Shape | Audit result / disposition |
| --- | --- |
| `Measurement(val id: Int, var value: Double)` | Source identity and source selections characterized. Exact JVM ABI awaits pinned compile and `javap`. |
| `Mixed(val enabled: Boolean, var count: Long)` | Source identity and category-two type identity characterized. Exact JVM slots and class-file layout await oracle output. |
| `Ref(val item: Measurement)` | Source identity and referenced class type characterized. Exact reference descriptor and storage flags await oracle output. |
| Plain constructor parameter | Existing distinct class-owned and constructor-owned parameter identities remain; it does not gain an accessor by implication. |
| Immutable `val` | A setter is not part of the source contract. Exact generated getter/storage details await oracle output. |
| `private[this]`, class-body public fields, `lazy`, visibility variants | Deferred; no ABI claim is made by this audit. |
| Secondary, defaulted, multiple-clause, or `using` constructors | Deferred. |
| Trait, case, generic, or externally compiled classes; inheritance bridges | Deferred. |
| Malformed owner/derived IDs, stale typed IDs, generated signature collisions | Future backend projection must reject before bytes are published; codegen is not implemented here. |
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
