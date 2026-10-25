# Inherited JVM accessor audit

This document records the bounded Scala 3.9.0 audit for inherited class-body
and primary-constructor `val`/`var` accessors requested by
[#1608](https://github.com/scytrowski/dotty-rs/issues/1608). The backend now
projects inherited generated accessors into a read-only metadata view and
emits the proven Object-rooted subset in #1611. Unit metadata validates
inherited method/accessor collisions before returning that view.

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
| `AccessorChild extends AccessorParent(7, 9L)` | Zero fields and four methods: its constructor plus the explicitly declared `readToken`, `readCount`, and `writeCount`. It invokes the immediate `AccessorParent.<init>(IJ)V`; no generated getter or setter is copied into the child. Reader call sites use symbolic owners `AccessorChild` for `token`, `count`, and `count_$eq`. |
| `Leaf extends Child` | Zero fields and only its constructor. It invokes `Child.<init>()V`; inherited methods remain inherited through both edges. |

The compiled oracle output was class-file major version 61 because no `-release`
or target override was supplied. That is independent of this repository's
backend target (major version 65). In `Parent.<init>`, Scala invokes
`Object.<init>` at bytecode offset 1, then stores `id` at offset 8 and `current`
at offset 13. In `AccessorParent.<init>(IJ)V`, it stores `token` at offset 2
and `count` at offset 7, then invokes `Object.<init>` at offset 11. These are
distinct oracle sequences: class-body initialization follows the super call,
while constructor-parameter accessor storage precedes it. They do not change
the Rust backend's existing verified initialization policy.

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
`inherited_accessor_sources_keep_declaring_identity_and_pin_typer_blocker`
verifies that body-field symbols are canonical `Field` symbols owned by
`Parent`, constructor accessor fields are owned by `AccessorParent`, and
constructor formals are distinct `Parameter` symbols owned by that class's
constructor. The child class declaration scope contains no copied `current`
or `token` field. These are session-local identities; their numeric arena
slots are not part of the contract.

The Typer resolves unqualified inherited public body fields as well as
`this.current`, `child.current`, and `parent.current` to a
`TermRefTarget::Symbol` naming the canonical `Parent.current` field. The
unqualified form carries the enclosing `Child` `ThisType` prefix, while an
explicit selection retains the exact receiver type as its prefix. The narrow
Typer fallback runs after direct lexical declarations and explicit/wildcard
imports, so local and imported declarations keep precedence. It uses the
existing bounded member lookup and does not copy fields into child scopes. Qualified constructor and
body accessor selections retain their existing canonical
`AccessorParent.token`/`count` identities; mutable receiver instability and
setter typing remain governed by the existing checks.

The source-to-class regression covers `current`, `this.current`, and `id` in
`Child`, plus `child.current`, `child.id`, and `parent.current` in `Reader`.
Getter calls use the canonical field's validated getter metadata and the
effective dispatch entry for the static receiver: `Child.current:()I`,
`Child.id:()J`, or `Parent.current:()I`. The emitted bodies contain no direct
backing-field reads, and `Child` receives neither copied fields nor generated
getter methods. A Java harness exercises the inherited reads and setter with
JDK 21 `-Xverify:all`.

Typed assignments to inherited mutable accessors lower through the validated
setter dispatch entry. The source receiver is evaluated once before the RHS;
both are saved in category-correct temporary locals and the assignment returns
`Unit`. The `invokevirtual` symbolic owner follows the receiver's static class,
while dispatch retains the Parent field and setter as the declaring origin.
`Child` and `Reader` therefore invoke `Child.current_$eq:(J)V` for a Child
receiver without accessing Parent's private field or declaring a copied setter.
Boolean setter assignments use the same path with the exact `(Z)V` descriptor.
The source integration harness verifies the order and single evaluation of a
call receiver and RHS, then reads the updated value through the inherited
getter under `java -Xverify:all`. Assignments to inherited `val`s remain
rejected by the Typer.

When source assembly contains inherited accessors but no unsupported source
selection, `JvmCompilationUnit::metadata` returns an effective accessor view
for each planned receiver. Each getter or setter entry records the receiver,
the physical declaring class, the canonical source field, the role, exact JVM
name and descriptor, and access flags. Ordinary source methods remain in
`dispatch_methods`; generated accessors do not receive fake method symbols or
replace ordinary entries. Same-signature ordinary and generated entries remain
separate collision evidence. Inherited entries do not appear in the child's
declared `accessors()` list.

The metadata projection is read-only. Before returning, it compares declared
source methods and generated getter/setter signatures across every validated
ancestor edge. A collision reports `InheritedJvmAccessorCollision` with a
reason (`SourceMethodRedefinesInheritedAccessor`,
`GeneratedAccessorRedefinesInheritedMethod`,
`GeneratedAccessorRedefinesInheritedFinalMethod`, or
`GeneratedAccessorRedefinesInheritedAccessor`, or
`IncompatibleInheritedAccessorResult`), both canonical origins
(`Method(SymbolId)` or `Accessor { field, role }`), the conflicting classes,
and the JVM name and descriptors. A result-only mismatch is rejected when
the parameter descriptors match, even if the full JVM descriptors differ;
different parameter descriptors remain valid overloads. Ordinary
method-to-method overrides continue through their existing validator;
accessor overrides remain unsupported. Unplanned inherited final methods
and other unplanned non-private source methods retain a conservative name-only
check for generated child accessors and report no descriptors. Emission accepts
one or two planned user-class edges rooted at `java/lang/Object` when every
edge has a validated accessible direct-parent constructor, collisions pass
preflight, and descendants declare no new accessors. Parent backing fields and
generated methods remain physically declared only in the parent; descendants
carry the inherited identity only in `dispatch_accessors()`. The typed
`UnsupportedJvmAccessorInheritance` error remains for descendants that add
their own accessor storage above an inherited accessor. Constructor and
body-field `var` tests cover the `Parent → Child → Leaf` projection,
`TokenParent → TokenChild` with a checked constructor argument, `val Long`
metadata, and unrelated same-spelled source fields.

For this collision audit, a source method named `current_=` projects to the
Scala 3 JVM setter name `current_$eq` before exact descriptor comparison. This
lets ordinary setter-shaped methods and generated `var` setters share the same
binary identity check without introducing synthetic source method symbols.

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

The current emission subset accepts an Object-rooted planned class chain within
the existing depth limit, with a validated immediate-parent constructor call
and no accessor override or generated/user-member collision. It does not copy
child fields or methods to satisfy symbolic resolution. Descendants with new
accessors, bridges, traits, external ancestors, inaccessible constructors, or
malformed hierarchy edges remain rejected before publication. Qualified
inherited getter and mutable setter lowering are implemented in #1612 and
#1613. Runtime dispatch and JDK verification are covered by #1615.
