# Inherited JVM accessor audit

This document records the bounded Scala 3.9.0 audit for inherited class-body
and primary-constructor `val`/`var` accessors requested by
[#1608](https://github.com/scytrowski/dotty-rs/issues/1608). It does not enable
accessor inheritance in the Rust backend. The existing typed rejection remains
in force until the source Typer can represent inherited selections and the JVM
projection can validate inherited generated members together with ordinary
methods.

## Pinned oracle and fixtures

The oracle checkout is Scala 3.9.0 revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
The compiled compiler identifies itself as `3.9.0-bin-SNAPSHOT-git-777528f`.
`tests/fixtures/jvm/InheritedAccessors.scala` is compiled with that checkout's
`dist/target/universal/stage/bin/scalac`; `javap -c -p` was used on the
resulting `Parent`, `Child`, `Leaf`, `AccessorParent`, `AccessorChild`, and
`Reader` classes. A Java harness calls and mutates both body and constructor
accessors using `java -Xverify:all` on JDK 21. The harness completed
successfully.

The issue's constructor example omitted the required `count` argument to
`AccessorParent`. The fixture supplies both arguments:
`AccessorChild extends AccessorParent(7, 9L)`.

## Observed Scala ABI

| Shape | Scala 3.9.0 class-file observation |
| --- | --- |
| `Parent` body `val id: Long` | One private-final `id:J` field, one public `id()J` getter. |
| `Parent` body `var current: Int` | One private `current:I` field, public `current()I` getter and `current_$eq(I)V` setter. |
| `Child extends Parent` | Zero fields; no copied getter or setter methods. Only its constructor and explicitly declared source methods are present. The constructor calls `Parent.<init>()V`. |
| Body accessor source call with static receiver `Child`, `Leaf`, or `Parent` | `Reader` emits `invokevirtual Child.current:()I`, `Leaf.current:()I`, or `Parent.current:()I`, respectively. A `Child` method reading its own inherited `current` also uses symbolic owner `Child`; its setter uses `Child.current_$eq:(I)V`. The symbolic owner follows the typed receiver class, while the physical method remains declared in `Parent`. |
| `AccessorParent(val token: Int, var count: Long)` | Private-final `token:I` and private `count:J` fields; public getters `token()I`, `count()J`, and setter `count_$eq(J)V`. The constructor descriptor is `(IJ)V`. |
| `AccessorChild extends AccessorParent(7, 9L)` | Zero fields and only its constructor method. It invokes the immediate `AccessorParent.<init>(IJ)V`; no child accessor methods are copied. Reader call sites use symbolic owners `AccessorChild` for `token`, `count`, and `count_$eq`. |
| `Leaf extends Child` | Zero fields and only its constructor. It invokes `Child.<init>()V`; inherited methods remain inherited through both edges. |

The compiled oracle output was class-file major version 61 because no `-release`
or target override was supplied. That is independent of this repository's
backend target (major version 65). Scala's constructor bytecode initializes
class-body fields before invoking `Object.<init>` in the observed fixture;
that observation does not authorize changing the Rust backend's existing
verified initialization policy.

## Override and boundary observations

Each case below was compiled independently with the pinned compiler. The
descriptions are the compiler outcomes, not a proposed Rust acceptance policy.

| Case | Oracle outcome |
| --- | --- |
| Child generated `var current` collides with a final parent `def current()` | Rejected: the variable cannot override the final method. |
| Child `override val current` over parent `var current` | Rejected: a value cannot override a mutable variable. |
| Child `override var current` over parent `val current` | Rejected: the overriding variable must be a stable immutable value. |
| Child `override def current` over parent `val current` | Rejected: the method is not a stable immutable value. |
| Override changes `current` from `Int` to `Long` | Rejected as an incompatible override. |
| Child method reads an inherited `private val` | Rejected as inaccessible. |
| Child method reads an inherited `protected val` | Compiles. |
| Concrete child omits an abstract parent `val` implementation | Rejected: the child must be abstract or implement the accessor. |
| Trait body accessor inherited by a class | Compiles. Trait storage/default-method ABI is outside this class-only audit. |
| Generic constructor accessor `Parent[A](val value: A)` inherited as `Parent[Int]` | Compiles. Erasure, bridge and generic signature policy remain outside this bounded backend support. |
| Covariant `override val current: String` over parent `val current: Object` | Compiles. The overriding child declares private-final `current:Ljava/lang/String;`, `current()Ljava/lang/String;`, and a public bridge `current()Ljava/lang/Object;`. This is an explicit override, not a copied inherited accessor. |
| Four-class chain with an inherited body accessor | Compiles. The Rust backend's separate supported hierarchy depth remains bounded. |

## Rust source and backend boundary

The focused source semantic regression
`inherited_accessor_sources_keep_declaring_identity_and_pin_typer_blocker` verifies
that body-field symbols are canonical `Field` symbols owned by `Parent`,
constructor accessor fields are owned by `AccessorParent`, and constructor
formals are distinct `Parameter` symbols owned by that class's constructor.
The child class declaration scope contains no copied `current` or `token`
field. These are session-local identities; their numeric arena slots are not
part of the contract.

The current Typer's first blocker is earlier than typed selection identity:
typing `Child.read` for the inherited body field returns
`TyperError::TermNameNotFound` at the source tree for `current`. The regression
pins this exact typed error rather than manufacturing a `TermRef`. Therefore
the required source-to-typed `TermRefTarget::Symbol(parent_field)` relation is
not established by today's frontend. Constructor-accessor inherited reads
remain similarly untyped. This issue does not change the Typer.

When source assembly contains inherited accessors but no unsupported source
selection, the batch reaches `JvmCompilationUnit::metadata` and fails with
`UnsupportedJvmAccessorInheritance { class: Child, parent: Parent }`. The
source-to-JVM regression asserts this backend failure and that no class files
are published. The metadata gate runs before inherited ordinary-method
collision validation, preventing a generated getter or setter from bypassing
final-method checks. The current unit also rejects a subclass that declares
accessors when its parent does not, and rejects constructor-accessor subclasses
before class metadata is projected.

## Ownership and prerequisites for future support

Keep three identities separate:

1. The Scala source selection resolves to the canonical declaring `Field`
   `SymbolId`; a child must not receive a copied source field or fabricated
   source `Method`.
2. The JVM invocation's symbolic owner follows the static type of the
   receiver (`Child`, `Leaf`, or `Parent` in the oracle), not necessarily the
   class that physically declares the accessor method.
3. Physical generated method metadata is derived from the declaring field and
   its role: getter, or mutable setter. Its JVM name, descriptor, visibility,
   and origin remain in `dotty-jvm`.

Future enablement requires the Typer to resolve supported inherited member
selections to the canonical declaring field while preserving source-to-typed
bindings. The backend must then project effective accessor methods for each
planned class without inserting generated source symbols. It must validate
exact JVM descriptors and receiver owners; detect getter/setter collisions
with user methods and inherited final methods; preserve immutable/mutable
override rules; and reject unsupported bridges, traits, external ancestors,
visibility forms, or malformed hierarchy edges before publishing artifacts.
No child fields or methods should be copied merely to satisfy symbolic
resolution.

The current narrow prerequisite for a future backend increment is an
Object-rooted planned class chain within the existing depth limit, with a
validated immediate-parent constructor call and no accessor override or
generated/user-member collision. Those restrictions are only a candidate
bounded acceptance surface: source typing and pinned oracle evidence must be
extended before implementation. The related hierarchy and dispatch work is
tracked separately in #1609–#1613.
