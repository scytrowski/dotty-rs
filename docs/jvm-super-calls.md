# Parameterized superclass constructors and explicit `super` calls

This is an audit of the source-to-JVM pipeline and a contract for the next
bounded implementation steps. Source compatibility targets Scala 3.9.0
revision `777528f19a58e794c9954a42f433373472ec57f8`; verifier rules refer to
Java SE 21. Issues #1369 and #1370 establish source typing and verifier
contracts for a narrow parent application. Issue #1371 adds bounded JVM
constructor lowering. Issue #1372 adds Typer support for an unqualified
`super` selection targeting one public, concrete, monomorphic method declared
directly by the current source class's sole non-Object class parent. JVM
emission of that typed shape is covered by subsequent bounded backend steps,
including the combined three-level constructor and direct-super regression.

## Reproduce the current behavior

The characterization tests exercise the real parser, namer, typer, and codegen
pipeline:

```text
cargo test -p dotty-typer --test typed_expressions parent_constructor --locked
cargo test -p dotty-typer --test typed_expressions unsupported_parent_constructor --locked
cargo test -p dotty-typer --test typed_expressions super_calls --locked
cargo test -p dotty --test jvm_source_compilation parameterized_parent_constructor --locked
cargo test -p dotty --test jvm_source_compilation typed_super_method_source_remains_rejected_before_jvm_emission --locked
```

For `class Derived(start: Int) extends Base(start)`, the parser preserves the
parent application in `Template.parents` as:

```text
Apply(
  function = Select(New(tpt = Ident("Base")), name = Ident("<init>")),
  args = [Ident("start")],
)
```

The constructor formal is a parameter owned by the primary constructor. The
Namer can resolve its declaration with
`SourceSemanticIndex::derived_symbol_at(constructor_symbol, source, param_tree)`.
That constructor-owned parameter identity is the source to use for a future
parameter load; it must not be replaced by a class accessor or guessed from
the parameter's text.

`SourceTyper::complete_class` projects each parent type independently from its
constructor application. Consequently `ClassInfo.parents` contains the parent
type (`TypeRef(Base)`), not constructor expressions. For a supported plain
parent application, the typer separately resolves and types the constructor
call under the child primary constructor's expression context. The application
and each original argument tree are mapped in `SourceTypedIndex`; argument
order and source `TreeId`s remain available to a later adapter. The selected
typed constructor reference carries the canonical constructor `SymbolId`, and
an argument such as `start` carries the `TermRefTarget::Symbol` of the
constructor-owned `Derived.<init>.start` parameter, not its class accessor.
The call must select the parent's primary constructor with one plain,
uncurried, non-generic clause; a private or unsupported signature fails during
typing. A parent type edge and a constructor invocation are different facts;
do not put argument trees into `ClassInfo.parents`.

For the supported source subset, the frontend adapter builds an explicit
`JvmDirectSuperConstructorPlan` from the original `Template.parents`
application and exact same-session `SourceTypedIndex` roots. The plan preserves
the canonical parent class and primary-constructor symbols and argument order.
`dotty-jvm` validates that proof against the child template, typed application,
class hierarchy, and constructor metadata. It lowers only Int/Boolean literals
and exact child primary-constructor parameter identifiers of primitive or
planned reference types. Emission places `aload_0`, source-ordered arguments,
and the parent `<init>` call before child field initializers. Unsupported
argument forms and unplanned reference types return typed errors without
publishing class files. The adapter must not retype expressions or infer
constructor targets from names or `ClassInfo.parents`.

Unqualified `super.value()` and `super.add(4)` now type through the real
source pipeline. The typed `Super` carries `Type::SuperType` with
`ThisType(Derived)` and the direct parent's canonical `TypeRef`; its `Select`
carries a `TermRefTarget::Symbol` for the declaration found on that parent or
its validated ancestors. The ordinary application path uses this selected
method type directly, without treating the call as virtual lookup on the
current class. The source-to-typed index retains the `Super`, `Select`, and
`Apply` roots.

