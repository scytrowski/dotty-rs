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
  implemented (§4, "Bounds, flexible and constant types"). Binder types
  (Milestone 3) are next; they also close variance-bearing `TYPEBOUNDS`.

Every entered symbol is still `SymbolInfo::Missing`: types are decoded on
request by address and are not yet attached to symbols.

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
| 2d. Binders and advanced types | `TypeLambda`, `Method`, `Poly`, refinements, ..., with `TypeArena::reserve`/`fill` | 3–4 |
| 3. Complete | `SymbolInfo::Complete(TypeId)`, `ClassInfo`, annotations | 5 |
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
  then wraps it. There is no `TypeLambda` decoder yet, and inventing a
  bounds-level variance field would be wrong, so a `TYPEBOUNDS` with markers is
  `UnsupportedBoundsVariance` (refused before any child is decoded). It closes
  with the binder milestone.
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
  spelling (`string_value`). Known
  limit: `dotty-tasty` rejects a name table that is not valid UTF-8, so a
  string with an unpaired surrogate cannot reach the unpickler and
  `Constant::StringUtf16` is not produced from TASTy yet. `CLASSconst` stores
  the decoded child type; it is not a class-symbol reference.
- A `SHAREDtype` to any of these returns the target's exact `TypeId`, and
  nothing is interned structurally: equal constants at different addresses
  keep different ids.

Everything else is `UnsupportedType { tag, address }`: it is never lowered to
`NoType`, `NoPrefix` or `Error`. This includes `TYPEREFin` / `TERMREFin`,
`ANNOTATEDtype`, `TYPELAMBDAtype`, method/poly/param types, refinements,
recursive and match types.

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
3. Binder types (`Method`, `Poly`, `TypeLambda`, `ParamRef`), including the
   variance-bearing `TYPEBOUNDS` that need a `TypeLambda`.
4. Advanced types (refinements, recursive, match types, annotations, ...).
5. Symbol completion (signatures, parents, self types, `ClassInfo`).
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
- `TYPEREFin`/`TERMREFin`, and every other type form beyond §4 "Types" —
  `UnsupportedType`; `TYPEBOUNDS` with variance markers —
  `UnsupportedBoundsVariance` until `TypeLambda` exists;
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
decoder yet (most often `TYPEBOUNDS` and `ANNOTATEDtype`). No `APPLIEDtype`
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

Nodes with no decoder yet (instances in the corpus):

| node | library | compiler |
|------|---------|----------|
| `ANNOTATEDtype` | 2,014 | 5,409 |
| `TYPELAMBDAtype` | 739 | 148 |

The 2c1 measurement counted `TYPEBOUNDS` 853 / 922 (two-sided 806 / 223,
alias-only 47 / 699, with variance 90 / 0) and `FLEXIBLEtype` 238 / 552 as
undecoded; the first two are now decoded (or explicitly deferred) as above.
`ANNOTATEDtype` belongs to the annotation work and `TYPELAMBDAtype` to the
binder milestone.

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
