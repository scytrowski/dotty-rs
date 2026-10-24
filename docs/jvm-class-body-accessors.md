# JVM class-body `val` / `var` audit

This audit records the Scala 3.9.0 class-file ABI for ordinary class-body
`val` and `var` declarations, and the current source-pipeline boundary in
`dotty-rs`. It is characterization only: it makes no production backend or
Typer changes and does not claim that Rust-emitted classes match Scala's
accessor ABI.

The Scala reference is pinned to
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8)
(reported by the local compiler as `3.9.0-bin-SNAPSHOT-git-777528f`). Oracle
classes were inspected with `javap -c -v -p`; `javac` and `java -Xverify:all`
were from JDK 21. The local compiler's default output is class-file major 61;
that target version is distinct from the JDK used to inspect and verify it.

## Reproduction

Compile the following independent source shapes with the pinned checkout's
`dist/target/universal/stage/bin/scalac`, then inspect each class with:

```sh
<scala39-checkout>/dist/target/universal/stage/bin/scalac -version
<scala39-checkout>/dist/target/universal/stage/bin/scalac -d <classes> \
  BodyOnly.scala BodyOnlyParens.scala Counter.scala Reader.scala
javap -classpath <classes> -c -v -p BodyOnly
javap -classpath <classes> -c -v -p BodyOnlyParens
javap -classpath <classes> -c -v -p Counter
javap -classpath <classes> -c -v -p Reader
```

The body-only forms were:

```scala
class BodyOnly:
  val seed: Int = 10
  var active: Boolean = true
  var reading: Double = 1.5
```

and the same body after `class BodyOnlyParens():`. Both emit the same member
shape: `seed:I` is `private final`, `active:Z` and `reading:D` are `private`;
all three getters are public (`seed()I`, `active()Z`, `reading()D`) and both
mutable members have public setters (`active_$eq(Z)V`, `reading_$eq(D)V`). The
body source order is retained in fields, accessors, and initialization.

The mixed accessor/body shape was:

```scala
class Counter(val id: Int):
  val initial: Int = 10
  var current: Long = 0L
  def snapshot(): Long = current
```

`javap` reports private-final `id:I` and `initial:I`, private `current:J`,
public getters `id()I`, `initial()I`, and `current()J`, and public
`current_$eq(J)V`. The source method `snapshot()J` invokes the getter
`Counter.current()J`. In `Reader`, `read(Counter)J` invokes that getter and
`write(Counter,long)V` invokes `Counter.current_$eq(J)V`; it does not access
the backing field directly.

```scala
class Reader:
  def read(c: Counter): Long = c.current
  def write(c: Counter, n: Long): Unit = c.current = n
```

## Initialization and verification

For both body-only forms, `<init>()V` invokes `Object.<init>()V` first, then
stores `seed`, `active`, and `reading` in source order. In `Counter.<init>(I)V`,
Scala first stores the constructor accessor `id` with `aload_0`, `iload_1`, and
`putfield Counter.id:I`; it then invokes `Object.<init>()V`, and initializes
`initial` and `current` in body order. Thus the accessor store uses
`UninitializedThis` before the superclass call, while the ordinary body stores
follow the call. The JDK 21 verifier and the Java harness accepted these
classes with `-Xverify:all` and checked the getter/setter behavior.

This constructor ordering is an observed Scala 3.9 ABI detail, not an
instruction-order requirement for the Rust backend. The existing Rust backend
requires the direct-super call before its accessor stores; that difference is
also recorded in [`jvm-stateful-class-contract.md`](jvm-stateful-class-contract.md).

## Visibility and naming boundaries

A separate pinned compile forced reads and writes of `private val`,
`private var`, and `private[this] val`:

```scala
class PrivateBody:
  private val hidden: Int = 2
  private var hiddenVar: Long = 3L
  private[this] val onlyThis: Boolean = true
  def expose(): Int = hidden
  def change(n: Long): Unit = hiddenVar = n
  def exposeThis(): Boolean = onlyThis
```

The emitted fields were private; vals were final, vars were not. Reads and
writes in class methods used direct `getfield` / `putfield` operations, and
`javap` showed no accessor methods for these private members. Scala 3.9 also
warned that `private[this]`
is ignored and deprecated, and emitted `onlyThis` with the same private-final
field flags as the plain private val. Do not infer a distinct JVM visibility
from the source qualifier.

Accessor names participate in Scala's member-name collision checks. The
pinned compiler rejected these forms:

```scala
class GetterCollision:
  val seed: Int = 1
  def seed(): Int = 2

class SetterCollision:
  var active: Boolean = true
  def active_=(value: Boolean): Unit = ()
```

Both fail with naming error E120. A backend must validate generated
getter/setter signatures against user members before publishing bytes.

## Current `dotty-rs` characterization

[`jvm_stateful_class_contract.rs`](../tests/jvm_stateful_class_contract.rs)
now exercises the real lexer, parser, namer, and `SourceTyper` on the body-only,
mixed `Counter`, and `Reader` examples. Body declarations map to canonical
class-owned `SymbolKind::Field` IDs, retain public visibility and `MUTABLE`
flags, preserve template order, and keep their typed initializer roots in the
source-to-typed index. `Counter.snapshot`, `Reader.read`, and the target of
`Reader.write` resolve to the canonical `current` field identity in the current
Typer model. This is source-semantic evidence only; it does not implement
Scala's generated getter/setter calls.

[`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs) records the
first current end-to-end failure for body-only classes, explicit empty
constructors, and mixed constructor/body fields: `dotty-jvm` returns
`UnsupportedJvmVisibility` for the public body field during plan validation.
The test also asserts no class artifact is published. The backend's accepted
field path is restricted to private storage and its generated accessor path
currently covers primary-constructor accessors. Public class-body accessor
emission, including backing-field flags, source-order initialization, getter
and setter bytecode, call-site lowering, and collision checks, remains
unsupported.

The frontend preserves the `private[this]` qualifier in the source AST, but
the shared symbol visibility is `Private`, just as it is for plain `private`;
consumers must inspect the source declaration metadata if that distinction is
needed. Existing characterization is in
`non_private_visibility_forms_are_canonicalized_and_self_forward_reads_type`
and `field_metadata_distinguishes_private_this_from_plain_private` in the
stateful contract test.

## Scope and follow-up

This audit covers ordinary initialized class-body primitive fields and the
listed accessor/visibility forms. It does not establish behavior for lazy or
abstract members, overrides/bridges, traits, generic or reference-valued body
fields, Java callers beyond the shown ABI, or combined Scala ABI/TASTy
emission. It makes no changes to production code. Future emission work needs
separate validated accessor metadata for body declarations, exact source and
typed identities, ordered initializer lowering, generated-signature
collision validation, and JDK verification/execution coverage.
