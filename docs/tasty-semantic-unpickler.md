# TASTy semantic unpickler

Status (`crates/dotty-tasty-unpickler`):

- Milestone 1, semantic index and symbol entering: complete.
- Milestone 2a, core type identity and reference resolution: implemented
  (§4, "Types").
- Milestone 2b, semantic name resolution (canonical session identities, the
  package contract, the resolver boundary, name-based `TYPEREF`/`TERMREF`):
  implemented (§4, "Name-based references").
- Milestone 2c1, compositional non-binder types (`Applied`, `And`, `Or`,
  `SuperType`, `ByName`): implemented (§4, "Types").
- Milestone 2c2, the non-binder core-model gaps (`TYPEBOUNDS` as `Bounds` or
  `AliasingBounds`, `Flexible`, lossless constants and `CLASSconst`):
  implemented (§4, "Bounds, flexible and constant types").
- Milestone 3a, binder identity (`TYPELAMBDAtype`, `PARAMtype`): implemented
  (§4, "Binders").
- Milestone 3b, the other two binder forms (`METHODtype`, `POLYtype`) on the
  same machinery: implemented (§4, "Binders").
- Milestone 3c, binder rebinding and variance-bearing `TYPEBOUNDS`:
  implemented (§4, "Variance-bearing `TYPEBOUNDS`"). This closes the binder
  milestone.
- Milestone 4a, recursive and refined types (`REFINEDtype`, `RECtype`,
  `RECthis`): implemented (§4, "Recursive and refined types").
- Milestone 4b1, compact annotated types (`ANNOTATEDtype` whose annotation is a
  type): implemented (§4, "Annotated types").
- Milestone 4b2a, full annotation constructor applications (`APPLY`/`NEW`) with
  semantic literal arguments, and `MethodParam.erased`: implemented (§4,
  "Full annotation applications").
- Milestone 4b2b, `SHAREDterm` annotation roots: implemented (§4, "Shared
  annotation trees"). No term-tree identity was needed: the link is followed
  and the tree read again, as Dotty does.
- Milestone 4c1, owner-space references (`TYPEREFin`, unsigned `TERMREFin`):
  implemented (§4, "Owner-space references"). It decodes 0 real nodes from the
  corpora, because the declaring classes belong to other units; see the
  measurement in §8.
- Milestone 4c2, name-designated references for refined and recursive members:
  implemented (§4, "Name-designated references"). The real `C { type T1; type
  T2 = T1 }` decodes.
- Milestone 4d, `MATCHtype` / `MATCHCASEtype`, and the regular-TASTy
  type-language audit: implemented (§4, "Match types"; §8). Milestone 4 is
  closed; the next work is Milestone 5 (symbol completion).
- Milestone 5a, type-tree projection and simple symbol completion: implemented
  (§4, "Type trees and simple completion"; §8). `VALDEF`, `PARAM`, `TYPEPARAM`
  and non-opaque non-template `TYPEDEF` symbols can now be completed; methods,
  constructors and classes stay `Missing`.

Entered symbols start `SymbolInfo::Missing`; only an explicit
`complete_symbol` (the simple kinds above) changes that. Types are otherwise
decoded on request by address.

Target: Scala 3.9.0 / TASTy 28.9.0. The wire format is documented in
[`tasty-format-3.9.0.md`](tasty-format-3.9.0.md) and the decoding API in
[`tasty-api.md`](tasty-api.md); this document does not repeat either.

## 1. Boundary

```text
TASTy bytes
    -> dotty-tasty                (wire decoding, raw/structured AST, address index)
    -> dotty-tasty-unpickler      (this crate: TASTy semantics)
    -> dotty-core                 (SemanticStore: symbols, types, scopes, ...)
```

The unpickler interprets TASTy that is already decoded. It does not:

- decode the TASTy binary format, or duplicate `dotty-tasty` APIs;
- discover or read classpath entries (JAR/JMOD/directories) — that stays in
  `dotty-classloader`;
- parse Scala source, infer types, or re-run type checking;
- put TASTy-specific types into `dotty-core`, which stays format-agnostic;
- introduce a second semantic model: it fills the canonical `Symbol`, `Type`,
  `Scope` and `ClassInfo` values of `dotty-core`.

References to symbols defined outside the current TASTy unit go through the
`SymbolResolver` port of `dotty-core` (Milestone 2b defines the port; the
classloader implements it in Milestone 6). The unpickler never loads files
itself.

## 2. Multi-pass architecture

TASTy has forward references, shared nodes, recursive binders and mutually
referencing symbols, so the unpickler is not a `RawTree -> Tree<Typed>`
function. It follows an enter-before-complete model:

| Pass | Result | Milestone |
|------|--------|-----------|
| 1. Enter | symbols, owners, declaration scopes; `SymbolInfo::Missing` | 1 |
| 2a. Type identity | address-keyed `TypeId`s; references, `THIS`, `SHAREDtype` | 2a |
| 2b. Name resolution | name-based `TYPEREF`/`TERMREF` through the prefix scope and the `SymbolResolver` port | 2b |
| 2c1. Compound types | `Applied`, `And`, `Or`, `SuperType`, `ByName` | 2c1 |
| 2c2. Core-model gaps | `Bounds`, `AliasingBounds`, `Flexible`, lossless constants, `CLASSconst` | 2c2 |
| 3a. Binder identity | `TypeLambda`, `ParamRef`, with `TypeArena::reserve`/`fill` | 3a |
| 3b. Methodic binders | `Method`, `Poly` on the shared binder machinery; `PARAMtype` to all three binder kinds | 3b |
| 3c. Binder rebinding | `dotty_core::rebind_type_lambda`; variance-bearing `TYPEBOUNDS`; `declared_variance: Option<Variance>` | 3c |
| 4a. Refined and recursive | `Refined`, `Recursive`, `RecThis` | 4a |
| 4b1. Compact annotated types | `Annotated` with `Annotation::compact(ty)`; full trees deferred | 4b1 |
| 4b2a. Full annotation applications | `APPLY`/`NEW` annotations with `AnnotationArguments`; `MethodParam.erased` from `ErasedParam` | 4b2a |
| 4b2b. `SHAREDterm` annotations | `AstView::resolve_shared_term`; shared `APPLY`/`NEW` reuse the direct decoder | 4b2b |
| 4c1. `*REFin` | `TYPEREFin` / unsigned `TERMREFin` owner-space resolution; `MemberSpace` in `MemberRequest` | 4c1 |
| 4c2. Name-designated refs | `TypeRefTarget` / `TermRefTarget` (`Symbol \| Name`); `lookup_structural_member` | 4c2 |
| 4d. Match types | `MATCHtype` / `MATCHCASEtype` to `Match` / `MatchCase`; type-language audit | 4d |
| 5a. Simple completion | type-tree projection; `Complete` for `VALDEF`/`PARAM`/`TYPEPARAM`/plain `TYPEDEF`; stable-term prefixes | 5a |
| 3. Complete | `SymbolInfo::Complete(TypeId)` for methods, `ClassInfo`, annotations | 5b-5d |
| 4. Typed AST | `AstArena<Typed>`, rehydrated without type inference | 7 |

## 3. Identity invariant

TASTy AST addresses are semantic identity anchors:

```text
definition address -> exactly one SymbolId
shared type address -> exactly one TypeId
```

Direct references (`TYPEREFdirect`, `TERMREFdirect`, `TYPEREFsymbol`,
`TERMREFsymbol`) resolve through the semantic index by address, never by
rendering a name and searching for it. Names are used only where TASTy itself
encodes a target by name. The type map is address identity, not structural
interning: two separately written, equal type trees have different addresses
and may have different `TypeId`s. A `SHAREDtype` node is an indirection with
no `TypeId` of its own: it resolves to the `TypeId` of the node it names.

Every address the unpickler follows comes from untrusted bytes, so both passes
reach the AST through one `AstView` (`ast_view.rs`), whose `tree_at` accepts
an address only if it is the start of a visible indexed node. There is one
definition of a valid AST reference, and one bound (`MAX_SHARED_DEPTH`) on
chained `SHAREDtype` links.

Owner and scope membership stay distinct, as in `dotty-core`: a symbol's
`owner` says who semantically holds it, while a `Scope` says where it can be
found by name. Symbols have no `children` field.

## 4. Reading TASTy definitions

### Names

Name references — in AST payloads and inside composite name-table entries —
are zero-based indexes into the name table in Scala 3.9.0 compiler output
(`<init>` is entry 14 and the signed constructor entry says `original: 14`;
`java.lang.Object` is `Qualified { 17, 18 }` over entries 15/16/18), which is
what `dotty-tasty`'s `NameRef` means (it was one-based until issue #9 was
fixed). The unpickler reads names itself (`names.rs`) because it spells them
differently from `TastyFile::render_name`: a signed name reads as its original
name (overloads are distinguished by definition address, not by signature
text), where `render_name` reports it as unsupported.

### Definition shape

For `class Foo[A](val x: A) { def bar[B](b: B): A }` the compiler emits, per
absolute AST address: `PACKAGE`, then `TYPEDEF Foo` with a `TEMPLATE` whose
header holds the class `TYPEPARAM A` and the constructor `PARAM x`, and whose
stats hold `DEFDEF <init>` and `DEFDEF bar`. `<init>` and `bar` carry their
own `TYPEPARAM`/`PARAM` nodes, so the class type parameter appears twice, at
two addresses. Nested nodes report addresses relative to their payload; the
absolute address comes from `TastyFile::ast_address_index`.

A `val x` constructor parameter is recognised by the *absence* of a
`PRIVATE`/`LOCAL` tail — a plain constructor parameter is `private[this]`.
It is not tagged `FIELDACCESSOR`, so that tag alone does not find it.

### Modifier and kind mapping

| TASTy | dotty-core |
|-------|------------|
| `PRIVATE` / `PROTECTED` / none | `Visibility::Private` / `Protected` / `Public` |
| `ABSTRACT FINAL SEALED CASE IMPLICIT GIVEN LAZY MUTABLE INLINE TRANSPARENT OPAQUE EXTENSION STATIC SYNTHETIC ERASED OVERRIDE` | the same-named `SymbolFlags` bit |
| `TYPEDEF` + template | `Class`, `Trait` (`TRAIT`) or `ModuleClass` (`OBJECT`) |
| `TYPEDEF`, no template | `TypeAlias` (also abstract and opaque types) |
| `VALDEF` + `OBJECT` | `Object` |
| `VALDEF` in a class-like owner | `Field` (`var` adds `MUTABLE`) |
| `DEFDEF <init>` / other `DEFDEF` | `Constructor` / `Method` |
| template-header `PARAM`, not `private[this]` | `Field` |
| other term `PARAM` | `Parameter` |
| `TYPEPARAM` | `TypeParameter` |

Names are `Term` for packages, `VALDEF`, `DEFDEF` and `PARAM`, and `Type` for
`TYPEDEF` and `TYPEPARAM`, so a module's term and its module class never share
an identity.

Not mapped yet: `ENUM`, `ARTIFACT`, `INLINEPROXY`, `MACRO`, `EXPORTED`,
`OPEN`, `INFIX`, `INVISIBLE`, `TRACKED`, `INTO` (no core flag),
`COVARIANT`/`CONTRAVARIANT` (variance is set when type parameters are
completed), the accessor roles `FIELDACCESSOR`, `CASEACCESSOR`,
`PARAMSETTER`, `PARAMALIAS`, `HASDEFAULT`, `STABLE`, and annotations.

Qualified access maps to `Visibility::PrivateWithin(Q)` /
`ProtectedWithin(Q)`. The modifier's qualifier tree is one of `TYPEREFpkg`
(a package by name), `TYPEREFsymbol` (an enclosing definition, by address),
or a `SHAREDtype` link to either. The qualifier comes from untrusted input, so
it is validated:

- a `SHAREDtype` target must be the start of a visible AST node (checked
  against the global AST index before anything is decoded); an address inside
  another node's payload, or past the end of the section, is
  `InvalidReferenceTarget { from, to }`;
- the resolved qualifier must be a package, class, trait or object that
  encloses the qualified definition, or the definition itself
  (`class A { private[A] ... }`); anything else is
  `InvalidQualifier { definition }`;
- a package qualifier is found among the owners of the definition and is never
  entered, so an untrusted name cannot create package symbols.

The qualifier is resolved after the definition's own symbol exists, because it
may be the definition itself, and an enclosing definition is always entered
before its members. Any other qualifier shape is
`UnpickleError::UnsupportedQualifier`, and a `TYPEREFsymbol` address with no
entered symbol is `InvalidReferenceTarget`.
`private[this]` is an unqualified `Private` (with `LOCAL`).

### Failure is atomic

`enter_symbols` either enters the whole unit or changes nothing. On any error
the symbols, scopes, types and annotations it allocated are freed
(`SemanticStore::checkpoint` / `rollback_to`, which truncate the append-only
arenas), and the index and package registry are restored, so a failed unit
leaves no orphan symbols or packages for later units to find. This is sound
because everything the call allocated is freed by truncation. Package symbols
and scopes can be shared with earlier units (see below), so the scopes that
survive the truncation also lose what the call declared in them: every
declaration into a scope is journaled and taken back out on failure, and the
package registry forgets the packages the call entered. Interned names and the
registered origin id are not undone; both are harmless.

### Packages

`TastyUnpickler::enter_symbols` enters a package symbol for every segment of a
`PACKAGE` node's path (`TERMREFpkg`), splitting the qualified name
structurally. A package is keyed by its path, so repeated `PACKAGE` nodes
share one symbol, and the `PACKAGE` node address maps to the innermost
package. Each package owns a declaration scope and is entered into its
parent's scope. The outermost package is owned by the session's explicit root
package (empty name, no owner). A path that is neither a
`TERMREFpkg` nor a `SHAREDtype` link to one (below) is
`UnpickleError::UnsupportedPackagePath`.

