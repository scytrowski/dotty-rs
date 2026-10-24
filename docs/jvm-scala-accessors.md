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

## Source identities, Typer readiness, and call-site boundary

`tests/jvm_stateful_class_contract.rs` exercises the real scanner, parser,
namer, and Typer on a `Counter(val id: Int, var count: Long)` plus a separate
`Reader`. It records the typed tree mapping for every source expression and
checks that selections remain `TermRefTarget::Symbol` references to the
original class-owned `Field`. Receiver expressions retain their own source
mapping and span; receiver references remain the original `this`, parameter,
or local expression. No getter or setter `Method` symbol is added to source
lookup. Successful `c.count = next` remains a source `Assign` whose lvalue is
the canonical field, whose RHS widens to `Long`, and whose result type is
canonical `Unit`.

The source matrix was also compiled with the pinned Scala 3.9.0 compiler built
from revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
The local launcher reported `3.9.0-bin-SNAPSHOT-git-777528f` on JDK 21.0.8.
The positive `Counter`/`Reader` fixture compiled successfully and `javap`
showed calls to `Counter.id()I`, `Counter.count()J`, and
`Counter.count_$eq(J)V` for `this`, parameter, stable local `val`, and mutable
local `var` receivers, including `val c` reads and writes of `count`. Negative
oracle controls reported `Reassignment to val id`,
`Found: Boolean / Required: Long`, and `value count is not a member of Other`
for the corresponding invalid sources.

| Source form | Pinned Scala 3.9.0 | Current Typer | Identity / first failure | Backend status |
| --- | --- | --- | --- | --- |
| `this.id`, `this.count` | Accepted; getter calls | Accepted | Canonical `Counter.id` / `Counter.count` fields | Accessor metadata exists; call-site dispatch is not integrated. |
| `c.id`, `c.count` | Accepted; getter calls | Accepted | Canonical `Counter` fields, with typed parameter receiver | #1562 must lower through the getter metadata. |
| `this.count = next`, `c.count = next` | Accepted; setter call | Accepted | Canonical mutable `Counter.count`; typed assignment is `Unit`, RHS is `Long` | #1562 must dispatch through the setter metadata. |
| Unqualified `count` in `Counter` | Accepted | Accepted | `Ident` resolves to the same canonical `count` field | No source accessor method is synthesized. |
| `val c = new Counter(...); c.id` | Accepted | Accepted | Selection keeps the local receiver and canonical `Counter.id` field | Requires the same getter dispatch as parameter receivers. |
| `val c = new Counter(...); c.count` | Accepted; getter call | Accepted | Selection retains the exact mapped local receiver and canonical mutable `Counter.count` field | Requires getter dispatch through #1555 metadata. |
| `val c = new Counter(...); c.count = next` | Accepted; setter call | Accepted | Typed `Assign` keeps the local receiver and canonical `count` field; result is `Unit`, RHS is `Long` | Requires setter dispatch through #1555 metadata. |
| `var c = new Counter(...); c.id` | Accepted | Rejected | `UnstableSelectionPrefix` at `c.id`; nested mappings roll back | Do not include until a Typer-owned change supports it. |
| `this.id = 2`, `c.id = 2` | Rejected: `Reassignment to val id` | Rejected: `AssignmentTargetImmutable` for canonical `id` | Error points to the assignment; no typed mappings survive | No setter exists for `val`. |
| `c.count = true` | Rejected: Boolean required to be Long | Rejected: `ExpectedExpressionTypeMismatch` (Boolean / Long) | Error points to RHS; nested mappings roll back | Invalid typed shape never reaches the backend. |
| `other.count` where `other: Other` | Rejected: member is absent | Rejected: `MemberNotFound` on `Other` | Error points to the selection; nested mappings roll back | No same-spelled field guess is allowed. |

The call-site boundary is confirmed by code inspection: `dotty-jvm` currently
routes typed field selections and assignments through its planned field
read/write paths. Those paths require the field to be owned by the current
class and emit `getfield`/`putfield`; they do not resolve
`JvmAccessorMetadata` and emit the generated `invokevirtual` methods. The
metadata is sufficient input for that future lowering, but does not currently
make external or local source accessor calls compile. #1560 therefore proves
the stable source forms ready for the optional backend work in #1562; it does
not implement that lowering. The mutable-local selection remains a separate
Typer-owned follow-up if that source form is brought into scope.

