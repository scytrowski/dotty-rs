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
- Milestone 5b, selected, singleton and annotated type trees: implemented (§4,
  "Selected, singleton and annotated type trees"; §8). `SELECTtpt`,
  `SINGLETONtpt` and `ANNOTATEDtpt` project, over a narrow term-`tpe`
  projection; `REFINEDtpt`, `LAMBDAtpt`, `MATCHtpt`, `BLOCK` and `HOLE` stay
  explicit refusals.
- Milestone 5c, lambda type trees and method completion: implemented (§4,
  "Lambda type trees and method completion"; §8). `LAMBDAtpt` projects to a
  `TypeLambda` (its type parameters are entered in pass 1) and ordinary `DEFDEF`
  methods complete to `Poly`/`Method`/`ByName` infos built by a format-agnostic
  parameter-symbol abstraction in `dotty-core`. Constructors, classes,
  `REFINEDtpt` and `MATCHtpt` stay deferred.

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
| 5b. Selected, singleton, annotated trees | `SELECTtpt`, `SINGLETONtpt`, `ANNOTATEDtpt`; term-`tpe` projection (`type_of_term`) | 5b |
| 5c. Lambdas and methods | `LAMBDAtpt` -> `TypeLambda`; `DEFDEF` -> `Poly`/`Method`/`ByName`; `method_type_from_symbols` and friends in `dotty-core` | 5c |
| 3. Complete | `SymbolInfo::Complete(TypeId)` for constructors, `ClassInfo`, annotations | 5d-5e |
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

`ENUM` maps to `SymbolFlags::ENUM`, and `EXPORTED` maps to
`SymbolFlags::EXPORTED`. These flags preserve the modifier facts only; this
does not name enum cases or synthesize export forwarders.

Not mapped yet: `ARTIFACT`, `INLINEPROXY`, `MACRO`, `OPEN`, `INFIX`,
`INVISIBLE`, `TRACKED`, `INTO` (no core flag),
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

### Companion links

After pass 1 has entered every identity in a unit,
`TastyUnpickler` links a `Class` or `Trait` to a same-named `Object` in the
same owner's declaration scope, and publishes the reciprocal link. Both
namespace and owner are checked, and each side must have exactly one matching
candidate. A `ModuleClass` is never an endpoint. A missing or ambiguous side
is left unlinked; symbol completion is not needed.

When several units share one store, carry a `TastySession` using
`with_session` / `into_session_parts`. It retains class declaration scopes as
well as the package registry, so a later unit can complete a pair under an
already-entered owner without forcing its `ClassInfo`. The package-only
`with_packages` / `into_parts` API remains available for callers that do not
need cross-unit class-scope lookup.

All candidate pairs are checked before either endpoint is changed. Repeating
the same discovery is idempotent; an endpoint already linked to a different
symbol returns `UnpickleError::ConflictingCompanion`. Since link publication is
the final step of pass 1, a failed entry leaves links from earlier units
unchanged.

The pinned library/compiler corpus counts and forward/reverse order survey are
recorded in [`tasty-companion-audit-3.9.0.md`](tasty-companion-audit-3.9.0.md).

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

Not entered: definitions inside method bodies and parameters of type-lambda
aliases. Companion links are published after the complete identity walk.

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
| `IDENTtpt` | exactly the embedded type; the name is validated as a name reference (as `readTpt` reads it) and never resolved |
| `APPLIEDtpt` | `Applied { tycon, args }`; with `Definitions::and_type` / `or_type` and two arguments, `And` / `Or` |
| `BYNAMEtpt` | `ByName` |
| `EXPLICITtpt` | exactly its child's type (no wrapper) |
| `TYPEBOUNDStpt` | one child `AliasingBounds` (`lo eq hi`), two `Bounds`, three the alias' own type |
| a semantic type node | that type, through `type_at` (`readTpt` falls back to `readType`) |
| `SELECTtpt`, `SINGLETONtpt`, `ANNOTATEDtpt` | see "Selected, singleton and annotated type trees" (5b) |
| `LAMBDAtpt` | see "Lambda type trees and method completion" (5c) |
| `REFINEDtpt` | see "Refined type trees" (5d2b) |
| `MATCHtpt`, `BLOCK`, `HOLE`, any other non-type tree | `UnsupportedTypeTree { address, tag }` |