A nested package whose path was already written elsewhere in the file (for
example inside an import) has a `SHAREDtype` link for its path instead. The
link is followed through the same validated, depth-bounded lookup as a shared
qualifier: an address that is not the start of a node is
`InvalidReferenceTarget`, a chain of more than `MAX_SHARED_DEPTH` links is
treated as a cycle, and a target that is not a `TERMREFpkg` is
`UnsupportedPackagePath`. `PackageNode::path_name()` itself still recognises
only a direct `TERMREFpkg`, because resolving a link needs the whole ASTs
payload, which the node does not have.

Package symbols are shared between units through a `dotty_core::Packages` registry
(path to symbol and scope) that the caller carries from one unpickler to the
next: `TastyUnpickler::with_packages` takes it and `into_parts` returns it
with the unit's index. A second unit in the same package reuses the symbol,
the owner chain and the scope, records the shared scope in its own index, and
declares its members into it. The registry belongs to one `SemanticStore`, and
a shared package keeps the origin of the unit that first entered it.
`TastyUnpickler::new` starts from an empty registry, so a lone unit behaves as
before.

The caller owns the session and passes the store's `Definitions` (bootstrapped
once) to `new`/`with_packages`. The unpickler never bootstraps, and every
reference without a prefix (`TYPEREFdirect`, `TERMREFdirect`, `TYPEREFpkg`,
`TERMREFpkg`) reuses `definitions.no_prefix`, so the same reference is the same
`TypeId` whichever adapter decoded it.

The package model is a session contract of `dotty-core` (`docs/dotty-core-design.md`
§9.1), not the unpickler's: one term-named `Package` symbol per path with an
explicit root, shared with the classloader, whose `PackageRegistry` now enters
packages through the same `dotty_core::Packages`. Dotty's package term and
module class are collapsed on purpose, so `TYPEREFpkg` and `TERMREFpkg` are one
identity and `THIS` may name a package. `LoadingSession::with_packages` /
`into_packages` carry the registry between the two adapters; the convergence
tests live in `dotty-classloader`.

**Default package.** A unit with no `package` clause is written as a `PACKAGE`
over `TERMREFpkg <empty>` (real Scala 3.9.0 output:
`tests/fixtures/semantic/DefaultPackage.scala`, and `scala/package.tasty`). The
unpickler normalises the wire spelling (`names.rs`, `package_segments`): a
leading `<empty>` or `<root>` segment is dropped and a bare empty name is the
root, so the unit's package is the session root, not a package named
`<empty>`. Its top-level classes are owned by and declared in the root, where
the classloader also puts a class with no `/`; a name-based reference to a
default-package class of another unit resolves in the root's scope. Which adapter runs first, and who owns the
registry between them, stays a caller decision until Milestone 6.

The semantic index differs from the sketch of the project document in two
deliberate ways. Scopes are keyed by the owning `SymbolId`, not by address,
because a package spans several `PACKAGE` nodes and several units, so no
single address identifies its scope, and a class scope has to be reachable
from the class symbol for `ClassInfo.declarations` anyway. The `types` map was
added by Milestone 2a, and the `trees` map is added by the milestone that
populates it (7), so no field is dead.

### Entering definitions

The walk follows the AST address index, because only the index reports
absolute addresses. Owners are known top-down, so a symbol is allocated after
its owner. Each `TYPEDEF`, `VALDEF`, `DEFDEF`, `TYPEPARAM` and `PARAM` gets
exactly one symbol, so the constructor's own copies of the class parameters
are distinct symbols owned by `<init>`: the constructor's `x: A` refers to
the constructor's `A` (`TYPEREFdirect` to its address), not to the class's.

Only packages and classes own a declaration scope. A definition is declared
in its owner's scope when it is a member of it: class members, the class's
own type parameters, and constructor parameters that are members (`Field`).
Method parameters, and a `private[this]` constructor parameter, are owned but
not declared in any scope.

A `TYPEPARAM`/`PARAM` node's name and modifiers are decoded from the node
itself, at its own address.

Not entered: definitions inside method bodies, parameters of type-lambda
aliases, and companion links.

### Types (pass 2a)

`TastyUnpickler::unpickle_type(address)` returns the `TypeId` of the type node
at an absolute AST address (`types.rs`). It runs after `enter_symbols`,
because references resolve through the index.

What decodes a type, what stays lazy, and what enters the index:

- A type is decoded only when it is asked for: by `unpickle_type`, or as a
  prefix or `SHAREDtype` target of a node that is. Nothing walks the AST looking
  for type-like nodes, and method bodies are not read.
- Decoding is cache-first: `index.type_at(address)` is returned if present.
  Otherwise the node is decoded, its type allocated, and its address recorded.
  Only nodes that allocate a type enter `index.types`; a `SHAREDtype` node does
  not.
- Nothing is attached to symbols yet (`SymbolInfo::Missing`); that is
  Milestone 5.

Forms decoded:

| TASTy | wire shape | semantic type |
|-------|------------|---------------|
| `TYPEREFdirect` | `ASTRef` | `TypeRef { NoPrefix, symbol }` |
| `TERMREFdirect` | `ASTRef` | `TermRef { NoPrefix, symbol }` |
| `TYPEREFsymbol` | `ASTRef Type` | `TypeRef { prefix, symbol }` |
| `TERMREFsymbol` | `ASTRef Type` | `TermRef { prefix, symbol }` |
| `TYPEREFpkg` / `TERMREFpkg` | `NameRef` | `TypeRef` / `TermRef { NoPrefix, package }` |
| `THIS` | `Type` | `ThisType { class }` |
| `SHAREDtype` | `ASTRef` | the `TypeId` of the named node |

- `*direct` names a local symbol by the address of its definition and has no
  prefix on the wire, so its prefix is `Type::NoPrefix`. This is a decision, not
  a default: nothing is dropped, because the encoding has nothing to drop.
- `*symbol` differs from `*direct` in carrying a real prefix (`THIS`, a
  package, another reference), which is decoded and kept. The two are not
  flattened into one form.
- The target of `*direct` and `*symbol` is `index.symbol_at(address)`. An
  address that is not the start of a node is `InvalidReferenceTarget`; a
  visible node with no entered symbol (a local definition, which pass 1 does
  not enter) is `MissingReferencedSymbol { from, to }`. The symbol must also
  be of the right kind, because the index only says that one exists: a
  `TYPEREF*` needs a type-namespace symbol, a `TERMREF*` a term-namespace one,
  and the argument of `THIS` a class, trait or module class; otherwise it is
  `InvalidReferenceKind { from, to }`.
- `TYPEREFpkg` / `TERMREFpkg` name a package by path. They resolve only to a
  package already in the `dotty_core::Packages` registry, and never create one, since
  the name is untrusted; any other package is `UnresolvedPackage`. Resolving
  packages outside the entered units belongs to the resolver.
- `THIS` is decoded only because it is the prefix of most real references. Its
  argument must be a class reference by address (or a package); only its symbol
  is kept, so nothing is allocated for the argument.
- `SHAREDtype` allocates nothing: `type_id(SHAREDtype(X)) == type_id(X)`,
  exactly, including through a chain of links. A chain longer than
  `MAX_SHARED_DEPTH`, which includes any cycle, is `InvalidReferenceTarget`.

Compound forms (Milestone 2c1):

| TASTy | wire shape | semantic type |
|-------|------------|---------------|
| `APPLIEDtype` | `Type Type*` | `Applied { tycon, args }` |
| `ANDtype` | `Type Type` | `And { left, right }` |
| `ORtype` | `Type Type` | `Or { left, right }` |
| `SUPERtype` | `Type Type` | `SuperType { this_type, super_type }` |
| `BYNAMEtype` | `Type` | `ByName { result }` |

- Every child is decoded through the same entry point as a top-level type
  (`type_at`), so it is cached, `SHAREDtype`-aware and name-resolved like any
  other, and a child's error is the error of the whole node (never wrapped).
- One compound node address owns one `TypeId`; equal trees at different
  addresses are different ids, and a `SHAREDtype` to a compound node returns
  its exact id.
- `And` / `Or` keep the operand order and nesting the compiler wrote: no
  commutative normalisation, no flattening. `BYNAMEtype` is a wrapper, never
  lowered to its result. `SUPERtype` is the type node, not the term `SUPER`.
- The grammar allows `APPLIEDtype Length Type Type*` with no arguments, and
  Dotty reads it as `appliedTo(Nil)`, the constructor itself. The node then
  owns the constructor's `TypeId` (like a `SHAREDtype` link, with its own
  address entry) instead of a second `Applied` spelling. A node whose
  indexed children disagree with its shape is `MalformedType`. Nesting is
  bounded by the AST index depth (1024), which the tests exercise at 1000
  nested intersections on a default test-thread stack.
- The structural decoders in `dotty-tasty` (`decode_applied_type`,
  `decode_and_type`, ...) return child trees whose offsets are relative to the
  node payload. The unpickler uses them to check the shape only and takes the
  children's absolute addresses from the AST index; using the relative trees
  would key a nested type under a wrong address.
- Neither corpus contains a `SUPERtype`, and a by-name parameter is
  written only as a tree-level `BYNAMEtpt`, so their tests retag nodes of the
  same wire shape.

Bounds, flexible and constant types (Milestone 2c2):

| TASTy | wire shape | semantic type |
|-------|------------|---------------|
| `TYPEBOUNDS` | `Type Type` | `Bounds { low, high }` |
| `TYPEBOUNDS` | `Type` (no upper bound) | `AliasingBounds { alias }` |
| `FLEXIBLEtype` | `Type` | `Flexible { underlying }` |
| `UNITconst` .. `STRINGconst` | the constant | `Constant(..)` |
| `CLASSconst` | `Type` | `Constant(Class(type))` |

- **Aliasing bounds.** The alias-only form is `Type::AliasingBounds`, not
  `Bounds { low: alias, high: alias }`: Dotty keeps `AliasingBounds` distinct
  and the distinction stays recoverable. The variant is not called `Alias`,
  which would collide with `SymbolKind::TypeAlias` (a symbol kind).
- **Variance markers.** Dotty does not attach the trailing variance markers to
  the bounds: it rewrites the parameter variances of an `HKTypeLambda` bound,
  then wraps it. The model has no bounds-level variance field either; the
  markers become the `declared_variance` of a rebound `TypeLambda` (see
  "Variance-bearing `TYPEBOUNDS`" below).
- **Flexible types.** `Flexible` is a preserved wrapper; the model never
  strips it. Only local member lookup looks through it (`lookup_owner`
  follows any chain of `Flexible` to a searchable prefix); a resolver still
  receives the original prefix. The real fixture is compiled with
  `-Yexplicit-nulls` (`tests/fixtures/semantic/explicit_nulls/`), the only
  setting under which the compiler writes the node.
- **Constants.** A constant type and a literal term are the same wire node, so
  every constant node decodes as a type. They come from `dotty-tasty`'s typed
  `ConstantValue` and are lossless: `Constant::Char(u16)`,
  `FloatBits(u32)`, `DoubleBits(u64)` keep every code unit, NaN payload and
  signed zero, and equality is bitwise. `STRINGconst` carries a `NameRef` and Dotty reads it as
  `readName().toString`, so any valid name entry is a string: a UTF-8 entry
  is its text and a derived entry (qualified, expanded, ...) is its rendered
  spelling (`string_value`). A signed entry (directly or nested in a derived one) renders with its signature, as Dotty's `SignedName.mkString` does: `f[with sig Signature(List(Int, 2),Unit)]`, unlike the member spelling used for lookup, which drops it. Derived names follow Dotty's `NameKinds`: a constructor default getter is `$lessinit$greater$default$N`, and the super/inline accessor, object-class and body-retainer kinds apply to the last segment of a qualified name (`a.super$b`, `a.b$`). Known
  limit: `dotty-tasty` rejects a name table that is not valid UTF-8, so a
  string with an unpaired surrogate cannot reach the unpickler and
  `Constant::StringUtf16` is not produced from TASTy yet. `CLASSconst` stores
  the decoded child type; it is not a class-symbol reference.
- A `SHAREDtype` to any of these returns the target's exact `TypeId`, and
  nothing is interned structurally: equal constants at different addresses
  keep different ids.

### Binders (Milestones 3a and 3b)

| TASTy | wire shape | semantic type |
|-------|------------|---------------|
| `TYPELAMBDAtype` | `Type (Type NameRef)*` | `TypeLambda { params, result }` |
| `POLYtype` | `Type (Type NameRef)*` | `Poly { params, result }` |
| `METHODtype` | `Type (Type NameRef)* Modifier*` | `Method { params, result, kind }` |
| `PARAMtype` | `ASTRef Nat` | `ParamRef { binder, index }` |

**The invariant.** `ParamRef.binder` is the exact `TypeId` of the `TypeLambda`,
`Poly` or `Method` it refers to, found by the binder's AST address only:

```text
binder AST address
    -> reserved TypeId
    -> published in TastySemanticIndex
    -> ParamRef children use that TypeId
    -> reserved slot filled with the final binder
```

