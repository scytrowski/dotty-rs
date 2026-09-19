# TASTy semantic unpickler

Status: Milestone 1 (semantic index and symbol entering) is complete
(`crates/dotty-tasty-unpickler`). Milestone 2 (core type references) is next.
Only pass 1 exists: every entered symbol is `SymbolInfo::Missing`; no type is
decoded.

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
| 2. Types | semantic `TypeId`s, using `TypeArena::reserve`/`fill` for binders | 2–4 |
| 3. Complete | `SymbolInfo::Complete(TypeId)`, `ClassInfo`, annotations | 5 |
| 4. Typed AST | `AstArena<Typed>`, rehydrated without type inference | 7 |

## 3. Identity invariant

TASTy AST addresses are semantic identity anchors:

```text
definition address -> exactly one SymbolId
shared type address -> exactly one TypeId
```

Direct references (`TYPEREFdirect`, `TERMREFdirect`, `SHAREDtype`, ...) resolve
through the semantic index by address, never by rendering a name and searching
for it. Names are used only where TASTy itself encodes a target by name.

Owner and scope membership stay distinct, as in `dotty-core`: a symbol's
`owner` says who semantically holds it, while a `Scope` says where it can be
found by name. Symbols have no `children` field.

## 4. Reading TASTy definitions

### Names

Name references — in AST payloads and inside composite name-table entries —
are **zero-based** indexes into the name table in Scala 3.9.0 compiler output
(`<init>` is entry 14 and the signed constructor entry says `original: 14`;
`java.lang.Object` is `Qualified { 17, 18 }` over entries 15/16/18). The
unpickler reads names itself (`names.rs`). `TastyFile::render_name` uses a
one-based convention and renders composite names such as package paths
incorrectly for real files, so it is not used here. A signed name reads as
its original name: overloads are distinguished by definition address, not by
signature text.

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

Qualified access (`private[X]`, `protected[X]`) is reported as
`UnpickleError::UnsupportedQualifiedModifier`: `Visibility` has no variant
for it yet, and widening or narrowing it would be a guess.

### Packages

`TastyUnpickler::enter_symbols` enters a package symbol for every segment of a
`PACKAGE` node's path (`TERMREFpkg`), splitting the qualified name
structurally. A package is keyed by its path, so repeated `PACKAGE` nodes
share one symbol, and the `PACKAGE` node address maps to the innermost
package. Each package owns a declaration scope and is entered into its
parent's scope. The outermost package has no owner. Any other path form is
`UnpickleError::UnsupportedPackagePath`. Package symbols are per unpickler;
see issue #12 for sharing them between units.

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

The index's payload for a `TYPEPARAM`/`PARAM` node omits the parameter's
name, so parameter names and modifiers come from the parent's structural
decoding (`TemplateStructure`, `DefDefBody`) and are paired with the index's
child addresses in wire order; a disagreement is
`UnpickleError::ParameterMismatch`.

Not entered: definitions inside method bodies, parameters of type-lambda
aliases, and companion links.

## 5. Errors

Malformed or unsupported TASTy input is reported as a typed `UnpickleError`.
Unsupported semantic shapes are never lowered to `Type::Error`. Variants are
added only when a pass needs them. Panics are reserved for internal compiler
bugs, not for external input.

## 6. Relationship with `dotty-classloader`

`dotty-classloader/src/tasty_symbol.rs` still holds a best-effort,
name-based `.tasty` reader. It is a compatibility bridge and stays untouched
until the unpickler reproduces its behaviour with tests (Milestone 6). The
unpickler crate does not depend on `dotty-classloader`.

## 7. Milestones

1. **Semantic index and symbol entering** — complete.
2. **Core type references** (`TypeRef`, `TermRef`, prefixes, `ThisType`,
   `SuperType`, constants, `Applied`, bounds, `And`/`Or`) — next.
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
`UnpickleError::DuplicateDefinition`.

Measured on real compiler output: all 37 small `dotty-tasty` fixtures enter
without error. On the manifest-backed corpora (`scala3-library` and
`scala3-compiler`, 2089 units, TASTy 28.8 — parsed leniently, outside the 3.9
target) 1669 units enter; of the 420 that do not, 419 stop at a qualified
access modifier (`private[X]`, issue #10) and one (`scala/package.tasty`) has a
nested `PACKAGE` whose path is a `SHAREDtype` reference.

Deliberately not supported yet:

- qualified access modifiers — `UnsupportedQualifiedModifier`, issue #10;
- `PACKAGE` paths other than a direct `TERMREFpkg`, such as a nested package
  whose path is a `SHAREDtype` — `UnsupportedPackagePath`; resolving it needs
  the shared-type resolution of Milestone 2;
- the modifiers listed in §4 (no matching core flag, variance, accessor roles)
  and annotations;
- an abstract type member is entered as `TypeAlias`; `SymbolKind` has no
  abstract-type kind;
- definitions inside method bodies (locals) and the parameters of type-lambda
  aliases;
- companion links (`SymbolLinks::companion`);
- sharing package symbols between TASTy units, issue #12.

Known issues in neighbouring crates that this work found: #9 (`render_name`
one-based), #11 (`.tasty` loading misses `val` constructor parameters), #13
(index payload of parameter nodes omits the name).

## 9. Review of Milestone 1

Answers to the review questions asked before Milestone 2:

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
6. **Is `dotty-core` still unaware of TASTy?** Yes; this work does not touch
   `dotty-core` (nor `dotty-tasty` or `dotty-classloader`).
7. **Did it avoid name heuristics where an address exists?** Yes. Names are
   read only to name symbols; no reference is resolved by name.
8. **Is the index sufficient for `TYPEREFsymbol`/`TERMREFsymbol`?** Yes for
   definitions of this unit: a `TYPEREFsymbol`/`TYPEREFdirect` carries the
   target's absolute address (in `Foo`, the class parameter type refers to
   address 9, the constructor's to its own copy at 49), and `symbol_at`
   resolves it. Missing: the `types` map for `SHAREDtype` caching and
   references to other units.
9. **Did real TASTy expose gaps in `dotty-core`?** `Visibility` cannot express
   `private[X]` (#10); `SymbolKind` has no abstract-type kind; `Definitions`
   has no root package.
10. **Can Milestone 2 be implemented without redesigning PR1?** Yes. It adds
    the `types` map and resolution on top of the existing index. Two
    dependencies: the nested-package path form needs `SHAREDtype`, and the
    `private[X]` failures (the bulk of the library corpus) need #10 first.