*Identity.* The projection is cached by tree address in its own map
(`type_tree_type_at`), never in the type-node map: a tree address is not a
type address, several tree addresses may project to one `TypeId` (an
`IDENTtpt` shares its embedded type's), and derived types are owned by the
projection. Derived types are not interned: equal `APPLIEDtpt` at two addresses
are two types.
A tree that links back to itself (`SHAREDterm` to an ancestor) is bounded by
the same `MAX_SHARED_DEPTH` as every shared chain, counted across the nesting
(5b; before it such a tree overflowed the stack).

*The special `&` / `|`.* Real `APPLIEDtpt` trees apply the `scala.&` and
`scala.|` aliases (1,022 library and 1,072 compiler constructors are named that
in the wire; upstream's `processAppliedType` canonicalizes them). They are
recognized by *symbol identity*: `Definitions` mints `and_type` and `or_type`,
and the unpickler declares those very symbols as `&` / `|` in its package registry's `scala` package (`Definitions::declare_special_aliases`, on the first tree projection or completion; entering a unit and `unpickle_type` add no package).
A same-named symbol of another owner stays an application, and so does an
application with other than two arguments.

**Completion** (`complete_symbol`, `complete_symbols`, module `completion`):

| definition | info |
|------------|------|
| `VALDEF`, `PARAM` | the projected type of its declared tree, as is (a by-name parameter stays `ByName`) |
| `TYPEPARAM` | its bounds tree; bounds are reused, another type is wrapped in a fresh `AliasingBounds` |
| non-template, non-opaque `TYPEDEF` | `toBounds` of the right-hand side: `type A = Int` is `AliasingBounds(Int)` while its right-hand side still projects to `Int` |

Opaque aliases follow the Scala 3.9 bounds/self-type contract below. Methods,
constructors, classes, traits, modules and packages are still refused by this
simple completion entry point, with no info written and no empty `ClassInfo`.
A body is never inspected, and a `ByName` or methodic right-hand side of a
type definition is `InvalidCompletedBounds`. `suppressIntoIfParam` (upstream)
is not applied; no real case was measured, so nothing was guessed.

### Opaque aliases (Scala 3.9 contract; implementation tracked by #529)

The pinned Scala 3.9 `TreeUnpickler` first gives an alias provisional empty
bounds while reading its right-hand side, then calls `toBounds` and
`SymDenotation.opaqueToBounds`. For an opaque alias owned by a class-like
symbol, `opaqueToBounds` replaces the public info with abstract bounds and
stores the implementation alias in a same-named refinement of the owner's
self type. The two views must remain distinct: external lookup sees the
bounds, while the defining owner retains the alias.

For an explicit `TypeBoundsTree`, the public bounds are its projected lower and
upper types; an alias RHS without explicit bounds gets Scala's empty bounds.
Generic aliases abstract those bounds over the RHS `LambdaTypeTree`'s type
parameters using Scala's higher-kinded type-lambda machinery. The implementation
alias kept on the owner uses the same canonical binders as the completed public
info. The source uses provisional bounds during RHS completion to break
self-reference; a completed same-named refinement is a true cyclic-reference
error.

The existing core model has the format-agnostic pieces for this contract:
`Bounds` for external bounds, `AliasingBounds` for ordinary alias payloads,
`TypeLambda`/`ParamRef` for generic binders, and `Refined` (or `Recursive`)
for owner-local self-type refinements. Do not expose the implementation alias
as the opaque symbol's public `AliasingBounds`. The unpickler must journal
changes to an already-completed owner's `ClassInfo` and preserve any existing
self type and refinements.

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

### Selected, singleton and annotated type trees (Milestone 5b)

Three more trees project, over the smallest term-`tpe` projection they need.
Nothing builds a `TreeId`: the projection is the tree's `tpe` and nothing else.

**Term projection** (`TastyUnpickler::unpickle_term_type`, module `term_type`)
follows `TreeUnpickler.readTree` for the path forms the corpora contain
(`SELECTtpt` qualifiers and `SINGLETONtpt` references):

| tree | projected type |
|------|----------------|
| `SHAREDterm` | the target's projection; no type or cache entry of its own |
| `IDENT name Type` | exactly the embedded type; the name is validated and never resolved (like `IDENTtpt`) |
| `SELECT name qualifier` (unsigned) | `TermRef { prefix: qualifier's tpe, member }`, by the selection below; a signed name is `UnsupportedSignedReference`, never the first overload |
| `QUALTHIS (IDENTtpt Type)` | `ThisType { class }` of the class the identifier's type reference names |
| a type-tree tag (`IDENTtpt`, `SELECTtpt`, `APPLIEDtpt`, ...) | that tree's own projection |
| any other semantic type node (`TERMREF*`, `THIS`, a constant, `SHAREDtype`, `RECthis`, ...) | that type, through `type_at` (`readTree` falls back to `readType`) |
| any other term (`INLINED`, `APPLY`, `BLOCK`, ...) | `UnsupportedTermTree { address, tag }` |

*Identity.* A term address has its own cache (`term_tree_type_at`, with
mark/rollback like the other two): only a type the projection itself builds
(`QUALTHIS`, a `SELECT`) needs an entry. An `IDENT` and a direct type node
return an id `type_at` already owns, so a second entry would only repeat it;
repeating any projection allocates nothing.

**`SELECTtpt name qualifier`** is a `TypeRef` to a type member of the
qualifier's `tpe`, made by `select_member_type`, which is the same
`resolved_member` policy as a name-based `TYPEREF` (`TERMREF`, `SELECT`): the
qualifier's own declarations first, then the resolver, a structural qualifier's
member selected by name with no symbol (`TypeRefTarget::Name`), overloads are
`AmbiguousMember`, never a first match. The Symbol-or-Name constructor
(`member_type`) is shared. Upstream's `completeSelect` first applies
`widenIfUnstable` and `accessibleDenot`; this model has no widening and no
access check: an unstable singleton qualifier (a method, a mutable member, a
constructor, a by-name parameter) is `UnstableSelectQualifier`, not selected
from, and access is left to a later symbol-completion step exactly as for
`TYPEREF` (a name-based `TYPEREF` with an unstable prefix keeps its
`UnsupportedResolutionPrefix`). A qualifier that is a term whose info is still
`Missing` stays `UnsupportedResolutionPrefix`: projection never completes a
symbol, the caller's order does.

**`SINGLETONtpt ref`** is exactly the `tpe` of its reference (no wrapper: a
`TermRef`, `ThisType` or constant type already is a singleton), if that is a
stable singleton: a constant, `ThisType`, `SuperType`, `RecThis`, or a
`TermRef` that is not an unstable path; proxies are looked through. Anything
else (a class or applied type, a mutable member, a method, a by-name
parameter) is `InvalidSingletonTypeTree`.

**`ANNOTATEDtpt tpt annotation`** is `Annotated { base tpe, annotation }`. The
annotation is decoded by `decode_annotation_tree`, the one decoder of a full
annotation tree, shared with `ANNOTATEDtype`: the `SHAREDterm` chain is followed
(`AstView::resolve_shared_term`), a constructor application is parsed once
(spine, type arguments, literal arguments), and the `AnnotationId` belongs to
the annotated tree, never to the tree the links end at, so two annotated trees
sharing one annotation tree have two annotations with equal payloads. A
non-constructor root is `UnsupportedAnnotationTree`. `Annotation.tree` stays
`None`. A `SELECTtpt` annotation class (`new p.Tag`) is now projected (it was
`UnsupportedAnnotationConstructor`).

All three take part in the shared transaction of 5a: the qualifier or base
projected and the failure later (member not found, unstable, invalid singleton,
a later argument refused) restores the arenas, the three caches, the `RecThis`
journal and the `SymbolInfo` journal.

### Lambda type trees and method completion (Milestone 5c)

**Symbol identity versus binder identity.** An entered parameter `SymbolId` is
*source* identity (a definition address); `ParamRef { binder, index }` is
identity *inside* a `Method`/`Poly`/`TypeLambda`. Both are needed and neither
replaces the other globally: a parameter's own `SymbolInfo` keeps naming symbols
(it is what was projected before any binder existed), while a binder built from
symbols names `ParamRef`s. The operation between the two is
`method_type_from_symbols` / `poly_type_from_symbols` /
`type_lambda_from_symbols` in `dotty-core` (see `dotty-core-design.md` §8): it
shares the memoized `rebind_type_lambda` traversal, replaces a `TypeRef` or
`TermRef` to an exact parameter `SymbolId` (never a name; whatever its prefix)
by the `ParamRef` of the new binder, copies and rebinds an already built inner
binder that mentions an outer parameter, never mutates a stored type, keeps the
id of a type that mentions no parameter, and is atomic.

**`LAMBDAtpt` type parameters (pass 1).** They exist before any type is
projected, so `enter_symbols` enters them: while a definition is entered, its
declared type trees (a value's or parameter's type, a type parameter's bounds, a
type alias' right-hand side, a method's result type) are scanned for `LAMBDAtpt`,
through the forms that can hold one (`APPLIEDtpt`, `BYNAMEtpt`, `EXPLICITtpt`,
`TYPEBOUNDStpt`, `ANNOTATEDtpt`'s base, `SHAREDterm`, nested `LAMBDAtpt`); method
bodies and other terms are never scanned. Each parameter is a `TypeParameter`
owned by the *enclosing definition or parameter symbol* (the lambda is not a
symbol) and a member of no scope; all the immediate parameters of a lambda are
entered before the walk continues into their bounds and the body, so bounds may
refer forward or to themselves. The whole-enter transaction owns these symbols
(store checkpoint plus the index clone: nothing is journaled at completion
time). A lambda reached through a `SHAREDterm` from a second owner keeps its
first owner (`lambda_owner`), and the conflict is recorded
(`has_lambda_owner_conflict`) and refused at projection
(`SharedLambdaOwnerConflict`); the same target from the same owner reuses the
entered symbols. A `(tree, owner)` visit set keeps a heavily shared tree from
being walked once per path.

**`LAMBDAtpt` projection.** `HKTypeLambda.fromParams(tparams, body.tpe)`: each
entered parameter is completed first by the ordinary type-parameter completion
(its bounds through `toBounds`), the body is projected, and the `TypeLambda` is
built by `type_lambda_from_symbols`. Its `TypeId` is the binder; bounds and body
refer to it through `ParamRef`s (F-bounds and nested lambdas included). Declared
variance comes from the `TYPEPARAM` modifiers: `COVARIANT`, `CONTRAVARIANT`,
`STABLE` (an explicit invariant marker), none is `None`, which is not
`Some(Invariant)`; nothing is put in `SymbolFlags`. Identity as for every
projection: cached by tree address, a link is the exact target, two written
lambdas are two binders.

**Method completion** (`complete_symbol` on an ordinary `DEFDEF`, module
`method`) follows `readParamss` and `methodType(paramss, resultType)` and never
reads the body:

* *Clauses* come from the structural decoder's `header_items` (wire order), not
  the grouped `parameters`: consecutive parameters of one tag are one clause, a
  tag change starts the next, `SPLITCLAUSE` is a boundary, `EMPTYCLAUSE` an empty
  term clause. Parameter addresses come from the AST index; clause markers are
  bare tags, not nodes, so the index lists the parameter nodes and then the
  result tree, aligned by tag (`MalformedDefinition` on disagreement).
* *No clause versus an empty clause.* `def f: T` (no clause at all) is
  `ByName { T }` (`ExprType`); `def f(): T` is a `Method` with no parameters;
  `def f[A]: A` has a clause and is a `Poly` (not by-name).
* *Kind* of a term clause is decided by its first parameter's entered flags:
  `IMPLICIT` is `Implicit`, else `GIVEN` is `Contextual`, else `Plain` (the order
  `METHODtype`'s `method_kind` uses, so both paths agree even for a parameter
  with both markers; a clause mixing them keeps it, no majority vote; the
  corpora have no such clause).
* *Build.* Parameters are completed first, in clause order (a later one may
  depend on an earlier one's type), the result tree is projected, and the clauses
  are built from last to first with the abstraction primitive, so
  `(x: A)(y: x.B): y.C` is `Method(x) { Method(y) { .. } }` with `x` in the inner
  parameter as `ParamRef(outer, 0)` and `y` in the result as `ParamRef(inner, 0)`.
  The projected parameter and result graphs are untouched.
* *Parameters.* `MethodParam.erased` is the entered `ERASED` flag (where a
  DEFDEF carries it; upstream turns it into an `ErasedParam` annotation of the
  info in `fromSymbols`, and this model keeps the boolean as it does for
  `METHODtype`). `varargs` is `false`: it is JVM `ACC_VARARGS`, which TASTy does
  not have (a Scala repeated parameter is the `<repeated>` type). A method type
  parameter has `declared_variance: None`.
* *Deferred, never dropped.* `INLINE`, `TRACKED` and `INTO` on a parameter
  make `fromSymbols` add an annotation (or, for `TRACKED`, a result refinement)
  that needs annotation classes `dotty-core` does not have: the method is
  `UnsupportedMethodParameterSemantics { address, tag }` and stays `Missing`.
  Constructors are completed too (Milestone 5d2a, below): their info is the
  owner class's effective result type, not the serialized return tree.
* *No cycle.* Completion never forces another method: a reference through a
  term still `Missing` is `UnsupportedResolutionPrefix` (a regression test), and
  the only nested completions are the method's own and lambda parameters, so no
  in-progress tracking is needed. An external class not resolvable leaves the
  method `Missing` with the real `UnresolvedPackage` / `UnresolvedMember`.
* *Idempotent and atomic.* A second call returns the stored info and allocates
  nothing; a failure after parameters completed (and, for a lambda, after its
  parameters) restores every `SymbolInfo`, arena and cache of the call.

Signed overload selection is unchanged: each overload has its own exact info,
`MemberSelector::Unique` still refuses to pick one.

### Class completion (Milestone 5d1)

`complete_symbol` on a `TYPEDEF` whose first child is a `TEMPLATE` and whose
entered kind is `Class`, `Trait` or `ModuleClass` (module `class`) publishes
`SymbolInfo::Complete(Type::ClassInfo)`. Order, as upstream's `readTemplate`:
the template header's `TYPEPARAM`/`PARAM` children are completed first (a parent
may mention them; a failure there fails the class), the parents are projected in
wire order, an explicit `SELFDEF` is projected as the self type, the pass-1
declaration scope is looked up, and only then is one `ClassInfo` allocated and
set. Nothing is written before the last step, so a failure leaves the class
`Missing` (header parameters completed by the call are restored with the rest
of the transaction); a second call returns the stored `ClassInfo`.

```text
ClassInfo {
    prefix:       definitions.no_prefix        // for every class, nested or not
    class:        the entered class SymbolId
    parents:      TypeIds in TemplateStructure order, direct parents only
    declarations: index.scope_of(class)        // the exact pass-1 ScopeId
    self_type:    Some(projected SELFDEF tree) | None without a SELFDEF
}
```

* **`prefix`** is `Definitions::no_prefix`, not upstream's `owner.thisType`: the
  repository normalizes it, and `dotty-classloader` builds the same shape (a
  classloader test checks both adapters against one contract).
* **`declarations` is the pass-1 scope**, not a copy: `TastySemanticIndex::scope_of`
  stays the unit-local fast path, `ClassInfo.declarations` is the cross-unit
  publication path. A unit whose own index has no scope for a class finds its
  members through `store.symbols[class].info -> ClassInfo -> declarations`
  (`lookup.rs`'s `declaration_scope_of`, which was already there and stays
  read-only: lookup never completes a class). Completing a class does *not*
  complete its members: they stay `Missing` until each is completed on its own
  (upstream's `unforcedDecls`), and parents are not completed either.
* **Class type parameters stay symbols.** A parent or self type mentions the
  class's `TYPEPARAM` `SymbolId`s (declarations in the class scope), never a
  `ParamRef`; only method, polymorphic and type-lambda binders abstract.
* **Parents (`type_of_parent`, upstream `readParentType`).** A parent is not
  always a type tree: `SHAREDterm` is followed to its target; `APPLY fun args` is
  the parent of `fun`, the arguments never read; `BLOCK expr stats` is the parent
  of `expr`, the statements never read; `TYPEAPPLY fun targs` is the parent of
  `fun` when its `NEW` type tree is already an application (decided on the wire,
  looking through links, `IDENTtpt` and `EXPLICITtpt`; the compiler writes
  `New(Parent[A])` and repeats the arguments; upstream skips them when the
  constructor has no type parameters) and otherwise `Applied { fun, targs }`; `SELECTin <init>
  (NEW tpt) owner` is the projection of `tpt` (any other name, or a qualifier
  that is not `NEW`, is `MalformedParentTree`); anything else is a type tree.
  A term that is not a constructor call (`IDENT`, `SELECT`, `TYPED`, `INLINED`,
  `IF`, `MATCH`, `TRY`, `LAMBDA`, `NAMEDARG`, `REPEATED`) is
  `UnsupportedParentTree`. A wrapper tree's result is cached by address, so a
  shared parent is one `TypeId`. Constructor arguments and block statements are
  never decoded, so an argument that cannot be read does not affect the parent
  (synthetic tests use unreadable arguments to prove it).
* **Parent order and aliases.** The order is the wire order; no superclass or
  trait reordering happens here. The projected type is stored as is:
  annotations and refinements are never erased and no alias is dealiased (upstream's
  `dealiasKeepAnnots.separateRefinements`); no parent in the corpora is an alias
  `TypeRef` (below), a `Refined` or an `Annotated`.
* **Self type.** `SELFDEF name tpt` is the projection of `tpt`; the name is
  syntax and gets no symbol. A class without a `SELFDEF` has `self_type: None`;
  nothing is manufactured. Every *object* has a `SELFDEF` whose tree is a
  `SINGLETONtpt` of the object itself, so every `ModuleClass` has
  `Some(self_type)`.
* **Lambdas in parents and self types (pass 1).** `enter_symbols` scans each
  parent along exactly the paths `type_of_parent` reads (a call's function only,
  a type application's function and, only when its constructor type is not
  already applied, its type arguments, the `NEW` type; never a
  constructor argument) and the `SELFDEF` tree with the ordinary
  `discover_identities` (Milestone 5d2c, `discovery.rs`, formerly
  `enter_lambdas_in`). Upstream reads parents in a compiler-internal `localDummy` context; that is not
  a declaration, so the parameters of such a lambda are owned by the *class*.
  They are not class members, so they never reach `ClassInfo.declarations`. The
  owner-conflict logic of 5c applies unchanged: a lambda reached through a
  `SHAREDterm` from a second class is refused for both. (`SharedLambdaOwnerConflict`
  is per address, as in 5c.)
* **Deferred.** Constructors complete as of 5d2a (below), and `REFINEDtpt` as
  of 5d2b (below); `MATCHtpt` stays an unsupported tree (Milestone 7).
  Symbol annotations are complete as of 5e1, and companion links as of 5e2;
  opaque aliases and the remaining tails are tracked in 5e3.

New errors: `MissingClassScope`, `UnsupportedParentTree`, `MalformedParentTree`,
`InvalidSelfTypeTree`; everything else is the existing external, unsupported or
malformed error that names the real cause.

### Constructor completion (Milestone 5d2a)

A `Constructor` `DEFDEF` symbol (the `<init>` of a `Class`, `Trait` or
`ModuleClass`) becomes `Complete`, mirroring `TreeUnpickler.readNewDef`:

```scala
val paramDefss = readParamss()
val tpt = readTpt()
val normalizedParamss = normalizeIfConstructor(paramDefss.nestedMap(_.symbol), true)
val resType = effectiveResultType(sym, normalizedParamss)
sym.info = methodType(normalizedParamss, resType)
```

* **The serialized return type tree is not the semantic result.** `readTpt()`
  still runs — a real Scala 3.9.0 constructor's wire return type is `Unit`,
  not the owner class, so its validation and its own type-tree projection
  cache entry are real, but the projected `TypeId` is discarded. The
  constructor's info is built from the owner class instead.
* **Clause grouping and the parameter guard are unchanged from 5c**: the same
  `clauses_of` and `INLINE`/`TRACKED`/`INTO` refusal
  (`UnsupportedMethodParameterSemantics`) apply to a constructor's clauses,
  unmodified — no second DEFDEF header parser, no constructor-specific
  parameter-semantics error.
* **Normalization** (`normalizeIfConstructor`), over the already-grouped
  clauses, once every clause's parameters are completed (the rule reads
  parameter flags): a leading type clause is kept and the rest normalized
  recursively; otherwise a leading implicit term clause gets an empty
  ordinary clause prepended; otherwise an all-`GIVEN` (contextual) sequence of
  term clauses gets an empty ordinary clause appended (a type clause never
  disqualifies the append; an *explicit* empty clause does, since it already
  satisfies the "at least one non-implicit, non-contextual clause" invariant);
  otherwise the clauses are unchanged. The inserted clause is a real empty
  `Clause::EmptyTerm`, turned into an ordinary empty `Method` binder by the
  same [`build_clause`] every ordinary method uses — normalization only
  changes the semantic clause sequence, never the wire or the AST.
* **Effective result.** Before any clause wraps it, the result is the owner
  class's plain reference (`Type::type_ref(no_prefix, owner)`), or — if the
  *normalized* clauses start with a type clause — the owner applied to that
  clause's own type-parameter symbols (`ctor.owner.typeRef.appliedTo(...)`).
  Both use the repository's canonical `no_prefix` convention (never a copied
  `ClassInfo.prefix`, a fresh `ThisType`, or a textual lookup), so a
  constructor's result and its owner's `ClassInfo.prefix` are always the same
  shape. The existing 5c abstraction (`poly_type_from_symbols` via
  `build_clause`) then turns references to the constructor's own
  type-parameter symbols into `ParamRef`s of its own `Poly` binder — the
  **constructor's** leading type clause, never the class header's type
  parameters of the same name: TASTy enters the two at different addresses,
  so pass 1 gives them different `SymbolId`s, and no substitution by name or
  `SymbolId` equality between the two is ever assumed.
* **The owner's `ClassInfo` completion is never required or triggered.** Only
  the owner's `SymbolId` and its class-like kind (`Class`/`Trait`/
  `ModuleClass`) matter (`ConstructorOwnerNotClassLike` otherwise); a class
  blocked on an external parent can still give its constructor a complete
  info. If the owner already holds `Complete(ClassInfo)`, that info is
  checked for `class == owner` and `prefix == no_prefix`
  (`MalformedOwnerClassInfo` on a mismatch), but its parents, self type and
  declarations are never read.
* **Parameter semantics** follow 5c unchanged: `erased` from the `ERASED`
  flag, `varargs` always `false` (TASTy has no JVM `ACC_VARARGS`), a method
  type parameter's `declared_variance: None`. Neither `TRACKED` nor `ERASED`
  constructor parameters occur in the 3.9.0 library or compiler corpora (0/0,
  confirming PR #85's count still holds); `TRACKED`'s `addParamRefinements`
  result-refinement semantics remain unimplemented and would refuse the
  constructor with the same `UnsupportedMethodParameterSemantics` as an
  ordinary method's.

New errors: `ConstructorOwnerNotClassLike`, `MalformedOwnerClassInfo`. The
now-unreachable `ConstructorCompletionDeferred` was removed.

Measured (real Scala 3.9.0 fixtures, `tests/constructors.rs`): a no-arg
constructor, an ordinary term clause, a class-header field's `PARAM` versus
the constructor's own distinct `PARAM` copy, a generic constructor's `Poly`
abstraction, a leading-implicit and a using-only constructor (with and
without a leading type clause), a curried generic constructor whose first
term clause is already ordinary (no synthetic clause), an object's
module-class constructor (never the `Object` term symbol), and a nested
class (`no_prefix`, no `Outer.this`) — see the file for the exact shapes.
Mutation-checked: disabling either the implicit-prepend or the
contextual-append normalization branch fails exactly the test for that shape.

### Refined type trees (Milestone 5d2b)

`REFINEDtpt` — a structural refinement written as a declared type, `Base {
type T = Int; def run(x: Int): Int }` — projects to `Refined`/`Recursive`,
mirroring Scala 3.9's `TreeUnpickler`:

```scala
val refineCls = symAtAddr.getOrElse(start, newRefinedClassSymbol(coordAt(start))).asClass
registerSym(start, refineCls)
typeAtAddr(start) = refineCls.typeRef
val parent = readTpt()
val refinements = readStats(refineCls, end)(using localContext(refineCls))
RefinedTypeTree(parent, refinements, refineCls)
```

and `TypeAssigner` folds the stats and closes over the refinement class's own
`ThisType`:

```scala
val refined = refinements.foldLeft(parent.tpe)(addRefinement)
tree.withType(RecType.closeOver(rt => refined.substThis(refineCls, rt.recThis)))
```

* **A synthetic `<refinement>` class, entered in pass 1.** Every `REFINEDtpt`
  reached while scanning a declared type tree gets one synthetic `Class`
  symbol (`tpnme.REFINE_CLASS`, `SymbolOrigin::Synthetic`), owned by the
  current semantic owner and **not** a member of its scope, entered through
  the *same* address-keyed `symbols` map every ordinary definition uses — at
  the `REFINEDtpt`'s own address. That address therefore legitimately names
  two independent things in two different maps: `symbol_at` gives the
  synthetic class, `type_tree_type_at` (after projection) the final
  `Refined`/`Recursive` graph. The class owns one real `Scope` and is
  immediately `SymbolInfo::Complete(ClassInfo { parents: vec![], self_type:
  None, prefix: no_prefix, declarations: <that scope> })` — pass 1 needs
  nothing external to know this (Dotty's `newRefinedClassSymbol` is a
  `newCompleteClassSymbol` with no parents), and constructing it does not
  require, or trigger, anything about the class's *own* eventual completion,
  the same non-triggering discipline 5d2a's constructors follow.
* **Every immediate member's identity is entered before any is scanned**
  (`enter_definition_header`/`enter_definition_body`, split out of the single
  `enter_definition` every other call site still uses as both halves back to
  back), mirroring upstream's `readStats` indexing a block before reading any
  of it: a member's declared type may name a sibling regardless of wire
  order. An immediate stat that is not `TYPEDEF`/`VALDEF`/`DEFDEF` is the
  explicit `UnsupportedRefinementStat`, never silently dropped. A `REFINEDtpt`
  reached again through a `SHAREDterm` from a different owner keeps its first
  owner and records a conflict (`SharedRefinementOwnerConflict`), the same
  policy `LAMBDAtpt` (5c) follows; none occur in the real corpora (below).
* **Projection** (`refinement.rs`): the pass-1 class is looked up and
  validated (never allocated again); the parent is projected first through
  the ordinary `type_of_tpt`, its `TypeId` preserved exactly; every immediate
  member's info is completed through the ordinary per-symbol `complete_in`
  and folded into an ordered `Refined` chain in wire order — never sorted,
  never flattened, a repeated member name the explicit
  `UnsupportedRefinementOverload` rather than a second binding or a
  first-wins shadow; the chain is closed over the synthetic class's
  `ThisType` with the new core operation `close_over_this` (below).
* **`close_over_this(store, parent, class)`** (`dotty-core`, format-agnostic)
  is Dotty's `RecType.closeOver`, built by extending the existing
  `rebind_type_lambda` graph transformer rather than writing a second copier:
  a bounded, memoized, allocation-free scan decides whether `ThisType { class
  }` is reachable at all (absent, `parent` itself is returned, exactly as
  `rebind_type_lambda` already returns an unchanged id for a subgraph that
  does not depend on the binder being built); present, one `Recursive` is
  reserved first, `parent` is copied with every matching `ThisType` replaced
  by that binder's *one* canonical `RecThis` (allocated once, shared by every
  further occurrence), and the `Recursive` is filled and returned. Nested
  `Method`/`Poly`/`TypeLambda`/`Recursive` binders are copied the same way
  `rebind_type_lambda` already copies them, keeping their own bound
  references consistent while a `ThisType` nested inside them is substituted
  too.
* **TASTy's actual encoding, found empirically against the real fixture
  (below), not assumed from source syntax: *any* reference to a member of the
  same refinement — an explicit `this.type` and a plain sibling-name alias
  (`type T2 = T1`) alike — goes through `THIS` of the refinement's own
  synthetic class**, reached through a `SHAREDtype` link back to the
  `REFINEDtpt` node's own address (upstream's `typeAtAddr(start) =
  refineCls.typeRef`, registered as soon as the tree starts being read).
  `TastyUnpickler::this_class` (`types.rs`) needed one new arm for this: a
  `THIS` argument whose tree is a full `REFINEDtpt` node resolves through
  `referenced_class` at its own address (the same address-based resolution
  `TYPEREFDIRECT`/`TYPEREFSYMBOL` already use), instead of falling to the
  generic `UnsupportedType`. This was found only once real-fixture tests
  existed to reach it; no synthetic wire test predicted it.