The sequence is one helper (`decode_methodic`, Dotty's `readMethodic`) shared
by all three forms. For a binder at address `B`:

1. validate the envelope (the AST children must be `1 + parameters`, and a
   `POLYtype` or `TYPELAMBDAtype` must have at least one parameter) and read
   the parameter names, so a malformed node fails before anything is reserved;
2. `TypeArena::reserve` gives the binder id `B'`;
3. `B -> B'` is recorded in the index, *before* any child is decoded;
4. a `PendingBinder { address, id, kind, arity }` is pushed (`arity` is `None` for a `RECtype`, which binds no parameters);
5. the parameter bounds, then the result, are decoded from the absolute
   addresses in the AST index (the structural decoder's trees are relative to
   the payload and only validate the shape, as in 2c1). Either order works once
   the binder is published; bounds first lets a bad parameter info fail before
   the result is read;
6. `TypeArena::fill` puts the `TypeLambda`, `Poly` or `Method` in the reserved
   slot (no second `TypeId` is allocated), and the pending binder is popped.

**Why pending metadata exists.** Between steps 3 and 6 the address entry
refers to an *unfilled* arena slot, and reading one panics. That is confined to
one decode transaction: on failure the store checkpoint and the index mark roll
back together, discarding the reservation. Ordinary consumers never see an
unfilled slot. Inside the window, a `PARAMtype` that names an in-progress binder
is validated from its recorded kind and arity, never from the arena; and
anything that would read a type's slot (member lookup on a prefix, or
validating a parameter info) first asks whether the id is pending. A member
lookup through a pending binder is `UnsupportedResolutionPrefix`, not a panic.
The state lives in the unpickler (a stack, searched by address so a `PARAMtype`
can name an outer binder, not just the latest), not in `dotty-core` or
`TastySemanticIndex`, and `with_pending_binder` pops it on every exit, so a
failed nested lambda restores the enclosing stack.

**`PARAMtype`.** The binder address must be the start of a node
(`InvalidBinderReference`). The binder is, in order: a pending binder; one
already decoded, which must be a `TypeLambda`, `Poly` or `Method`
(`InvalidBinderKind`); or one not
yet decoded, which is decoded on demand. The index is then checked against the
arity (`InvalidParameterIndex`, for `index >= arity`). Decoding a binder on
demand can re-enter this very `PARAMtype` through the binder's own children
(the result of `[A] =>> A` is the `PARAMtype` that started it), so the node's
address is looked up again afterwards and the `ParamRef` the recursive path
made is returned instead of allocating a second one (`DuplicateType`). The
on-demand chain is bounded like a `SHAREDtype` chain (`MAX_SHARED_DEPTH`), so a
`PARAMtype` that names itself, or two that name each other, are an
`InvalidReferenceTarget`, not a stack overflow.

**Parameter infos** of a `TYPELAMBDAtype` or `POLYtype` must be `Bounds` or
`AliasingBounds` (`InvalidTypeParameterBounds`). Neither carries variance of
its own, so every parameter is `Invariant`. A `Poly` is reserved and published
before its bounds are read, so an F-bound (`[A <: Ord[A]]`) names the poly
being built. An empty `POLYtype` or `TYPELAMBDAtype` is `MalformedType`,
because Dotty's `PolyType` and `HKTypeLambda` require a parameter, although the
wire grammar (`Type NameRef*`) does not.

**`METHODtype`** parameters are term parameters: a `TermName` and any type (not
bounds). Unlike a poly, `()` is a valid clause, so an empty method decodes and
owns an id. The modifier tail is the clause kind, as Dotty's
`methodTypeCompanion`:

| tail | `MethodKind` |
|------|--------------|
| none | `Plain` |
| `IMPLICIT` | `Implicit` |
| `GIVEN` | `Contextual` |
| `IMPLICIT` and `GIVEN` (either order) | `Implicit` (`IMPLICIT` wins, as in Dotty) |
| any other modifier | `InvalidMethodModifier { address, tag }` |

Implicit and contextual (`using`) clauses are different kinds in `dotty-core`
and are never merged; a repeated modifier is harmless, as in Dotty's flag set.
Dotty's reader fails on any other modifier byte (`readParamNamesAndMods`), which
is `InvalidMethodModifier` here, and otherwise collects the tail into a flag set
where `methodTypeCompanion` tests `IMPLICIT` before `GIVEN`; the same precedence
is kept. Its pickler writes `GIVEN` or `IMPLICIT`, never both.
A `PARAMtype` to a method is a reference to one of its *term* parameters, so a
dependent result (`(x: Box): x.Out`) is a `TypeRef` whose prefix is
`ParamRef { binder: <that method's id>, index: 0 }`. Selecting a member *by name* from
a `ParamRef` prefix would need the parameter's type to be looked through, so it
is `UnsupportedResolutionPrefix`, as for any prefix this pass cannot search (a
reference written by symbol, as in the fixture, needs no lookup). A later clause may name an
earlier one, `(x: Box)(y: x.Out)`: the inner method is the outer's result, and
its `PARAMtype` resolves by address to the OUTER binder while the inner one is
also pending. The same holds for `Poly -> Method -> Method`, where three
binders are pending at once and each reference finds its own.

**`MethodParam.erased` and `varargs`.** `varargs` is the JVM `ACC_VARARGS`
distinction (a Java `T...`), which `METHODtype` does not encode: Scala's
repeated parameter is the parameter's *type*. It is `false`, never inferred
from position or name. `erased` is derived as Dotty's `MethodType.hasErasedParams`
does: a parameter is erased when the outer chain of `Annotated` wrappers on its
*type* holds an annotation whose class is exactly
`scala.annotation.internal.ErasedParam` (`defn.ErasedParamAnnot`). Milestone 4b1
audited the real wire (Scala 3.9.0, `Erased.scala`): that annotation is a full
tree, `new ErasedParam` (root `APPLY`), which Milestone 4b2a decodes, so
`(erased z: Int, w: Int) => w` decodes with the first parameter erased and the
second not, and the `Annotated` wrapper stays on `MethodParam.ty`. The class is
identified by symbol owner path (`SemanticStore::class_path`: every owner a
package, so a class nested in an object never matches), not by the text of a
name. Nothing inside a type argument counts (`List[Int @ErasedParam]` is not
erased), and no `DEFDEF` flag is read: the type node is self-contained, and a
`METHODtype` in a function or refinement type has no `DEFDEF`.

`SHAREDtype` to a lambda returns its exact binder id, including one that
is still pending (as in Dotty, which registers the lambda before reading its
parts); a parameter *info* that is such a link is rejected as not bounds. Real
TASTy links only backwards to finished nodes, so a link to an enclosing binder
is malformed input that can only build a cyclic type; consumers must not assume
acyclicity beyond `ParamRef`. Two lambdas that look
alike at different addresses are different binders, and their `ParamRef`s
differ. In practice the compiler shares equal lambdas, so real output rarely
contains such a pair; the test is synthetic.

### Recursive and refined types (Milestone 4a)

| TASTy | wire shape | semantic type |
|-------|------------|---------------|
| `RECtype` | `Type` | `Recursive { parent }` |
| `RECthis` | `ASTRef` | `RecThis { binder }` |
| `REFINEDtype` | `NameRef Type Type` | `Refined { parent, name, info }` |

**`RECtype` is a binder.** Its `TypeId` is the binder identity, so it follows
the same sequence as the methodic binders: validate the node (one child),
`reserve`, publish `R -> R'` in the index, push a `PendingBinder` of kind
`Recursive`, decode the parent, `fill`, pop. There is no second id. The pending
state is the same one: `is_pending` covers a recursive binder, so member lookup
through one that is still being decoded is `UnsupportedResolutionPrefix`, not a
read of an unfilled slot. A `Recursive` binds no parameters, so it has *no
arity* (`PendingBinder.arity` is `None`, not a made-up number), and a
`PARAMtype` naming one, pending or completed, is `InvalidBinderKind`.

**`RECthis` resolves its binder by address only.** The binder is, in this
order: a pending one (its recorded id, no arena read, and it must be
`Recursive`); one already decoded (checked to be a `Recursive`); or one not yet
decoded, which is decoded on demand, and only when the node is a `RECtype`. That
decode can reach this very `RECthis` through the parent, so the node's address
is looked up again afterwards and the `RecThis` the recursive path made is
returned; a second one would be a `DuplicateType`. The on-demand chain is bounded
like a `SHAREDtype` chain (`MAX_SHARED_DEPTH`). An address that is not a node, a
node that is not a `RECtype`, a pending binder that is not recursive, and a
chain past the bound are all `InvalidReferenceTarget`; no recursive-specific error
was needed. A `RECthis` that names itself is that error, not a loop. The address must be the `RECtype` itself: a `SHAREDtype` link to one is refused too, in either decode order, because Dotty's `RECthis` is `typeAtAddr(readAddr())` on that exact address, and a link's own address is never registered there (only registering nodes and link *targets* are), so it is a lookup failure in Dotty as well.

**One canonical `RecThis` per binder.** Dotty's `RecType` keeps one `recThis`, so
every `RECthis` naming one binder, at whatever address, gets the same `TypeId`;
`RecThis` values of different binders are never shared. This is compatible with
address identity: each address maps to exactly one type, and several addresses
may map to one. The `Recursive -> RecThis` cache is adapter state (the
unpickler, not `dotty-core`). It spans `unpickle_type` calls, so it is
*journaled*: a failed call pops the entries it added, in step with the store
rollback, because the freed ids are handed out again and a surviving entry would
pair a new binder with a stale `RecThis` id. A test decodes a failing call and then
a different recursive type that reuses the ids, at a different position.

**`REFINEDtype`.** Both children come from absolute AST addresses
(`children[0]` the parent, `children[1]` the info); the structural decoder gives
the shape and the name. The name is a *term* name unless the info is
`TYPEBOUNDS`, in which case it is a *type* name, Dotty's rule exactly
(`if nextUnsharedTag == TYPEBOUNDS then name.toTypeName`). The tag is that of
the info *after* following `SHAREDtype` (and `SHAREDterm`) links, through
`AstView::next_unshared_tag`, which validates every target and is bounded like
any other link chain; the namespace is never guessed from the text of the name.
A signed refinement name is refused (`UnsupportedSignedReference`) rather than
stripped. Nested refinements keep the order and nesting TASTy writes: the inner
one is the outer's parent, nothing is flattened. `Method`, `Poly`, `Bounds` and
`TypeLambda` infos compose (Methodic.scala and RecursiveRefined.scala).

**Refinement members (Milestone 4c2).** `Refined` holds a parent, a `Name` and
an info, with no `SymbolId` for the member, so a by-name reference to it is not
a symbol reference: see "Name-designated references". No synthetic symbol and
no text search is used.
The
self-referential shape does decode: `Base { def me: this.type }`
is `Recursive -> Refined -> ByName -> RecThis`, the `RecThis` naming the exact
`Recursive` id, in either decode order.

### Annotated types (Milestone 4b1)

```text
ANNOTATEDtype Length parent_Type annotation
```

Scala 3.9's `TreeUnpickler` reads the annotation in one of two forms, chosen by
the **first tag of the payload** (`isCompactAnnotTypeTag`, mirrored exactly by
`dotty_tasty::tasty::is_compact_annot_type_tag`):

| first tag | form | result |
|-----------|------|--------|
| `APPLIEDtype`, `SHAREDtype`, `TYPEREF`, `TYPEREFdirect`, `TYPEREFsymbol`, `TYPEREFin` | compact: a type | `Type::Annotated { underlying, annotation }`, `Annotation { ty, tree: None }` |
| `APPLY`, `NEW` | full: a constructor application | `Annotation` with type and arguments (below) |
| `SHAREDterm` | full: a link to an annotation tree | the tree at the end of the link chain, read as above (below) |
| anything else | full: an annotation tree | `UnsupportedAnnotationTree { address, annotation_address, tag }` |

The set is upstream's and no wider: `TYPEREFpkg`, `TERMREF*`, `THIS` and the
binder types look like types and are not in it, so they mean a tree (a test
pins the whole 0..=255 tag range). The outer `ANNOTATEDtype` is understood in
both cases; `UnsupportedType { tag: ANNOTATEDtype }` is no longer produced.

The structural decoder validates the shape; both children come from the AST
index by absolute address (`children[0]` the parent, `children[1]` the
annotation), and a count that disagrees is `MalformedType`. The parent is
decoded first, through the ordinary type pipeline, as Dotty does, so it keeps
address identity, `SHAREDtype`, binders, refinements and rollback. A compact
annotation is decoded as a normal type and must be a `TypeRef` or `Applied`,
which is what `CompactAnnotation` asserts. The wire tag only says "a type": a
`SHAREDtype` may reach anything, or a binder still being decoded, and either is
`InvalidCompactAnnotationType { address, annotation_type }`. The annotation
keeps its whole type: `Applied { tycon, args }` is stored as such, not reduced
to its class, because retaining annotations carry their arguments there.