The typer rejects trait-qualified or outer-qualified super, use outside an
ordinary source class instance method, no direct non-Object parent, multiple
parents, generic/non-plain parent types, missing inherited declarations,
ambiguous overload buckets, non-method terms, and private, abstract, static, or extension
methods. Failures are typed (`UnsupportedSuper`, `MemberNotFound`, or
`OverloadedSelectionDeferred`) and never rewrite the expression as a virtual
call. For the accepted unqualified shape, `dotty-jvm` checks the exact
`SuperType`, direct-parent hierarchy edge, canonical selected method symbol,
effective method on the parent, and method plan before emitting `aload_0`,
ordered arguments, and `invokespecial` with the immediate parent as owner.
Ordinary selected calls still emit
`invokevirtual`.

The untyped AST for the supported spelling `super.value()` is a selection
whose qualifier is `TreeKind::Super { qual: This(None), mix: None }` and whose
name is `value`; an application adds an `Apply` with the argument list. The
parser also retains the qualifier of `super[Base]` in `mix`, and the outer
qualifier in `Base.super`. The typer produces a `Type::SuperType` and canonical
selected member for the supported unqualified super form; the other
qualifiers remain rejected.

## Target semantic and bytecode contracts

Typing of an unqualified super call retains canonical identity, not
only a member name. For `Derived extends Base` with `super.value()`, the typed
relation is:

```text
Super(qual = This(None), mix = None)
  : Type::SuperType {
      this_type = ThisType(Derived),
      super_type = TypeRef(Base),
    }
Select(super, value)
  : TermRef { prefix = SuperType, target = Symbol(Base.value) }
Apply(select, [])
  : exact result type of the canonical Base.value symbol
```

`Base.value` above means its canonical `SymbolId`, including its declaring
owner and signature. Resolve it in the typer starting at the direct parent and
retain the exact selected symbol through lowering. The JVM symbolic owner for
`invokespecial` is the immediate parent, allowing `ACC_SUPER` selection to
choose a method inherited through it. A string lookup in the JVM adapter is
not proof of the source selection.

### Constructor arguments before superclass initialization

The parent `Apply` and every argument tree need a typed association that keeps
their original `TreeId`s and exact argument order. The frontend owns this proof:
the typer has the constructor signature, source types, and constructor-owned
parameter identities. The JVM adapter consumes the proof; it must not re-type
source or infer values from `ClassInfo.parents`.

The first implementation slice is deliberately small:

| Argument form | Frontend typing | Initial JVM lowering slice |
| --- | --- | --- |
| `Int` or `Boolean` literal | Type against the matching parent constructor formal | Push the matching JVM constant |
| Exact primary-constructor parameter | Require a constructor-owned parameter with a compatible primitive or planned reference type | Load that formal from its verified local slot |
| One ordinary argument clause | Check arity and each formal/actual pair in source order | Emit one descriptor-matching argument sequence |
| Other expression, side effect, field read, `this`, branch, nested allocation, or multiple clauses | May receive ordinary frontend typing when supported | Reject as unsupported before publishing class output |

This backend subset is narrower than what the typer may understand. In
particular, successful source typing is not permission to emit an arbitrary
expression before `super.<init>`.

The bounded constructor code shape is:

```text
aload_0
<safe parent arguments in source order>
invokespecial DirectParent.<init>(descriptor)V
<ordinary field initializers and constructor body>
```

Argument loads must use the child constructor's actual local layout. Long and
double values occupy two local/stack slots if they are ever admitted; they are
outside the first primitive slice. `max_stack` must cover the receiver plus
the peak argument stack. The parent constructor descriptor includes only
formal arguments (for example `(I)V`); `this` is implicit and is not written
into the descriptor.

The current `dotty-classfile` validated-constructor path requires `aload_0`,
then allows only a bounded sequence of integer constants and `iload`/`aload`
from initialized constructor parameters, followed by exactly one direct-parent
`invokespecial <init>`. It checks owner, void return, descriptor arity and
categories, local initialization, and exact reference assignability before
analyzing the remaining body as post-super code. `aload_0` cannot be loaded as
an argument, and field access, calls, allocation, stores, arithmetic, and
branches are rejected before initialization. The ordinary `InvokeSpecial`
flow after the prologue still handles constructor calls on pending `new`
allocations; this separate prologue performs the `UninitializedThis`
transition after the direct-parent call succeeds normally.