* **A gap this milestone left, closed by Milestone 5d2c.** A `REFINEDtpt`
  reached *only* through a `SELECTtpt` qualifier's own type resolution or
  *only* through a `SHAREDtype` link rather than a `SHAREDterm` one was not
  entered in pass 1 (the declared-type-tree scanner only descended through a
  fixed whitelist of `TypeTree` tags), so projection reached it with no
  synthetic class: `MissingRefinementClass`/`InvalidRefinementClass`, a real,
  typed failure, never silently wrong, but not a completion either. Measured
  at 2 library `Trait` self-types (of 669 entered). See "Semantic identity
  discovery parity" below for the fix.

New errors: `SharedRefinementOwnerConflict`, `UnsupportedRefinementStat`,
`MissingRefinementClass`, `InvalidRefinementClass`, `MissingRefinementScope`,
`MalformedRefinedTypeTree`, `UnsupportedRefinementOverload`, `CloseOverThis`
(wraps a `close_over_this` failure).

Measured (real Scala 3.9.0 output, `tests/refined_tpt_enter.rs`,
`tests/refined_tpt_project.rs`, `tests/refined_tpt_real.rs`): pass-1 identity,
scope and `ClassInfo` shape; two-stage member entering; shared-owner conflict;
an unsupported stat kind; atomic rollback and a clean retry on a malformed
refinement; a plain non-recursive chain; a self-reference closing into one
`Recursive`; identity/idempotence of repeated projection; a duplicate member
name refused as an overload, with atomic rollback; and, against the real
`tests/fixtures/semantic/RecursiveRefined.scala` fixture (all 8 of its
`REFINEDtpt` trees, one per structural-type parameter) — a type-alias member,
an upper-bound member, a method member, members folding in wire order, a
sibling reference and an explicit `this.type` (both close over), a
`SHAREDtype`-shared `Int` bounds instance between two members' own fresh
`AliasingBounds` wrappers, structural lookup on the resulting graph, and that
closing an already-closed graph a second time is a no-op. `close_over_this`
itself has 13 core, format-agnostic tests in `dotty-core`, independent of
TASTy. Mutation-checked: disabling the overload check, disabling
`close_over_this` entirely, disabling its reachability pre-scan, and
disabling its `RecThis` canonicalization each independently fail exactly the
tests built to catch them.

