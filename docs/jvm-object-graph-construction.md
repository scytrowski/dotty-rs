# JVM object graph construction audit

Issue #1492 records the current source, backend, and verifier boundaries for
constructing and passing more than one object. It is an audit increment: it
does not enable any new bytecode form. The source-backed regression is
`object_graph_construction_audit_keeps_frontend_identity_before_emission_boundary`
in [`tests/jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs),
using [`ObjectGraphConstruction.scala`](../tests/fixtures/jvm/ObjectGraphConstruction.scala).

## What the source frontend proves

The fixture goes through the real lexer, parser, namer, and source typer in one
semantic store. An observer immediately before JVM emission checks canonical
class, constructor, method, parameter, and local symbols. In particular:

- `new Holder(new Counter(seed))` is represented as nested constructor
  `Apply(Select(New(...), <init>), ...)` trees. The selected constructor
  references are the canonical `Holder.<init>` and `Counter.<init>` symbols,
  and the expression types resolve to the corresponding planned classes.
- `new Holder(makeCounter(seed))` keeps `Runner.makeCounter` as the selected
  method. The typer does not rewrite a call result into a constructor or lose
  its method identity.
- `new Holder(counter)` refers to the exact source method parameter symbol.
  An immutable local alias and `new Holder(current)` over a mutable local both
  retain their exact local symbols. The
  constructor's source class parameter and its derived constructor-owned
  formal are distinct symbols with distinct owners.
- `new Link(this)` in an instance method is accepted for the exact enclosing
  class type. Its emitted constructor call is verified by JDK 21. The source
  fixture does not generalize this observation to another receiver class or
  to a constructor argument whose type cannot be proven exactly.
- `new Holder(null)` retains the `Null` constant and `Definitions.null_type`;
  the frontend does not guess a `Holder`-specific null type.
- `new OwnedHolder(seed).get()` and `create(seed).value().get()` retain their
  exact selected constructor and method symbols through the complete call
  chains.
- The `OwnedHolder.inner` initializer stays a typed `new Counter(seed)` tree
  and uses the exact `OwnedHolder.<init>` parameter symbol.

The nested-allocation, reference-argument, field-initializer, and chained-call
shapes all pass source typing in this fixture. Their present failures occur
after the frontend boundary where noted below. Wrong constructor arity and
primitive type cases remain ordinary Typer errors. That distinction matters
for follow-up work: #1494 owns Typer call contracts only if an actual typing
defect is found; the observed emission gaps belong to later JVM stages.

## Observed pipeline matrix

The source test records the current first failure and checks that the failed
batch publishes no class files.

| Source form | Source typing | First current JVM failure | Follow-up |
| --- | --- | --- | --- |
| `new Holder(counter)` for an exact reference parameter | Succeeds; argument keeps the canonical parameter symbol | Supported and emitted | Existing support |
| `new Holder(current)` for an exact immutable reference local | Succeeds; argument keeps the canonical local symbol | Supported and emitted | Existing support |
| `new Holder(current)` for an exact mutable reference local | Succeeds; argument keeps the canonical local symbol | `Codegen(JvmConstructorCallArgumentTypeMismatch { .. })` | #1495 |
| `new Holder(saved)` for an exact current-instance reference field | Succeeds; argument keeps the canonical field symbol | `Codegen(JvmConstructorCallArgumentTypeMismatch { .. })` | #1495 |
| `new Link(this)` in a method of `Link` | Succeeds with exact `Link` type | Supported and emitted; `Link.<init>(LLink;)V` verifies under JDK 21 | Existing support |
| `new Holder(new Counter(seed))` | Succeeds; both constructor selections remain canonical | `Codegen(JvmConstructorCallArgumentTypeMismatch { .. })` | #1496 |
| `new Holder(makeCounter(seed))` | Succeeds; selected factory method remains canonical | `Codegen(JvmConstructorCallArgumentTypeMismatch { .. })` | #1495 |
| `new Holder(null)` | Succeeds; null stays `Definitions.null_type` | `Codegen(JvmConstructorCallArgumentTypeMismatch { .. })` | #1495, with explicit null contract |
| `private val inner: Counter = new Counter(seed)` | Succeeds; initializer is a typed constructor application | `Codegen(UnsupportedJvmReferenceFieldInitializer { .. })` | #1497 |
| `new Counter(if flag then 1 else 2)` as a constructor argument | Succeeds with a typed `If` argument | `Codegen(UnsupportedJvmConstructorArgument { .. })` | #1495 |
| Wrong constructor arity or primitive argument type | Rejected by the typer with its arity/type error | No JVM plan | Existing Typer behavior |

The current mismatch error is the first surfaced error for the source forms
above; it does not establish that each argument has a separate typing defect.
The observer proves that source typing completed before this codegen error.
Tests should keep asserting the observed typed failure until a follow-up
changes the contract to a more specific error.

## Already-supported receiver forms

Object creation and later calls have a bounded working path that this audit
preserves:

- `new Counter(seed).get()` initializes the allocation before invoking
  `Counter.get`.
- `makeCounter(seed).get()` calls the factory, then invokes `Counter.get` on
  its initialized result.
- `create(counter).value().get()` chains calls on initialized results.
- An exact reference parameter can be passed to `new Holder(counter)`.

The positive source regression decodes the emitted owners and descriptors and,
when JDK validation is enabled, compiles and runs a Java harness with
`java -Xverify:all`. A new allocation nested inside constructor arguments or a
reference-valued constructor argument remains outside the current emission
contract. A successful selection or a typed result alone does not prove that
the emitter can represent it.

## Scala 3.9.0 and JDK 21 oracle

The fixture was also compiled with the repository's Scala 3.9.0 compiler
artifacts and run on JDK 21 with `-Xverify:all`. The `Runner.create` bytecode
uses this valid sequence:

```text
new Holder
dup
new Counter
dup
iload_1
invokespecial Counter.<init>(I)V
invokespecial Holder.<init>(LCounter;)V
areturn
```

The factory-argument form is also valid on the JVM while the outer allocation
is still uninitialized:

```text
new Holder
dup
aload_0
iload_1
invokevirtual Runner.makeCounter(I)LCounter;
invokespecial Holder.<init>(LCounter;)V
areturn
```

The returned factory value is initialized and is passed as the constructor
argument; the lower stack entry remains the separate uninitialized
`Holder` allocation. Scala's `OwnedHolder.<init>` calls `Object.<init>` first,
then executes `new Counter`, its constructor, and one `putfield inner:LCounter;`.
These observations define useful target bytecode shapes. The repository
regression does not claim that dotty-rs emits them today.

The oracle source is the `ObjectGraphConstruction.scala` fixture plus the
same-class `Link(this)` case. The Scala compiler reports version 3.9.0 and its
JAR manifest has `Git-Hash: 777528f`, matching the prefix of the repository's
pinned revision `777528f19a58e794c9954a42f433373472ec57f8`; the oracle run uses
JDK 21. The
checked-in regression covers dotty-rs's current typed trees and actual
emission boundary; it does not make local Scala/JDK installations a required
test dependency. The source fixture is compiled and executed separately
against the Scala oracle during this audit. The Java oracle harness completed
all assertions under `java -Xverify:all`, including nested allocation,
factory and local constructor arguments, field initialization from `new`,
chained calls, and same-class `this`.

## Two-live-allocation verifier contract

JVMS 21 assigns each `new` instruction its own `uninitialized(offset)` type.
`dup` creates another alias of that same allocation identity. The current
class-aware CFG analyzer stores only one
`pending_allocation: Option<UninitializedAllocation { site, class }>` in a
frame. Its existing regression rejects a second `new` while the first
allocation is pending, reporting the second instruction and first allocation
site. That single slot cannot validate the nested sequence above.

A verifier that supports the sequence needs allocation-site identities in
each local and operand-stack value, rather than one frame-wide pending slot.
For the nested example its state progression is:

1. `new Holder` creates `uninitialized(holder_site, Holder)`; `dup` copies that
   identity.
2. `new Counter` creates a distinct
   `uninitialized(counter_site, Counter)`; `dup` copies that identity.
3. `Counter.<init>` must consume the `counter_site` receiver, match `Counter`,
   and on normal completion rewrite only remaining aliases of
   `counter_site` to initialized `Object(Counter)`.
4. `Holder.<init>(LCounter;)V` must consume the still-uninitialized
   `holder_site` receiver and an initialized exact `Object(Counter)` argument;
   on normal completion it rewrites only remaining aliases of `holder_site`.

For the factory form, a verifier must permit a call on an already initialized
receiver and accept its initialized exact result while the distinct
`Holder` allocation remains pending below it on the operand stack. It must
still reject using, storing, returning, branching on, or passing an
uninitialized allocation as an ordinary value. Constructor invocation must
match both the receiver allocation site and its class. Failed or exceptional
constructor flow must not mark aliases initialized.

The state must also define joins and exception edges, multiple aliases in
locals and stack positions, repeated allocations, and `UninitializedThis`
inside constructors. In particular, a frame-wide list of pending class names
is insufficient: identity is the bytecode allocation site, and each
`invokespecial <init>` initializes only aliases of its receiver site. #1493
owns this classfile verifier extension. #1496 and #1495 own the emitter and
argument integration that can consume the verifier contract.

## Follow-up ownership

| Issue | Scope |
| --- | --- |
| #1493 | Classfile verifier state for multiple live allocation sites, exact alias initialization, and invalid-use checks |
| #1494 | Typer call-selection contracts, only where source typing needs a correction |
| #1495 | Exact reference constructor arguments, including mutable locals, call results, and the explicit null boundary; preserve the already-working parameter, immutable-local, and same-class `this` forms |
| #1496 | Nested `new` lowering with separate allocation identities |
| #1497 | Reference fields initialized from `new` in constructors |
| #1498 | Chained call receivers and preserving the already-working receiver forms |
| #1499 | End-to-end integration after the lower-level contracts are ready |

This audit changes tests and documentation only. Emission remains gated by the
existing typed errors until those follow-ups implement their own bounded
contracts and regressions.

Related contracts: [object creation](jvm-object-creation-contract.md),
[reference fields and constructor arguments](jvm-reference-fields-constructors.md),
[reference CFG](jvm-reference-cfg-contract.md), and
[JVM backend](jvm-backend.md).
