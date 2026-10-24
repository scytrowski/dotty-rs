# JVM class-body `val` / `var` audit

This audit records the Scala 3.9.0 class-file ABI for ordinary class-body
`val` and `var` declarations and the supported source pipeline in `dotty-rs`.
The backend emits the bounded accessor ABI and lowers the typed source forms
listed below; this does not claim full Scala ABI compatibility or change Typer
semantics.

The Scala reference is pinned to
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8)
(reported by the local compiler as `3.9.0-bin-SNAPSHOT-git-777528f`). Oracle
classes were inspected with `javap -c -v -p`; `javac` and `java -Xverify:all`
were from JDK 21. The local compiler's default output is class-file major 61;
that target version is distinct from the JDK used to inspect and verify it.

## Reproduction

The oracle checkout was clean at the pinned revision. Its exact compiler path
was `dist/target/universal/stage/bin/scalac`; `scalac -version` reported
`3.9.0-bin-SNAPSHOT-git-777528f`. Compile the source shapes below into separate
output directories so duplicate class names cannot mask one another:

```sh
SCALAC=<scala39-checkout>/dist/target/universal/stage/bin/scalac
"$SCALAC" -version
"$SCALAC" -d <body-classes> \
  BodyOnly.scala BodyOnlyParens.scala Counter.scala Reader.scala
"$SCALAC" -d <kinds-classes> FieldKinds.scala
"$SCALAC" -d <plain-parameter-classes> PlainParam.scala
"$SCALAC" -d <init-classes> InitReads.scala
"$SCALAC" -d <private-classes> PrivateBody.scala
"$SCALAC" -d <inheritance-classes> Inheritance.scala
javap -classpath <body-classes> -c -v -p BodyOnly
javap -classpath <body-classes> -c -v -p BodyOnlyParens
javap -classpath <body-classes> -c -v -p Counter
javap -classpath <body-classes> -c -v -p Reader
java -Xverify:all -classpath <body-classes> VerifyAudit
java -Xverify:all -classpath <kinds-classes> FieldKindsHarness
java -Xverify:all -classpath <init-classes> InitReadsHarness
```

The three harnesses above were compiled into their corresponding output
directories with JDK 21 `javac -classpath <classes> -d <classes>
<harness>.java`. `PlainParam.scala`, `Inheritance.scala`, and the private and
collision examples were also compiled with the pinned `scalac` into separate
directories; collision sources were expected to fail.

Compile the negative duplicate and accessor-collision examples independently;
each pinned `scalac` invocation exits with status 1 and reports E120.

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
  val fromId: Int = id
  def snapshot(): Long = current
  def snapshotThis(): Long = this.current
```

`javap` reports private-final `id:I` and `initial:I`, private `current:J`,
public getters `id()I`, `initial()I`, and `current()J`, and public
`current_$eq(J)V`. The setters for `Long` and `Double` use category-two
parameter slots (`lload_1` / `dload_1`) and `max_locals` includes both slots.
The source methods `snapshot()J` and `snapshotThis()J` invoke the getter
`Counter.current()J`. The body initializer for `fromId` reads constructor
parameter local 1 after the superclass call; it does not create a field or
getter for the plain constructor formal. A separate `class PlainParam(x: Int)`
with `val copy: Int = x` likewise emits only `copy:I` and `copy()I`, and reads
`x` directly from the constructor parameter slot.

```scala
class PlainParam(x: Int):
  val copy: Int = x
```

```scala
class Reader:
  def read(c: Counter): Long = c.current
  def write(c: Counter, n: Long): Unit = c.current = n
```

In `Reader`, `read(Counter)J` invokes `Counter.current()J` and
`write(Counter,long)V` invokes `Counter.current_$eq(J)V`; neither accesses the
backing field directly. This is the positive Scala oracle fixture compiled by
the reproduction command above.

The Rust Typer characterization separately checks the immutable assignment
`c.seed = n` and reports `AssignmentTargetImmutable`. That negative case is not
part of the Scala oracle fixture or its successful `scalac` invocation.

A separate `FieldKinds` oracle covered every planned JVM value descriptor:

```scala
class Planned(val id: Int)
class FieldKinds:
  val intValue: Int = 1
  var boolValue: Boolean = true
  var longValue: Long = 9000000000L
  var floatValue: Float = 1.5f
  var doubleValue: Double = 2.5
  val reference: Planned = new Planned(7)
```

The backing fields were private with descriptors `I`, `Z`, `J`, `F`, `D`, and
`LPlanned;`; immutable `intValue` and `reference` fields had `ACC_FINAL`.
Public methods used the corresponding getter descriptors `()I`, `()Z`, `()J`,
`()F`, `()D`, and `()LPlanned;`. Mutable members had public setters with
descriptors `(Z)V`, `(J)V`, `(F)V`, and `(D)V`. A JDK 21 harness verified the
getters, setters, reference getter, and the `PlainParam` example under
`-Xverify:all`.

## Initialization and verification

For both body-only forms, `<init>()V` invokes `Object.<init>()V` first, then
stores `seed`, `active`, and `reading` in source order. In `Counter.<init>(I)V`,
Scala first stores the constructor accessor `id` with `aload_0`, `iload_1`, and
`putfield Counter.id:I`; it then invokes `Object.<init>()V`, and initializes
`initial` and `current` in body order. Thus the accessor store uses
`UninitializedThis` before the superclass call, while the ordinary body stores
follow the call. The JDK 21 verifier and the Java harness accepted these
classes with `-Xverify:all` and checked the getter/setter behavior.

The pinned compiler also accepted earlier, self, and forward reads:

```scala
class InitReads:
  val earlier: Int = 1
  val copied: Int = earlier
  val self: Int = this.self
  val forward: Int = later
  val later: Int = 2