Corpus impact: of the real `REFINEDtpt` roots measured by type-tree context
(library: 16 `DEFDEF` results, 5 `PARAM` types, 4 non-template `TYPEDEF`
right-hand sides, 2 `VALDEF` types — 27 total; a handful more in the
compiler), all but the 2 documented `Trait` self-types above now complete
instead of failing with the pre-5d2b `UnsupportedTypeTree`. `unexpected
errors: 0` holds across every one of the twelve run permutations
`type_corpus.rs` exercises (both corpora; with and without compiler builtins;
symbols-then-classes ordering; the classes-not-completed ablation).

### Semantic identity discovery parity (Milestone 5d2c)

Milestone 5d2b's projection of `REFINEDtpt` was correct; the problem was
earlier. Pass 1's declared-type-tree scanner (`enter_lambdas_in`, since
Milestone 5c) only descended through a fixed whitelist of `TypeTree` tags
(`SHAREDterm`, `LAMBDAtpt`, `REFINEDtpt`, `APPLIEDtpt`, `BYNAMEtpt`,
`EXPLICITtpt`, `TYPEBOUNDStpt`, `ANNOTATEDtpt`), but real semantic projection
is not that shallow: `type_of_tpt` can hand off to `type_of_term` (a
`SELECTtpt` qualifier, a `SINGLETONtpt` reference), which can hand off to
`type_at`/`decode_type` (an embedded `IDENT` type, a selection's semantic
prefix), which can reach `this_class` (a `THIS`/`QUALTHIS` class reference) —
and any of those hops can land on a `SHAREDtype` link whose target is, or
contains, a `LAMBDAtpt`/`REFINEDtpt` identity the old whitelist never visited.
Exactly the 2 library `Trait` self-types 5d2b measured and documented as a
known gap.

**The fix (`discovery.rs`): a mode-aware walker, not a wider whitelist.** Four
`DiscoveryMode`s name the structural layers projection actually has, and
`discover_identities` dispatches on the mode, following *exactly* the child
addresses the corresponding projection function would read next:

| mode | mirrors | routing highlights |
|------|---------|---------------------|
| `TypeTree` | `type_of_tpt` | `SELECTtpt`/`SINGLETONtpt` → `TermType`; an unhandled tag (a bare semantic type node in tpt position) → `SemanticType`; `MATCHtpt`/`BLOCK`/`HOLE` stay unwalked |
| `TermType` | `type_of_term` | `IDENT`'s embedded type → `SemanticType`; `SELECT`'s qualifier → `TermType`; `QUALTHIS`'s qualifier `IDENTtpt`'s embedded type → `ClassRef`; an unsupported term shape (`APPLY`, `BLOCK`, ...) has no arm and the walk simply stops there — pass 1 never becomes a general term-body walker |
| `SemanticType` | `type_at`/`decode_type` | `SHAREDtype` → same mode, bounded; `TYPEREF`/`TERMREF` prefix → `SemanticType` (the member itself is never resolved, §15 below); `THIS` → `ClassRef`; `RECthis`/`PARAMtype` are left alone (binder identities, not pass-1 symbols) |
| `ClassRef` | `this_class` | `TYPEREFdirect`/`TYPEREFsymbol`/`SHAREDtype` mirror `this_class`'s own grammar; a `REFINEDtpt` reached directly (the critical case, below) enters it |

Discovery still allocates only the two identity-bearing forms
(`enter_lambda_tpt`, `enter_refined_tpt`, both unchanged) and the ordinary
symbols/scopes those already enter; it never calls `type_of_tpt`,
`type_of_term`, `type_at`, `complete_in` or the `SymbolResolver`, never
allocates a `TypeId`, and never resolves a member or a package by name — pass
1 stays reference reachability only.