Under JVMS 21 §4.10.1, local 0 begins as `UninitializedThis` with the
`flagThisUninit` flag. Arguments must be evaluated without exposing that value
or allowing it to escape. A normal completion of the legal direct-parent
`<init>` invocation initializes the value and clears the flag; exceptional
completion leaves the uninitialized object unusable. Returning while the flag
remains set is invalid. The initial analyzer should continue rejecting
branches across this state transition rather than pretending it can merge
pre-super and post-super states. See [JVMS 21 §4.10.1.9](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.9).

The verifier must validate the actual constant-pool method reference owner as
the direct parent, the name `<init>`, the exact descriptor, and a `void`
return. It consumes argument values from the operand stack and then the
uninitialized receiver. Keep the current no-argument constructor validation
and post-super analysis intact as a regression baseline.

### Explicit direct-super methods

The explicit-super slice accepts only an unqualified
`super.method(args)` whose target is a public, monomorphic, concrete,
ordinary instance method reachable through the planned direct parent. It must
preserve the exact typed target symbol, JVM name, descriptor, visibility, and
argument order. The JVM adapter should represent this as a distinct
`SpecialSuper` invocation kind (or a separate checked target), emit
`aload_0`, the arguments, then one `invokespecial DirectParent.method(...)`.

The narrow slice rejects qualified `super[Base]`, `Base.super`, trait/mixin
selection, private or inaccessible methods, abstract methods, static methods,
methods with unsupported/generic or erased signatures, declarations not
reachable through the direct parent, and any target that is missing or
ambiguous. These should become specific typed errors before class publication.

Constructor and ordinary method uses of `invokespecial` are separate verifier
cases. A constructor invocation consumes an uninitialized allocation and
performs its allocation-specific initialization transition. An ordinary
direct-super method invocation occurs only with an initialized receiver whose
exact class is the emitting class. `dotty-classfile` validates that the
`Methodref` is non-constructor, names the proved direct parent, has a supported
descriptor, and receives compatible initialized arguments. The class-file
builder also requires `ACC_SUPER`. This transfer does not use pending-`new`
constructor handling or rewrite allocation aliases.

With `ACC_SUPER`, JVMS 21 §6.5 `invokespecial` selects the method starting at
the current class's direct superclass when the symbolic reference names a
superclass and the resolved method is not `<init>`. Constructor invocation is
explicitly excluded from that selection rule. The adapter uses the direct
parent as the symbolic owner even when the selected canonical declaration is
further up the validated ancestry; the JVM then applies the specified
selection rule. Trait and qualified-super dispatch need separate semantics. See [JVMS 21
§6.5 `invokespecial`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.invokespecial).

Method descriptors encode the formal arguments and return type, not the
implicit receiver. `<init>` must have a `void` result. The verifier checks
invocation argument values against this descriptor. See [JVMS 21
§4.3.3](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.3).

Ordinary calls continue to use `invokevirtual` and the declaring method's
`Methodref` owner so dynamic dispatch remains intact. Existing no-argument
super-constructor chains and ordinary virtual calls are the compatibility
baseline for all follow-up work.

The source fixture `ThreeLevelConstructorSuper.scala` exercises the combined
two-edge shape. `ChainMiddle(seed)` forwards its own constructor parameter to
`ChainBase(seed)`, and `ChainLeaf(seed)` independently forwards its parameter
to `ChainMiddle(seed)`. Each constructor performs one call to its immediate
parent before its own initializers. `ChainMiddle.value` calls
`ChainBase.value` with `invokespecial`; `ChainLeaf.value` and
`ChainLeaf.parentValue` call `ChainMiddle.value` with `invokespecial`. The
inherited `ChainBase.doubled` method retains `invokevirtual ChainBase.value`,
so a `ChainLeaf` receiver dispatches to `ChainLeaf.value`. The JDK-gated test
checks owners with `javap -c -v` and executes the chain under
`java -Xverify:all`, including a negative seed.

## Characterization matrix and follow-up ownership

