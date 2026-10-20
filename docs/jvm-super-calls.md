# Parameterized superclass constructors and explicit `super` calls

This is an audit of the current source-to-JVM pipeline and a contract for the
next bounded implementation steps. Source compatibility targets Scala 3.9.0
revision `777528f19a58e794c9954a42f433373472ec57f8`; verifier rules refer to
Java SE 21. This document does not claim that parameterized parent calls or
explicit `super` method calls are implemented.

## Reproduce the current behavior

The characterization tests exercise the real parser, namer, typer, and codegen
pipeline:

```text
cargo test -p dotty --test jvm_source_compilation parameterized_parent_constructor --locked
cargo test -p dotty --test jvm_source_compilation super_method_source_is_rejected_by_typer_before_jvm_emission --locked
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

`SourceTyper::complete_class` currently calls `project_parent_type` for each
parent. That projection strips a parent `Apply` down to its function and
projects the `New` type. Consequently `ClassInfo.parents` contains the parent
type (`TypeRef(Base)`), not constructor expressions. Neither the parent
application nor its argument is mapped in `SourceTypedIndex`. A parent type
edge and a constructor invocation are different facts; do not put argument
trees into `ClassInfo.parents`.

The current codegen accepts only the accessible public no-argument direct
parent constructor. The parameterized source above reaches codegen but fails
with `JvmCodegenError::InvalidJvmSuperclass` and
`JvmSuperclassIssue::ParentConstructorUnavailable`; no class files are
published. This is later than parent-argument typing, which is currently
absent.

For all characterized source forms below, parsing accepts a method selection,
but source typing fails first with
`TyperError::UnsupportedExpression { expression_kind: "expression or declaration tree", .. }`.
No JVM output is published. Current cases include unqualified
`super.value()` and `super.increment()`, qualified `super[Base].value()`,
`Base.super.value()`, and `super.value()` in a class with no declared parent.
The same generic Typer error is not yet a sufficiently precise long-term
diagnostic for unsupported `super` forms.

The untyped AST for the supported spelling `super.value()` is a selection
whose qualifier is `TreeKind::Super { qual: This(None), mix: None }` and whose
name is `value`; an application adds an `Apply` with the argument list. The
parser also retains the qualifier of `super[Base]` in `mix`, and the outer
qualifier in `Base.super`. The current typer does not produce a typed
`Type::SuperType` or selected member for these trees.

## Target semantic and bytecode contracts

Future typing of an unqualified direct-parent call should retain canonical
identity, not only a member name. For `Derived extends Base` with
`super.value()`, the intended typed relation is:

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
owner and signature. Resolve it in the typer against the direct parent and
retain the exact selected symbol through lowering. A string lookup in the JVM
adapter is not proof of the source selection.

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

The current `dotty-classfile` validated-constructor path requires the first
two instructions to be exactly `aload_0; invokespecial DirectParent.<init>()V`.
It checks direct ownership, `<init>`, no arguments, and void return, then
analyzes the remainder as post-super code. The general `InvokeSpecial` flow
currently routes to allocation-constructor handling, which expects a pending
`new` object and `<init>`. It does not model the distinct
`UninitializedThis` transition for a parameterized parent call.

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

The initial explicit-super slice should accept only an unqualified
`super.method(args)` whose target is a public, monomorphic, concrete,
ordinary instance method declared by the one planned direct parent. It must
preserve the exact typed target symbol, JVM name, descriptor, visibility, and
argument order. The JVM adapter should represent this as a distinct
`SpecialSuper` invocation kind (or a separate checked target), emit
`aload_0`, the arguments, then one `invokespecial DirectParent.method(...)`.

The narrow slice rejects qualified `super[Base]`, `Base.super`, trait/mixin
selection, private or inaccessible methods, abstract methods, static methods,
methods with unsupported/generic or erased signatures, inherited declarations
whose owner is not the direct parent, and any target that is missing or
ambiguous. These should become specific typed errors before class publication.

Constructor and ordinary method uses of `invokespecial` are separate verifier
cases. A constructor invocation consumes `UninitializedThis` and performs the
initialization transition. An ordinary direct-super method invocation occurs
only after `this` is initialized and consumes an ordinary initialized receiver
plus the selected method's arguments. `dotty-classfile` must validate that the
method reference is non-constructor, names the direct parent, has an exact
descriptor, and receives compatible initialized values. Do not route it
through pending-`new` constructor handling.

With `ACC_SUPER`, JVMS 21 §6.5 `invokespecial` selects the method starting at
the current class's direct superclass when the symbolic reference names a
superclass and the resolved method is not `<init>`. Constructor invocation is
explicitly excluded from that selection rule. The first slice intentionally
uses the one direct parent as both checked symbolic owner and selection start;
trait and qualified-super dispatch need separate semantics. See [JVMS 21
§6.5 `invokespecial`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.invokespecial).

Method descriptors encode the formal arguments and return type, not the
implicit receiver. `<init>` must have a `void` result. The verifier checks
invocation argument values against this descriptor. See [JVMS 21
§4.3.3](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.3).

Ordinary calls continue to use `invokevirtual` and the declaring method's
`Methodref` owner so dynamic dispatch remains intact. Existing no-argument
super-constructor chains and ordinary virtual calls are the compatibility
baseline for all follow-up work.

## Characterization matrix and follow-up ownership

| Source shape | Current observed result | Required boundary / next owner |
| --- | --- | --- |
| `class Derived(start: Int) extends Base(start)` | Parent `Apply` and `start` are preserved by the parser; typer stores only `TypeRef(Base)` in `ClassInfo.parents` and indexes neither argument; codegen rejects unavailable `()V` constructor and publishes no classes | Type and retain parent argument trees, keeping `ClassInfo.parents` type-only — [#1369](https://github.com/scytrowski/dotty-rs/issues/1369); validate pre-super initialization state and descriptor in class files — [#1370](https://github.com/scytrowski/dotty-rs/issues/1370); lower the proven bounded source forms — [#1371](https://github.com/scytrowski/dotty-rs/issues/1371) |
| `super.value()` targeting direct `Base.value` | Parser preserves `Super` selection; typer returns generic `UnsupportedExpression`; no class file is published | Resolve the canonical direct-parent method symbol — [#1372](https://github.com/scytrowski/dotty-rs/issues/1372); separately validate initialized-receiver `invokespecial` — [#1373](https://github.com/scytrowski/dotty-rs/issues/1373); lower only a checked target as special dispatch — [#1374](https://github.com/scytrowski/dotty-rs/issues/1374) |
| Parameterized parent plus explicit direct-super calls, run on the JVM | Not implemented; no output path exists for the required combination | Source-to-classfile-to-JDK integration and runtime behavior — [#1375](https://github.com/scytrowski/dotty-rs/issues/1375) |
| `extends Base` with accessible `Base.<init>()V` | Supported and verified by existing source-backed regression | Preserve direct-parent ownership and post-super analysis in `dotty-jvm` and `dotty-classfile` |
| `base.value()` or inherited `derived.value()` | Supported virtual calls use the selected declaration's owner and `invokevirtual` | Preserve current dynamic-dispatch and `Methodref` behavior |
| Qualified/mixin super, no direct parent, inaccessible/abstract/unsupported selected method | Currently rejected in typer with generic `UnsupportedExpression`; no output | Replace with explicit typed errors while keeping these shapes rejected in the initial slice — #1372 and #1374 |

The seven follow-up issues are deliberately separate: source typing proof,
verifier support, and lowering can be reviewed and tested at their owning
boundaries. End-to-end coverage belongs after those contracts exist. Do not
add permanent ignored or expected-failure feature tests as a substitute for
implemented behavior.