* **The critical case: `THIS → SHAREDtype → REFINEDtpt`.** Milestone 5d2b's
  own fixture investigation found that *any* reference to a member of a
  refinement (an explicit `this.type` or a plain sibling alias) is encoded as
  `THIS` of the refinement's own synthetic class, reached through a
  `SHAREDtype` link back to the `REFINEDtpt` node's own address. `ClassRef`
  mode's `REFINEDtpt` arm enters that identity directly, never routing it
  through `SemanticType` decoding (which has no arm for a bare `REFINEDtpt`
  tag, matching `decode_type`'s own lack of one).
* **Direct/symbol reference targets are not recursively scanned as
  definitions**, except for that one case: when a `TYPEREFdirect`/
  `TYPEREFsymbol`/`TERMREFdirect`/`TERMREFsymbol`'s target address is itself a
  `REFINEDtpt` node, that identity is entered for the *current* owner
  (`enter_reference_target`). An ordinary reference's target is otherwise left
  alone — discovery is a bounded reachability check, not a general definition
  walker.
* **A reference to an already-entered `REFINEDtpt` is idempotent, mirroring
  Dotty's `symAtAddr.getOrElse(start, newRefinedClassSymbol(...))`
  exactly: get the existing identity, never re-derive its owner.** The first
  implementation of the critical case above re-ran the ordinary
  owner-conflict bookkeeping on every hidden-route visit, which
  misclassified a member naming its *own* enclosing refinement (`x: this.type`
  inside the very refinement `x` is a member of) as a second, conflicting
  owner — found and fixed by `tests/discovery.rs`'s
  `a_member_naming_its_own_enclosing_refinement_through_this_keeps_the_refinements_true_owner`,
  mutation-checked: removing the idempotency check makes the test, and two of
  5d2b's own real-fixture tests, fail exactly as expected.
  `enter_referenced_refined_tpt` checks `symbol_at` first; only a genuinely
  unentered address is entered fresh, owned by the declared-type position
  discovery began from. Genuine cross-definition sharing (5d2b's `SHAREDterm`
  case) is untouched: it is still detected the same way, at the `TypeTree`
  routing layer, before any reference-target special-casing runs.
* **Memoization includes the mode.** `first_identity_scan` is keyed by
  `(tree, owner, mode)`, not `(tree, owner)`: the same address can legitimately
  be visited first under a shallow mode (`SemanticType`, which has no arm for
  a bare `REFINEDtpt` tag) and later under a richer one (`ClassRef`, which
  does) for the same owner, and the first, shallower visit must not suppress
  the second. `tests/discovery.rs`'s
  `an_identity_first_visited_under_a_shallow_mode_is_still_entered_under_a_richer_one`
  is this exact scenario.
* **Annotation argument trees are not walked** (`ANNOTATEDtpt`/`ANNOTATEDtype`'s
  base is; the annotation constructor form, an `APPLY`/`NEW` term, is not):
  every annotation class in both corpora is a plain class reference, never a
  structural refinement or a type lambda, so mirroring the full
  constructor-spine walk would add real complexity for a measured zero. The
  corpus run below reports zero missing identities with this scope, not an
  unverified assumption.

New errors: none — every route above either enters an already-modelled
identity (`enter_lambda_tpt`/`enter_refined_tpt`, with their existing errors)
or is a structural no-op; a malformed/cyclic link is still the existing
`MalformedType`/`InvalidReferenceTarget`.

Measured (`tests/discovery.rs`, a hand-built fixture, address-settled like
5d2b's own): a `REFINEDtpt` hidden behind a `SELECTtpt` qualifier is entered;
one hidden behind `THIS`, with and without an extra `SHAREDtype` hop, is
entered; two owners reaching one hidden `REFINEDtpt` through `SELECTtpt`
still conflict (`SharedRefinementOwnerConflict`, no duplicate class); a
poisoned `REFINEDtpt` nested only inside an unsupported `APPLY` term body is
never discovered; the mode-sensitive-memo scenario above; the self-reference
false-conflict fix above; and that projection now succeeds end-to-end for a
`REFINEDtpt` only reachable through a hidden route. Every 5d2a/5d2b regression
suite (`tests/refined_tpt_enter.rs`, `tests/refined_tpt_project.rs`,
`tests/refined_tpt_real.rs`, `tests/lambda_tpt.rs`, and the rest of the
workspace) is unchanged and still green — this milestone changes *when* pass 1
discovers an identity, not what `REFINEDtpt`/`LAMBDAtpt` project to.

Corpus impact: the 2 library `Trait` self-types 5d2b measured as
`MissingRefinementClass`/`InvalidRefinementClass` no longer fail for a missing
pass-1 identity (no occurrence of either error, or of the "refinement
semantic-state error" completion bucket, anywhere in any of the sixteen run
permutations). `unexpected errors: 0` holds throughout, and no existing
completion count regresses (`Trait`, `Class`, `Method`, `Constructor`,
`Field`, `Parameter`, `TypeParameter`, `TypeAlias`, `ModuleClass` completion
counts are byte-identical to the pre-5d2c corpus run in every permutation).

Milestone 5d is closed by this issue; the roadmap continues at 5e (symbol
annotations, companion links, opaque aliases), driven by the next measured
corpus gap.

#### Review follow-up: three logic bugs, route attribution, a reachability oracle

PR #102's review of this milestone found three real logic bugs in the
first cut of `discovery.rs`, all fixed in the same PR before merge:

* **Owner-conflict suppression.** The idempotency check above
  (`enter_referenced_refined_tpt` returning early once `symbol_at` exists) also
  suppressed a *genuine* independent owner reaching an already-entered
  identity through a hidden route — it could never tell that case apart from a
  member naming its own enclosing refinement. Fixed by `owner_is_within`:
  `owner`'s ownership chain is walked to check whether it leads to the
  refinement class itself; only then is the visit treated as a self-reference.
  A genuinely independent owner still gets `SharedRefinementOwnerConflict`,
  the same policy a direct `SHAREDterm` share already had.
  `tests/discovery.rs`'s
  `a_hidden_refinement_reached_from_two_owners_through_this_still_conflicts`
  is the regression, mutation-checked (reverting `owner_is_within` to the
  unconditional idempotent version makes it fail as expected, without
  disturbing the self-reference test above).
* **Silent invalid reference targets.** `discover_identities`'s top-level
  `tag_at`-is-`None` fallback returned `Ok(())`, and `reference_target` (the
  direct/symbol reference-target helper) checked only that a leaf shape
  decoded, never that its target address was a visible node. Both silently
  swallowed a malformed or out-of-range address that issue #101 §18 requires
  to fail typed. Fixed: the fallback is now `InvalidReferenceTarget`, and
  `reference_target` validates its result through `AstView::is_node` before
  returning it. This is a deliberate tightening from some pre-5d2c code's
  "defer the error to projection" habit to "pass 1 fails eagerly," so five
  pre-existing tests whose fixtures relied on the old, lenient behavior
  (`tests/class_parents.rs`'s parent-cannot-be-projected test,
  `tests/type_identity.rs`'s four shared-link/invalid-target tests) were
  updated to assert the (correct) eager failure instead.
* **`SHAREDtype` depth inconsistency.** Discovery threaded `SHAREDtype`
  resolution through the same general nesting-depth counter (`MAX_TREE_DEPTH`,
  256) every other structural descent uses, while real projection
  (`type_at`/`this_class`) bounds a `SHAREDtype` *chain* on its own, much
  tighter `MAX_SHARED_DEPTH` (16). A long-enough chain of unrelated nesting
  could let a genuinely cyclic or overlong `SHAREDtype` chain through pass 1,
  only to fail later at projection. Fixed by `AstView::resolve_shared_type`,
  an exact mirror of the pre-existing `resolve_shared_term`: it resolves and
  validates a whole `SHAREDtype` chain in one bounded call, independent of the
  caller's own depth budget. Two new `AstView` unit tests exercise it directly.

All three were mutation-tested (revert the fix, confirm the regression test
fails, restore, confirm the restoration is byte-identical to the pre-fix
file), and the full corpus run afterward was byte-identical to the run before
these fixes: none of the three bugs was ever triggered by real Scala library
or compiler code, only by adversarial fixtures built to reach them.

The same review asked for the three items disclosed as scope gaps in the
original PR body to be closed before merge:

* **A dedicated hidden-`LAMBDAtpt` regression**
  (`tests/discovery.rs`'s `a_lambdatpt_hidden_behind_a_selecttpt_qualifier_is_entered`):
  a `LAMBDAtpt` reached only through a `SELECTtpt` qualifier's `SHAREDterm`
  link (`discover_term_type`'s `is_type_tree_tag` redirect, the same mechanism
  that finds a hidden `REFINEDtpt` this way) is entered with every immediate
  `TYPEPARAM` symbol present, and projecting it never fails with
  `MissingEnteredSymbol`.
* **Route attribution** (`DiscoveryRoute`, in `discovery.rs`): every recursive
  call threads a small enum naming the nearest named hop it is about to visit
  through (`SHAREDterm`, `SHAREDtype`, `IDENTtpt`, `IDENT`, `SELECTtpt`,
  `SINGLETONtpt`, `SELECT`, `QUALTHIS`, `THIS`, a §11 reference target, an
  ordinary structural descent, or the root declared-type position itself). The
  route active when a `LAMBDAtpt`/`REFINEDtpt`'s *first* owner is recorded is
  kept in the index (`lambda_route`/`refined_route`) purely for reporting — no
  arm of the walker branches on it. The label always names the *nearest* hop,
  not a distant one: a `REFINEDtpt` reached through `SELECTtpt`'s qualifier's
  own `SHAREDterm` link is attributed to `SHAREDterm`, not `SELECTtpt`,
  because the link, not the qualifier, is the last hop before the identity;
  when the same `SELECTtpt` names its target *directly* (no link in between —
  the real library shape this milestone was built for), the route is
  `SELECTtpt` itself. `tests/discovery.rs`'s
  `each_entered_identity_is_attributed_to_the_hop_that_led_to_it` fixes both
  shapes, plus the `THIS`-direct (§11 reference target) and `THIS → SHAREDtype`
  (§10 critical case) distinction, and the direct declared-type-position case.
* **A projection-reachability oracle** (`reachability.rs`, new module): an
  independent cross-check, run after `enter_symbols`, that starts from the
  *wire* rather than from the walker's own traversal. It finds every
  `LAMBDAtpt`/`REFINEDtpt` node physically present in the AST section — whether
  or not any discovery call ever visited it — and classifies each one as
  `Entered` (has an owner), `OutOfScope` (not structurally reachable — a local
  definition's declared type or a pattern binder's, or a node only reachable
  through an unsupported term shape), or `Unaccounted`: reachable but not
  entered, always a genuine parity gap. `identity_reachability` is a public
  function (`dotty_tasty_unpickler::tasty_unpickler::identity_reachability`)
  so both `tests/discovery.rs`'s unit-level fixtures and
  `tests/type_corpus.rs`'s corpus run can call it; the corpus run asserts
  `lambda_unaccounted + refined_unaccounted == 0` and prints reachable/entered/
  out-of-scope counts, owner-conflict counts and the route-attribution
  histogram per identity kind. Measured (all sixteen run permutations):
  `unaccounted: 0` throughout; scala3-library's real `REFINEDtpt` route
  histogram shows `SELECTtpt: 3` (the exact real-library shape this milestone
  fixes) alongside the ordinary `root`/`structural` routes pass 1 always had.
  **Two review passes hardened what "reachable" means**, both against the same
  underlying risk: a wire-driven check is only as trustworthy as its own
  definition of reachability.
  * The first cut's `in_body` tested only physical ancestor tag membership
    (`VALDEF`/`DEFDEF`/`BLOCK`/`CASEDEF`/`LAMBDAtpt` anywhere above an
    address), which is too coarse in two directions: a `VALDEF`'s own
    declared-type child (or a `DEFDEF`'s own result) has a `VALDEF`/`DEFDEF`
    ancestor too, so a real regression there would have been misreported as
    an expected shape rather than a parity gap; and a node reachable only
    through an unsupported term shape with *no* `VALDEF`/`DEFDEF`/`BLOCK`/
    `CASEDEF` ancestor at all (a loose tree named only by a `SHAREDterm` link
    sitting inside an `APPLY` argument, `tests/discovery.rs`'s own
    `poison`/`poisoned` fixture) would have been misreported as `Unaccounted`
    — a false-positive parity gap on a shape discovery is *correct* to skip.
  * Fixed by replacing the ancestor check with a second, independent
    implementation of discovery's own four dispatch tables (`TypeTree`/
    `TermType`/`SemanticType`/`ClassRef`, mirrored, not called — a bug in
    discovery's own dispatch cannot also hide from this check), walked from
    every declared-type root found by structurally re-deriving the *same*
    top-down topology `enter_all`/`enter_package`/`enter_template`/
    `enter_definition_body`/`enter_parameters` themselves follow (a file's
    top-level `PACKAGE`s, their `TYPEDEF`/`VALDEF`/`DEFDEF` members, a
    class's own header parameters/parents/self type) — never a flat "every
    `VALDEF`/`DEFDEF`/`TYPEPARAM` tag anywhere in the file" scan, which an
    intermediate version of this fix tried and which wrongly treated a local
    definition buried in a method's own `BLOCK` body as a root, turning two
    real `scala3-library` files ($eq$colon$eq, $less$colon$less: nineteen
    `LAMBDAtpt`s each reachable only from within a local method's body) into
    false-positive `Unaccounted` corpus failures — caught immediately by the
    corpus run itself, before merge. Five `reachability.rs` unit tests cover
    the final shape, each mutation-tested: an identity at a `VALDEF`'s/
    `DEFDEF`'s own declared-type/result position (`Unaccounted` when
    unentered), one inside an unsupported `APPLY` in a `DEFDEF`'s
    right-hand side (`OutOfScope`), the `poisoned`-shaped case (a loose tree
    named only through a link inside an unsupported `APPLY` argument,
    `OutOfScope`), and a local `DEFDEF` nested in an enclosing `DEFDEF`'s
    `BLOCK` body (`OutOfScope`, the regression the corpus run itself caught).

### Serialized symbol annotations (Milestone 5e1, tail indexing)

```text
ANNOTATION Length tycon_Type full_annotation_Type
```

Scala 3.9's `TreeUnpickler` reads a definition's annotations as part of its
modifier tail and stores them directly on the symbol denotation
(`Symbol.annotations: Vec<AnnotationId>` in `dotty-core`, still always empty
before this milestone). This is a distinct wire node from `ANNOTATEDtype`/
`ANNOTATEDtpt` (Milestones 4b1/5b): those pair an `underlying` type/tree with
its annotation directly; `ANNOTATION` splits out a `tycon` field alongside the
same kind of annotation tree (`full_annotation`), and only ever appears inside
a `TYPEDEF`/`VALDEF`/`DEFDEF`/`TYPEPARAM`/`PARAM`'s tail, never as a type's own
child.

This increment adds only the pass-1 half: an address-backed index of where
each `ANNOTATION` entry is, in wire order, with no semantic decoding yet
(`Symbol.annotations` is still not populated). Later increments build the
completion that turns an indexed address into an `AnnotationId`.

**A relative-offset trap.** `dotty-tasty`'s `DefinitionTail::Annotation`
carries the tail entry's own `RawNode`, whose `offset` field is *not* an
absolute AST address: `RawNode::reader()` always starts a fresh `Reader` at
position 0 over the node's own payload slice (`Reader::new(self.payload)`),
so every offset `read_definition_tail` records is relative to the enclosing
definition's payload, not the file's AST section. Nothing in the unpickler had
read that offset before, so nothing had needed to notice; the semantic
identity invariant this crate depends on (§3) makes every other address
absolute, and an index keyed by a payload-relative number would silently
collide across definitions and never match an address a real reference (or a
later completion call) names. This was caught and fixed *before* any address
was published: the actual addresses pass 1 indexes come from the AST's own
child edges instead (`AstView::children`, built top-down over absolute
offsets by `dotty-tasty`'s own address indexer), by filtering a definition's
or parameter's immediate children for the `ANNOTATION` tag — the same
mechanism every other structural child (a class's `TEMPLATE`, a method's
`TYPEPARAM`/`PARAM`) is already found through in `enter.rs`, not the
tail-decoded `RawNode` at all. `DefinitionTail`/`DeclaredModifiers::from_tail`
still skip `Annotation` entries entirely, exactly as before this milestone —
they answer only namespace/flags/visibility.

**The index.** `TastySemanticIndex::annotation_tail_at(address)` returns
`Some(&[u32])`, the `ANNOTATION` child addresses of the definition/parameter
entered at `address`, in wire order — `Some(&[])` when it was entered with no
`ANNOTATION` children, `None` when no symbol was ever entered there at all.
Distinguishing "entered, zero annotations" from "never entered" here (rather
than only at the eventual completion-state layer, §5e-to-come) means a survey
or a later completion call can tell a `TYPEPARAM`/`PARAM`/local definition
pass 1 skipped from one it indexed and found empty, without re-parsing the
tail itself. It is populated once, when `enter_symbol` allocates the symbol
(`enter_definition_header`'s three branches, `enter_parameter`), needs no
incremental-rollback bookkeeping of its own (pass 1's own failure path already
rolls back the whole index as one unit, unlike pass 2's per-address maps), and
is unaffected by unit order or retry.

**Fixtures.** `tests/fixtures/semantic/SymbolAnnotations.scala` (real Scala
3.9.0 compiler output, via `generate.sh`) covers a class annotation, an
annotated `val`/`def`/type alias, an annotated constructor value parameter, an
annotated method type parameter, a term parameter with two annotations
(`@SymbolMarker @SymbolTagged(tag = "p")`, keeping wire order), an
unannotated parameter and an unannotated class — the last one showing that
Scala 3.9 always attaches a compiler-synthesized `@SourceFile` annotation to
every top-level class, a genuine `ANNOTATION` wire entry rather than one of
the compiler-internal, non-serialized annotations §5e-to-come's completion
must not synthesize (`LazyBodyAnnotation` and friends). Eleven `enter.rs` unit
tests pin the exact indexed addresses and counts over these fixtures, each
mutation-tested.

**Corpus (Milestone 5e1 tail-indexing survey; `unaccounted: 0` throughout).**
`scala3-library`: 4091 `ANNOTATION` tail entries over 60407 entered
definitions/parameters, 3431 of them annotated (`TYPEDEF` 2235 entries/1682
annotated, `DEFDEF` 1318/1243, `VALDEF` 364/332, `PARAM` 120/120, `TYPEPARAM`
54/54); `scala3-compiler`: 4652 entries over 94685 entered, 3349 annotated
(`TYPEDEF` 3085/1876, `VALDEF` 1128/1035, `PARAM` 287/287, `DEFDEF` 152/151,
`TYPEPARAM` 0/0). Both corpora skew heavily toward one annotation per
definition (2944/2982 respectively) with a long tail up to 24 and 234 on a
handful of outliers. The counts are identical across every
builtins/completion/classes/reverse permutation the corpus test runs, as
expected: this survey reads only pass 1's own index, never completion.

### Serialized symbol annotations (Milestone 5e1, the shared decoder and completion)

**The shared payload decoder.** `TastyUnpickler::decode_annotation_payload(ast,
at, annotation_at, depth) -> Result<AnnotationId, UnpickleError>`, extracted
from `decode_annotated_type`'s inline compact/full split (§4's "Annotated
types" above), is the one lower-level annotation decoder now shared by
`ANNOTATEDtype` and serialized symbol annotations: compact when the payload's
own tag is `is_compact_annot_type_tag`, full otherwise (reusing
`decode_annotation_tree` unchanged). `ANNOTATEDtpt` keeps calling
`decode_annotation_tree` directly, since it has never wrapped a compact form
and this refactor's job was to preserve, not extend, existing behavior
(verified byte-identical corpus annotation counts before/after, plus a
mutation of the compact branch caught by 15 of the 55 `tests/annotated.rs`
tests).

**`complete_symbol_annotations`.** The `ANNOTATION` wrapper a tail entry
carries (`ANNOTATION Length tycon_Type full_annotation_Type`) is not
`ANNOTATEDtype`/`ANNOTATEDtpt`'s `{underlying, annotation}` pair: it splits
out a `tycon` field alongside the same kind of annotation tree
(`full_annotation`). `TastyUnpickler::complete_symbol_annotations(address) ->
Result<Vec<AnnotationId>, UnpickleError>` decodes every `ANNOTATION` tail
entry pass 1 indexed for `address` (`RawNode::decode_annotation` validates
the wrapper's shape), in wire order, by calling
`decode_annotation_payload` on each entry's `full_annotation` child — `tycon`
is read only far enough to validate that shape, never given independent
semantic weight: the full annotation tree's own root already carries the
constructor's type (a compact tag is a type outright; a full constructor's
`NEW`/`APPLY` spine resolves its own class), so nothing is lost by not
reading it a second time. If a real annotation is ever found where the two
disagree, that is future evidence to revisit this choice, not something this
milestone tries to detect. `complete_symbols_annotations(addresses)` is the
all-or-nothing batch entry, mirroring `complete_symbols`.

**Independence from `SymbolInfo`.** Annotation completion and `complete_symbol`
are separate semantic dimensions: neither inspects, sets or requires the
other, so a symbol's annotations can complete while its `SymbolInfo` is still
`Missing`, and completing `SymbolInfo` never touches `Symbol.annotations`.
Regression tests cover both directions, plus the specific case the issue asks
for: a symbol whose own completion already succeeded is untouched by a later,
unrelated annotation-completion failure on the very same symbol.

**Completion state, identity, atomicity.** `Vec::is_empty()` on
`Symbol.annotations` cannot tell "not completed" from "completed with zero
annotations", so a new adapter-local `annotations_completed: HashSet<SymbolId>`
on `TastyUnpickler` is the state a repeated call checks (idempotent: returns
the same `AnnotationId`s again, allocates nothing). Because arena truncation
cannot restore a *mutated field* of a symbol that already exists (the same
problem `SymbolInfo` completion already solved), a new `annotations_journal:
Vec<(SymbolId, Vec<AnnotationId>, bool)>` records each symbol's old
`Symbol.annotations` and old completion-state membership before a call
changes them, mirroring `info_journal` exactly (`Transaction.annotations`,
rolled back or cleared in `finish_transaction` the same way). This makes both
levels of atomicity issue #16–19 asks for fall out for free: a symbol's own
multiple annotations are only ever *written* once, after every one of them has
decoded successfully (so a mid-list failure never needs to "undo" a partial
write — there was never a partial write to undo), and a batch spanning several
symbols rolls back an *earlier* symbol's already-committed write through the
journal when a *later* one in the same batch fails. Each `ANNOTATION`
occurrence still owns its own `AnnotationId`, never merged with another equal
payload's, mirroring the existing `ANNOTATEDtype` occurrence-identity policy.
Both mutations (disabling the idempotence check; disabling the journal
rollback) are caught by dedicated `tests/symbol_annotations.rs` tests.

**A package address is a typed error, not a panic (review fix).** A package
symbol is entered through `enter_package`, which inserts it into the index
directly (`insert_symbol`) and never calls `insert_annotation_tail`: a
`PACKAGE` node has no `ANNOTATION` tail in the wire format at all, unlike
every `TYPEDEF`/`VALDEF`/`DEFDEF`/`TYPEPARAM`/`PARAM`, which always gets one
indexed (even empty) through `enter_symbol`. An initial version of
`complete_symbol_annotations` `.expect()`ed a tail to exist whenever a symbol
did, which is true for every address this milestone means to support but not
for a package address — a real, well-formed input, not a bug, so panicking on
it broke this crate's own "never panic on wire/caller input" invariant. Fixed
with a new typed `UnsupportedAnnotationCompletion { address, kind }`, and two
regressions (`complete_symbol_annotations` alone, and inside a batch) pin a
package address failing cleanly instead.

**Fixtures and tests.** 22 `tests/symbol_annotations.rs` tests exercise every
fixture shape from the tail-indexing increment above through the real
completion API (basic completion, identity, idempotence, `SymbolInfo`
independence in both directions, partial-multi-annotation atomicity, and
batch all-or-nothing), stubbing in `SymbolMarker`/`SymbolTagged` (the
fixture's own annotation classes, defined in sibling units) and
`scala.annotation.internal.SourceFile` (the compiler's own synthesized
annotation on every top-level class, confirmed real rather than assumed: an
early guess at its class name/package was verified directly against the real
fixture's decoded output before being written into the tests). One finding
worth noting for anyone reading wire order literally: `@SymbolMarker
@SymbolTagged(tag = "p")` in source decodes with `SymbolTagged` *first* in
`ANNOTATION` tail order — the wire order is not simply left-to-right source
order, and no code here assumes it is beyond "whatever order pass 1 indexed
them in, is the order completion preserves".

**Completion-outcome corpus survey.** `tests/type_corpus.rs` completes every
definition/parameter with at least one indexed `ANNOTATION` entry through the
real `complete_symbol_annotations` and classifies the result, `unexpected: 0`
throughout. Like every other unit-only, single-file completion measurement in
this document (`TYPELAMBDAtype`/`PARAMtype` after 3a, `APPLIEDtype` after
2c1, ...), most failures are `external` — an annotation class this
one-unit-at-a-time session never entered, which a classpath resolver
(Milestone 6) is expected to unlock — and the two corpora diverge sharply
because of it: `scala3-library` decodes most of its 3431 attempts (2108,
order-dependent: 943–2135 across the corpus test's permutations, 1084–2362
external, plus 176 `UnsupportedAnnotationConstructor` and 36
`UnsupportedAnnotationArgument`, both stable across permutations since they
never depend on resolution order), while `scala3-compiler`'s 3349 attempts
decode 0 — its own annotation classes are defined in units later in
compilation order than this survey ever enters, so every one of them (plus a
stable 8 `UnsupportedAnnotationConstructor`) is `external`. No
`InvalidAnnotationType`, `UnsupportedAnnotationTree` or `MalformedType`
occurs in either corpus.

**Cross-adapter storage convergence with `dotty-classloader`.**
`dotty-classloader/tests/cross_adapter_annotations.rs` confirms both adapters
share the "attach `AnnotationId`s to `Symbol.annotations`, `tree` always
`None`, annotation type always class-like" contract (issue #129 §33) —
`ClassLoader::enter_annotations` (JVM classfile `RuntimeVisibleAnnotations`)
on one side, `complete_symbol_annotations` on the other. Milestone 6
(classloader/`SymbolResolver` integration) has not landed, so the two
adapters cannot yet run against one shared `SemanticStore` — `ClassLoader::new`
bootstraps its own `Definitions` internally, and `Definitions::bootstrap` is
documented as exactly-once per store — so the test runs each adapter over its
own store and compares the contract their results satisfy, not one shared
arena. One divergence is documented and asserted rather than hidden:
`dotty-classloader` does not (yet) map a classfile's decoded element values
into `AnnotationArguments::Known` (they stay exclusively in its own
JVM-facing `SemanticAnnotation` sidecar), so every classfile annotation is
`Unavailable` regardless of its real element count, while an argument-free
TASTy annotation is `Known([])` — a real, allowed divergence per §33 ("do not
require equal payloads"), not a bug either side needs to fix for Milestone
5e1.

Milestone 5e1 is complete.

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
   - 5b: `SELECTtpt`, `SINGLETONtpt`, `ANNOTATEDtpt` and the narrow term-`tpe`
     projection they need — complete;
   - 5c: ordinary `DEFDEF` methods and `LAMBDAtpt` with its local type
     parameters, over a symbol-abstraction primitive in `dotty-core` — complete;
   - 5d1: `ClassInfo` for classes, traits and module classes, parents, self
     types and cross-unit declaration scopes — complete;
   - 5d2a: constructor completion (`normalizeIfConstructor`, the effective
     owner-class result) — complete;
   - 5d2b: `REFINEDtpt`'s synthetic refinement class, `close_over_this` and the
     `Refined`/`Recursive` projection — complete;
   - 5d2c: semantic identity discovery parity — a mode-aware pass-1 walker
     mirroring every supported projection route to `LAMBDAtpt`/`REFINEDtpt`
     — complete; Milestone 5d is closed;
   - 5e: symbol annotations, companion links, opaque aliases and the
     remaining tails, in steps:
     - 5e1: serialized symbol annotations — pass-1 `ANNOTATION` tail
       indexing, the corpus survey, the shared payload decoder,
       `complete_symbol_annotations`/`complete_symbols_annotations`, the
       completion-outcome corpus survey and the `dotty-classloader`
       storage-convergence check all landed — complete;
     - 5e2: companion links — identity discovery, pair validation and
       `SymbolLinks::companion` publication after the complete identity walk —
       complete;
     - 5e3: opaque aliases and the remaining tails.
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
- the modifiers listed in §4 (no matching core flag, variance, accessor roles);
- an abstract type member is entered as `TypeAlias`; `SymbolKind` has no
  abstract-type kind;
- definitions inside method bodies (locals) and the parameters of type-lambda
  aliases;
- signed `TERMREFin`, and every other type form beyond §4 "Types" —
  `UnsupportedType`;
- signed term references and inherited members (§4, "Name-based references");
  cross-unit class members resolve through a completed `ClassInfo` only;
- a `REFINEDtpt` reached only through a `SELECTtpt` qualifier's own type
  resolution, or only through a `SHAREDtype` (rather than `SHAREDterm`) link
  to it, is not entered in pass 1 and fails projection with the explicit
  `MissingRefinementClass`/`InvalidRefinementClass` rather than completing (2
  library self-types measured, §5d2b below);
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
| `Class`, `Trait`, `ModuleClass`, `Method`, `Constructor` | 1,234, 669, 944, 15,645, 2,921 (library) | 0 | kind deferred (5d), all still `Missing` |

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
declarations being reachable from another unit (5c). Separately, the
special aliases: the unpickler declares `scala.&` / `scala.|` on its first
type-tree projection or completion (`Definitions::declare_special_aliases`;
no file declares them). That makes the `scala` package exist, which by itself
adds decodes against the pre-5a numbers (by address 224,977 -> 226,200 in the
library and 329,361 -> 350,003 in the compiler without builtins; named `TYPEREF`
10,088 -> 10,329 and 5,435 -> 5,647). The corpus test declares them in every
run, with and without completion, so the completion figures above are
completion alone.

### Selected, singleton and annotated trees after 5b (library / compiler)

Wire shapes (all real nodes, whatever they decode to; identical with and
without builtins). `SHAREDterm` roots are reported by the tag their chain ends
at.

| tree | root population |
|------|-----------------|
| `SELECTtpt` qualifier, library (5,445) | `SHAREDtype` 2,096, `SELECT` (112) 2,981, `TERMREFsymbol` 14, `TERMREF` 116, `TERMREFdirect` 37, `TERMREFpkg` 180, `APPLIEDtpt` 14, `REFINEDtpt` 3, `IDENTtpt` 1, 3 through one `SHAREDterm` (2 to `SHAREDtype`, 1 to `SELECT`) |
| `SELECTtpt` qualifier, compiler (7,836) | `SHAREDtype` 2,896, `SELECT` 4,532, `TERMREFsymbol` 5, `TERMREF` 180, `TERMREFdirect` 17, `TERMREFpkg` 148, `QUALTHIS` 47, `IDENTtpt` 10, `SELECTtpt` 1 |
| `SINGLETONtpt` ref, library (2,566) | `SHAREDtype` 1,111, `SELECT` 271, `TERMREFsymbol` 740, `TERMREF` 11, `TERMREFdirect` 349, `THIS` 49, `QUALTHIS` 12, `TRUEconst` 11, `FALSEconst` 6, `INTconst` 6 |
| `SINGLETONtpt` ref, compiler (2,807) | `SHAREDtype` 831, `SELECT` 540, `TERMREFsymbol` 1,141, `TERMREF` 11, `TERMREFdirect` 176, `THIS` 14, `QUALTHIS` 32, `TRUEconst` 47, `FALSEconst` 4, `STRINGconst` 4, `INLINED` 7 |
| `ANNOTATEDtpt` base, library (1,878) | `APPLIEDtpt` 1,411 (+2 shared), `IDENTtpt` 407, `ANNOTATEDtpt` 29, `SELECTtpt` 14, `SINGLETONtpt` 14, `REFINEDtpt` 1 |
| `ANNOTATEDtpt` base, compiler (121) | `APPLIEDtpt` 89 (+7 shared), `IDENTtpt` 20 (+3 shared), `SELECTtpt` 2 |
| `ANNOTATEDtpt` annotation, library / compiler | `APPLY` 1,715 / 64, `SHAREDterm -> APPLY` 163 / 57 (chain length 1); no other root; class trees `IDENTtpt` 269 / 60, `SELECTtpt` 1,486 / 0, `TYPEREF` 60 / 46, `SHAREDtype` 62 / 15; 0 term arguments in all 1,878 / 121; one library spine is `APPLY(TYPEAPPLY(SHAREDterm ..))`, whose function is a link and stays `UnsupportedAnnotationConstructor` |

The term forms therefore needed are exactly: `SHAREDterm`, `SELECT`,
`QUALTHIS`, the semantic type nodes, a few tree tags, and constants for
`SINGLETONtpt`. `IDENT` never occurs (it is supported and unit-tested); the
only unsupported real term is `INLINED` (147): 4 library and 10 compiler
symbols end at `UnsupportedTermTree`, all reported, none guessed.

Completion, rerun unchanged on the wider projection (kinds and order as in 5a;
library no builtins -> builtins / compiler no builtins -> builtins; delta over
5a):

| kind | 5a -> 5b | delta |
|------|----------|-------|
| `Field` (`VALDEF`) | 726 -> 829 / 3,463 -> 3,497 becomes 730 -> 844 / 3,469 -> 3,503 | +4, +15 / +6, +6 |
| `Field` (`PARAM`) | 971 -> 1,005 / 535 -> 545 becomes 972 -> 1,006 / 535 -> 545 | +1, +1 / 0 |
| `Parameter` | 11,296 -> 12,408 / 5,941 -> 7,618 becomes 11,601 -> 12,760 / 6,206 -> 7,880 | +305, +352 / +265, +262 |
| `TypeParameter` | 2 -> 14,684 / 2 -> 956 becomes 19 -> 14,703 / 2 -> 956 | +17, +19 / 0 |
| `TypeAlias` | 15 -> 135 / 627 -> 654 becomes 32 -> 152 / 736 -> 761 | +17, +17 / +109, +107 |
| `Object` | 944 / 2,170 | 0 |

By tree (library with builtins, `outcomes_by_new_trees` in the report):
symbols whose declared tree contains a `SELECTtpt` complete 315 times (others:
external 544, refused inside by `REFINEDtpt`/`LAMBDAtpt` 75, `INLINED` 4, prefix
still `Missing` 2, method/class kinds 1,094 out of scope); with a
`SINGLETONtpt` 36 (external 4, refused 9); `ANNOTATEDtpt` alone 48 (external
90, kinds out of scope 51). The exact per-kind, per-tree table is the report's
"completed symbols by kind and 5b trees" line. What remains refused by tag,
library: `LAMBDAtpt` 353, `REFINEDtpt` 11; compiler: `LAMBDAtpt` 78, `REFINEDtpt`
3 (all `SELECTtpt`, `SINGLETONtpt` and `ANNOTATEDtpt` refusals of 5a are
gone). 0 unexpected errors.

Failure migration of the completion attempts on the wider projection
(library, builtins): external child failures (a class of another unit) are
the large bucket; no completion ends as `UnstableSelectQualifier` or
`InvalidSingletonTypeTree`, so every real singleton and every real select
qualifier was stable and a singleton. The report cannot tell an annotation
class from another external type inside one tree, so "annotation type
external" is part of the external bucket, not counted apart. The 2 (library)
and 43 (compiler) `Parameter` symbols whose tree selects from a term that is
still `Missing` are `UnsupportedResolutionPrefix`; the one probed (library
`Mirror`, the qualifier is an earlier `PARAM`) is a dependency whose own
completion had not succeeded, not a projection gap.

Member resolution. Rerunning the reference corpus after completion moves
nothing that 5a did not: named `TYPEREF` + `TERMREF` unsupported prefixes stay
73 (library) and 291 (compiler), the by-address decodes are unchanged in the
compiler and +4 in the library (the 5 `ANNOTATEDtype` nodes whose `NEW` class is
a `SELECTtpt`, 2 direct and 3 shared, were `UnsupportedAnnotationConstructor`;
that outcome is now 0). Every path that gets past the prefix still meets a
class declared in another unit: cross-unit declaration scopes (5d) remain the
next blocker for reference decoding, not the projection.

### Lambdas and methods after 5c (library / compiler)

`LAMBDAtpt` roots: 390 (library) / 83 (compiler). Contexts: type alias
right-hand side or class parent 233 + 3 nested / 83; type parameter bounds
146 + 8 nested / 0. Type parameters per lambda: 1 (230 / 81), 2 (157 / 2), 3
(3 / 0). Bounds roots: `TYPEBOUNDStpt` 545 / 85, `LAMBDAtpt` 8 / 0. Body roots:
`TYPEBOUNDStpt` 252 / 20, `APPLIEDtpt` 106 / 60, `MATCHtpt` 22 / 0, `REFINEDtpt`
6 / 1, `IDENTtpt` 2 / 2, `ANNOTATEDtpt` 1 / 0, `LAMBDAtpt` 1 / 0. 202 / 82 refer
to their own parameters. No parameter carries a variance modifier and no
`SHAREDterm` links to a lambda, so shared-owner conflicts are 0 in both corpora
(covered by synthetic wire). 366 of 390 library lambdas (all 83 compiler) are in
declared type-tree positions and have their parameters entered; the 24 others
(class parents, 5d) are not entered, and projecting one would be
`MissingEnteredSymbol`.

Method shapes (ordinary `DEFDEF`s, library / compiler; constructors are
surveyed apart and stay `Missing`):

| | library | compiler |
|---|---|---|
| methods / constructors | 15,645 / 2,921 | 32,926 / 4,525 |
| no parameter clause | 3,647 | 9,241 |
| explicit `EMPTYCLAUSE` (methods) | 2,086 | 4,045 |
| `SPLITCLAUSE` 1 / 2 / 3 | 816 / 84 / 2 | 5,443 / 391 / 1 |
| term clauses 0 / 1 / 2 / 3 / 4 | 4,249 / 10,453 / 829 / 108 / 6 | 9,428 / 17,369 / 5,720 / 408 / 1 |
| type clauses 0 / 1 / 2 | 12,218 / 3,360 / 67 | 32,061 / 857 / 8 |
| deepest clause list | 6 | 5 |
| widest clause | 44 | 15 |
| top sequences | `P` 6,196, none 3,647, `()` 2,078, `T P` 1,844, `T` 602, `T P implicit` 219, `T implicit` 203 | `P` 11,471, none 9,241, `P using` 4,521, `()` 3,755, `using` 1,803 |
| methods mentioning an own parameter | 3,157 | 863 |

Own-parameter references (methods): across clauses 3,150 / 863; a type
parameter in a parameter type 2,737 / 648; a term parameter in the result 354 /
173; a type parameter in the result 345 / 33; a type parameter in a type
parameter's bounds (F-bounds) 32 / 1; a term parameter in a parameter type 7 /
12. So abstraction is exercised by thousands of real signatures, not only by
synthetic tests. Parameter modifiers on method parameters: `SYNTHETIC` 465 /
7,924, `GIVEN` 1,518 / 7,215, `HASDEFAULT` 514 / 2,199, `IMPLICIT` 590 / 592,
`INLINE` 21 / 111; `ERASED`, `TRACKED` and `INTO`: 0 / 0 (`ERASED` is covered by
the real `ErasedParams` fixture and synthetic wire). No clause mixes `GIVEN` and
`IMPLICIT` parameters. Repeated parameters were not counted separately: `T*` is
the `<repeated>` type applied like any other, with no method-level adaptation
to defer.

Constructors (for 5d): 2,921 / 4,525; a leading type clause 1,011 / 181 (owner
class arity 1..13 in the library, 0..3 in the compiler); the sequence `()` alone
1,566 / 2,826, `T P` 523 / 52, `T ()` 360 / 47, `P` 342 / 946, `P using`
(compiler) 302; `EMPTYCLAUSE` 1,932 / 2,935; return trees `SHAREDtype` 1,980 /
3,377 and `TYPEREF` 940 / 1,148; own-parameter references (type parameters of
the class in parameter types) 636 / 94; parameter modifiers `GIVEN` 1,087 / 397,
`IMPLICIT` 30 / 235, `HASDEFAULT` 22 / 249; `TRACKED` and `ERASED` 0.

Completion, library (no builtins -> builtins) / compiler; the simple symbols are
completed first and the methods afterwards, so the simple figures compare with
5b:

| kind | 5b | 5c | note |
|------|----|----|------|
| `Method` | `Missing` | 6,133 -> 8,569 of 15,645 / 2,327 -> 2,549 of 32,926 | |
| `Constructor` | `Missing` | `Missing` (2,921 / 4,525, `ConstructorCompletionDeferred`) | |
| `TypeAlias` | 32 -> 152 / 736 -> 761 | 32 -> 299 / 736 -> 827 | +0, +147 / +0, +66 (lambda right-hand sides) |
| `TypeParameter` (completed, incl. by a lambda) | 19 -> 14,703 / 2 -> 956 | 19 -> 15,315 / 2 -> 1,033 | +612 (519 newly entered lambda parameters) / +77 (85) |
| `Parameter`, `Field`, `Object` | as 5b | same | |

Method failures (library builtins): external child failure 7,042; unsupported
parameter semantics (`INLINE`) 15; `REFINEDtpt` / `MATCHtpt` in a signature 13;
dependency still `Missing` 4; `INLINED` term 2. Compiler builtins: external
30,251, `INLINE` 86, dependency still `Missing` 40. No completion cycle,
malformed clause, abstraction failure, unstable qualifier or invalid singleton
occurs; unexpected errors: 0. Refusals by tag now: `REFINEDtpt` 30 (11 without
builtins) and `MATCHtpt` 20 in the library, `REFINEDtpt` 4 in the compiler:
projecting lambda bodies and method results reaches trees that were hidden
behind the lambda refusal. `LAMBDAtpt`: 0.

`SymbolInfo` after 5c (library builtins): `Method` `Complete` 8,569 /
`Missing` 7,076 (5b: 0 / 15,645); `Constructor` `Missing` 2,921;
`Class`/`Trait`/`ModuleClass` `Missing` 1,234 / 669 / 944; `Parameter` 12,760 /
6,392; `TypeParameter` 15,315 / 369; `TypeAlias` 299 / 138; `Field` 1,850 / 837.
Compiler builtins: `Method` 2,549 / 30,377; `Constructor` `Missing` 4,525;
`Class`/`Trait`/`ModuleClass` `Missing` 2,080 / 242 / 2,170; `TypeParameter`
1,033 / 360; `TypeAlias` 827 / 292.

Reference and type decodes. Entering lambda parameters lets references to them
by address resolve: by address 226,204 -> 226,548 (library) and 350,003 ->
350,094 (compiler), builtins 271,385 -> 271,729 and 372,271 -> 372,362. Named
`TYPEREF`/`TERMREF` decodes and their unsupported prefixes (73 / 291) are
unchanged, and so are `APPLIEDtype`, `TYPEBOUNDS`, `ANNOTATEDtype` and
`REFINEDtype`: method infos are not read by reference decoding, and classes
declared in other units (cross-unit `ClassInfo`/scopes, 5d) remain the
dominant blocker (most method failures are `UnresolvedMember` / `UnresolvedPackage`).

### Classes after 5d1 (library / compiler)

Wire survey (independent of what completes), library / compiler. Classes by
number of parents: `Class` 1 / 2 / 3 / 4 / 5+ = 508 / 467 / 90 / 133 / 36 (lib)
and 1,151 / 274 / 324 / 264 / 67 (compiler); `ModuleClass` 718 / 190 / 31 / 2 /
3 and 1,091 / 874 / 63 / 42 / 100; `Trait` 352 / 142 / 129 / 23 / 23 and 162 /
73 / 6 / 0 / 1. Parent roots: an `APPLY` constructor call for every `Class` and
`ModuleClass` superclass (1,234 + 944 / 2,089 + 2,170), a `BLOCK` 0 / 4, and
type trees otherwise (`APPLIEDtpt` 1,068 / 266, `IDENTtpt` 665 / 862, `SELECTtpt`
419 / 1,721, `SHAREDtype` 295 / 481, `TYPEREF` 296 / 509). No parent is a
`SHAREDterm`, so that branch is covered by synthetic wire only. Constructor
spines: `APPLY>SELECTin>NEW>` then `SHAREDtype` 989 / 2,010, `TYPEREF` 459 / 792,
`IDENTtpt` 243 / 870, `SELECTtpt` 4 / 17; `APPLY>TYPEAPPLY>SELECTin>NEW>` then
`APPLIEDtpt` 425 / 93, `APPLIEDtype` 22 / 0, `SELECTtpt` 18 / 0, `IDENTtpt` 1 / 3;
curried (`APPLY>APPLY>...`) 17 / 468. Arguments per `APPLY`: 0 for 1,962 / 3,615
(1: 128 / 972, 2: 66 / 99, 3-7: 39 / 56). `BLOCK` parents: 2 and 3 statements,
2 each (compiler only).

`TYPEAPPLY` parents: the `NEW` type is already applied (the arguments are
skipped) 440 / 149; it is bare, so the arguments are applied, 43 / 60. Whether
upstream's zero-type-parameter compatibility branch occurs cannot be told from
the wire or without completing the parent class (a bare `NEW` type over a class
with no type parameters would be the case); nothing in the corpora produced a
malformed `Applied`, but the argument count is not checked against the class's
arity, which needs the parent class completed.

Completed parents by shape (Object stubbed): `TypeRef` 2,154 (of which none an
alias) and `Applied` 557 in the library; every parent is one of the two. Self
definitions: library 23 `Class` (roots `ANNOTATEDtpt` 10, `APPLIEDtype` 4,
`APPLIEDtpt` 3, `TERMREFpkg` 3, `SHAREDtype` 2, `IDENTtpt` 1), 155 `Trait` (118
`SINGLETONtpt`, ...) and all 944 `ModuleClass`es (`SINGLETONtpt`); compiler 61 /
33 / 2,170. None has a nested `LAMBDAtpt`, so the parent/self lambda scan is
covered by real fixtures and synthetic wire. Of the self trees, 13 + 140 + 943
decode and 10 + 15 external in the library (compiler: 60 + 18 + 2,170, 1 + 15).

Completion, path order, library / compiler (`ClassInfo` field sanity, for every
completed class: `class`, `prefix == no_prefix`, the exact pass-1 scope, the
scope's owner, parent count and self presence match the wire: 389 / 224 classes
without the stubs, 1,952 / 1,700 with them):

| kind | 5c | 5d1 (builtins) | 5d1 (+ `Object`, `AnyRef` stubs) |
|------|----|----------------|----------------------------------|
| `Class` | `Missing` | 325 of 1,234 / 111 of 2,080 | 535 / 437 |
| `Trait` | `Missing` | 47 of 669 / 46 of 242 | 556 / 182 |
| `ModuleClass` | `Missing` | 17 of 944 / 67 of 2,170 | 861 / 1,081 |
| `Method` (class completion first) | 8,569 / 2,549 | 9,450 / 2,718 | 10,928 (5c with the stubs: 9,648) / 5,403 (4,231) |

The stubs are empty classes for `java.lang.Object` and `scala.AnyRef`, which the
compiler and the classpath supply: every class has one of them as a parent, so
without them every class fails there first. Nothing else is stubbed (the
library corpus defines the primitive classes itself, in later units). Where the
classes that fail with the stubs fail, library: header parameter 117 (`Class`
108, `Trait` 9), parent 351 (`Class` 210, `ModuleClass` 51, `Trait` 90 incl. 2
`REFINEDtpt`), self type 10 (`Class` 2, `Trait` 8), 416 whose failing reference
lies in a shared link's target, and 1 ambiguous member of a module class; every
other one is an `UnresolvedMember` or `UnresolvedPackage`. The unresolved names are classpath and later units, not `ClassInfo`:
`package` (package objects) 89, `java.util.function` 86, `Type` 66, `_root_` 61,
`util` 43, `java.util` 32, `ImpureFunction1` 30, `scala.math` 29,
`scala.collection.*` 37; compiler: `scala.deriving` 517, `_root_` 244,
`Predef` 190, `scala.reflect` 174, `scala.collection.immutable` 169, `Int` 109
(the compiler corpus has no library), `dotty.tools.dotc.core` 107. Unexpected
errors: 0.

Reference decodes with class completion first, library / compiler, builtins
(without the stubs; with them in parentheses), against the same run without
class completion:

| | before | after |
|---|--------|-------|
| named `TYPEREF` decoded | 11,925 / 7,621 (13,215 / 9,204) | 12,248 / 11,097 (14,569 / 20,836) |
| named `TERMREF` decoded | 1,742 / 9,174 (1,742 / 9,174) | 1,762 / 9,790 (1,954 / 14,948) |
| decoded by address | 271,729 / 372,362 (281,578 / 385,117) | 273,533 / 397,438 (285,898 / 457,396) |
| named references, unsupported prefix | 65 / 265 | 65 / 241 |

Every gain is a former `UnresolvedMember` (the external counts drop by the same
number: 323 / 3,476 named `TYPEREF`), and the compiler's unsupported-prefix
references fall by 24. The `REFin` measurement is unchanged: **0** of 115 / 12
`TYPEREFin` decode in path order, in either run. The oracle explains why: the
108 library queries that reach it all have one owner class, whose completion
fails on `member Int` (`scala.Int` is defined by a *later* unit, `Int.tasty`);
the compiler's 2 fail on `member package`. No `REFin` owner ends the corpus with
a `ClassInfo`, so the mechanism is proved by the two-unit test
(`tests/class_scopes.rs`), not by the corpus.

*Processing order is a lower bound.* The manifest corpora are processed in path
order and a class can only use units entered before it. The same run in the
reverse order (stubs) completes `Class` 120 / 329, `Trait` 377 / 191,
`ModuleClass` 765 / 1,072 and `Method` 4,151 / 4,898; named `TYPEREF` decodes
7,387 / 12,212. Neither order is a maximum: the two bracket what a schedule that
retries a class after its dependencies are entered could reach. That needs an
orchestration layer (6), not a registry in the unpickler.

Findings for later milestones: `_root_` is written as a package name by 276
(library) / 764 (compiler) `TERMREFpkg` nodes and resolves to no package
(61 / 244 class failures): the root package should be the empty path, as
`<root>` already is. The `package` failures (89 / 33) are *not* a naming bug:
entering `scala/package.tasty` does put `package` (term) and `package$` (type)
in the `scala` scope. They are references entered before the defining unit in
path order (226 of 405 library references to `<package>.package` name a unit that
sorts later) or to a library the compiler corpus does not contain (549 of 584
compiler references are to `scala.package`); they need the retry/orchestration
layer and the classpath, not a fix here.

### Constructors after 5d2a (library / compiler)

7,446 constructors total (2,921 library / 4,525 compiler, matching PR #85's
count exactly — no constructor gained or lost an entry). Completion, path
order, with the `Object`/`AnyRef` stubs, class completion first:

| corpus | entered | completed | external child failure |
|--------|---------|-----------|--------------------------|
| library | 2,921 | 2,264 (77.5%) | 657 |
| compiler | 4,525 | 0 (0%) | 4,525 |

The library number is real signal: it rises through the run permutations
exactly as class completion's own numbers do (2,147 / 2,197 / 2,202 / 2,264
across the four library configurations), tracking the same processing-order
and classpath dependencies class completion already has, because a
constructor's *parameters* go through the same completion path as an
ordinary method's — only its own *result* is classpath-independent. The
compiler number is not a constructor-completion regression: **every**
compiler-corpus constructor fails on a parameter or the discarded return
tree needing a library type the compiler-only corpus does not contain (the
same "the compiler corpus has no library" limitation 5d1's measurement
already found for classes), confirmed by `unexpected errors: 0` in every one
of the twelve run permutations and by neither `ConstructorOwnerNotClassLike`
nor `MalformedOwnerClassInfo` appearing even once anywhere in the corpus —
every entered constructor has a well-formed class-like owner.

Class and ordinary-method completion counts are byte-for-byte unchanged from
the 5d1/5c measurements at every one of the twelve run permutations (`class
completion (5d1)` reports the identical `1,959 / 1,700 / 224 / 209 / 394 /
224 / 0 / 0 / 1,262 / 1,592` `ClassInfo` counts, and `Method (DEFDEF)`
reports the identical completed/failed counts): constructor completion adds
new completions without touching any existing one.

**Update (#90):** `_root_` now resolves like `<root>` (`package_segments`
drops a leading, or lone, `_root_` segment the same way). Rerunning the
measurement above: the library run (with the `Object`/`AnyRef` stubs)
`UnresolvedPackage` total for class completion drops from 303 to 241 (−62,
close to the reported 61, since a few of those references also occur outside
a `ClassInfo` failure's *first* reported cause) and the number of completed
`ClassInfo`s rises from 1,952 to 1,959; `_root_` no longer appears in the
unresolved-names table, replaced by the next thing that unit needed (e.g.
`Serializable`). The compiler run's `UnresolvedPackage` total drops from
1,541 to 1,297 — exactly 244, matching the reported count — but the number of
completed classes is unchanged (1,700): with `_root_` resolved, those classes
now fail one step later on an `UnresolvedMember` (`parent` failures rise from
165 to 281), because the compiler corpus still has no library to complete
`scala.*` members against. The `package` (package-object) finding above is
unaffected and remains open for the orchestration layer, not this fix.

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
| `MATCHtpt`, `SINGLETONtpt`, `IDENTtpt`, `SELECTtpt`, `LAMBDAtpt`, ... | type trees | not `unpickle_type` inputs: projected by `unpickle_type_tree_type` (5a/5b) where supported, typed-tree rehydration (Milestone 7) otherwise |
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

## 10. Opaque aliases (Milestone 5e3)

Opaque alias completion follows the Scala 3.9 `opaqueToBounds` split. The
alias symbol publishes only external `Bounds` (or a `TypeLambda` returning
bounds). If the RHS can be resolved, its implementation is retained as a
same-named `AliasingBounds` refinement in the defining class's self type.
Generic public bounds and implementations use type lambdas whose parameter
references point to their own canonical binders. Bounds are projected before
the private implementation, so an unavailable implementation type does not
hide public bounds. The owner is completed when possible; otherwise its
implementation is held in the TASTy session and applied when the owner's full
`ClassInfo` is later completed. A cycle or malformed tree remains an error and
the completion transaction rolls back both public and owner state.

The ignored corpus survey `opaque_corpus` uses the pinned Scala 3.9 library and
compiler fixtures, with only `scala.Any`/`Nothing`/`Null`, `java.lang.Object`,
and `scala.AnyRef` supplied as synthetic classpath stubs. It first enters every
unit, then completes aliases using each unit's saved address index; this keeps
all cross-unit declarations available before type projection. The audit asserts
that the complete semantic result is identical in both path orders. All 4
library aliases (3 generic) complete with public bounds and owner-local
implementations. Of 24 compiler aliases (5 generic), 22 complete in both
orders; one needs the unavailable `scala.reflect` package and one is malformed.
Three compiler aliases had a resolvable implementation retained in an owner
self type.

The survey filters opaque-marked class definitions out of the alias count and
reports unresolved external references separately from malformed,
unsupported, cyclic and unexpected cases. Run it with:

```text
cargo test -p dotty-tasty-unpickler --release --test opaque_corpus -- --ignored --nocapture
```

## 11. Review of Milestone 1

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