| Source shape | Current observed result | Required boundary / next owner |
| --- | --- | --- |
| `class Derived(start: Int) extends Base(start)` | Supported for one public planned parent primary constructor and one plain clause containing Int/Boolean literals or exact child primary-constructor parameters. The emitted prologue loads `this`, then source-ordered arguments, then invokes the canonical parent `<init>` descriptor before field initializers; `ClassInfo.parents` remains `TypeRef(Base)`. Primitive, literal, and planned reference argument paths are covered by `ParameterizedSuper.scala` | Preserve exact frontend source/typed identity proof, hierarchy validation, and verifier behavior; broaden argument forms only in a separately scoped issue |
| `super.value()` targeting a method inherited through the direct parent | Typer produces a source-mapped `Super → Select → Apply` targeting the canonical declaring symbol; `dotty-jvm` verifies the ancestor path and emits `invokespecial` with the immediate parent as symbolic owner; JDK21 CI verifies execution | Preserve canonical symbol and verifier proofs in the combined source-to-JVM integration from [#1375](https://github.com/scytrowski/dotty-rs/issues/1375) |
| `super.add(n)`, Boolean/reference results, or a branched super call | The planned direct-parent method, exact descriptor, argument order, and result are checked; branches receive classfile stack-map frames and the mandatory JDK gate verifies execution | Preserve the approved direct-parent public instance-method subset; broader signatures and call shapes need a scoped issue |
| Parameterized parent plus explicit direct-super calls in one source-to-JVM run | `SuperCalls.scala` covers the two-class combination; `ThreeLevelConstructorSuper.scala` verifies Base/Middle/Leaf constructor forwarding, both immediate-parent super owners, and inherited virtual dispatch in the full three-class chain | Preserve canonical per-edge and per-method identities; the JDK 21+ gate inspects `javap -c -v` and runs `java -Xverify:all` — [#1407](https://github.com/scytrowski/dotty-rs/issues/1407) |
| `extends Base` with accessible `Base.<init>()V` | Supported and verified by existing source-backed regression | Preserve direct-parent ownership and post-super analysis in `dotty-jvm` and `dotty-classfile` |
| `base.value()` or inherited `derived.value()` | Supported virtual calls use the selected declaration's owner and `invokevirtual` | Preserve current dynamic-dispatch and `Methodref` behavior |
| Qualified/mixin super, no direct parent, inaccessible/abstract/unsupported selected method | Rejected in typer or the JVM adapter with typed shape, owner, or metadata errors; no class artifact is published | Keep these shapes rejected in the bounded slice — [#1374](https://github.com/scytrowski/dotty-rs/issues/1374) |

Source typing proof, verifier support, and bounded lowering remain separate
contracts at their owning boundaries. The integration fixtures exercise the
two- and three-class parameterized-constructor/super-call combinations end to
end; unsupported forms stay explicitly rejected until a scoped follow-up adds
them.

## Combined parameterized construction and super dispatch

`SuperCalls.scala` keeps `Derived(start: Int) extends Base(start)` and the real
`Super` selections in the same source batch. The source integration test
`super_calls_fixture_combines_parameterized_construction_and_special_dispatch`
checks major version 65, exact parent/field metadata, a single `(I)V`
constructor on Base and Derived, no phantom `()V` constructors, and constructor
stack/local bounds. `Derived.<init>` forwards local 1 to `Base.<init>(I)V`;
`Base.<init>` initializes private mutable `current:I` after Object initialization.

Decoded Methodrefs and separate `javap` class disassemblies distinguish
`invokespecial Base.value/increment` from ordinary `invokevirtual` calls in
Base.doubled and Runner.evaluate. The JDK harness checks 20, 22, 10, 11, 12,
zero, and signed negative inputs with `java -Xverify:all`. A separate source
extension checks canonical selected method SymbolIds and exact reference frames
at ordinary branch target PCs. The unchanged fixture itself is branchless and
requires no StackMapTable. Both full fixture builds use independent frontend
sessions and output directories and must produce identical ordered bytes.

Negative integration cases assert exact errors and an empty output batch for
private/wrong-arity/wrong-type parent calls, pre-super `this`, branch/new/effectful
parent arguments, and unsupported or inaccessible super selections. The existing
`parameterized_parent_constructor_arguments_keep_their_source_mappings` test
also rejects borrowing a typed parent application/argument from another class.
Focused classfile tests reject wrong constructor/method owners, pre-super
allocation or effects, an initialized receiver reused as a constructor target,
virtual constructor invocation, and branched constructors at their actual
validation layers. These checks preserve the bounded initialization contract.

Nested allocation arguments, arbitrary effectful pre-super evaluation, branched
constructors, qualified/trait super, hierarchy-aware joins, and Scala
ABI/TASTy emission remain deferred. Transitive reference assignability is
limited to two validated same-unit ancestry edges; see the
[single-inheritance contract](jvm-single-inheritance.md).
