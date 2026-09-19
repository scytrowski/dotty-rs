# TASTy semantic unpickler

Status (`crates/dotty-tasty-unpickler`):

- Milestone 1, semantic index and symbol entering: complete.
- Milestone 2a, core type identity and reference resolution: implemented
  (§4, "Types").
- Milestone 2b, compound non-binder types: next.

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

References to symbols defined outside the current TASTy unit will go through
an external resolver abstraction (`SymbolResolver`, Milestone 6). The unpickler
never loads files itself.

## 2. Multi-pass architecture

TASTy has forward references, shared nodes, recursive binders and mutually
referencing symbols, so the unpickler is not a `RawTree -> Tree<Typed>`
function. It follows an enter-before-complete model:

| Pass | Result | Milestone |
|------|--------|-----------|
| 1. Enter | symbols, owners, declaration scopes; `SymbolInfo::Missing` | 1 |
| 2a. Type identity | address-keyed `TypeId`s; references, `THIS`, `SHAREDtype` | 2a |
| 2b. Types | compound types (`Applied`, bounds, `And`/`Or`, ...), then binders with `TypeArena::reserve`/`fill` | 2b–4 |
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
tests live in `dotty-classloader`. Which adapter runs first, and who owns the
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

Everything else is `UnsupportedType { tag, address }`: it is never lowered to
`NoType`, `NoPrefix` or `Error`. This includes the name-based `TYPEREF` /
`TERMREF` (a member looked up by name in a prefix needs the resolver; a
heuristic lookup by name is exactly what the address model avoids) and
`TYPEREFin` / `TERMREFin`.

`unpickle_type` is atomic in the same way as `enter_symbols`: on failure every
type it allocated is freed (`SemanticStore::checkpoint` / `rollback_to`) and
every address it recorded is forgotten, so a failure half way through a prefix
chain leaves nothing reachable.

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
   - 2b: compound non-binder types (`SuperType`, constants, `Applied`, bounds,
     `And`/`Or`, `ByName`) — next. The measurement in §8 shows that name-based
     `TYPEREF`/`TERMREF` are the largest remaining gap, so 2b should decide how
     they resolve against the entered units before the resolver exists.
3. Binder types (`Method`, `Poly`, `TypeLambda`, `ParamRef`).
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
- name-based `TYPEREF`/`TERMREF`, `TYPEREFin`/`TERMREFin`, and every other
  type form beyond §4 "Types" — `UnsupportedType`;
- packages outside the registry — `UnresolvedPackage`;
- reconciling the package registry with the classloader's own (Milestone 6, issue #5).

Defects this work found in neighbouring crates were fixed there: `NameRef`
is zero-based (#9), qualified visibility is `Visibility::PrivateWithin` /
`ProtectedWithin` (#10), and a category-five node with a padded length prefix
(`161, 0, 253`) is indexed at its tag instead of one byte later (found by the
Milestone 2a corpus measurement, which saw references to real addresses that
named no node).

### Type pass measurement

`type_corpus.rs` enters every unit of the two corpora (one store and one
package registry per corpus, as a classpath would have) and decodes every
reference node (`SHAREDtype`, `*direct`, `*pkg`, `THIS`, `*symbol`, name-based
`TYPEREF`/`TERMREF`) as its own root. Nested prefixes are therefore counted
again as roots, so the figures measure coverage of the forms, not distinct
types. The measurement is `#[ignore]`d in CI (run it with `--ignored`); a
smaller test runs over the small fixtures.

| | scala3-library | scala3-compiler |
|---|---|---|
| units reaching the supported subset | 941 / 941 | 1148 / 1148 |
| reference nodes | 381,546 | 1,007,987 |
| decoded | 160,391 (42%) | 289,176 (29%) |
| unexpected errors | 0 | 0 |
| `MissingReferencedSymbol` | 34,188 | 116,377 |
| `UnresolvedPackage` | 9,137 | 51,337 |

Every `MissingReferencedSymbol` target is something pass 1 documents as not
entered: a local definition (inside a `val`/`def` body, a block, or a pattern
`case`, including those in constructor arguments) or a parameter of a
type-lambda alias. The measurement asserts that no target is anything else, so
a definition pass 1 should have entered would fail it. No unit decodes all its reference nodes:
every unit refers to `java.lang.Object` or `scala.*` by name.

Unsupported tags, most common first (library / compiler), which order PR 2b:

| tag | node | library | compiler |
|-----|------|---------|----------|
| 117 | `TYPEREF` (by name) | 140,593 | 387,780 |
| 115 | `TERMREF` (by name) | 14,021 | 120,003 |
| 163 | `TYPEBOUNDS` | 11,599 | 587 |
| 161 | `APPLIEDtype` | 8,232 | 38,404 |
| 153 | `ANNOTATEDtype` | 1,372 | 1,231 |
| 170 | `TYPELAMBDAtype` | 757 | 145 |
| 165 | `ANDtype` | 624 | 743 |
| 167 | `ORtype` | 406 | 1,048 |
| 193 | `FLEXIBLEtype` | 140 | 575 |

By far the largest gap is the name-based reference, which is what the
compiler writes for anything defined outside the current unit; it is 2–3x the
rest combined. Unresolved packages (`scala`, `java.lang`, ...) are the same
gap: the package is not in the registry until its own unit has been entered.

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
