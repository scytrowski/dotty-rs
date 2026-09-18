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

## 4. Errors

Malformed or unsupported TASTy input is reported as a typed `UnpickleError`.
Unsupported semantic shapes are never lowered to `Type::Error`. Variants are
added only when a pass needs them. Panics are reserved for internal compiler
bugs, not for external input.

## 5. Relationship with `dotty-classloader`

`dotty-classloader/src/tasty_symbol.rs` still holds a best-effort,
name-based `.tasty` reader. It is a compatibility bridge and stays untouched
until the unpickler reproduces its behaviour with tests (Milestone 6). The
unpickler crate does not depend on `dotty-classloader`.

## 6. Milestones

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

## 7. Known unsupported forms

Everything beyond crate scaffolding: no pass is implemented yet. This section
will list the concrete semantic forms each milestone deliberately defers.