Oracle reproduction, from the pinned checkout, uses
`dist/target/universal/stage/bin/scalac -version`, then compiles the issue's
unchanged `Counter`/`Reader` source with
`dist/target/universal/stage/bin/scalac -d <output> <source>`. Inspect the
successful output with `javap -classpath <output> -c -p Counter Reader`; compile
the invalid assignments and unrelated receiver as separate negative controls
to capture their first diagnostics. The focused Rust regressions assert
canonical `SourceTypedIndex` edges and rollback on those same invalid shapes.

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
| Getter/setter JVM names, descriptors, flags, backing storage, and slot layout | JVM-only ABI projection in `dotty-jvm` | Derive from the pinned compiler oracle and canonical source metadata. Keep JVM names out of frontend contracts. `JvmAccessorMetadata` records checked backing-field and getter / optional setter signatures with `JvmAccessorOrigin::ConstructorParameter` or `JvmAccessorOrigin::ClassBodyField`; it never fabricates a constructor formal for a body field or a source `Method`. |
| Constructor accessor stores | `dotty-jvm` constructor initializer and class emitter | Copy the exact constructor-owned formal into private storage after direct-super initialization and before ordinary class-body field initializers. Keep accessor stores separate from typed source RHS plans. |
| Encoding and verification of fields and bytecode | `dotty-classfile` | Receive validated field and instruction plans; keep byte-level encoding separate from source semantics. |
| Source call-site support where current Typer behavior changes | Separate backend/frontend issue | #1584 should validate constructor accessors through `accessor_bindings()` and body-field accessors through their canonical field plus source/typed-RHS proof. It must not invent a derived formal for body fields. This metadata increment does not enable call lowering or authorize Typer changes. |

Before publishing any future class bytes, the projection must validate the
owner, source-to-derived parameter binding, field/parameter type agreement,
mutability, visibility, and explicit source order. A malformed or stale
identity must be a typed rejection. Generated JVM signatures must be checked
for collisions against user-declared and other generated members; the policy
must reject ambiguity before artifact publication rather than silently
shadowing either member. Accessor metadata performs these checks. The emitter
uses constructor accessor metadata for checked backing fields, stores,
getters, and setters. #1581 also installs and initializes ordinary public
class-body backing fields in source order, including through a validated
implicit no-argument constructor. Body-field getter and setter method emission
and source call lowering remain later increments.

## Scope matrix

| Shape | Audit result / disposition |
| --- | --- |
| `Measurement(val id: Int, var value: Double)` | Emits private `id:I` and `value:D` storage, exactly one initialized store from slots 1 and 2–3 after `Object.<init>`, public `id()I` / `value()D` getters, and public `value_$eq(D)V`; `val id` has no setter. |
| `Mixed(val enabled: Boolean, var count: Long)` | Emits private `enabled:Z` and `count:J` storage, public `enabled()Z` / `count()J` getters and public `count_$eq(J)V`; the setter uses two local slots for its Long parameter. |
| `Ref(val item: Measurement)` | Emits private-final planned reference storage, copies the exact reference formal after direct-super initialization, and emits `item()LMeasurement;`. |
| Plain constructor parameter | Existing distinct class-owned and constructor-owned parameter identities remain; it does not gain an accessor by implication. |
| Immutable `val` | Oracle confirms private-final storage and public getter with no setter. |
| Ordinary class-body public `val`/`var` | Metadata projection is covered by #1580 using the #1578 Scala 3.9.0 ABI audit and #1579 exact source/RHS proof. Backing fields are private (final for `val`); getters are public and vars also have public `_$eq` setters. #1581 stores the exact typed RHS into that private storage after the direct-super call; body-field getter/setter methods and call lowering are still deferred. |
| `private[this]`, `lazy`, other visibility variants | Deferred; no ABI claim is made by this audit. |
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

The source-to-JVM acceptance regression
`constructor_accessor_source_batch_matches_the_jvm_abi_across_fresh_frontends`
extends this proof through the test source assembler. It compiles
`Measurement(val id: Int, var value: Double)`,
`Mixed(first: Int, val count: Long, var enabled: Boolean)`, and a mutable
`LongBox` in one genuine lexer/parser/namer/typer session, then repeats the
entire batch in a fresh semantic session and compares ordered class names and
all class bytes. Its source observer checks original class, field, and
constructor-formal IDs, owner, source ordinal, mutability, visibility, and
the absence of accessor metadata for the plain `first` formal. Decoded
classfiles and `javap -c -v -p` verify class version, constructor descriptors
and slots, private storage, exact getter/setter signatures, field references,
and bytecode. A strict JDK 21 Java harness calls the generated methods under
`java -Xverify:all`, mutates independent instances, checks `val` has no setter,
and exercises Double, Long, and Boolean values. The emitted constructor calls
`Object.<init>` before copying accessor values; this is the backend's verified
initialization order, distinct from Scala's constructor instruction order.