A compact annotation is `Annotation::compact(ty)`: `arguments` is
`Known([])` and `tree` is `None`, which is lossless because it *is* its type
(Dotty's `CompactAnnotation.tree` is `TypeTree(tpe)`). A full annotation is
different: its constructor, type and term arguments live in a tree, and the
semantic unpickler does not build `AstArena<Typed>` yet. See "Full annotation
applications" for what is decoded of it. Note the upstream comment: for Scala 3.9 the only
compact annotations are the capture-checking `retains` family, and the
pickler writes a `CompactAnnotation` as a type only for source version 3.9 or
later.

Identity follows the rest of the pass. One address is one `TypeId` and one
`AnnotationId` (decoding it again allocates nothing); a `SHAREDtype` to an
annotated node returns that node's `TypeId`, never its parent and never a
second annotation. Annotations are not interned: two nodes with equal
compact types get two `AnnotationId`s, and `p @a1 @a2` stays two nested
`Annotated` types in the order written (inner annotation allocated first). A
failed call rolls back both arenas and the type index.

`Annotated` is a *proxy for member lookup*, as Dotty's `AnnotatedType` is a
`CachedProxyType` whose `underlying` is the parent: a name-based reference
whose prefix is `Annotated(C, ann)` looks `C`'s declarations up, just as for
`Flexible` (and any nesting of the two, bounded). The wrapper stays in the
graph and is never stripped.

`Annotated` inside a `TypeLambda` that a variance marker rebinds is rebound by
the existing rebinder: the annotation is a new one when its type names the
lambda, and reused otherwise (tested with real wire shapes).

### Full annotation applications (Milestone 4b2a)

`Annotation` now carries `arguments: AnnotationArguments`:

```text
Annotation { ty, arguments: Unavailable | Known(Vec<AnnotationArgument>), tree }
AnnotationArgument { name: Option<TermName>, value: AnnotationValue::Constant(Constant) }
```

`Unavailable` and `Known(vec![])` are different facts: the producer did not
reconstruct the arguments, versus there are none. `Annotation::new(ty, tree)`
keeps meaning `Unavailable` (the classloader and every old call site are
unchanged), `Annotation::compact(ty)` is known-empty, and `with_arguments`
gives an ordered list. `Annotation` is no longer `Copy`. `tree: None` now means
"no reconstructed typed tree is attached", not "no payload"; Milestone 7 may
attach one without changing the arguments. `rebind_type_lambda` rebinds class
literals inside arguments (`Constant::Class(TypeId)`) and reuses an annotation
that names nothing rebound; the only value variant is a constant, because the
corpora contain nothing else (see the survey).

A full annotation rooted at `APPLY` or `NEW` is read as a constructor call:

```text
APPLY (TYPEAPPLY (SELECTin <init> (NEW tpt)) targs) args
APPLY (SELECTin <init> (NEW tpt)) args
NEW tpt
```

- The annotation type is `tpt` (a type, or the type child of an `IDENTtpt`),
  as `Applied { tycon, targs }` when the constructor has type arguments, never
  reduced to the class; it must be a `TypeRef` or `Applied`
  (`InvalidAnnotationType`).
- The arguments are those of every `APPLY` layer, innermost first, as Dotty's
  `allTermArguments` (`fn` before the `Apply` around it); a `NAMEDARG` gives
  `Some(name)`; order is kept and duplicate names are not merged.
- Arguments are literals and class literals only. A `TYPED`, `BLOCK`,
  `INLINED`, reference or any other form is `UnsupportedAnnotationArgument`:
  no wrapper is stripped and nothing is evaluated.
- Any other spine part (a selection that is not the constructor, a second
  `TYPEAPPLY`, a class tree that is not a type such as `SELECTtpt`) is
  `UnsupportedAnnotationConstructor`. A root that is neither `APPLY`/`NEW` nor
  a `SHAREDterm` ending at one stays `UnsupportedAnnotationTree`.
- Children are found by absolute address in the AST index; the structural
  decoder's node-relative trees are used for names and shape only. Identity
  and rollback are the annotated type's: one address, one `TypeId`, one
  `AnnotationId`; a failure at any argument frees everything the call
  allocated.

### Shared annotation trees (Milestone 4b2b)

```text
ANNOTATEDtype Length parent_Type SHAREDterm target_ASTRef
```

The old roadmap said these needed a term-tree identity design. They do not.
Scala 3.9's `TreeUnpickler` reads

```scala
case SHAREDterm => forkAt(readAddr()).readTree()
```

so the reader follows the address and reads the tree again; nothing is cached
by tree address. The annotation is `Annotation(readTree())`, so it is the tree
that counts, not the link. This layer does the same:

- `AstView::resolve_shared_term(start, from)` follows `SHAREDterm` links (only
  those) to the first other tree. Every link target must be a visible node and
  the chain is bounded by `MAX_SHARED_DEPTH` (16 links, the annotation's own
  included). A self link, a cycle, a non-node target or an overlong chain is
  `InvalidReferenceTarget { from: the ANNOTATEDtype, to: the address that could
  not be followed }`, never a recursion.
- A constructor application (`APPLY`/`NEW`) at the end is decoded by the same
  function as a directly written one; only the tree address differs. Errors
  report the enclosing `ANNOTATEDtype` as `address` and the tree the chain ended
  at as `annotation_address`.
- Any other target tree is `UnsupportedAnnotationTree { address: the
  ANNOTATEDtype, annotation_address: the target, tag: the target's tag }`. No
  `TYPED`, `BLOCK`, `SELECT` or other expression is stripped or evaluated, and
  a type-like target is not reclassified as a compact annotation (the tree path
  is `Annotation(readTree())`, and no corpus target has that shape).
- The `AnnotationId` belongs to the enclosing `ANNOTATEDtype`. It is not cached
  by target address: two `ANNOTATEDtype` nodes linking to one tree get two
  annotations with equal payloads, while one `ANNOTATEDtype` decoded twice (or
  reached through a `SHAREDtype`) is one `TypeId` and one `AnnotationId`. The
  term tree's address is not in the type index, and `Annotation.tree` stays
  `None`.
- Typed-tree identity (one `TreeId` per term node) is Milestone 7's concern; it
  is separate from semantic annotation identity, which needs none.

### Match types (Milestone 4d)

```text
MATCHtype     Length bound_Type scrutinee_Type CaseType*
MATCHCASEtype Length pattern_Type result_Type
```

Scala 3.9 reads `MatchType(readType(), readType(), until(end)(readType()))`
and builds a case as `defn.MatchCaseClass.typeRef.appliedTo(readType(),
readType())`. Both decode by absolute child address through `type_at`, the
structural decoder validating the shape only:

- `MATCHCASEtype` is normalized to `Type::MatchCase { pattern, result }`. The
  wire carries the pattern and the result and nothing else, so nothing is lost,
  and Dotty's internal `MatchCaseClass` carrier is not added to `Definitions`
  (no symbol is looked up or made).
- `MATCHtype` is `Type::Match(MatchType { bound, scrutinee, cases })`: the
  first child is the bound, the second the scrutinee (never conflated), the rest
  the cases in wire order, none flattened, reduced or deduplicated. A match
  type with no case is accepted: the grammar is `CaseType*` and upstream passes
  the list on unchecked.
- A case that captures type variables is `[X] =>> MatchCase(p, r)` on the wire
  and is a `TYPELAMBDAtype` whose result is the `MATCHCASEtype`; it uses the
  ordinary binder machinery, so its `ParamRef`s name that exact lambda. Every
  case entry may also be a `SHAREDtype` link to an existing case (exact
  `TypeId`, no new allocation). The shape of a case is not checked: reduction
  and legality are the typer's, as upstream's reader does not check them.
- Failure at any child rolls back every allocation of the call, including the
  binder of an earlier captured case; `rebind_type_lambda` already walks both
  forms, and a test rebinds an outer lambda around a match type whose case has
  its own binder.
- `MATCHtpt` (191) is the source syntax, a tree; it is not a type and
  `unpickle_type` does not accept it. Milestone 7 rehydrates it as a typed tree
  whose `tpe` may be a `Match`.

### Type trees and simple completion (Milestone 5a)

Pass boundary: pass 1 enters symbol identity and scopes; passes 2-4 decode
semantic `Type` wire nodes lazily; pass 5a projects type *trees* and completes
simple symbols. A projected type is a tree's `tpe`, not a typed AST node: no
`TreeId`, no `AstArena<Typed>`.

**Projection** (`TastyUnpickler::unpickle_type_tree_type`, module
`type_tree`) follows `TreeUnpickler.readTpt`:

| tree | projected type |
|------|----------------|
| `SHAREDterm` | the target's projection (`forkAt(readAddr()).readTpt()`); no type or cache entry of its own, bounded like every shared chain |
| `IDENTtpt` | exactly the embedded type; the name is never resolved |
| `APPLIEDtpt` | `Applied { tycon, args }`; with `Definitions::and_type` / `or_type` and two arguments, `And` / `Or` |
| `BYNAMEtpt` | `ByName` |
| `EXPLICITtpt` | exactly its child's type (no wrapper) |
| `TYPEBOUNDStpt` | one child `AliasingBounds` (`lo eq hi`), two `Bounds`, three the alias' own type |
| a semantic type node | that type, through `type_at` (`readTpt` falls back to `readType`) |
| `SELECTtpt`, `SINGLETONtpt`, `REFINEDtpt`, `LAMBDAtpt`, `ANNOTATEDtpt`, `MATCHtpt`, `BLOCK`, `HOLE`, any other non-type tree | `UnsupportedTypeTree { address, tag }` |

*Identity.* The projection is cached by tree address in its own map
(`type_tree_type_at`), never in the type-node map: a tree address is not a
type address, several tree addresses may project to one `TypeId` (an
`IDENTtpt` shares its embedded type's), and derived types are owned by the
projection. Derived types are not interned: equal `APPLIEDtpt` at two addresses
are two types. `LAMBDAtpt` waits for the entering of type-lambda parameters.

*The special `&` / `|`.* Real `APPLIEDtpt` trees apply the `scala.&` and
`scala.|` aliases (1,022 library and 1,072 compiler constructors are named that
in the wire; upstream's `processAppliedType` canonicalizes them). They are
recognized by *symbol identity*: `Definitions` mints `and_type` and `or_type`,
and a session declares those very symbols as `&` / `|` in its `scala` package.
A same-named symbol of another owner stays an application, and so does an
application with other than two arguments.

**Completion** (`complete_symbol`, `complete_symbols`, module `completion`):

| definition | info |
|------------|------|
| `VALDEF`, `PARAM` | the projected type of its declared tree, as is (a by-name parameter stays `ByName`) |
| `TYPEPARAM` | its bounds tree; bounds are reused, another type is wrapped in a fresh `AliasingBounds` |
| non-template, non-opaque `TYPEDEF` | `toBounds` of the right-hand side: `type A = Int` is `AliasingBounds(Int)` while its right-hand side still projects to `Int` |

Opaque aliases are `OpaqueAliasDeferred`; methods, constructors, classes,
traits, modules and packages are `UnsupportedSymbolCompletion { kind }`, with no
info written and no empty `ClassInfo`. A body is never inspected, and a
`ByName` or methodic right-hand side of a type definition is
`InvalidCompletedBounds`. `suppressIntoIfParam` (upstream) is not applied; no
real case was measured, so nothing was guessed.

Each public call is one atomic transaction (the store checkpoint, the type and
tree caches, the `RecThis` journal and a **`SymbolInfo` journal**: arena
truncation cannot restore a field of a symbol that already existed, so the old
info is recorded before each write and put back on failure). Completion is per
symbol: an unsupported definition never undoes another symbol's completion.
`complete_symbols` is the one batch, all or nothing. Completion never forces
another symbol: a stable term takes part in lookup only if the caller completed
it first.

**Stable term prefixes.** `lookup_owner` follows, in one bounded chain of
`MAX_PROXY_DEPTH` steps, proxies, `Applied` to its constructor (type arguments
ignored, the prefix untouched) and a stable term to its *completed* info: a
`Field` that is not `MUTABLE`, a `Value`, or a `Parameter` whose type is not
`ByName`. Not followed: mutable fields, variables, methods, constructors,
by-name parameters and type aliases (no dealiasing). `is_illegal_prefix` is
unchanged. A cycle ends as `UnsupportedResolutionPrefix`.

### Owner-space references (Milestone 4c1)

```text
TYPEREFin Length NameRef prefix_Type ownerSpace_Type
TERMREFin Length NameRef prefix_Type ownerSpace_Type
```

**Prefix versus owner space.** The prefix is how the resulting reference is
*viewed*; the owner space is where its declaration is *found*. For an ordinary
`TYPEREF`/`TERMREF` they coincide, which is why the wire has one child. Scala 3.9's
`TreePickler` (`pickleExternalRef`) writes the two apart for a symbol of another
compilation unit that is `Private`, or a class that the prefix's member of that
name does not denote (`isShadowedRef`), and stores `sym.owner.typeRef` as the
space. The name searched in the prefix would find another declaration or none,
so the reader must look in the space, as `TreeUnpickler` does
(`owner.decl(name)`, not `prefix.member(name)`).

**Resolution contract** (`dotty-core`, format-agnostic):

```rust
pub enum MemberSpace { Prefix, Explicit(TypeId) }
pub struct MemberRequest { prefix, name, selector, space: MemberSpace }
```

Ordinary `TYPEREF`/`TERMREF` send `MemberSpace::Prefix`, exactly as before;
`REFin` sends `Explicit(ownerSpace)` with the *original* prefix. A resolver
that receives `Explicit` searches the declarations of that type, never the
prefix's members. `NoResolver` still answers `Ok(None)`.

**Decoding** (`decode_in_reference` in `types.rs`):

1. `decode_in_reference` validates the wire shape only. The two children are
   taken by absolute address from `AstView::children` (prefix first, owner space
   second) and decoded through `type_at`: the structural decoder's trees are
   node-relative and are never a semantic key.
2. The namespace is the tag's: `TYPEREFin` is `Namespace::Type`, `TERMREFin`
   is `Namespace::Term`. It is not inferred from the name or the result; a
   resolver answer in the other namespace is `ResolverFailure(Malformed)`.
3. The owner space must be a type whose declaration scope is understood
   (`lookup_owner`: `ThisType`, a class-like or package `TypeRef`, an object
   through its module class, looked through `Flexible`/`Annotated`). The lookup is
   `lookup_declaration`: exact `Name` + `Namespace` in that owner's own scope
   (unit index, package registry, or completed `ClassInfo`); it is
   `ownerSpace.decl(name)`, not inheritance-aware member lookup.
4. One candidate is the result; several are `AmbiguousMember` (a malformed
   duplicate type is ambiguous too, never first-wins). If the scope has no
   answer or is unknown, the resolver is asked with the original prefix and
   `Explicit(ownerSpace)`; the prefix is never searched as a fallback. `Ok(None)`
   is `UnresolvedMember` (with the space in its `space` field) or, for a space
   with no declaration semantics, `UnsupportedResolutionSpace`. This is
   deliberately not `UnsupportedResolutionPrefix`: the two are different inputs.
5. **Owner validation.** When the space denotes a concrete owner symbol, a
   resolver answer must have exactly that owner
   (`symbol.owner == expected owner`); a same-named symbol of another owner is
   `ResolverFailure(Malformed)`, not a success.
6. The result is `TypeRef { prefix, symbol }` / `TermRef { prefix, symbol }`
   with the **original prefix**. The owner space is resolution metadata and is
   not stored; `dotty-core`'s `Type` gained no TASTy concept.

**Deferred, each with a typed error:**

- **Signed `TERMREFin`** is `UnsupportedSignedReference`, as a signed `TERMREF`
  already is: the resolver has only `MemberSelector::Unique`, so a signature is
  neither stripped nor used to pick an overload.
- **`QualSkolemType`.** Dotty wraps a prefix that is not `isLegalPrefix` (an
  unstable singleton) in a `QualSkolemType`. The model has none, and none was
  added. A prefix that is a `TermRef` to a method, constructor or variable (looked
  through proxies) is `IllegalTypePrefix`; every other prefix is kept exactly. No
  real node needed it (see the measurement).
- **`asSeenFrom`.** Upstream applies `.asSeenFrom(prefix)` to the found
  declaration. `NamedRef` stores only `prefix + SymbolId`, and entered symbols are
  still `SymbolInfo::Missing`, so no denotation is built here. The boundary is:
  the exact declaration symbol, and the exact reference prefix; later symbol
  completion interprets the symbol as seen from the prefix.
- **Pending binders.** The prefix and the owner space are checked separately for a
  binder still being decoded (looking through proxies): a pending prefix is
  `UnsupportedResolutionPrefix`, a pending space `UnsupportedResolutionSpace`;
  an unfilled arena slot is never read.

**Identity and atomicity.** A `REFin` owns one `TypeId` at its address; the same
address decoded twice and a `SHAREDtype` link to it give the same `TypeId`, in
either order, with no second lookup. A failure after both children decoded (a
wrong-owner resolver answer, for one) rolls back the types and index entries
like every other node, and a retry works.

**Refinement members are a different problem.** `Refined` stores a parent, a
`Name` and an info, with no `SymbolId` for the member, and `TypeRef`/`TermRef` are
symbol-based. A by-name reference through a `RecThis` or a `Refined` therefore
cannot be an owner-space declaration, and no synthetic symbol or text search
was added: that is Milestone 4c2.

### Name-designated references (Milestone 4c2)

`TypeRef`/`TermRef` hold a `target`: `Symbol(SymbolId)` or `Name(TypeName |
TermName)`, Dotty's `NamedType` designator `Symbol | Name` (see
`dotty-core-design.md`). Every producer of a resolved reference keeps creating
`Symbol` targets: `*direct`, `*symbol`, `*pkg`, resolved ordinary named refs,
resolver answers, builtins, the classloader and `TYPEREFin`/`TERMREFin`.
`REFin` never falls back to a name: an unresolved owner-space reference stays
`UnresolvedMember`/`UnsupportedResolutionSpace`.

An ordinary `TYPEREF`/`TERMREF name prefix` is resolved in this order, unchanged
by 4c2: local unique symbol, `AmbiguousMember`, resolver symbol. Only when the
prefix is *structural*, that is `Refined`, `Recursive` or `RecThis` (looked
through `Flexible`/`Annotated`), and neither the local scope nor the resolver
knows the member, does the reference become `prefix + Name`. Everything else
keeps its error: a missing external class member is still `UnresolvedMember`
(a name is not a fallback for missing classpath data), a prefix with no lookup
semantics is still `UnsupportedResolutionPrefix`, and a signed `TERMREF` is
still `UnsupportedSignedReference` (its signature is semantic, so a plain
`TermName` cannot carry it).

**Pending binders.** In `C { type T1; type T2 = T1 }` the `T1` is
`TYPEREF T1 (RECthis R)` and is read while `R` is still being decoded (Scala's
`RecType(rt => registeringType(rt, readType()))` registers first). Building
`prefix + Name` needs nothing from `R`'s slot: only the type of the prefix
(the canonical `RecThis`, already allocated) is inspected, and the binder is
never read. The `Refined` graph is the only place the member lives, and
`lookup_structural_member(RecThis(R), T1)` reads it after construction. A `Refined`
or `Recursive` prefix that is itself a slot still being decoded keeps
`UnsupportedResolutionPrefix`.

**Identity.** One address owns one `TypeId`, and target kind is part of a
reference's identity. A `SHAREDtype` to a name-designated reference returns that
exact id in either decode order. Decoding the `RECthis` first reaches the
reference through the binder decoded on demand, so an outer node reached again
while its own child decodes the binder now returns the inner decode's type
instead of allocating a second one (`DuplicateType` before this milestone,
unreachable then).

**Real result.** `C { type T1; type T2 = T1 }` decodes to
`Recursive R { Refined T2 (Refined T1 C, bounds) = AliasingBounds(TypeRef
{ prefix: RecThis(R), target: Name(T1) }) }`; `T1` is not replaced by its
bounds and `lookup_structural_member` recovers the `T1` refinement's info.

### Variance-bearing `TYPEBOUNDS` (Milestone 3c)

`TYPEBOUNDS Length Type Type? Variance*`, with `STABLE`, `COVARIANT` or
`CONTRAVARIANT` markers after the types.

**Declared variance is not absent variance.** `TypeParam.declared_variance` is
`Option<Variance>`: `None` means nothing was declared (a standalone
`[A] =>> A`, a `POLYtype`, a classfile generic method), `Some(Invariant)` is an
explicit declaration, which is what `STABLE` writes. Dotty keeps the two apart
(`HKTypeLambda.isDeclaredVarianceLambda = variances.nonEmpty`, the list may hold
`Invariant`; the pickler writes markers only for such lambdas, and `pickleVariances`
writes `STABLE` for an invariant entry in a lambda that declares any). The field
is *declared* variance only; inferred variance belongs to a later typer.

**Placement, as in Dotty's `readType`.** Alias-only:
`AliasingBounds(readVariances(lo))`, so the markers apply to the one child.
Two-sided: `createNullableTypeBounds(lo, readVariances(readType()))`, so they
apply to the *upper* bound only; `low` is the plain decoded child, and a
two-sided node whose `low` is a lambda but whose `high` is not leaves both as
decoded: never a marker on `low`.

**Why a rebinding, not a field change.** `readVariances` calls
`HKTypeLambda.withVariances`, which builds a *new* lambda (`newLikeThis`) and
substitutes the old binder's parameter references in the parameter infos and
the result (`paramInfos.mapConserve(_.subst(this, x))`). The `PARAMtype`s in the
file still name the old address. Copying the cached lambda and changing a
variance would leave every nested `ParamRef` on the old `TypeId`; mutating it
would change a type other addresses share.

**`dotty_core::rebind_type_lambda(store, source, variances)`** is that
operation, format-agnostic (no address, no TASTy concept in `dotty-core`):

- it reserves the new binder's id before transforming the children, so a
  reference to the binder inside them resolves to it, and rewrites every
  reachable `ParamRef` of the old binder to the same parameter of the new one;
- it is a memoized graph transformation, so a shared node is transformed once,
  and a node with no dependence on the rebound binder keeps its own id (no
  structural interning);
- a nested `Method`, `Poly` or `TypeLambda`, and a `Recursive` with its
  `RecThis`, is copied under a reserved id and remapped, so the copy is
  internally consistent (conservatively: whenever it is reached);
- an `Annotated` type gets a new annotation with the transformed type (the
  stored one is never mutated, an unaffected one is reused); a `ClassInfo` gets
  transformed `prefix`, `parents` and `self_type`, and keeps its class symbol
  and scope;
- the `match` over `Type` has no wildcard arm, so a new variant forces a review;
- it is atomic: a store checkpoint is rolled back on any error (not a lambda,
  variance-count mismatch, a reached slot that is reserved but unfilled, a cycle,
  or a graph deeper than 512), and interned names are not touched.

**Identity in the unpickler.** The child `TYPELAMBDAtype` is decoded normally
first and stays the type cached at its own address, with no declared variance;
a `SHAREDtype` to it still returns that original. The bounds hold a *fresh
derived* lambda (declared variances, its own `ParamRef`s, no AST address).
Decoding the bounds again returns the cached bounds, so no second derived lambda
is made, and both decode orders (child first, bounds first) give the same
relationship. A target that is not a `TypeLambda` (a `Poly`, an unrelated type)
is left as it is, exactly as Dotty's `readVariances` does (`case _ => tp`): the
markers are consumed and the bounds hold the decoded child. For a lambda, a
marker count different from the arity is `BoundsVarianceArityMismatch` (nothing
is truncated or padded), and a lambda still being decoded, whose slot is not
filled, is `BoundsVarianceTargetPending` rather than a guess. Valid Scala 3.9
output reaches neither error. `UnsupportedBoundsVariance` is gone.

The real fixture cases (`Bounds.scala`): `+`, `-`, mixed `[+A, B, -C]` (with
`STABLE` for `B`), a variance on the upper bound of a two-sided node, an
F-bounded parameter, and both bounds of a two-sided node being `SHAREDtype`
links to one lambda (only the upper carries the marker). An unannotated
`type F[A] = ...` writes no marker at all in the fixture, so `STABLE` shows up
beside another declared variance.

Everything else is `UnsupportedType { tag, address }`: it is never lowered to
`NoType`, `NoPrefix` or `Error`. This includes match types.

`unpickle_type` is atomic in the same way as `enter_symbols`: on failure every
type it allocated is freed (`SemanticStore::checkpoint` / `rollback_to`) and
every address it recorded is forgotten, so a failure half way through a prefix
chain leaves nothing reachable. The resolver participates in the same
transaction trivially: it takes `&SemanticStore` and cannot allocate.

### Session identities

One `SemanticStore` has one `Definitions` and one `dotty_core::Packages`, and
the caller owns them. `TastyUnpickler::new` / `with_packages` take the
store's `Definitions` (bootstrapped once by the caller; the unpickler never
bootstraps), and every reference without a prefix (`TYPEREFdirect`,
`TERMREFdirect`, `TYPEREFpkg`, `TERMREFpkg`) reuses `definitions.no_prefix`,
so the same reference is the same `TypeId` whichever adapter decoded it.

The package model is a session contract of `dotty-core`
(`docs/dotty-core-design.md` §9.1), not the unpickler's: one term-named
`Package` symbol per path, an explicit root that is also the unnamed package,
each package declared in its owner's scope. It is shared with the classloader,
whose `PackageRegistry` enters packages through the same `Packages`. Dotty's
package term and package module class are collapsed on purpose, so `TYPEREFpkg`
and `TERMREFpkg` are one identity, and `Type::ThisType` may name a package
(the core doc comment says so). `LoadingSession::with_packages` /
`into_packages` carry the registry between the two adapters; the convergence
tests live in `dotty-classloader`. Which adapter runs first, and who owns the
registry between them, stays a caller decision until Milestone 6.

### Name-based references (pass 2b)

```text
address-bearing ref -> semantic index        (TYPEREFsymbol, *direct, ...)
name-bearing ref    -> prefix scope, then SymbolResolver   (TYPEREF, TERMREF)
```

Never a search by rendered name across owners. For
`TYPEREF name prefix` / `TERMREF name prefix` the unpickler:

1. decodes the prefix to a `TypeId` (recursively, cached by address);
2. interns the name in `Namespace::Type` (`TYPEREF`) or `Namespace::Term`
   (`TERMREF`), so a type never matches a term of the same text;
3. finds the prefix's lookup owner and declaration scope (`lookup.rs`):
   `ThisType`, `TypeRef` of a class, trait, module class or package, and
   `TermRef` of a package or an object are understood. The scope is the unit's own
   (`TastySemanticIndex::scope_of`), the session package registry's
   (`Packages::scope_of`), or the `declarations` of a completed `ClassInfo`, in
   that order; `Symbol` has no `declarations` field;
4. calls `Scope::lookup_all`: one candidate is the result, several are
   `AmbiguousMember` (overloads are never resolved by taking the first);
5. when the prefix is understood but the scope has no answer or is unknown
   here, or the prefix has no lookup semantics, asks the resolver, whose
   answer is checked for its namespace. `Ok(None)` gives `UnresolvedMember`
   (or `UnsupportedResolutionPrefix` for a prefix that has no lookup
   semantics); a resolver `Err` is `ResolverFailure`, never lowered to
   "not found".

`TYPEREFpkg` / `TERMREFpkg` and the argument of `THIS` follow the same rule:
a package in the registry resolves, any other is asked of
`SymbolResolver::resolve_package`, and `UnresolvedPackage` only when the
resolver does not know it either. A package is never created from a name.
`THIS` may name its class through a name-based `TYPEREF`.

Deferred, each with a typed error and not a guess:

- **Signed term references** (`SIGNED` / `TARGETSIGNED`) are
  `UnsupportedSignedReference`. The signature is neither stripped nor
  ignored, and no overload is picked. Selecting by signature is a follow-up
  that extends `MemberSelector`.
- **Objects** are searchable prefixes through their module class, derived
  and not stored: the owner's scope declares `Foo` (`Object`) and `Foo$`
  (`ModuleClass`), the pair TASTy itself names. `SymbolLinks::companion` is not
  used, because it links a class to its companion object. An object with no
  single module class in its owner's scope is `UnsupportedResolutionPrefix`.
- **Cross-unit class members.** `TastySemanticIndex` is unit-local, so a class
  entered by another unit is found only through the package registry (its
  top-level classes are declared in the package scope) or, once completed, its
  `ClassInfo`. The members of another unit's class are the resolver's or
  Milestone 5's. Nothing hides this with name search.
- **Inheritance.** Lookup sees the prefix's own declarations, not its parents'.

## 5. Errors

Malformed or unsupported TASTy input is reported as a typed `UnpickleError`.
Unsupported semantic shapes are never lowered to `Type::Error`. Variants are
added only when a pass needs them. Panics are reserved for internal compiler
bugs, not for external input.

## 6. Relationship with `dotty-classloader`

`dotty-classloader/src/tasty_symbol.rs` holds a best-effort, name-based
`.tasty` reader. It is a compatibility bridge, not the target model: it is
maintained (issues #11 and #16 fixed constructor fields and qualified
visibility there) but is replaced only when the unpickler reproduces its
behaviour with tests (Milestone 6). The unpickler crate does not depend on
`dotty-classloader`.

## 7. Milestones

1. **Semantic index and symbol entering** — complete.
2. **Core types**, in increments:
   - 2a: type identity and reference resolution (`TypeRef`, `TermRef`,
     prefixes, `ThisType`, `SHAREDtype`) — complete;
   - 2b: semantic name resolution (canonical `Definitions`/`NoPrefix`, the
     package contract, the `SymbolResolver` port, name-based
     `TYPEREF`/`TERMREF`) — complete. The resolver *interface* moves earlier
     than classloader *integration*, which stays in Milestone 6;
   - 2c1: compositional non-binder types (`Applied`, `And`, `Or`,
     `SuperType`, `ByName`) — complete;
   - 2c2: the core-model gaps the 2c1 measurement exposed (`Bounds`,
     `AliasingBounds`, `Flexible`, lossless constants, `CLASSconst`) —
     complete.
3. Binder types, in two steps:
   - 3a: binder identity (`TypeLambda`, `ParamRef`, reserve/publish/fill) —
     complete;
   - 3b: `Method` and `Poly` on the same machinery, `PARAMtype` to all three
     binder kinds, and the `MethodParam` audit (`erased`/`varargs`) — complete;
   - 3c: binder rebinding and variance-bearing `TYPEBOUNDS` — complete.
4. Advanced types, in steps:
   - 4a: `Refined`, `Recursive`, `RecThis` — complete;
   - 4b1: compact `ANNOTATEDtype` and the annotation corpus survey — complete;
   - 4b2a: full `APPLY`/`NEW` annotations with semantic arguments, and
     `MethodParam.erased` from `ErasedParam` — complete;
   - 4b2b: `SHAREDterm` annotation roots, followed to their constructor tree
     — complete;
   - 4c1: `TYPEREFin` / unsigned `TERMREFin` owner-space resolution — complete;
   - 4c2: name-designated refined / recursive member references and
     `lookup_structural_member` — complete;
   - 4d: `Match` / `MatchCase` and the final type-language audit — complete;
     Milestone 4 is closed.
5. Symbol completion, in steps:
   - 5a: type-tree projection, simple symbol infos and completed stable-term
     prefixes — complete;
   - 5b: `DEFDEF` method and constructor signatures, `LAMBDAtpt` and its local
     type parameters;
   - 5c: `ClassInfo`, parents, self types, cross-unit declaration scopes;
   - 5d: symbol annotations, companion links, opaque aliases and the
     remaining tails.
6. Classloader integration and the `SymbolResolver` boundary.
7. Typed AST.

Each milestone is delivered as one or more reviewable PRs that keep the whole
workspace green.

## 8. Current state and known unsupported forms

Milestone 1 delivers `TastyUnpickler::enter_symbols`, `TastySemanticIndex`
(`symbol_at`, `scope_of`, `symbol_count`) and the mappings of §4. Definition
addresses map to exactly one `SymbolId`; a second entry for an address is
`UnpickleError::DuplicateDefinition`. Milestone 2a adds
`TastyUnpickler::unpickle_type` and `TastySemanticIndex::type_at` /
`type_count`; a second `TypeId` for an address is
`UnpickleError::DuplicateType`.

Measured on real compiler output: all 37 small `dotty-tasty` fixtures enter
without error. On the manifest-backed corpora (`scala3-library` and
`scala3-compiler`, 2089 units, TASTy 28.8 — parsed leniently, outside the 3.9
target) all 2089 units enter (941 and 1148). The last one to do so,
`scala/package.tasty`, has a nested `PACKAGE` whose path is a `SHAREDtype`
link (issue #29).

Deliberately not supported yet:

- qualified-access qualifiers other than a package name or an enclosing
  definition — `UnsupportedQualifier` (none occur in the corpora);
- `PACKAGE` paths other than a direct `TERMREFpkg` or a `SHAREDtype` link to
  one — `UnsupportedPackagePath`;
- the modifiers listed in §4 (no matching core flag, variance, accessor roles)
  and annotations;
- an abstract type member is entered as `TypeAlias`; `SymbolKind` has no
  abstract-type kind;
- definitions inside method bodies (locals) and the parameters of type-lambda
  aliases;
- companion links (`SymbolLinks::companion`);
- signed `TERMREFin`, and every other type form beyond §4 "Types" —
  `UnsupportedType`;
- signed term references, cross-unit class members and
  inherited members (§4, "Name-based references");
- packages and members outside the entered state with no resolver that knows
  them — `UnresolvedPackage`, `UnresolvedMember`;
- wiring the classloader in as a `SymbolResolver` (Milestone 6).

Defects this work found in neighbouring crates were fixed there: `NameRef`
is zero-based (#9), qualified visibility is `Visibility::PrivateWithin` /
`ProtectedWithin` (#10), and a category-five node with a padded length prefix
(`161, 0, 253`) is indexed at its tag instead of one byte later (found by the
Milestone 2a corpus measurement, which saw references to real addresses that
named no node).

### Type pass measurement

`type_corpus.rs` enters every unit of the two corpora (one store and one
package registry per corpus, in path order, as a classpath would have) and
decodes every reference node as its own root, with `NoResolver`. Nested
prefixes are therefore counted again as roots, so the figures measure coverage
of the forms, not distinct types. The measurement is `#[ignore]`d in CI (run it
with `--ignored`); a smaller test runs over the small fixtures.

Milestone 2a reported 140,593 / 387,780 `TYPEREF` and 14,021 / 120,003 `TERMREF`
*errors*. Those counted every root whose decode failed on a nested name-based
node, so they overstated the nodes: the corpora hold 21,360 / 64,125 `TYPEREF`
and 4,482 / 36,762 `TERMREF` nodes.

Name-based references after 2b (library / compiler):

| | scala3-library | scala3-compiler |
|---|---|---|
| `TYPEREF` nodes | 21,360 | 64,125 |
| decoded from entered state | 10,085 (47%) | 5,435 (8%) |
| need external resolution | 11,150 (52%) | 58,236 (91%) |
| prefix without lookup semantics | 79 (0.4%) | 272 (0.4%) |
| `TERMREF` nodes | 4,482 | 36,762 |
| decoded from entered state | 1,742 (39%) | 9,174 (25%) |
| need external resolution | 2,205 (49%) | 23,572 (64%) |
| signed (`UnsupportedSignedReference`) | 518 (12%) | 3,876 (11%) |
| prefix without lookup semantics | 12 (0.3%) | 26 (0.1%) |
| ambiguous | 0 | 0 |
| unexpected errors | 0 | 0 |

"Need external resolution" is `UnresolvedMember` or `UnresolvedPackage`: a
well-formed reference whose target is not in the entered state (the library
refers to `java.lang` and `scala.*` from outside; the compiler corpus refers
to the library). The few `TYPEREF` nodes left over (46 / 182) are not
broken down here.

Order of processing matters: units are entered in path order into one store, so
a package member from a later unit is unresolved for an earlier one. The
figures are therefore a lower bound of what a session with every unit entered
would resolve locally, not a property of TASTy. Class members of another unit
are not visible at all (§4).

Every `MissingReferencedSymbol` target is something pass 1 documents as not
entered: a local definition (inside a `val`/`def` body, a block, or a pattern
`case`, including those in constructor arguments) or a parameter of a
type-lambda alias (34,215 / 117,268). The measurement asserts that no target is
anything else.

### Compound types after 2c1 (library / compiler)

Each row is a node of the corpus decoded as its own root. "Child failures"
name the first failing child, so a node that did not decode is not an
unsupported node: the decoder for the form works, and a child could not be
built.

| node | nodes | decoded | external | local | unsupported form | ambiguous / signed / prefix |
|------|-------|---------|----------|-------|------------------|-----------------------------|
| `APPLIEDtype` | 9,350 / 13,854 | 4,573 (49%) / 561 (4%) | 3,103 / 13,158 | 847 / 22 | 806 / 109 | 0 / 0 / 21 and 4 |
| `ANDtype` | 548 / 791 | 139 / 52 | 93 / 360 | 252 / 378 | 64 / 0 | 0 / 0 / 0 and 1 |
| `ORtype` | 285 / 989 | 25 / 71 | 241 / 905 | 14 / 4 | 5 / 9 | 0 |
| `BYNAMEtype` | 28 / 19 | 6 / 4 | 6 / 15 | 0 | 16 / 0 | 0 |
| `SUPERtype` | 0 / 0 | | | | | |

"External" is `UnresolvedMember` / `UnresolvedPackage`, "local" is a reference
to a definition pass 1 does not enter, "unsupported form" a child with no
decoder yet (at the time, most often `TYPEBOUNDS` and `ANNOTATEDtype`). No `APPLIEDtype`
is `UnsupportedType`. There were 0 unexpected errors in either corpus.

The compiler corpus mostly applies types from the library (`List`, `Option`,
...), which are not entered there: 13,158 of its 13,854 `APPLIEDtype` nodes
fail on an external constructor or argument, which a classpath resolver
(Milestone 6) is expected to turn into decodes. The real `SUPERtype` does not
occur in either corpus, so the decoder is covered by a retagged node.

### Bounds, flexible and constant types after 2c2 (library / compiler)

Same method: each node decoded as its own root, with `NoResolver`.

| node | nodes | decoded | external | local | unsupported form | deferred variance |
|------|-------|---------|----------|-------|------------------|-------------------|
| `TYPEBOUNDS` | 853 / 922 | 35 / 584 | 721 / 327 | 0 / 8 | 7 / 3 | 90 / 0 |
| `FLEXIBLEtype` | 238 / 552 | 66 / 17 | 166 / 531 | 0 | 6 / 4 | |
| constant nodes (12 primitive kinds) | 18,480 / 56,500 | all | 0 | 0 | 0 | |
| `CLASSconst` | 998 / 1,895 | 971 / 1,657 | 27 / 238 | 0 | 0 | |

The decoded `TYPEBOUNDS` are 34 alias-only and 1 two-sided in the library, 584
alias-only and 0 two-sided in the compiler. Most of the rest fail on a bound that
names a type outside the entered state (external), which the resolver
milestone is expected to unlock. Every
primitive and string constant decodes. Order dependence still applies: the
counts are a lower bound, since a unit decodes against the types earlier units
entered. There were 0 unexpected errors in either corpus.

### Binder types after 3a (library / compiler)

Same method: each node decoded as its own root. The library's TASTy does not
declare `scala.Any` or `scala.Nothing` (the compiler defines them), yet every
type-lambda parameter is bounded by them, so with `NoResolver` alone no lambda
can decode and the binder path would go unmeasured. The measurement therefore
runs twice: as before ("no builtins"), and with just `scala.Any`,
`scala.Nothing` and `scala.Null` entered ("compiler builtins provided"), which
leaves every other external reference failing as it should.

| node | run | nodes | decoded | external | unsupported form | deferred variance | binder errors |
|------|-----|-------|---------|----------|------------------|-------------------|---------------|
| `TYPELAMBDAtype` | no builtins | 739 / 148 | 0 / 0 | 737 / 148 | 1 / 0 | 1 / 0 | 0 |
| `PARAMtype` | no builtins | 1,672 / 169 | 0 / 0 | 1,666 / 169 | 3 / 0 | 3 / 0 | 0 |
| `TYPELAMBDAtype` | builtins | 739 / 148 | 562 / 72 | 169 / 76 | 1 / 0 | 7 / 0 | 0 |
| `PARAMtype` | builtins | 1,672 / 169 | 1,430 / 75 | 217 / 94 | 3 / 0 | 22 / 0 | 0 |

No `TYPELAMBDAtype` or `PARAMtype` is `UnsupportedType` for its own form, no
binder is invalid (0 `InvalidBinderReference`, `InvalidBinderKind`,
`InvalidParameterIndex` or `InvalidTypeParameterBounds`), and there were 0
unexpected errors in any run. What remains is child resolution: an external
type in a parameter's bounds or in the result, and the odd unsupported child
(`ANNOTATEDtype`, since 4b1 a compact or full-tree case). (`TYPEBOUNDS` with variance markers, which used to be a
deferred bucket here, decode since 3c.) "Binder decoded
on demand" counts `PARAMtype` roots asked for before their binder had a type:
1,672 / 169 with no builtins (every lambda fails, so none is ever cached) and
242 / 94 with them (the lambdas that fail and are rolled back). Order
dependence applies as before, so the counts are a lower bound.

The builtin run also moves the earlier tables: with `Any`/`Nothing` provided,
`TYPEBOUNDS` decodes 639 / 706 nodes (from 35 / 584), all now reachable
lambdas' parameter infos among them.

`ANNOTATEDtype` had no decoder at 3a; see "Annotated types after 4b1".

### Binder types after 3b (library / compiler)

Same method, same two runs. `METHODtype` and `POLYtype` are rare as type nodes
(most method types are in `DEFDEF` trees, not in the type language), so the
real-compiler fixture `Methodic.scala` carries most of the coverage; the corpus
confirms nothing unexpected.

| node | run | nodes | decoded | external | unsupported form | binder errors |
|------|-----|-------|---------|----------|------------------|---------------|
| `METHODtype` | no builtins | 3 / 4 | 0 / 0 | 3 / 4 | 0 | 0 |
| `POLYtype` | no builtins | 1 / 0 | 0 / 0 | 1 / 0 | 0 | 0 |
| `METHODtype` | builtins | 3 / 4 | 3 / 0 | 0 / 4 | 0 | 0 |
| `POLYtype` | builtins | 1 / 0 | 1 / 0 | 0 | 0 | 0 |
| `PARAMtype` | builtins | 1,672 / 169 | 1,431 / 75 | 217 / 94 | 0 | 0 |

`TYPELAMBDAtype` is unchanged (562 / 72 decoded with builtins). Decoded
`PARAMtype` roots by the kind of node their binder address names, with
builtins: `TYPELAMBDAtype` 1,430 / 75, `POLYtype` 1 / 0, `METHODtype` 0 / 0
(the four compiler methods are external). Binder identity errors: 0, unexpected
errors: 0, and no binder form is reported as an unsupported form in any run.
The `PARAMtype` counts of 3a moved by one (1,431, and 241 decoded on demand)
because the one library `POLYtype` now decodes.

### Variance-bearing `TYPEBOUNDS` after 3c (library / compiler)

Same method, same two runs. The compiler corpus has none (0). The library has 90.

| run | total | decoded | external child | pending target | arity mismatch | rebind failure | unexpected |
|-----|-------|---------|----------------|----------------|----------------|----------------|------------|
| no builtins | 90 / 0 | 0 / 0 | 90 / 0 | 0 | 0 | 0 | 0 |
| builtins | 90 / 0 | 77 / 0 | 13 / 0 | 0 | 0 | 0 | 0 |

With builtins, 77 of the 90 decode and the other 13 fail only on an external
type in the lambda (the library's own collection types, which a classpath
resolver would supply). No well-formed marker fails for being a marker: 0
pending targets, 0 arity mismatches, 0 rebind failures, 0 binder errors and 0
unexpected errors in any run. Decoded `TYPELAMBDAtype`, with builtins: library
490 standalone plus 77 as the source of a variance application (567 in all, up
from 562: the same lambdas, now also reachable through decoded bounds), compiler
72 standalone and none as a source. The builtin run moves the earlier tables:
library `TYPEBOUNDS` decode 716 (from 639), `PARAMtype` 1,445 (from 1,431), of
which `TYPELAMBDAtype` binders 1,444 and `POLYtype` 1. `TYPELAMBDAtype` counts as
"standalone" when it is not the direct child of a variance-bearing `TYPEBOUNDS`.

### Refined and recursive types after 4a (library / compiler)

Same method, same two runs (each node decoded as its own root; "builtins" =
`scala.Any`/`Nothing`/`Null` entered).

| node | run | nodes | decoded | external | unsupported prefix | unsupported form | binder errors |
|------|-----|-------|---------|----------|--------------------|------------------|---------------|
| `REFINEDtype` | no builtins | 54 / 118 | 18 / 1 | 25 / 117 | 2 / 0 | 9 / 0 | 0 |
| `RECtype` | no builtins | 2 / 0 | 0 / 0 | 0 | 2 / 0 | 0 | 0 |
| `RECthis` | no builtins | 2 / 0 | 0 / 0 | 0 | 2 / 0 | 0 | 0 |
| `REFINEDtype` | builtins | 54 / 118 | 23 / 1 | 20 / 117 | 2 / 0 | 9 / 0 | 0 |
| `RECtype` | builtins | 2 / 0 | 0 / 0 | 0 | 2 / 0 | 0 | 0 |
| `RECthis` | builtins | 2 / 0 | 0 / 0 | 0 | 2 / 0 | 0 | 0 |

The real corpus has very few recursive types: 2 `RECtype` and 2 `RECthis` in the
library, none in the compiler. All four fail the same way, as the prefix of a
by-name member (`UnsupportedResolutionPrefix`, see "No member lookup"), so no
`RECthis` decodes there: unique recursive binders named 0, canonical `RecThis`
ids 0 (as measured after 4a; Milestone 4c2 makes both roots decode, see "Name-designated references after 4c2"), and the two `RECthis` roots are asked for before their binder has a type
(binder decoded on demand 2). The corpus test also asserts the canonicalization
invariant (one `TypeId` per binder) for every decoded `RECthis`, which the
fixtures exercise instead. The 9 library refinements with an unsupported form
fail on a child (`ANNOTATEDtype`; since 4b1 each is a full annotation tree, see
below), the rest on an external type. Recursive
binder errors: 0, unexpected errors: 0 in every run, and none of the three forms
is reported as an unsupported form.

### Annotated types after 4b1 (library / compiler)

Same method, same two runs (each `ANNOTATEDtype` decoded as its own root;
"builtins" = `scala.Any`/`Nothing`/`Null` entered). Every node is classified by
the first tag of its annotation payload, taken from the AST index.

| | library | compiler |
|---|---------|----------|
| `ANNOTATEDtype` nodes | 2,014 | 5,409 |
| compact | 17 | 0 |
| full tree | 1,997 | 5,409 |

Compact by wire head: library `SHAREDtype` 12, `APPLIEDtype` 4, `TYPEREF` 1;
none of `TYPEREFdirect`, `TYPEREFsymbol`, `TYPEREFin`; the compiler has none.
Full tree by root tag (all observed): library `APPLY` 1,976 and `SHAREDterm`
21; compiler `APPLY` 5,206, `NEW` 147 and `SHAREDterm` 56. (`SHAREDterm` is a
term link to a tree written earlier, which is why it is a tree, not a
`SHAREDtype`.)

| compact outcome (library) | no builtins | builtins |
|---------------------------|-------------|----------|
| decoded | 1 | 3 |
| external child | 8 | 6 |
| full tree in the parent | 8 | 8 |
| `TYPEREFin`/`TERMREFin` deferred | 0 | 0 |
| local missing, unsupported child or prefix, invalid compact type, malformed | 0 | 0 |

The 8 compact nodes "with a full tree in the parent" have a parent type that
itself contains a full annotation tree; they surface that deferral, not a fault
of the compact form. No compact annotation was a
`TYPEREFin`, so the `TYPEREFin` deferral (Milestone 4c) does not show yet.

| full outcome | library no builtins / builtins | compiler |
|--------------|--------------------------------|----------|
| deferred as `UnsupportedAnnotationTree` | 1,723 / 1,755 | 2,085 / 2,102 |
| the parent failed first | 274 / 242 | 3,324 / 3,307 |
| decoded (never expected) | 0 | 0 |

Effect on the other forms: what used to be `UnsupportedType { tag: 153 }` is
now either a decode, a deferral or an earlier failure. No form gained a decode
from the compact case beyond the 1 (3 with builtins) compact annotated types
themselves and one library `TYPEBOUNDS` (35 to 36 decoded, 716 to 717 with
builtins). The 9 library
`REFINEDtype` nodes that used to fail on an `ANNOTATEDtype` child all still
fail, now as `UnsupportedAnnotationTree` (0 decode, 0 move to another failure):
their annotations are full trees. Likewise `APPLIEDtype` 107, `BYNAMEtype` 15,
`ANDtype` 7, `FLEXIBLEtype` 3 and `ORtype` 2 (library) report a full tree in place
of an unsupported form; `TYPELAMBDAtype`, `PARAMtype`, `TYPEBOUNDS` and the
other counts are unchanged, apart from a few nodes whose earlier failure was
`UnsupportedType(153)` and is now an external type reached first. 0 unexpected
errors in any run. The unsupported forms still reached are `TYPEREFin` (13 in
the library, 12 in the compiler; Milestone 4c) and, in the library, 5
`REFINEDtpt`.

### Full annotation applications after 4b2a (library / compiler)

Same method, same two runs (each `ANNOTATEDtype` decoded as its own root; the
parent is asked for first to tell a parent failure from the annotation's own).

Wire shapes of the 1,976 / 5,353 full `APPLY`/`NEW` annotations (spines, with
the class tree tag under `NEW`): `APPLY(SELECTin(NEW))` with class tree
`SHAREDtype` 1,258 / 3,911, `TYPEREF` 213 / 839, `IDENTtpt` 249 / 456, `SELECTtpt`
2 / 0; `APPLY(TYPEAPPLY(SELECTin(NEW)))` with `SHAREDtype` 201 / 0 and `TYPEREF`
53 / 0; and, in the compiler only, a bare `NEW` with no application: 147
(`SHAREDtype` 126, `TYPEREF` 21), all of them `@unchecked`. Term arguments per
annotation: 0 for 1,969 / 5,353 and 1 for 7 / 0; the 7 are all `STRINGconst`,
positional. There are no named arguments, no `TYPEAPPLY` argument other than
types, and no nested `APPLY`. Tags found anywhere below an annotation root
(links not followed): `SHAREDtype`, `TYPEREF`, `TERMREFpkg`, `IDENTtpt`, `NEW`,
`SELECTin`, `TYPEAPPLY` and `STRINGconst`, plus a few type-argument forms
(`TERMREFdirect`, `THIS`, `TERMREF`, `ORtype`, `SELECTtpt` with its `SELECT`); nothing that
needs another value variant.

| root | run | total | decoded | parent failed first | external | local missing | constructor unsupported | argument unsupported |
|------|-----|-------|---------|---------------------|----------|---------------|-------------------------|----------------------|
| `APPLY` library | no builtins | 1,976 | 459 | 300 | 1,210 | 5 | 2 | 0 |
| `APPLY` library | builtins | 1,976 | 631 | 258 | 1,080 | 5 | 2 | 0 |
| `APPLY` compiler | both | 5,206 | 0 | 3,123 / 3,106 | 2,083 / 2,100 | 0 | 0 | 0 |
| `NEW` compiler | both | 147 | 0 | 147 | 0 | 0 | 0 | 0 |
| `SHAREDterm` library | both | 21 | 17 | 1 | 0 | 0 | 3 | 0 |
| `SHAREDterm` compiler | both | 56 | 0 | 54 | 2 | 0 | 0 | 0 |

The 2 constructor failures are the `SELECTtpt` class trees (a selection whose
type needs its qualifier). The 5 local misses are references to definitions
pass 1 does not enter. Everything else that fails is an external type: the
compiler's annotations name library classes (such as `unchecked`) that
are not entered in the compiler corpus, so no compiler annotation decodes
here, including all 147 `NEW` roots, whose parent already fails. The `NEW`
shape is therefore covered by synthetic wire and by the shape survey, not by a
decoded real one (no small Scala source we tried makes the compiler write a
bare `NEW`). No argument is ever unsupported in either corpus, and there were
0 unexpected errors in every run. (`SHAREDterm` rows: Milestone 4b2b, see
"Shared annotation trees".)

Effect on the other forms (library; the compiler is unchanged), decoded before
-> after 4b2a, no builtins / builtins:

| node | no builtins | builtins |
|------|-------------|----------|
| `REFINEDtype` | 18 -> 18 | 23 -> 32 |
| `APPLIEDtype` | 4,578 -> 4,587 | 5,442 -> 5,471 |
| `BYNAMEtype` | 6 -> 12 | 8 -> 23 |
| `ANDtype` | 139 -> 140 | 184 -> 185 |
| `ORtype` | 25 -> 25 | 173 -> 175 |
| `FLEXIBLEtype`, `TYPEBOUNDS`, `TYPELAMBDAtype`, `PARAMtype`, `METHODtype` | unchanged | unchanged |

The 9 library refinements that failed on a full annotation now all get past it:
with builtins all 9 decode, without them all 9 fail on an external type. The
rest of the deferred full annotations under a compound form (14 `APPLIEDtype`,
1 `ANDtype`) are the `SELECTtpt` and `SHAREDterm` cases. Erased parameters:
0 `METHODtype` roots name `ErasedParam` in either corpus (3 / 4 method roots),
so the corpus decodes 0 erased `MethodParam`s; the real-compiler fixture
`Erased.scala` carries that coverage, and the survey asserts that no decoded
method type is erased without naming `ErasedParam`.

### Shared annotation trees after 4b2b (library / compiler)

All 77 `SHAREDterm` roots follow one link to an `APPLY` (library 21, compiler
56): link depth 1 for all, no link whose target is another `SHAREDterm`, no
invalid chain, no target other than `APPLY`. Targets are reused: in the library
11 distinct trees serve 21 annotations (8 trees once, 2 three times, 1 seven
times); in the compiler 40 serve 56 (29 once, 7 twice, 3 three times, 1 four
times). Each outer annotation keeps its own `AnnotationId`.

| root | total | decoded | parent failed first | external | constructor unsupported |
|------|-------|---------|---------------------|----------|-------------------------|
| `SHAREDterm` library | 21 | 17 | 1 | 0 | 3 |
| `SHAREDterm` compiler | 56 | 0 | 54 | 2 | 0 |

The 3 constructor failures are the two `SELECTtpt` class trees already counted
for direct `APPLY` roots, reached through links; the compiler's 54 + 2 fail on
external parents or classes, as its direct annotations do. Both runs (with and
without builtins) agree, invalid reference 0, unexpected 0.

Decoded before -> after (library; the compiler is unchanged): `APPLIEDtype`
4,590 -> 4,604 without builtins and 5,474 -> 5,488 with; `ANDtype` 140 -> 141
and 185 -> 186. `ANNOTATEDtype`, `ORtype`, `BYNAMEtype`, `TYPEBOUNDS`,
`REFINEDtype`, `RECtype`, `RECthis` are unchanged: the 14 `APPLIEDtype` and 1
`ANDtype` that had deferred on a shared annotation now decode.

Annotation coverage: compact 17 (library), direct full `APPLY`/`NEW` 7,329
(1,976 + 5,206 + 147), `SHAREDterm` to `APPLY` 77. No real annotation root form
is unsupported. What remains inside supported roots is a `SELECTtpt` class tree
(2 direct + 3 shared, library) and everything that needs a classpath or symbol
completion.

### Simple completion after 5a (library / compiler)

Completion was measured by completing every eligible symbol of a unit, in
document order, before decoding its reference nodes (`type_corpus`, "simple
symbols completed first"), sharing one store per corpus. Results, library (no
builtins / builtins) and compiler, per symbol kind:

| kind (definition) | entered | completed (no builtins -> builtins) | main failures |
|-------------------|---------|-------------------------------------|---------------|
| `Object` (`VALDEF`) | 944 / 2,170 | 944 / 2,170 | none |
| `Field` (`VALDEF`) | 1,180 / 7,433 | 726 -> 829 / 3,463 -> 3,497 | external 383 / 3,725; unsupported tree 71 / 244 |
| `Field` (`PARAM`) | 1,507 / 1,914 | 971 -> 1,005 / 535 -> 545 | external |
| `Parameter` (`PARAM`) | 19,152 / 38,708 | 11,296 -> 12,408 / 5,941 -> 7,618 | external; unsupported tree 1,556-1,566 / 1,413-1,425; prefix 43 (compiler) |
| `TypeParameter` | 15,165 / 1,308 | 2 -> 14,684 / 2 -> 956 | external without `Any`/`Nothing` |
| `TypeAlias` (`TYPEDEF`) | 437 / 1,119 | 15 -> 135 / 627 -> 654 | unsupported tree 269 / 338; opaque 4 / 24 |
| `Class`, `Trait`, `ModuleClass`, `Method`, `Constructor` | 1,234, 669, 944, 15,645, 2,921 (library) | 0 | kind deferred (5b/5c), all still `Missing` |

0 unexpected errors; `Package` symbols stay `Missing` too. Type trees the pass
refuses, by tag, library (no builtins): `SELECTtpt` 843, `ANNOTATEDtpt` 807,
`LAMBDAtpt` 353, `SINGLETONtpt` 40, `REFINEDtpt` 11; compiler: `SELECTtpt` 1,786,
`LAMBDAtpt` 78, `SINGLETONtpt` 67, `ANNOTATEDtpt` 62, `REFINEDtpt` 2. Type-tree
roots by definition context (all tags, the `SHAREDterm` ones by their target)
are in the corpus report; the dominant roots are `IDENTtpt` (7,962 `PARAM`,
6,071 `DEFDEF` results in the library), `APPLIEDtpt`, and `SHAREDtype`. The
library's 4,008 `DEFDEF` result trees that are `APPLIEDtpt` and 15,739
`IDENTtpt` constructors show what 5b must project: `SELECTtpt` (`x.T` written
as a tree) and `ANNOTATEDtpt` are the two largest refusals, `LAMBDAtpt` the
third.

Member resolution. The measurement did not change any decode count: with
completion the named `TYPEREF`/`TERMREF` roots that fail with an unsupported
prefix fall from 91 to 73 (library, both runs) and from 298 to 291 (compiler)
so 18 and 7 references get past the prefix, and every one of them becomes an
`UnresolvedMember` / external failure instead: their prefix's class is declared
in another unit, and a declaration scope is known only through the unit's own
index (cross-unit scopes are 5c). The remaining prefixes are unchanged term
paths (`SHAREDtype -> TERMREF` 56, `TERMREF` and `TERMREFdirect` shapes). No
downstream root (`APPLIEDtype`, `TYPEBOUNDS`, `TYPELAMBDAtype`, `PARAMtype`,
`ANNOTATEDtype`, `REFINEDtype`) gains a decode. The next blocker is a class's
declarations being reachable from another unit (5c). Separately, declaring
`scala.&` / `scala.|` in the builtins run adds 241 named library `TYPEREF`
decodes (11,684 -> 11,925) and 212 in the compiler (7,409 -> 7,621); that is
the special aliases resolving, not completion.

### Match types after 4d (library / compiler)

`MATCHtype` and `MATCHCASEtype` occur in neither corpus (0 / 0 in both), so 4d
changes no decode count; the decoder is covered by synthetic wire only. Real
match-type source appears as `MATCHtpt`: 27 trees in the library (23 with an
explicit bound; 6 with one case, 20 with two, 1 with three) and none in the
compiler. Unexpected errors: 0.

#### Regular-TASTy type-language audit (Scala 3.9.0)

| tags | forms | state |
|------|-------|-------|
| `TYPEREFdirect`, `TERMREFdirect`, `TYPEREFsymbol`, `TERMREFsymbol`, `TYPEREFpkg`, `TERMREFpkg`, `THIS`, `SHAREDtype`, `TYPEREF`, `TERMREF` | references and links | decoded; a `TERMREF` with a signed name is `UnsupportedSignedReference` |
| `TYPEREFin`, `TERMREFin` | owner-space references | decoded; signed `TERMREFin` is `UnsupportedSignedReference` (signature selection deferred) |
| `RECtype`, `RECthis`, `REFINEDtype`, `SUPERtype`, `ANDtype`, `ORtype`, `APPLIEDtype`, `TYPEBOUNDS`, `ANNOTATEDtype`, `BYNAMEtype`, `FLEXIBLEtype` | compound types | decoded |
| `POLYtype`, `METHODtype`, `TYPELAMBDAtype`, `PARAMtype` | binders | decoded |
| constants (`UNITconst` ... `CLASSconst`) | literal types | decoded |
| `MATCHtype`, `MATCHCASEtype` | match types | decoded (this milestone) |
| `MATCHtpt`, `SINGLETONtpt`, `IDENTtpt`, `SELECTtpt`, `LAMBDAtpt`, ... | type trees | not `unpickle_type` inputs: typed-tree rehydration (Milestone 7) |
| (`QualSkolemType`) | Dotty's skolem prefix of a mutable or methodic term | not modeled: such a `REFin` prefix is `IllegalTypePrefix` |
| `ERRORtype` | error type | Best-Effort TASTy only, not regular TASTy |
| 166, 168, 184-189 | unassigned | reserved |

No unsigned regular-TASTy semantic `Type` form is left unsupported. This is not
full TASTy support: signed overload selection, typed trees, symbol completion
(infos, `ClassInfo`, parents, self types, symbol annotations) and classpath
resolution are separate milestones (5, 6, 7), and most real nodes still fail
only because they name symbols outside the entered state.

### Owner-space references after 4c1 (library / compiler)

Same four runs (`--ignored --nocapture`). Each `REFin` root is measured with its
two children first decoded on their own, so a failure is attributed to the prefix
or to the owner space. The earlier "13 / 12 reached" figures were reached errors,
not totals.

| | library | compiler |
|---|---------|----------|
| `TYPEREFin` nodes | 115 | 12 |
| `TERMREFin` nodes | 0 | 0 |
| decoded | 0 | 0 |
| prefix and owner space both decode (no builtins / builtins) | 107 / 108 | 2 / 2 |
| prefix fails | 5 (1 external, 4 local definition not entered) / 4 | 7 |
| owner space fails (external) | 3 | 3 |
| resolver unresolved (both children decode, the owner scope is another unit's) | 107 / 108 | 2 |
| ambiguous, signed, resolver malformed, illegal prefix (`QualSkolem`), other child, unexpected | 0 | 0 |

**Why 0 decode.** A `REFin` names a symbol of *another compilation unit* (that is
what `pickleExternalRef` means), and the local lookup sees a class's scope only
from its own unit until symbols are completed (Milestone 5) or a classpath
resolver answers (Milestone 6). So the nodes now get as far as the decision that
needs a resolver, instead of `UnsupportedType`. To check that they are
representable, the corpus measurement adds an oracle: after every unit is entered,
each `REFin` whose children decoded is looked up in the owner class's own scope, as
entered by the unit that owns it. Result: **every one has exactly one declaration**
in it (library 107 / 108, compiler 2), none is missing, ambiguous or symbol-less.
So all real `REFin` targets are representable by a `SymbolId`, and no synthetic
symbol is needed.

Shapes. `TYPEREFin` prefixes: library `TERMREFdirect` 106 (+4 through `SHAREDtype`),
`TERMREFsymbol` 4, `APPLIEDtype` 1; compiler `TERMREFdirect` 4 (+4 shared),
`THIS` 1 (+1 shared), `APPLIEDtype` (1 shared), `FLEXIBLEtype` 1. Owner spaces:
always a `TYPEREF` of a class (115 in the library, of which 111 through
`SHAREDtype`; 12 in the compiler, 11). The 106 library `TERMREFdirect` prefixes
all come from one unit, `IArray$package`, and all name `T`. No prefix was a
method, constructor or variable, so **no real node needs `QualSkolemType`**.

Downstream (library / compiler, no builtins -> builtins): no root gained or lost
a decode. Compared with 4b2a, `ANNOTATEDtype`, `APPLIEDtype`, `TYPEBOUNDS`
(the one node that failed as unsupported `TYPEREFin` now fails on a local
definition that was not entered, in both corpora), `TYPELAMBDAtype`,
`PARAMtype`, `REFINEDtype`, `RECtype` and `RECthis` decode the same numbers; the
`unsupported form` counts fall by the `REFin` nodes and `need external` rises by
the `REFin` prefixes that reach an external class (library +9, compiler +7).
The two library `RECtype`/`RECthis` roots still fail on
`UnsupportedResolutionPrefix`: that is input for 4c2. 0 unexpected errors in any
run.

### Name-designated references after 4c2 (library / compiler)

Same four runs. `UnsupportedResolutionPrefix` is surveyed on the wire shape of
the prefix of each name-based `TYPEREF`/`TERMREF` root (before / after 4c2):

| prefix shape | library before | library after | compiler |
|--------------|----------------|---------------|----------|
| `RECthis` (direct + through `SHAREDtype`) | 3 | 0 | 0 |
| `TERMREF`, `TERMREFdirect`, `TERMREFsymbol` (direct or shared) | 91 | 91 | 296 |
| `TYPEREF` | 0 | 0 | 2 |
| `APPLIEDtype` (builtins only) | 1 | 1 | 0 |
| `Refined`, `Recursive`, `ParamRef`, `SuperType`, `And`/`Or`, binders, bounds, `NoPrefix` | 0 | 0 | 0 |

Only `RecThis` was a structural prefix in the data, so `ParamRef` support was
not needed and none was added. The remaining prefixes are term paths (`x.T`
where `x` is a `val`, object or parameter): those have symbols, and finding a
member through the *type* of `x` needs symbol completion (Milestone 5), not a
name designator. Name targets created: `TypeRef` over `RecThis` 3 in the
library (with and without builtins), 0 `TermRef`, 0 in the compiler. External
failures are unchanged (`need external` identical in all four runs).

Roots (library, no builtins -> builtins; the compiler is unchanged):

| node | no builtins | builtins |
|------|-------------|----------|
| `RECtype` | 0 -> 2 | 0 -> 2 |
| `RECthis` | 0 -> 2 | 0 -> 2 |
| `REFINEDtype` | 18 -> 20 | 32 -> 34 |
| `TYPEBOUNDS` | 36 -> 38 | 717 -> 719 |
| `APPLIEDtype` | 4,587 -> 4,590 | 5,471 -> 5,474 |
| named `TYPEREF` | 10,085 -> 10,088 | 11,681 -> 11,684 |
| `BYNAMEtype`, `ANNOTATEDtype`, `TYPELAMBDAtype`, `PARAMtype`, `FLEXIBLEtype`, named `TERMREF` | unchanged | unchanged |

Both real `RECtype`/`RECthis` roots now decode. 0 unexpected errors in any run.

## 9. Review of Milestone 1

This is a record of the review made when Milestone 1 was delivered; §8 is the
current state. Answers to the review questions asked before Milestone 2:

1. **Does every TASTy definition have a stable `SymbolId`?** Every package,
   class, trait, object, module class, type member, `val`/`var`, method,
   constructor, and type/term parameter reachable from a `PACKAGE` does.
   Locals in method bodies and type-lambda parameters do not.
2. **Are forward references possible without duplicate symbols?** Yes. Pass 1
   enters every definition before any reference is resolved, and the index
   rejects a second symbol for an address.
3. **Are nested owners correct?** Yes: `Foo` → package, `A`/`x`/`bar`/`<init>`
   → `Foo`, `B`/`b` → `bar`, the constructor's copies → `<init>`.
4. **Are namespaces preserved?** Yes. A class, its companion object and the
   module class are three symbols, in distinct term/type namespaces.
5. **Are scopes populated without duplicating ownership state?** Yes. Symbols
   carry an owner; scopes carry membership; symbols have no `children`.
6. **Is `dotty-core` still unaware of TASTy?** Yes. Its only change is the
   format-agnostic `Visibility::PrivateWithin` / `ProtectedWithin` (#10);
   at that time `dotty-tasty` and `dotty-classloader` were untouched.
7. **Did it avoid name heuristics where an address exists?** Yes. Names are
   read only to name symbols; no reference is resolved by name.
8. **Is the index sufficient for `TYPEREFsymbol`/`TERMREFsymbol`?** Yes for
   definitions of this unit: a `TYPEREFsymbol`/`TYPEREFdirect` carries the
   target's absolute address (in `Foo`, the class parameter type refers to
   address 9, the constructor's to its own copy at 49), and `symbol_at`
   resolves it. (The `types` map for `SHAREDtype` caching was missing then;
   Milestone 2a added it. References to other units still wait for the
   resolver.)
9. **Did real TASTy expose gaps in `dotty-core`?** `Visibility` could not
   express `private[X]` (#10, since added); `SymbolKind` has no abstract-type
   kind; `Definitions` has no root package.
10. **Can Milestone 2 be implemented without redesigning PR1?** Yes, and 2a did
    exactly that: it added the `types` map and resolution on top of the
    existing index. The one corpus failure then, a nested `PACKAGE` with a
    `SHAREDtype` path, was fixed separately (#29).
