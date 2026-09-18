# TASTy semantic unpickler

Status: in progress (`crates/dotty-tasty-unpickler`). Milestone 1 (semantic
index and symbol entering) is being implemented; no pass is complete yet.

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

1. **Semantic index and symbol entering** — in progress.
2. Core type references (`TypeRef`, `TermRef`, prefixes, `ThisType`,
   `SuperType`, constants, `Applied`, bounds, `And`/`Or`) — next.
3. Binder types (`Method`, `Poly`, `TypeLambda`, `ParamRef`).
4. Advanced types (refinements, recursive, match types, annotations, ...).
5. Symbol completion (signatures, parents, self types, `ClassInfo`).
6. Classloader integration and the `SymbolResolver` boundary.
7. Typed AST.

Each milestone is delivered as one or more reviewable PRs that keep the whole
workspace green.

## 8. Known unsupported forms

No pass is implemented yet; the building blocks (index, names, mappings) are
in place. Deferred so far: qualified access modifiers, the modifiers listed in
§4, abstract-type-member kind, definitions local to method bodies, and sharing
package symbols between TASTy units. This section will list the concrete
semantic forms each milestone deliberately defers.