```

All five declarations become private-final fields with public getters. The
constructor emits getter calls for `earlier`, `self`, and `later` while
initializing `copied`, `self`, and `forward`. A JDK 21 `-Xverify:all` harness
observed `1, 1, 0, 0, 2` from those getters, respectively; accepted typing does
not make self or forward reads observe the later assigned values.

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

class ConstructorGetterCollision(val seed: Int):
  def seed(): Int = 2

class ConstructorSetterCollision(var active: Boolean):
  def active_=(value: Boolean): Unit = ()

class Duplicate:
  val same: Int = 1
  val same: Int = 2
```

Both fail with naming error E120. A backend must validate generated
getter/setter signatures against user members before publishing bytes.
The pinned compiler also rejects duplicate body field names and the same
getter/setter collisions when the conflicting accessor originates from a
constructor `val`/`var` parameter. The Rust Namer currently accepts duplicate
same-name body fields as distinct `Field` symbols; the existing
`duplicate_source_fields_are_not_yet_rejected_by_frontend` test records that
frontend limitation. This audit adds no speculative collision behavior to
production code.

A minimal inheritance compile emitted `Parent.current()I` and
`Parent.current_$eq(I)V` on the declaring class. `Child.read()I` invoked the
symbolic method reference `Child.current:()I`, which resolves to the inherited
getter; `Child` declared no field or accessor of its own. This pins only the
observed binary owner for that Scala shape. Rust accessor inheritance, override
validation, bridges, and broader inherited ABI behavior remain deferred.

```scala
class Parent:
  var current: Int = 1
class Child extends Parent:
  def read(): Int = current
```

## Current `dotty-rs` characterization

[`jvm_stateful_class_contract.rs`](../tests/jvm_stateful_class_contract.rs)
now exercises the real lexer, parser, namer, and `SourceTyper` on the body-only,
mixed `Counter`, and `Reader` examples. Body declarations map to canonical
class-owned `SymbolKind::Field` IDs, retain public visibility and `MUTABLE`
flags, preserve template order, and keep their typed initializer roots in the
source-to-typed index. The test asserts `definition_of` provenance, completed
primitive `TypeId`s, exact RHS `TreeId<Typed>` mappings and type targets. The
`fromId` initializer resolves to the constructor-owned derived `Parameter`
symbol, distinct from the canonical class-owned constructor-accessor `Field`.
Unqualified and `this.current` reads, `c.current`, and the selected left side
of `c.current = n` all resolve to the canonical body `Field` symbol; the
source-to-typed map retains the original RHS and assignment-left identities.
Assignment to immutable `c.seed` fails with `AssignmentTargetImmutable`
and publishes no typed RHS. A separate test types earlier, self, and forward
body-field initializer references to their exact field symbols. These are
Typer facts only; they do not implement Scala's generated getter/setter calls
or prove runtime initialization safety.

[`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs) builds
`PlainPublic` plans only from unqualified `ValDef`s in the exact source
template body, after checking canonical `SourceSemanticIndex::definition_of`
provenance and the exact source-to-typed RHS binding. The plan carries an
immutable `JvmPublicBodyFieldSourceProof` checked against the source AST,
semantic index, typed index, and caller-owned store. It records mutability and
template order; `JvmCompilationUnit` checks both against the canonical public
`Field` plans and rejects reordered public body fields. Metadata projects
private backing-field flags and origin-aware getter/setter signatures for
proven body fields. Issues #1582 and #1583 emit one public getter per body
accessor and one public setter per mutable body accessor through the shared
accessor bytecode path. Immutable fields have no setter.
The proof also checks the typed RHS node's source position against the exact
untyped RHS, so a caller-created source-to-typed map cannot substitute another
initializer. The source adapter collects fields for both
body-only/no-argument and parameterized primary-constructor shapes. Issue
#1581 lowers these exact typed initializers after the direct-super call; issue
#1582 emits one getter per checked body accessor and #1583 emits one setter per
checked mutable body accessor through the same bytecode path used by
constructor accessors. Setter signatures are collision-checked before class
file emission.

Body-bearing zero-argument classes retain the `ImplicitNoArg` source shape and
carry their checked field plans through metadata validation and emission. Their
backing fields initialize after the direct-super call; approved public body
accessors receive public getters and mutable body accessors receive public
setters. #1584 lowers supported typed reads and assignments through those
getters and setters, while preserving the original canonical field symbols
and source-to-typed mappings. Generated getter and setter signatures are
checked against each other and planned source methods before class-file
emission.

The frontend preserves the `private[this]` qualifier in the source AST, but
the shared symbol visibility is `Private`, just as it is for plain `private`;
consumers must inspect the source declaration metadata if that distinction is
needed. Existing characterization is in
`non_private_visibility_forms_are_canonicalized_and_self_forward_reads_type`
and `field_metadata_distinguishes_private_this_from_plain_private` in the
stateful contract test.

## Scope and follow-up

This audit covers ordinary initialized class-body primitive fields, exact
planned reference fields, their getter/setter ABI, and the listed
accessor/visibility forms. It does
not establish behavior for lazy or abstract members, overrides/bridges,
traits, generic fields, broader inheritance, or combined Scala ABI/TASTy
emission. Public class-body getters and setters have JDK
verification/execution coverage. For the accepted typed selections and
assignments, the backend calls the checked public getter/setter methods;
private source fields retain their existing direct access path. Mutable-local
receiver stability, accessor inheritance, traits, generics, and Scala
ABI/TASTy integration remain outside this subset.
