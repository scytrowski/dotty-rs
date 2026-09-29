# dotty-core Design

Status: design document for the shared compiler foundation. The semantic model
and the shared source, diagnostics, and token contracts are implemented in
`dotty-core`; this document describes the intended boundaries for everything
built on top of them. It is self-contained: every type referenced below has
its authoritative definition in this file, not in chat history or prior
drafts.

Revision note: this revision applies a partner review of the first draft.
Every change below is either a **blocker fix** (the prior draft was
internally inconsistent — e.g. an ID type with nowhere to store its payload),
a **major fix** (underspecified in a way likely to force a rewrite later), or
a **minor fix/clarification** (wording or a documented, accepted tradeoff).
Each is called out inline as `[BLOCKER n]` / `[MAJOR n]` / `[MINOR n]` the
first time it's addressed, so the resolution can be traced back to the
concern that raised it. A few resolutions go slightly further than the
review asked, where grounding the review's concern in Dotty's actual
`compiler/src/dotty/tools/dotc/ast/{Trees,untpd,tpd}.scala` (audited at the
pinned `3.9.0` tag, commit `777528f19a58e794c9954a42f433373472ec57f8` —
reproduce with `git clone --branch 3.9.0 https://github.com/scala/scala3.git`)
surfaced a sharper or more general fix than the one proposed. Those are
marked `[MAJOR 1, extended]`.

## 1. Scope and goal

Today the workspace has a shared frontend foundation (`dotty-core`), a lexer
pipeline (`dotty-lexer`), the initial source parser infrastructure
(`dotty-parser`), and two independent binary codecs (`dotty-tasty`,
`dotty-classfile`). There is still no namer, typer, or typed AST pipeline.

`[MINOR 3]` The four future consumers do not all need the same slice of the
model, and the introduction should not imply they do:

- The **source parser** needs only the shared syntax-tree and name model:
  `NameInterner`, `Span`/`SourceSpan`, `AstArena<Untyped>`. It has no reason
  to touch `SymbolTable`, `TypeArena`, or `ScopeArena`.
- The **classfile loader**, **TASTy semantic unpickler**, **namer**, and
  **typer** need the full shared semantic model (symbols, types, scopes),
  because they are the components that give syntax trees meaning.

They belong in one foundational crate because typed trees are exactly the
place where the syntax layer (owned by the parser) and the semantic layer
(owned by everything else) connect — a `Tree<Typed>` node carries a `TypeId`
inline. Building the semantic model four times, once per consumer, would
duplicate the hardest part of the compiler and guarantee the four copies
drift apart.

`dotty-core` is a new crate that gives all of them one shared model:

```text
                             ┌─────────────────────┐
                             │      dotty-core      │
                             │  Symbol / Type / AST │
                             └─────────┬────────────┘
                                       │
          ┌────────────────────────────┼────────────────────────────┐
          │                            │                            │
          ▼                            ▼                            ▼
   source frontend              classfile loader             TASTy adapter
   (dotty-lexer + parser)       (dotty-classfile               (dotty-tasty
                               today: raw only)               today: raw only)
          │                            │                            │
          └────────────────────────────┼────────────────────────────┘
                                       │
                                       ▼
                                  SemanticStore
                                       │
                                       ▼
                                     typer
                                       │
                                       ▼
                                   Typed AST
```

`dotty-core` itself must stay foundational: it owns shared source, diagnostic,
token, symbol, type, and tree contracts, but knows nothing about the concrete
lexer implementation, JVM classfiles, TASTy's wire format, or type inference.
Those stay in their own crates and depend on `dotty-core`, never the other way
around.

This mirrors, at the semantic layer, the layering principle AGENTS.md already
states for `dotty-tasty`: "Keep the binary, raw AST, structured AST, and
file-level APIs deliberately separated." `dotty-core` is the structured
*semantic* layer that source, classfile, and TASTy structured/raw layers will
eventually project onto.

## 2. Where this sits in the existing workspace

Current workspace members and their dependency edges:

```text
dotty-core     (shared source, diagnostics, token, and semantic contracts)
dotty-lexer    -> dotty-core
dotty-parser   -> dotty-core
dotty-classfile (no deps; raw ClassFile/ConstantPool/etc. type skeleton only)
dotty-tasty    (no deps; raw + structured wire-format codec)
dotty (root)   -> dotty-tasty, dotty-classfile (facade re-exporting `tasty`, `classfile`)
```

`dotty-parser` is an incremental Scala 3.9.0 source parser with an expression
pipeline from explicit/context-function literals and `Expr1` through
operator/simple expressions, an initial source-level pattern grammar, the
first case/match clause layer, and source-level definition metadata including
annotations and supported modifiers. The rest
of the Scala grammar is added incrementally; full grammar coverage, namer,
typer, and compiler orchestration remain future work. The parser boundary and
`AstArena<Untyped>` ownership are established. See
[`docs/parser-design-3.9.0.md`](parser-design-3.9.0.md) for its current scope.

`[MAJOR 6]` The prior draft left it open whether classfile/TASTy semantic
adapters live inside the future `dotty-typer` crate or in a dedicated module.
They must not live in the typer: the typer consumes an already-populated
`SemanticStore` and must not know how a `.class` or `.tasty` file is decoded,
the same way it must not know how source text is lexed. `dotty-core` slots in
with a dedicated adapter/loader layer between the binary codecs and
everything that consumes `dotty-core`:

```text
                         dotty-core
                       ▲      ▲      ▲
                       │      │      │
                (future)   dotty-tasty   dotty-classfile
                dotty-parser       │          │
                       │            ▼          ▼
                       │    dotty-tasty-sema  dotty-classfile-sema
                       │            │          │
                       └────────────┴────┬─────┘
                                         ▼
                                  (future) dotty-typer
                                         │
                                         ▼
                                  (future) dotty-compiler
```

Binary semantic adapters do not belong to the typer implementation, and they
do not belong inside `dotty-tasty`/`dotty-classfile` either (see below). The
exact crate/module packaging of the adapter layer is left open (§16); the
constraint that matters now is the dependency shape, not the file layout.

The former standalone source, diagnostics, and token crates are now modules of
`dotty-core`. This gives all frontend components one source-position model,
diagnostic range type, and parser-facing token contract without conversion
boundaries between sibling crates.

`dotty-tasty` and `dotty-classfile` do not depend on `dotty-core` today, and
should not gain that dependency as part of this design. Both are
  described in their own docs (`tasty-api.md`,
  `docs/classfile-format-jdk25.md`) as binary/structural codecs with no
  symbol or type knowledge. The semantic adapters that map their structured
  output onto `dotty-core::Symbol`/`Type` are new code living in the
  dedicated adapter layer above, never inside the codec crates themselves —
  this keeps `dotty-tasty` and `dotty-classfile` independently useful as pure
  codecs, which is an explicit non-goal to disturb.

## 3. Crate layout

```text
crates/
├── dotty-core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── ids.rs
│       ├── names.rs
│       ├── source/
│       │   ├── mod.rs
│       │   ├── source_text.rs
│       │   ├── span.rs
│       │   ├── line_index.rs
│       │   └── source_span.rs
│       ├── diagnostics/
│       │   ├── mod.rs
│       │   └── diagnostic.rs
│       ├── token/
│       │   ├── mod.rs
│       │   ├── kind.rs
│       │   ├── token_def.rs
│       │   ├── value.rs
│       │   ├── token_source.rs
│       │   └── scanner_event.rs
│       │
│       ├── ast/
│       │   ├── mod.rs
│       │   ├── phase.rs
│       │   ├── arena.rs
│       │   ├── tree.rs
│       │   ├── common.rs
│       │   ├── untyped.rs
│       │   ├── typed.rs
│       │   ├── modifiers.rs
│       │   └── visitor.rs
│       │
│       ├── types/
│       │   ├── mod.rs
│       │   ├── arena.rs
│       │   ├── ty.rs
│       │   ├── binder.rs
│       │   ├── method.rs
│       │   ├── class_info.rs
│       │   ├── annotation.rs
│       │   ├── constant.rs
│       │   └── flags.rs
│       │
│       ├── symbols/
│       │   ├── mod.rs
│       │   ├── table.rs
│       │   ├── symbol.rs
│       │   ├── kind.rs
│       │   ├── scope.rs
│       │   ├── flags.rs
│       │   ├── origin.rs
│       │   └── completion.rs
│       │
│       └── store/
│           ├── mod.rs
│           └── semantic_store.rs
│
├── dotty-tasty/       (unchanged; semantic adapter is future work, not here)
├── dotty-classfile/   (unchanged; semantic adapter is future work, not here)
├── dotty-lexer/
└── dotty-parser/      (SourceText + TokenSource -> AstArena<Untyped>)
```

Changes from the first draft's layout:

- `source/` now contains both the shared source-text/range utilities and the
  semantic `Span`/`SourceSpan` wrappers.
- `types/binder.rs` no longer holds a `Binder`/`BinderArena` type — binder
  identity is `TypeId` (`[BLOCKER 1]`, §8) — but the file is kept for the
  `reserve`/`fill` cyclic-construction API that binder-shaped types need.
- `types/annotation.rs` is added (`[MAJOR 3]`, §8).
- `context/` is renamed to `store/`, and `SemanticContext` to
  `SemanticStore` (`[MAJOR 5]`, §10).

`ScopeArena` is still folded into `symbols/scope.rs`, next to `Scope`,
instead of a separate arena file — a minor simplification versus scattering
one-line arena structs across extra files; every arena with a non-trivial
query API (`AstArena`, `TypeArena`, `SymbolTable`) still gets its own file.

`dotty-core` is a workspace member and the root `dotty` facade exposes its
shared contracts as `dotty::core`, alongside `dotty::tasty` and
`dotty::classfile`. `dotty-parser` consumes those contracts directly and does
not depend on the root facade. This keeps the public entry point stable for
future parser and semantic adapters.

## 4. `ids.rs` — opaque identities

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymbolId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NameId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnnotationId(u32);

/// Opaque handle for "which classpath entry a classfile-derived symbol came
/// from." `dotty-core` does not know what a classfile is; the adapter that
/// constructs `SymbolOrigin::Classfile` assigns and interprets this ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClassfileOriginId(u32);

/// Same idea for `.tasty` files. See `[MINOR 1]` in §9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TastyOriginId(u32);
```

`BinderId` from the first draft is removed — see `[BLOCKER 1]` in §8: binder
identity is `TypeId`, not a separate ID space. `NameId`, `AnnotationId`,
`ClassfileOriginId`, and `TastyOriginId` are new relative to the original
proposal (§5, §8/`[MAJOR 3]`, §9/`[MINOR 1]`).

For AST nodes, the phase-indexed identity is a distinct Rust type per phase:

```rust
pub struct TreeId<P> {
    raw: u32,
    _phase: PhantomData<P>,
}
```

so `TreeId<Untyped>` and `TreeId<Typed>` cannot be confused at a call site —
`fn typecheck(tree: TreeId<Untyped>) -> TreeId<Typed>` rejects a typed tree
passed by mistake at compile time, not at runtime.

**Invariant that must be documented on every ID type:** an ID is only valid
relative to the arena that allocated it. Nothing prevents constructing a
`SymbolId` from one `SymbolTable` and indexing a different one; that is a
logic bug, not a memory-safety issue, and it is the same tradeoff every
arena-based compiler makes (`id-arena`, `la-arena`, etc.). `SemanticStore`
(§10) exists specifically so a compilation session has exactly one instance
of each arena, minimizing the chance of mixing IDs across arenas.

## 5. `names.rs` — interned, namespaced names

Scala distinguishes the term and type namespaces (`class Foo` / `val Foo` can
coexist). Do not use raw `String` for names anywhere in `dotty-core`.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Namespace {
    Term,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Name {
    text: NameId,
    namespace: Namespace,
}

pub struct TermName(Name);
pub struct TypeName(Name);

impl TermName {
    pub fn as_name(&self) -> &Name;
}
```

No string-interning type exists anywhere in the workspace today
(`dotty-core::token::TokenValue` is currently just `None` — no identifier payload
yet). `names.rs` defines one:

```rust
pub struct NameInterner {
    strings: Vec<Box<str>>,
    lookup: std::collections::HashMap<Box<str>, NameId>,
}

impl NameInterner {
    pub fn intern(&mut self, text: &str) -> NameId;
    pub fn resolve(&self, id: NameId) -> &str;
}
```

`[MINOR 2]` This stores the interned bytes twice (once in `strings`, once as
the `HashMap` key). That is an accepted, conscious simplicity tradeoff for
the foundation PR, not an oversight — do not "fix" it preemptively. If
profiling later shows the interner matters, the options are, in increasing
order of effort: `HashMap<Arc<str>, NameId>` sharing the allocation with
`strings`; a `hashbrown` raw-entry lookup into the backing `strings` storage
directly; or an existing interning crate (`lasso`, `string-interner`).

This needs to be usable before a full `SemanticStore` exists, because the
future parser interns identifier text while building `AstArena<Untyped>`,
long before a namer or typer runs. So `NameInterner` is a standalone type
that `SemanticStore` *owns* (as its `names` field) rather than something
private to symbol/type machinery — the parser is handed `&mut SemanticStore`
(or just `&mut NameInterner`, if the pipeline wants to construct the interner
before anything else exists) purely to intern identifiers.

## 6. `source/` — positions

`[BLOCKER 3]` The first draft's `Span { source: SourceId, range, point }`
assumed every tree has a real source. That is false for a compiler: trees
can come from parsed source, from a `.tasty` file, or from a compiler-
generated transform (default getters, generated accessors, closure methods,
lowering phases) — none of which necessarily have a meaningful `SourceId`.
Introducing a sentinel (`SourceId::NONE`, `SourceId(u32::MAX)`) would leak an
"is this actually valid?" check into every consumer that reads a span.

The fix is to separate "a range, optionally with a diagnostic point" from
"which source that range is in," and make the *source* part optional at the
tree, not inside `Span` itself:

```rust
pub struct Span {
    pub range: TextRange,
    pub point: Option<u32>,
}

pub struct SourceSpan {
    pub source: SourceId,
    pub span: Span,
}

pub struct Tree<P: AstPhase> {
    pub kind: TreeKind<P>,
    pub position: Option<SourceSpan>,
    pub ty: P::TypeInfo,
}
```

`Option<SourceSpan>` is sufficient for the foundation PR. A stronger
three-way `enum Position { Source(SourceSpan), Synthetic, None }` is
possible later if "no position because compiler-generated" ever needs to be
distinguished from "no position because not yet assigned" for diagnostics;
nothing in the foundation PR needs that distinction yet, so it is not
introduced speculatively.

`[BLOCKER 3]` terminology fix: `point` is a single `u32` offset, not a range,
so it cannot "mark" a sub-span. The corrected description: `point` identifies
the primary diagnostic offset inside the enclosing range — e.g., for
`foo.bar`, `range` covers the whole selection while `point` is the start
offset of `bar`.

As before: do not store line/column per node. `dotty-core::source` provides
`LineIndex` for offset → line/column mapping; all frontend layers reuse it
rather than adding a second mapping mechanism.

## 7. AST phase model

The parser-facing coverage audit is maintained separately in
[`docs/dotty-core-parser-readiness.md`](dotty-core-parser-readiness.md). That
document is authoritative for the distinction between parser-owned syntax,
lowered forms, and explicitly feature-gated Scala 3.9 constructs. The AST
model in this section is the current foundation baseline, not a claim that
every Dotty `untpd` helper has already been mirrored. The implemented parser
boundary and its incremental expression grammar are documented in
[`docs/parser-design-3.9.0.md`](parser-design-3.9.0.md).

### `ast/phase.rs`

`[MAJOR 2]` extends this trait with phase-specific definition and template
metadata, resolving where source-level modifiers and parser-only template
clauses live (see "Definitions" below):

```rust
pub trait AstPhase: sealed::Sealed {
    type TypeInfo;
    type ExtraNode;
    type DefMetadata;
    type TemplateMetadata;
}

pub enum Untyped {}
pub enum Typed {}

impl AstPhase for Untyped {
    type TypeInfo = ();
    type ExtraNode = UntypedNode;
    type DefMetadata = Modifiers;
    type TemplateMetadata = UntypedTemplateMetadata;
}

impl AstPhase for Typed {
    type TypeInfo = TypeId;
    type ExtraNode = std::convert::Infallible;
    type DefMetadata = ();
    type TemplateMetadata = ();
}
```

Invariants this must enforce:

- `Tree<Untyped>` carries no semantic type.
- `Tree<Typed>` always carries a `TypeId`.
- `UntypedNode` variants (surface-syntax-only constructs) cannot appear in a
  `Typed` tree — `ExtraNode = Infallible` makes constructing
  `TreeKind::PhaseSpecific` on the typed phase uninhabited.
- `Modifiers` (syntactic `abstract`/`final`/`override`/annotations-as-written)
  cannot appear on a `Typed` definition node — `DefMetadata = ()` makes it
  disappear at the type level once typing has produced `SymbolFlags` and
  resolved annotations instead.
- `TemplateMetadata` retains untyped `derives` and ordered `uses` clauses,
  including each `UseRef.initially` bit; it is `()` after typing.

This is a case where Rust can express an invariant Dotty's own `Tree[T]`
leaves to convention. In real Dotty (`Trees.scala`), `DefTree` stores syntax-
level modifiers as a mutable, untyped attachment (`private var myMods:
untpd.Modifiers | Null`) on *every* tree regardless of phase, and only
*restricts the public accessor* to `untpd.DefTree` at the extension-method
level (`extension (mdef: untpd.DefTree) def mods: untpd.Modifiers`) — the
storage exists on typed trees too, it's just conventionally unread. `dotty-
core` makes the stronger choice available in Rust: `Modifiers` is physically
absent from a `Tree<Typed>`, not just conventionally unread. This is safe to
diverge from Dotty on because nothing the foundation PR needs (§15) requires
recovering original modifier syntax from an already-typed tree; if a later
tooling use case needs it, it can be added back as an explicit side table
keyed by `TreeId<Untyped>`, mirroring how `NamerState` already works (§9),
rather than as a live field on `Tree<Typed>`.

### `ast/arena.rs`

Kept as proposed:

```rust
pub struct AstArena<P: AstPhase> {
    nodes: Vec<Tree<P>>,
}

impl<P: AstPhase> AstArena<P> {
    pub fn alloc(&mut self, tree: Tree<P>) -> TreeId<P>;
    pub fn get(&self, id: TreeId<P>) -> &Tree<P>;
    pub fn get_mut(&mut self, id: TreeId<P>) -> &mut Tree<P>;
}
```

No `Box`/`Rc`/`Arc` per node — children are referenced by `TreeId<P>` into
the same arena.

### `ast/tree.rs` — `TreeKind<P>`

`[MAJOR 7]` The first draft deferred the authoritative node list to "the
original notes." That makes the document dependent on chat history, which
defeats its purpose. This section gives the full, final definition.

`[MAJOR 1]` The first draft also said the original node list should be
"transcribed as-is" without checking which *fields* of each shared node
differ between untyped and typed trees. That check was done against the
real Scala 3.9.0 compiler source (`ast/Trees.scala`, `ast/untpd.scala`); the
findings below are load-bearing, not cosmetic.

**Real Dotty keeps several fields on shared, phase-generic tree classes
permanently typed as `untpd.*`, regardless of the enclosing phase** — e.g.
(`Trees.scala`):

```scala
case class This[+T <: Untyped] private[ast] (qual: untpd.Ident)
case class Super[+T <: Untyped] private[ast] (qual: Tree[T], mix: untpd.Ident)
abstract class ImportOrExport[+T <: Untyped] {
  val expr: Tree[T]
  val selectors: List[untpd.ImportSelector]
}
case class Inlined[+T <: Untyped] private[ast] (call: tpd.Tree, bindings: List[MemberDef[T]], expansion: Tree[T])
```

`This.qual`/`Super.mix` are always `untpd.Ident` (a `this`/`super` qualifier
never gets its own semantic type independent of the enclosing class, so
Dotty never bothers typing it). `Import`/`Export` selectors are always
`untpd.ImportSelector` (an import selector is pure syntax; it never needs a
semantic type). `Inlined.call` is always `tpd.Tree` — the *opposite*
direction, a permanently-typed field on a node that can otherwise appear
untyped.

`[MAJOR 1, extended]` This is fine in Dotty because Dotty has no arena
boundary between typed and untyped trees — they are just ordinary heap
objects, so an `untpd.Ident` living inside a `tpd.This` costs nothing extra
and creates no dangling-arena risk. `dotty-core` is different: `Tree<Untyped>`
and `Tree<Typed>` live in **separate arenas**, so a hard-coded `TreeId<Untyped>`
field inside a node that can appear in `TreeKind<Typed>` is a real hazard
the review's "Revised Foundation Invariants" list already worried about in
the abstract (`[MAJOR 2]`) — and it is strictly worse than that for the
TASTy adapter case: a `.tasty` file is unpickled straight into
`AstArena<Typed>` with **no corresponding `AstArena<Untyped>` in existence at
all**, so a field typed `TreeId<Untyped>` would be impossible to populate
from that path, not just risky.

**Resolution — a general invariant stronger than Dotty's own design needs:**
*every `TreeId` field inside `TreeKind<P>` is `TreeId<P>`. No shared node
hard-codes `TreeId<Untyped>` or `TreeId<Typed>` regardless of the enclosing
phase.* Concretely:

- `This.qual` / `Super.mix`: since all they ever carry is an optional class-
  name identifier with no children of its own, they are modeled as a plain
  value (`Option<Name>`) instead of a tree reference at all, sidestepping
  the arena question entirely.
- `Import`/`Export` selectors: modeled as an `ImportSelector<P>` whose
  optional `renamed`/`bound` trees are `TreeId<P>`, not
  `TreeId<Untyped>`. In the typed phase these get a trivial `TypeId` (the
  selector's own type is never semantically meaningful); the small
  "wasted" `ty` field is cheaper than a cross-arena reference.
- `Inlined.call`: real Dotty's `call: tpd.Tree` is a forward reference used
  by the inliner to report where an inlined call originated. Inline
  expansion is out of scope for the foundation PR (§15), so `Inlined.call` is
  **not included yet** — `TreeKind<P>::Inlined` foundation shape is
  `{ bindings: Vec<TreeId<P>>, expansion: TreeId<P> }`. Adding call
  provenance back is deferred to whichever future PR implements the inliner,
  at which point the always-typed-`call` question can be solved deliberately
  (e.g. a side table keyed by `TreeId<P>`, the same pattern used for
  `NamerState`) instead of by copying Dotty's field type under time pressure.
- `Template`: real Dotty's `Template` carries a combined
  parents-followed-by-derived-classes list only pre-typing (`derived` is
  always `Nil` after typing except in the untyped-only `DerivingTemplate`
  subclass). The Rust foundation keeps `parents` separate and stores
  untyped-only `derives` and Scala 3.9 `uses` in
  `Template<Untyped>::metadata`; `Template<Typed>::metadata` is `()`.
- `DefDef`/`ValDef`/`TypeDef` modifiers: resolved above via
  `AstPhase::DefMetadata` (`[MAJOR 2]`).

Definitions:

```rust
pub enum TreeKind<P: AstPhase> {
    Ident(Ident),
    Select(Select<P>),

    This(This),
    Super(Super<P>),

    Literal(Literal),

    Apply(Apply<P>),
    TypeApply(TypeApply<P>),

    New(New<P>),
    Typed(TypedExpr<P>),
    NamedArg(NamedArg<P>),

    Assign(Assign<P>),
    Block(Block<P>),
    If(If<P>),

    Match(Match<P>),
    CaseDef(CaseDef<P>),

    Return(Return<P>),
    While(While<P>),
    Try(Try<P>),

    Closure(Closure<P>),

    ValDef(ValDef<P>),
    DefDef(DefDef<P>),
    TypeDef(TypeDef<P>),
    Template(Template<P>),

    PackageDef(PackageDef<P>),

    Import(Import<P>),
    Export(Export<P>),

    TypeTree(TypeTree),
    SingletonTypeTree(SingletonTypeTree<P>),
    AppliedTypeTree(AppliedTypeTree<P>),
    RefinedTypeTree(RefinedTypeTree<P>),
    LambdaTypeTree(LambdaTypeTree<P>),
    MatchTypeTree(MatchTypeTree<P>),
    ByNameTypeTree(ByNameTypeTree<P>),
    TypeBoundsTree(TypeBoundsTree<P>),

    Bind(Bind<P>),
    Alternative(Alternative<P>),
    UnApply(UnApply<P>),

    Annotated(Annotated<P>),

    Quote(Quote<P>),
    Splice(Splice<P>),
    QuotePattern(QuotePattern<P>),
    SplicePattern(SplicePattern<P>),

    Inlined(Inlined<P>),

    PhaseSpecific(P::ExtraNode),
}
```

Deliberately **not** included yet, matching Dotty's real classification of
these as internal/derived rather than primary surface or typed-output nodes:
`Labeled` (used only for desugared `while`/pattern-matching gotos), `Hole`
(quote-pickling only, "will never be in a TASTy file" per Dotty's own doc
comment), `SeqLiteral`/`JavaSeqLiteral` (desugared varargs), and the
`InlineIf`/`InlineMatch`/`SubMatch` boolean-flagged subclasses of
`If`/`Match` (modeled later as a flag on `If`/`Match` if needed, not as
separate variants). These can be added incrementally without touching
`AstPhase` once inline handling and pattern desugaring are in scope.

Shared node payload definitions (`ast/common.rs`, `ast/typed.rs`):

```rust
pub struct Ident {
    pub name: Name,
}

pub struct Select<P: AstPhase> {
    pub qualifier: TreeId<P>,
    pub name: Name,
}

/// `qual` is the optional class-name qualifier of `qual.this`. It is a plain
/// name, not a tree: seeSyntax audit above for why this isn't `TreeId<P>`.
pub struct This {
    pub qual: Option<Name>,
}

/// `mix` is the optional trait qualifier of `C.super[mix]`; same reasoning
/// as `This.qual`.
pub struct Super<P: AstPhase> {
    pub qual: TreeId<P>,
    pub mix: Option<Name>,
}

pub struct Apply<P: AstPhase> {
    pub function: TreeId<P>,
    pub args: Vec<TreeId<P>>,
    pub kind: ApplyKind,
}

pub enum ApplyKind {
    Regular,
    Using,
}

pub struct Import<P: AstPhase> {
    pub expr: TreeId<P>,
    pub selectors: Vec<ImportSelector<P>>,
}

pub struct Export<P: AstPhase> {
    pub expr: TreeId<P>,
    pub selectors: Vec<ImportSelector<P>>,
}

pub struct ImportSelector<P: AstPhase> {
    pub imported: Name,
    pub renamed: Option<TreeId<P>>,
    pub bound: Option<TreeId<P>>,
}

pub struct Template<P: AstPhase> {
    pub constructor: TreeId<P>,
    pub parents: Vec<TreeId<P>>,
    pub self_val: Option<TreeId<P>>,
    pub body: Vec<TreeId<P>>,
    pub metadata: P::TemplateMetadata,
}

pub struct Inlined<P: AstPhase> {
    pub bindings: Vec<TreeId<P>>,
    pub expansion: TreeId<P>,
}
```

The remaining shared nodes (`Literal`, `New`, `TypedExpr`, `NamedArg`,
`Assign`, `Block`, `If`, `Match`, `CaseDef`, `Return`, `While`, `Try`,
`Closure`, `ValDef`, `DefDef`, `TypeDef`, `PackageDef`, the type-tree family,
`Bind`, `Alternative`, `UnApply`, `Annotated`, `Quote`/`Splice`/
`QuotePattern`/`SplicePattern`) do not have phase-divergent field *shapes*
beyond ordinary `TreeId<P>` child substitution — every child reference is
`TreeId<P>`, matching the general invariant above. `ValDef<P>`/`DefDef<P>`/
`TypeDef<P>` each carry a `metadata: P::DefMetadata` field for the
`Modifiers`-or-nothing split described above.

`Template<P>` additionally carries `metadata: P::TemplateMetadata`:
`Template<Untyped>` retains parser-only `derives` and ordered `uses` through
`UntypedTemplateMetadata`, while `Template<Typed>` carries `()`.

### `ast/untyped.rs`

`UntypedNode` holds surface-syntax-only constructs:

```rust
pub enum UntypedNode {
    Error(ErrorNode),

    ModuleDef(ModuleDef),

    Function(Function),
    FunctionWithMods(FunctionWithMods),
    PolyFunction(PolyFunction),

    InfixOp(InfixOp),
    PrefixOp(PrefixOp),
    PostfixOp(PostfixOp),

    Parens(Parens),
    Tuple(Tuple),

    ForYield(ForYield),
    ForDo(ForDo),

    GenFrom(GenFrom),
    GenAlias(GenAlias),

    PatDef(PatDef),

    ExtensionMethods(ExtensionMethods),

    InterpolatedString(InterpolatedString),

    ContextBounds(ContextBounds),
    ContextBoundTypeTree(ContextBoundTypeTree),

    Number(NumberLiteral),

    Throw(Throw),

    ParsedTry(ParsedTry),
}
```

`GenFrom.check_mode` uses the untyped-only `GenCheckMode` enum to preserve
Scala 3.9's pattern-checking policy. The parser derives this value from the
source version and an optional `case` prefix; for-comprehension lowering
consumes it later.

`NumberLiteral` retains its source-backed `text: NameId` together with a
`NumberKind`: `Whole(radix)`, `Decimal`, or `Floating`. This mirrors Scala's
parser distinction before typing; suffixed numeric tokens that Scala converts
directly to semantic literals are not forced through `NumberKind`.

This is the foundation subset of Dotty's actual `untpd`-only node types. The
remaining differences called out by the parser-readiness audit are explicit
lowering or feature-policy decisions, rather than silently dropped source
payloads.

### `ast/typed.rs`

Does not redeclare node kinds; it only provides
`TypedTree`/`TypedTreeId`/`TypedAst` type aliases and a `TypedAstBuilder`
whose constructors all *require* a `TypeId` argument, so a typed tree cannot
be built without one by construction, not just by convention:

```rust
pub type TypedTree = Tree<Typed>;
pub type TypedTreeId = TreeId<Typed>;
pub type TypedAst = AstArena<Typed>;

pub struct TypedAstBuilder<'a> {
    arena: &'a mut AstArena<Typed>,
}

impl TypedAstBuilder<'_> {
    pub fn ident(&mut self, name: Name, ty: TypeId, position: Option<SourceSpan>) -> TreeId<Typed>;
    pub fn apply(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed>;
}
```

### `ast/modifiers.rs`

Source modifiers are not the same as semantic symbol flags — a parsed
`override` keyword and a resolved "this symbol overrides a parent member"
fact are different things computed at different phases; conflating them
would force the namer to invent flags for syntax the parser never saw and
vice versa.

```rust
pub struct Modifiers {
    pub visibility: Option<VisibilitySyntax>,
    pub modifiers: Vec<Modifier>,
    pub annotations: Vec<TreeId<Untyped>>,
}

pub enum Modifier {
    Trait,
    Enum,
    EnumCase,
    Abstract,
    Final,
    Sealed,
    Case,
    Var,
    Update,
    Implicit,
    Given,
    Impure,
    Lazy,
    Override,
    Inline,
    Transparent,
    Opaque,
    Open,
    Infix,
    Tracked,
    Into,
    Erased,
}
```

`Trait` and `Enum` are parser-level definition flags. They preserve the source
distinction between `trait T` and `class T` when both use the shared
`TypeDef`/`Template` tree family; enum definitions use the same family rather
than a dedicated tree kind. These flags must not be inferred from constructor
shape or body contents. Later source flags such as `Case` can compose with
this metadata without changing the phase boundary. The namer maps `Enum` to
`SymbolFlags::ENUM` and `EnumCase` to `SymbolFlags::ENUM | SymbolFlags::CASE`;
enum identity does not require a new `SymbolKind`.

`Var` is attached to the existing untyped `PatDef`, matching Scala's
parser: `var` is a modifier on a `PatDef`, not a separate `VarDef` payload.
`Tracked` and `Update` are retained as modifier values for the
capture-checking dialect; enabling that dialect still requires its separate
capture-set AST contract.

`ParamAccessor` and `PrivateLocal` are parser-level constructor-parameter role
markers rather than written keywords. `ParamAccessor` records that a class
constructor parameter contributes an accessor; `PrivateLocal` records a plain
constructor parameter that does not. The latter must not be represented as
`VisibilitySyntax::Private`, because that would falsely claim that `private`
was written in the source.

`[MAJOR 2]` `Modifiers.annotations: Vec<TreeId<Untyped>>` is safe now that
`Modifiers` only exists as `Untyped::DefMetadata` — a `Modifiers` value can
only ever be reached from an untyped definition node, so its
`TreeId<Untyped>` annotation references always point into the same arena
they were built from. This was the underlying inconsistency `[MAJOR 2]`
flagged: with `Modifiers` as a plain field of a phase-generic `ValDef<P>`/
`DefDef<P>`, a *typed* `ValDef<Typed>` would have held it too, forcing
`Vec<TreeId<Untyped>>` onto a typed tree. Tying `Modifiers` to
`AstPhase::DefMetadata` closes that gap by construction.

## 8. Type model

### `types/ty.rs`

```rust
pub enum Type {
    NoType,
    Error(ErrorType),
    NoPrefix,

    TermRef { prefix: TypeId, target: TermRefTarget },
    TypeRef { prefix: TypeId, target: TypeRefTarget },

    ThisType { class: SymbolId },
    SuperType { this_type: TypeId, super_type: TypeId },

    Constant(Constant),

    Applied { tycon: TypeId, args: Vec<TypeId> },
    Bounds { low: TypeId, high: TypeId },       // genuine `>: low <: high`
    AliasingBounds { alias: TypeId },           // `= alias`; not Bounds{alias, alias}
    ByName { result: TypeId },
    Flexible { underlying: TypeId },            // explicit-nulls flexible type

    And { left: TypeId, right: TypeId },
    Or { left: TypeId, right: TypeId },

    Refined { parent: TypeId, name: Name, info: TypeId },

    /// `parent` may itself contain `RecThis { binder }` values where `binder`
    /// is the `TypeId` this very `Type::Recursive` value is stored under.
    /// See `[BLOCKER 1]` below for how that self-reference is constructed.
    Recursive { parent: TypeId },
    RecThis { binder: TypeId },

    Method(MethodType),
    Poly(PolyType),
    TypeLambda(TypeLambda),

    /// `binder` is the `TypeId` of the enclosing `Method`/`Poly`/`TypeLambda`
    /// value itself — not a separate `Binder`/`BinderId`. See `[BLOCKER 1]`.
    ParamRef { binder: TypeId, index: u32 },

    /// `MatchType.cases` holds, in order, `MatchCase` nodes or `TypeLambda`s
    /// whose result is a `MatchCase` (Dotty's `[X] =>> MatchCase(p, r)`, a
    /// case with captures; the captures are ordinary `ParamRef`s to that
    /// lambda). `MatchCase` is the case itself: Dotty's carrier, an
    /// `AppliedType` of its internal `MatchCaseClass`, is not modeled.
    Match(MatchType),
    MatchCase { pattern: TypeId, result: TypeId },

    Annotated { underlying: TypeId, annotation: AnnotationId },

    Wildcard { bounds: TypeId },

    JavaArray { element: TypeId },

    ClassInfo(ClassInfo),
}
```

`TermRef`/`TypeRef` designate their member the way Dotty's `NamedType`
designator does, `Symbol | Name`:

```rust
pub enum TypeRefTarget { Symbol(SymbolId), Name(TypeName) }
pub enum TermRefTarget { Symbol(SymbolId), Name(TermName) }
```

A `Symbol` target is a stable declaration identity, used whenever a
declaration symbol exists, and not the symbol's `Name`, so a symbol rename
doesn't require walking every type that references it. A `Name` target is
the selection `prefix.name` where the member has no `SymbolId`, such as a
member of a structural refinement (`C { type T1; type T2 = T1 }`, where `T1`
is selected from the `RecThis`). It is a real semantic reference: not `Error`,
`NoType`, a placeholder or an unresolved string. It carries no copy of the
member's info; the source of truth stays the `Refined` graph, which
`lookup_structural_member` reads once the graph is complete (`Refined`,
`Recursive`, `RecThis`, `Flexible`/`Annotated` proxies; outer refinement wins;
namespace is part of the `Name`; an unfilled binder, a bad `RecThis` binder and
a cyclic path are typed errors). `Symbol(S)` and `Name(N)` are different
targets even when `S` is called `N`. Consumers that need a symbol use
`Type::reference_symbol()`, which is `None` for a name target
(`annotation_class` and the unpickler's `lookup_owner` answer `None`, never
by comparing text); rebinding rebinds the prefix and keeps the target. No
synthetic symbol is ever allocated for a refinement member. A name target is
not a substitute for `SymbolResolver`.

`Bounds` is a genuine `>: low <: high` range; `AliasingBounds` is the info of
an alias (`= alias`) and is deliberately not `Bounds { low: alias, high: alias }`,
matching Dotty's `AliasingBounds`. It is not named `Alias`, which would collide
with `SymbolKind::TypeAlias`. `Flexible` is a real wrapper (Dotty's
`FlexibleType`) that model code must not strip; only member lookup sees through
it. `Annotated` is the same kind of proxy (Dotty's `AnnotatedType` is a
`CachedProxyType`): member lookup sees through it to its `underlying`, and it
is never stripped from the graph. An `Annotation` is its type
`ty`, its term `arguments` and an optional typed `tree`. `arguments` is
`Unavailable` (the producer did not reconstruct them) or `Known(list)`, and the
two are different facts: `Annotation::new` means unavailable, `Annotation::compact`
(the type Scala 3.9's `CompactAnnotation` wraps) is known-empty and loses
nothing. `tree: None` means "no typed tree attached", not "no payload"; the
TASTy adapter fills `arguments` (literals, class literals) from a full
annotation's constructor call and leaves the typed tree to Milestone 7.
`Annotation` is not `Copy`. Variance markers that TASTy writes after `TYPEBOUNDS` belong to a
`TypeLambda` bound, not to the bounds, so the model has no bounds-level
variance: they become the `declared_variance` of a rebound `TypeLambda`.

### `[BLOCKER 1]` Binder identity is `TypeId`, not a separate `BinderId`

The first draft had `BinderArena`/`Binder { id: BinderId, kind: BinderKind }`
storing only a *kind*, while the actual parameter/result data lived inside
`MethodType`/`PolyType`/`TypeLambda`, and `ParamRef { binder: BinderId,
index }` referenced the empty `Binder` object instead of the type that
actually owns the parameter. Resolving a `ParamRef` back to its bound
parameter — which the testing plan (§14) requires — was impossible without
inventing a second reverse-mapping table the design never defined.

The fix: make the semantic type itself the binder identity, matching how
Scala 3's actual type model works (a `ParamRef` points at the enclosing
`MethodType`/`PolyType`/`HKTypeLambda` value directly, not at an auxiliary
object).

```rust
pub struct MethodType {
    pub params: Vec<MethodParam>,
    pub result: TypeId,
    pub kind: MethodKind,
}

pub struct PolyType {
    pub params: Vec<TypeParam>,
    pub result: TypeId,
}

pub struct TypeLambda {
    pub params: Vec<TypeParam>,
    pub result: TypeId,
}
```

None of these carry their own `binder`/`id` field any more — once one of
them is allocated in the `TypeArena`, the `TypeId` it was allocated under
*is* its binder identity, exactly the way `Type::Recursive`'s own `TypeId`
already serves as the binder identity for any `RecThis` nested inside it. (The TASTy adapter keeps one canonical `RecThis` per `Recursive`, as Dotty's `RecType` keeps one `recThis`; that cache is adapter state, not part of the model.)
`BinderId`, `Binder`, `BinderArena`, and `BinderKind` are removed entirely
(§3, §4, §10) — matching them against a `Type` variant via `match` already
tells you the "kind" the old `BinderKind` existed to record.

This requires `TypeArena` to support constructing a self-referential
structure: the `PolyType`'s `params` need to contain `ParamRef { binder,
index }` values whose `binder` is the very `TypeId` the `PolyType` is about
to be stored under, before that `TypeId` exists. `types/arena.rs` gets a
two-phase allocation API for exactly this:

```rust
pub struct ReservedTypeId(TypeId);

impl ReservedTypeId {
    pub fn id(&self) -> TypeId;
}

impl TypeArena {
    /// Reserves a slot (backed by a `Type::NoType` placeholder) and returns
    /// its `TypeId` immediately, before the real value is known.
    pub fn reserve(&mut self) -> ReservedTypeId;

    /// Overwrites a reserved slot with its real value once all
    /// self-references to it have been constructed.
    pub fn fill(&mut self, id: ReservedTypeId, ty: Type) -> TypeId;
}
```

Example, for `def head[A](xs: List[A]): A`:

```rust
let poly_binder = types.reserve();
let param_a = types.alloc(Type::ParamRef { binder: poly_binder.id(), index: 0 });
// ... build `List[A]` and the enclosing `MethodType` using `param_a` ...
let poly = types.fill(poly_binder, Type::Poly(PolyType {
    params: vec![type_param_a],
    result: method_type_id,
}));
```

**Invariant, consistent with §12's error-handling policy:** every reserved
`TypeId` must be `fill`-ed before anything other than the code that reserved
it reads that slot. Reading an unfilled slot (still `Type::NoType`) from
outside the construction sequence that reserved it is a compiler bug, not a
recoverable error — the same class of internal invariant as an
out-of-range arena index.

`types/binder.rs` is kept only for this `reserve`/`fill` API; it no longer
defines a `Binder` type.

**How an adapter uses it (TASTy, Milestones 3a and 3b).** An adapter that reads a
format where a parameter reference names its binder by a stable key (TASTy: the
binder's AST address) ties the knot like this, for all three binder forms
(`TypeLambda`, `Poly`, `Method`) through one sequence: reserve the `TypeId`, publish
`key -> id` in its own address index, decode the children (a `ParamRef` now
resolves the key to the id), then `fill`. Between publishing and `fill` the
address entry refers to an unfilled slot, so during that window the adapter must
not read the slot, and it keeps the binder's kind and arity itself to validate a
reference to it. That state is the adapter's decoding state: it is not stored in
`dotty-core`. A failed decode discards the reservation with the rest of the
transaction (`SemanticStore::rollback_to` truncates reserved slots).

**Rebinding, not copying.** A binder's identity is its `TypeId`, so changing a
binder (for example applying declared variances to a `TypeLambda`, as Dotty's
`withVariances` does) is not a copy with one field changed: every `ParamRef`
inside the copy still names the old id. It requires a new binder and a
substitution of the old binder's references (Dotty's `subst`).
`rebind_type_lambda(store, source, declared_variances)` in `types/rebind.rs` is
that operation, and it is format-agnostic: it knows nothing of AST addresses.

- The new binder id is reserved *before* the children are transformed, and every
  reachable `ParamRef` of the old binder becomes the same parameter of the new
  one. The source graph is left unchanged.
- The transformation is memoized over `TypeId`s: a node reachable twice becomes
  one node, and a node that does not depend on the rebound binder keeps its id.
  Nothing is interned structurally.
- Nested `Method`/`Poly`/`TypeLambda` binders and `Recursive` types are copied
  under a reserved id, with their own `ParamRef`/`RecThis` remapped, so a copy is
  internally consistent (conservatively, whenever one is reached).
- `Annotated` gets a new annotation with the transformed type (an unaffected
  annotation is reused, a stored one is never mutated); `ClassInfo` gets
  transformed `prefix`/`parents`/`self_type` and keeps its class symbol and
  declaration scope, because a symbol table is not a type graph.
- The `match` over `Type` has no wildcard arm: adding a variant forces the
  rebinder to be reviewed.
- It is atomic (a store checkpoint is rolled back on any error) and returns a
  typed `TypeRebindError` for a source that is not a lambda, a variance count that
  differs from the arity, an unfilled slot, a cycle, or a graph deeper than 512.
  Nothing panics for a caller's semantic mismatch.

**Abstracting parameter symbols.** The same traversal builds binders from
*symbols*: `method_type_from_symbols(store, params, result, kind)`,
`poly_type_from_symbols` and `type_lambda_from_symbols` (`MethodParamSpec` /
`TypeParamSpec` carry the symbol and what the parameter says) are Dotty's
`MethodType.fromSymbols`, `PolyType.fromParams` and `HKTypeLambda.fromParams`.
The new binder id is reserved first; every `TypeRef` / `TermRef` whose target is
an exact parameter `SymbolId` (never a name, whatever its prefix, as `subst`
does) becomes `ParamRef { binder, index }` of it, in the parameters' own types
or bounds (so an F-bound names the binder) and in the result. An already built
nested binder that mentions a parameter is copied and rebound under a reserved
id, so building clauses from last to first needs no mutation of the inner one;
a type that mentions no parameter keeps its id; shared subgraphs are transformed
once; annotations are replaced, never mutated. The distinction is deliberate: a
parameter *symbol* is source identity (a definition), a `ParamRef` is identity
inside a binder, and a parameter's own info keeps naming symbols. A symbol given
for two positions is `DuplicateParameterSymbol`; the call is atomic. It knows
nothing of TASTy, so a namer can reuse it.

The TASTy adapter uses `rebind_type_lambda` for `TYPEBOUNDS` variance markers (Milestone 3c),
keeping the wire lambda cached at its own address and putting the derived
lambda in the bounds.

### `types/method.rs`, `types/constant.rs`, `types/class_info.rs`, `types/annotation.rs`

```rust
pub struct MethodParam {
    pub name: TermName,
    pub ty: TypeId,
    pub erased: bool,
    pub varargs: bool,
}

pub enum MethodKind {
    Plain,
    Implicit,
    Contextual,
}

pub struct TypeParam {
    pub name: TypeName,
    pub bounds: TypeId,
    pub declared_variance: Option<Variance>,
}

pub enum Variance {
    Invariant,
    Covariant,
    Contravariant,
}

pub enum Constant {
    Unit,
    Null,
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Char(u16),        // one UTF-16 code unit
    Int(i32),
    Long(i64),
    FloatBits(u32),   // IEEE-754 bit pattern
    DoubleBits(u64),  // IEEE-754 bit pattern
    String(NameId),
    StringUtf16(Vec<u16>),
    Class(TypeId),
}
```

`declared_variance` is `None` when nothing was declared (a standalone
`[A] =>> A`, a `PolyType`, a classfile generic method) and
`Some(Variance::Invariant)` for an explicit invariant declaration (TASTy's
`STABLE` marker). Dotty keeps the same two states: `HKTypeLambda`'s
`isDeclaredVarianceLambda` is `variances.nonEmpty`, and the list may hold
`Invariant`. Absence is deliberately not a `Variance` member (an "unspecified"
member would let code treat it as a variance). The field is *declared*
variance only; structural or inferred variance belongs to a later typer.

`varargs` is the JVM `ACC_VARARGS` distinction only. A TASTy `METHODtype` does not
encode it (a Scala repeated parameter is part of the parameter's *type*), so the
TASTy adapter always sets it to `false` and never infers it. `erased` is derived
in Dotty from an `ErasedParamAnnot` on the parameter's type, not from a clause
modifier, and the TASTy adapter derives it the same way: an annotation with the
exact class `scala.annotation.internal.ErasedParam` in the outer chain of
`Annotated` wrappers of the parameter type (`SemanticStore::has_annotation`),
with the wrapper left in place. The `MethodKind` of a TASTy
`METHODtype` comes from its `IMPLICIT`/`GIVEN` modifier tail.

`Constant::String` uses `NameId` (interned, see §5) rather than an owned
`String`, so AST literals and future TASTy constant pool entries share one
string table instead of allocating separately. `StringUtf16` is the
lossless representation for Scala string values containing an unpaired
UTF-16 surrogate; well-formed surrogate pairs are normalized to scalar
values in `Constant::String`.

Constants are lossless: every constant TASTy can write is representable
exactly. `Char` is a 16-bit code unit (a Scala `Char` can be a lone
surrogate), and floating-point constants hold their bit patterns, so NaN
payloads and negative zero survive; equality is bitwise. Use
`Constant::float`/`double` and `as_float`/`as_double` (and `char`/`as_char`)
instead of matching the bit variants when the numeric value is what matters.

```rust
pub struct ClassInfo {
    pub prefix: TypeId,
    pub class: SymbolId,
    pub parents: Vec<TypeId>,
    pub declarations: ScopeId,
    pub self_type: Option<TypeId>,
}
```

`[BLOCKER 2]` `declarations: ScopeId` lives **only** here — see §9 for why it
is removed from `Symbol`.

`[MAJOR 3]` The first draft's `Type::Annotated { underlying, annotation:
AnnotationId }` referenced an `AnnotationId`/`Annotation`/`AnnotationArena`
that the document never defined — an enum variant referencing an undefined
identity type. Real Dotty stores annotations in two places that must not be
conflated: a `List[Annotation]` directly on the symbol denotation
(`SymDenotation.myAnnotations`), and a single `Annotation` on
`AnnotatedType` for type-level annotations (`@unchecked`-style). Both are
included in the foundation, since `ANNOTATION_TAG` is already common in the
real TASTy corpus per `docs/category-five-coverage-3.9.0.md` ("Real corpus"
for tag 173) and deferring it would leave the future TASTy adapter unable to
represent ordinary annotated code:

```rust
pub struct Annotation {
    pub ty: TypeId,
    pub tree: Option<TreeId<Typed>>,
}

pub struct AnnotationArena {
    annotations: Vec<Annotation>,
}

impl AnnotationArena {
    pub fn alloc(&mut self, annotation: Annotation) -> AnnotationId;
    pub fn get(&self, id: AnnotationId) -> &Annotation;
}
```

`Annotation.tree` is optional because a foundation-era annotation may be
known only by its class `TypeId` (e.g. while the classfile/TASTy adapter is
still being written) before argument trees are modeled; `Symbol` gets a
matching `annotations: Vec<AnnotationId>` field (§9), mirroring Dotty's
symbol-level list.

### `types/arena.rs`

```rust
pub struct TypeArena {
    types: Vec<Type>,
}

impl TypeArena {
    pub fn alloc(&mut self, ty: Type) -> TypeId;
    pub fn get(&self, id: TypeId) -> &Type;
    pub fn get_mut(&mut self, id: TypeId) -> &mut Type;
    pub fn reserve(&mut self) -> ReservedTypeId;
    pub fn fill(&mut self, id: ReservedTypeId, ty: Type) -> TypeId;
}
```

No interning in the foundation PR, as before. A future `intern` method can
be added once it's clear which `Type` variants are safe to dedupe (structural
types with binders need care: two `Poly` types are only interchangeable if
their binders are alpha-equivalent, which plain structural `Eq` won't give
for free).

## 9. Symbol model

```rust
pub struct Symbol {
    pub name: Name,
    pub owner: Option<SymbolId>,
    pub kind: SymbolKind,
    pub flags: SymbolFlags,
    pub info: SymbolInfo,
    pub origin: SymbolOrigin,
    pub annotations: Vec<AnnotationId>,
    pub position: Option<SourceSpan>,
    pub links: SymbolLinks,
}
```

`[BLOCKER 2]` The first draft had both `Symbol.declarations: Option<ScopeId>`
and `ClassInfo.declarations: ScopeId` — two independently mutable fields for
the same fact, able to silently diverge (`Symbol(Foo).declarations =
Scope#10` while `ClassInfo(Foo).declarations = Scope#12`). `declarations` is
removed from `Symbol` entirely; `ClassInfo.declarations` (§8) is the sole
authority. Member/class lookup goes through:

```text
Symbol -> SymbolInfo::Complete(TypeId) -> Type::ClassInfo -> ScopeId
```

`Symbol` stays identity-plus-metadata; `Type`/`ClassInfo` stays the carrier
of semantic structure. A symbol that is not (yet, or ever) a class simply has
no `ClassInfo` and therefore no declarations to look through.

`ClassInfo` is therefore also the *publication boundary* between compilation
units. The TASTy adapter enters a class's scope in pass 1 and keeps it in its
unit-local index (a fast path); once the class is completed, `ClassInfo.declarations`
is that exact `ScopeId` (never a copy), and any other unit sharing the store finds
the members through the symbol's completed info, with no registry of class scopes
anywhere. Completing a class does not complete its members (they stay `Missing`,
as upstream's `unforcedDecls`) or its parents. Every adapter builds
`ClassInfo.prefix` as `Definitions::no_prefix` (not upstream's `owner.thisType`),
`class` as the class's own symbol, `declarations` as a scope owned by that class,
and `self_type` as `None` unless the source states one; parents are ordered,
direct-only semantic types and may differ in richness between adapters (TASTy
keeps `Applied` parents, a classfile only erased classes).

A constructor's completed info (the TASTy adapter's Milestone 5d2a) reuses
the same `no_prefix` convention for the type it constructs, independently of
`ClassInfo`: `Type::type_ref(no_prefix, class)`, or that applied to the
constructor's own leading type-parameter symbols, never the class's
completed `ClassInfo.prefix` copied by value, a fresh `ThisType`, or a
textual lookup. This is why a constructor can complete before, after, or
regardless of whether its owner's `ClassInfo` ever completes: both read the
same session-wide canonical prefix rather than depending on each other.

`close_over_this` (the TASTy adapter's Milestone 5d2b, `types/rebind.rs`) is
built by extending the same `Rebinder` graph transformer that
`rebind_type_lambda` and the symbol-abstraction primitives already use, rather
than writing a second graph copier: a `close_over: Option<(SymbolId,
TypeId)>` field on `Rebinder` turns every reachable `ThisType { class }`
matching the target into the one canonical `RecThis` of a given binder,
allocated once and reused for every further occurrence, the same "reserve the
binder first, then transform" order the lambda and recursive-type binders
already follow. A bounded, memoized, allocation-free pre-scan decides whether
anything actually depends on the target class at all; if not, the input id is
returned unchanged (mirroring `rebind_type_lambda`'s existing "a subgraph with
no dependency on the binder being built keeps its own id"), so no `Recursive`
is reserved, let alone left unfilled, for the common non-recursive case.
`ClassInfo.prefix`'s `no_prefix` convention and this operation are
independent: closing a refinement's `ThisType` over its own synthetic class
needs nothing from that class's `ClassInfo` (which, per the pass-1/projection
split above, is complete and parent-less from the moment the class is
entered), the same non-dependence a constructor's completed info already has
on its owner's.

A term's `SymbolInfo::Complete(TypeId)` is its declared type. Member lookup
may read it (read-only, never completing) to find the declaration scope of a
*stable* term prefix (`x.T`): an immutable field, value or by-name-free
parameter whose completed type is a class reference or an application of one.
`Definitions::and_type` / `or_type` are the canonical identities of the
`scala.&` / `scala.|` aliases, declared in the `scala` package by `Definitions::declare_special_aliases`; a source-level application
of them is normalized to `Type::And` / `Type::Or` by identity, never by text.

`[MAJOR 4]` The first draft's `linked: Option<SymbolId>` was meant to cover
both `class <-> companion object` and `module value <-> module class`, but a
bare link only answers "linked to something," never "how." Real Dotty stores
exactly **one** mutable link per class-like denotation —
`registeredCompanion: Symbol` (`SymDenotations.scala`, default `NoSymbol`) —
and derives everything else (`companionModule`, `companionClass`,
`linkedClass`) from that one field plus ordinary `SymbolKind`/name
navigation; it does not store three separate link fields. `dotty-core`
follows the same shape, simpler than what the review proposed:

```rust
#[derive(Default)]
pub struct SymbolLinks {
    /// The class's companion object symbol, or the object's companion class
    /// symbol. Meaningful only when `kind` is `Class`, `Trait`, `Object`, or
    /// `ModuleClass`.
    pub companion: Option<SymbolId>,
}
```

`module_class`/`source_module`-style navigation (an `Object` symbol's
backing `ModuleClass`, and vice versa) is a derived query over `owner` +
`kind` + `companion`, not a stored field — adding it as a free function once
the namer exists is cheap, and storing it redundantly would reopen exactly
the two-sources-of-truth problem `[BLOCKER 2]` just closed.

`SymbolKind` gives a stable category:

```rust
pub enum SymbolKind {
    Package,
    Class,
    Trait,
    Object,
    ModuleClass,
    Method,
    Constructor,
    Field,
    Value,
    Variable,
    Parameter,
    TypeParameter,
    TypeAlias,
    Local,
}
```

`SymbolFlags` is a bitset of orthogonal properties layered on top —
deliberately not one flags enum trying to do both jobs:

```rust
bitflags! {
    pub struct SymbolFlags: u64 {
        const PRIVATE      = ...;
        const PROTECTED    = ...;
        const ABSTRACT     = ...;
        const FINAL        = ...;
        const SEALED       = ...;
        const CASE         = ...;
        const IMPLICIT     = ...;
        const GIVEN        = ...;
        const LAZY         = ...;
        const MUTABLE      = ...;
        const INLINE       = ...;
        const TRANSPARENT  = ...;
        const OPAQUE       = ...;
        const EXTENSION    = ...;
        const STATIC       = ...;
        const SYNTHETIC    = ...;
        const JAVA_DEFINED = ...;
        const ERASED       = ...;
        const OVERRIDE     = ...;
        const ENUM         = ...;
        const EXPORTED     = ...;
    }
}
```

`[MINOR 1]` `SymbolOrigin` as a bare tag enum records only a *category*, not
enough provenance for future incremental-compilation invalidation (which
needs to know *which* source file, `.tasty` file, or classpath entry). Give
the payload-bearing variants opaque, core-owned IDs now rather than
documenting a false promise:

```rust
pub enum SymbolOrigin {
    Source(SourceId),
    Classfile(ClassfileOriginId),
    Tasty(TastyOriginId),
    Synthetic,
    Builtin,
}
```

`ClassfileOriginId`/`TastyOriginId` (§4) are opaque `u32` handles that
`dotty-core` does not interpret — the classfile/TASTy adapter layer (§2)
assigns and resolves them against its own classpath-entry or file table, so
`dotty-core` still does not depend on concrete `dotty-classfile`/
`dotty-tasty` types.

`SymbolInfo::{Missing, Deferred(CompletionId), Complete(TypeId), Error}`
bakes in lazy completion from day one — `CompletionId` stays an opaque
placeholder; no completer engine ships in the foundation PR.

`Scope` uses `HashMap<Name, SmallVec<[SymbolId; 2]>>` because overloaded
methods (`def foo(x: Int)` / `def foo(x: String)`) are ordinary, not an edge
case — a bucket of one avoids a heap allocation for the common case:

```rust
pub struct Scope {
    pub owner: Option<SymbolId>,
    entries: std::collections::HashMap<Name, smallvec::SmallVec<[SymbolId; 2]>>,
}

impl Scope {
    pub fn enter(&mut self, name: Name, symbol: SymbolId);
    pub fn remove(&mut self, symbol: SymbolId);
    pub fn lookup(&self, name: &Name) -> Option<SymbolId>;
    pub fn lookup_all(&self, name: &Name) -> &[SymbolId];
}
```

**Owner vs. scope stay distinct types.** Owner answers "who semantically
holds this symbol"; scope answers "where can this be found by name lookup."
They usually agree but are not the same concept (e.g. an imported name is in
scope somewhere it isn't owned). `Symbol` never gets a `children:
Vec<SymbolId>` field; child membership is reconstructed via scopes.

`SourceSemanticIndex` is also a `dotty-core` contract. It maps source trees to
canonical and derived semantic identities and stores declaration scopes and
source contexts. The namer populates this index, while the typer consumes it;
keeping it in core lets the typer remain independent of the namer crate.

`NamerState` (the `TreeId<Untyped> -> SymbolId` side table) explicitly does
**not** live in `dotty-core`. It belongs to the future `dotty-typer` crate's
namer, because it is intermediate compiler state, not part of the semantic
world model. AST nodes are never mutated to carry a `symbol: SymbolId` field
directly, for the same reason `[BLOCKER 2]` removed `Symbol.declarations`: it
would create a second, potentially-stale source of truth alongside the
tree's own `ty`.

### 9.1 Package identities (`packages.rs`)

`Packages` is the session's package registry, one per `SemanticStore`, shared
by every adapter (source frontend, TASTy unpickler, classfile loader) so the
same path is the same `SymbolId` whichever saw it first. Contract:

- one `SymbolKind::Package` symbol per path. Dotty's package term plus
  package module class are deliberately collapsed, so `TYPEREFpkg` and
  `TERMREFpkg` share one identity and `Type::ThisType` may name a package;
- named in `Namespace::Term`, after its own segment, `SymbolInfo::Missing`;
- an explicit root (empty path): empty name, no owner, own scope, also the
  unnamed package. Dotty has two packages here: `<root>` and, below it, the
  default package `<empty>` that a unit with no `package` clause lives in. The
  core collapses them into this one root, as it collapses package term and
  module class; adapters normalise: `<empty>` and `<root>` are the empty path; every named package's owner chain ends there and each
  package is declared in its owner's scope;
- a shared package keeps the origin of the adapter that entered it first;
- transactional: `mark` / `roll_back_to` forget newer packages and
  undeclare them from surviving owners.

### 9.2 The symbol resolver port (`resolution.rs`)

`SymbolResolver` is the format-agnostic boundary between an adapter and
whatever can supply symbols the adapter did not enter itself (later, the
classloader). A request is `MemberRequest { prefix: TypeId, name: Name,
selector, space: MemberSpace }` or a package path; the answer is a `SymbolId`. It contains no wire
concept: no addresses, name-table references, tags or wire signatures, so it
lives in `dotty-core`, not in an adapter, and the unpickler never depends on
the classloader.

`Ok(None)` means "this resolver cannot resolve it" and never "does not exist";
`Err(ResolutionError)` (ambiguous, malformed state) is never lowered to `None`.
Resolvers take `&SemanticStore`: until the port has a transactional contract
they do not allocate, so a failed decode leaves nothing to undo. Overload
selection by signature is a `#[non_exhaustive]` extension of `MemberSelector`;
today only `Unique` exists. `NoResolver` answers `None` to everything.

`MemberSpace` says where the declaration is looked for. `Prefix` (ordinary
references) searches the members of `prefix`. `Explicit(TypeId)` (Scala's
`TYPEREFin` / `TERMREFin`, written for private or shadowed symbols) searches
the declarations of that type, the declaring owner, while `prefix` stays only
how the reference is viewed: the resolver must not fall back to the prefix,
and an adapter accepts an answer only if it belongs to the requested owner.
The distinction is a general member-resolution concept, so no address or tag
appears in the port.

## 10. `store/semantic_store.rs`

`[MAJOR 5]` `SemanticContext` is renamed to `SemanticStore`. The prior name
risked a real collision: the future typer will need an actual dynamic
context — current owner, current lexical scope, imports in scope, expected
type/prototype, typing mode, enclosing contexts — which in Dotty is
literally called `Context`. Naming today's persistent arena aggregate
`SemanticContext` would force an awkward rename exactly when it's most
disruptive (once the typer already depends on the old name).

```rust
pub struct SemanticStore {
    pub names: NameInterner,
    pub symbols: SymbolTable,
    pub types: TypeArena,
    pub scopes: ScopeArena,
    pub annotations: AnnotationArena,
}
```

`annotations: AnnotationArena` is added versus the first draft, matching
`[MAJOR 3]`. `binders: BinderArena` is removed, matching `[BLOCKER 1]` — a
binder is just a `Type` living in `types`, not a separate arena.

This aggregate is intentionally "dumb storage," not a typing context. Once
the typer exists, it defines its own, separate type for the dynamic parts:

```rust
pub struct TypingContext<'a> {
    pub store: &'a mut SemanticStore,
    pub owner: SymbolId,
    pub scope: ScopeId,
    // expected type, typing mode, imports, etc. — defined when the typer is built.
}
```

## 11. Boundary with the binary/wire crates

This is the part that most needs to be explicit, since it's where the design
touches code that already exists.

- **`dotty-tasty`** keeps its own `NameRef` (a zero-based wire-table index)
  and `SignedName`/`NameSignature` types exactly as they are. These are wire
  concepts — they describe *where a name's bytes live in a `.tasty` file*,
  not what the name means. A future TASTy semantic adapter, living in the
  dedicated adapter layer from §2 (not inside `dotty-tasty`, and not inside
  `dotty-typer` — `[MAJOR 6]`), is responsible for walking
  `dotty_tasty::name_table` entries and calling `NameInterner::intern` to
  produce `dotty_core::Name` values, and for turning
  `dotty_tasty::ast::StructuredNode` trees into `AstArena<Typed>` plus
  `Symbol`/`Type` entries. `dotty-tasty` itself gains no new dependency and
  no new knowledge of symbols or types.
- **`dotty-classfile`** is currently a "type skeleton only" (its own
  `lib.rs` doc comment says so: no decoder/encoder/bounded reader exists
  yet). Its raw `ClassFile`/`ConstantPool`/`FieldInfo`/`MethodInfo`/
  `Attribute` stay exactly as scoped in `docs/classfile-format-jdk25.md`. The
  semantic loader (raw `ClassFile` → `Symbol`/`ClassInfo`/`Scope`) is new
  code in the same dedicated adapter layer, added later once `dotty-
  classfile` actually has a decoder to adapt.
- **`dotty-parser`** produces `AstArena<Untyped>` directly — no intermediate
  representation. Per `[MINOR 3]`, it needs a `NameInterner` and the
  `Span`/`SourceSpan` types, but nothing else from `SemanticStore`. Its source
  grammar is incremental: the current frontend reaches from simple/operator
  expressions and control flow through source-level definitions, definition
  annotations/modifiers, and initial class/trait/object template structure.
  Later parser increments expand that
  grammar without changing this semantic boundary.
- **The future typer** is the first component to touch every part of
  `dotty-core` at once: it reads `AstArena<Untyped>`, populates `symbols`/
  `types`/`scopes`/`annotations` on a `SemanticStore`, and produces
  `AstArena<Typed>`. It consumes an already-populated store; it does not
  decode classfiles or TASTy files itself.

`dotty-core` must not gain a dependency on `dotty-tasty`, `dotty-classfile`,
a future `dotty-parser`, or a future `dotty-typer`. This is the one hard
constraint from the original design and nothing above requires relaxing it.

## 12. Error handling policy inside `dotty-core`

AGENTS.md requires that library code "must not panic on malformed TASTy
input" and return typed errors instead. That rule is scoped to *decoding
untrusted external bytes* (TASTy files, classfiles, source text). It does not
translate directly to `dotty-core`'s arenas, whose IDs are never derived from
untrusted bytes — they are handed out by the same arena that will later
index with them, so an out-of-range `SymbolId`/`TypeId`/`TreeId<P>` reaching
`get`/`get_mut` indicates a compiler-internal bug (e.g. an ID leaked across
two independently constructed arenas), not malformed input.

Given that distinction, the foundation PR should:

- Let `AstArena::get`, `TypeArena::get`, `SymbolTable::get`, `Scope::lookup`,
  etc. panic (via slice indexing, `expect`, or similar) on an invalid
  internally-issued ID, exactly like `id-arena`/`la-arena` do. Document this
  explicitly on each `get`/`get_mut` doc comment so it isn't mistaken for an
  oversight. The same applies to reading a `TypeArena::reserve`-d slot before
  it has been `fill`-ed (§8) — that is the same class of internal-invariant
  violation, not a recoverable error.
- Reserve `Result`-returning APIs in `dotty-core` for genuinely fallible
  semantic operations added later (e.g. a future `intern`-with-limits, or a
  future bounded-recursion tree walk) — none of which are in scope for the
  foundation PR.
- Leave AGENTS.md's untrusted-input rule fully binding on the adapters in
  §11: a TASTy or classfile adapter that maps decoded wire data onto
  `dotty-core` must itself validate before calling `alloc`/`intern`, the same
  way `dotty-tasty::ast` already validates AST references before trusting
  them.

## 13. New dependencies

The workspace is currently dependency-light: `unicode-ident` (lexer),
`serde`/`serde_json` (dev-only, `dotty-tasty`). This design introduces two
candidate dependencies that need an explicit decision before the foundation
PR lands:

| Dependency | Used for | Alternative if declined |
| --- | --- | --- |
| `bitflags` | `SymbolFlags`, `types/flags.rs` | Hand-rolled `u64` newtype with `const` associated flags and manual `Debug`; more boilerplate, zero new dependency |
| `smallvec` | `Scope`'s overload buckets | Plain `Vec<SymbolId>`; simpler, one extra allocation per overloaded name (rare relative to total symbol count) |

Recommendation: take both — they are small, widely used, and remove real
boilerplate/allocations — but this is a call for whoever reviews the
foundation PR, not something to slip in silently given the project's
otherwise deliberate minimalism.

## 14. Testing plan

AGENTS.md's testing requirements ("Add a focused unit test for every new
parser, encoder, validator, tag, enum variant, or meaningful edge case," "Add
assertions for exact error variants... not only `is_err()`") apply here as
much as to `dotty-tasty`. Concretely, for the foundation PR:

```text
crates/dotty-core/tests/
├── ast_phase_safety.rs
├── ast_structure.rs
├── types.rs
├── binders.rs
├── symbols.rs
├── scopes.rs
└── semantic_fixtures.rs
```

- **Phase safety** (`ast_phase_safety.rs`): the interesting assertion is
  "this does not compile," which needs `compile_fail` doctests on
  `ast/phase.rs` and `ast/tree.rs` rather than the `trybuild` crate — this
  keeps the phase-safety proof dependency-free, consistent with §13's
  minimalism concern. Cover, as `compile_fail` cases: constructing
  `TreeKind::PhaseSpecific(..)` on `Typed`; constructing a `ValDef<Typed>`
  with a `Modifiers` value as its `metadata`. Runtime tests in the same file
  cover the parts that *do* need to compile: a `Tree<Typed>` always has a
  real `TypeId` after `TypedAstBuilder::ident`/`apply`/etc.
- **`binders.rs`**: build a `Poly`/`Method`/`TypeLambda` via
  `TypeArena::reserve`/`fill`, resolve a nested `ParamRef` back through its
  `binder: TypeId`, and assert it round-trips to the exact `TypeParam`/
  `MethodParam` it was bound to — this is the guarantee `[BLOCKER 1]` exists
  to make possible. Also test the unfilled-reservation-is-a-bug invariant
  (§12) with a `#[should_panic]` test, not a `Result`.
- **One test per remaining `Type` variant construction**, matching the "one
  test per enum variant" rule already applied to TASTy tags in
  `docs/testing-strategy.md`.
- **`symbols.rs`**: one test asserting `Symbol` has no `declarations` field
  (i.e. lookup only works through `ClassInfo`, `[BLOCKER 2]`); one test
  building a class + companion pair and asserting `SymbolLinks::companion`
  resolves both directions (`[MAJOR 4]`); one test per `SymbolOrigin`
  variant, including that `Classfile`/`Tasty` carry their opaque origin ID
  (`[MINOR 1]`).
- **Scope overload tests**: `enter` two symbols under one name, assert
  `lookup_all` returns both in insertion order; `remove` one, assert
  `lookup` still resolves the other.
- **Semantic fixtures** (`semantic_fixtures.rs`): small, named Scala
  fragments building the intended symbol/type shape by hand (not by parsing
  — no parser exists yet): generic identity method, overloads, class +
  companion via `SymbolLinks`, path-dependent type (`trait Foo { type T };
  def f(x: Foo): x.T`), higher-kinded type parameter
  (`trait Functor[F[_]]`), contextual parameter
  (`def show[A](x: A)(using Show[A]): String`), type lambda
  (`[X] =>> Either[String, X]`), match type, and one fixture with an
  annotation (`@deprecated class Foo`) exercising `Symbol.annotations` +
  `AnnotationArena`. Each fixture test builds the `SemanticStore` state a
  namer/typer *would* produce and asserts its shape — this both documents
  the intended shape and pins it before any namer exists to produce it
  automatically.

`cargo fmt`, `cargo test --workspace --all-targets`, `cargo clippy
--workspace --all-targets -- -D warnings`, and `git diff --check` (the
existing CI job in `.github/workflows/ci.yml`) all apply unchanged to the new
crate; no new CI job is needed for the foundation PR.

## 15. Definition of done for the foundation PR

- `TreeId<Untyped>` and `TreeId<Typed>` are statically distinct types.
- A `Tree<Typed>` always carries a `TypeId`; a `Tree<Untyped>` never does.
- Untyped-only syntax (`UntypedNode`) cannot type-check as part of a `Typed`
  tree, and `Modifiers` cannot appear as a typed definition's metadata.
- Every `TreeId` field inside `TreeKind<P>` is `TreeId<P>` — no shared node
  hard-codes a reference into the other phase's arena.
- A tree's source position is `Option<SourceSpan>`; nothing depends on a
  `SourceId` sentinel value to mean "no source."
- `TermRef`/`TypeRef` designate a `SymbolId` or, for a member with no
  symbol, a `Name`, and every `Type` variant that needs one carries a
  `prefix`.
- `MethodType`, `PolyType`, and `TypeLambda` are binders via their own
  `TypeId`; `ParamRef` identifies `(binder: TypeId, index)` and resolves back
  to the exact bound parameter, with a test proving it.
- `Type::Annotated` and `Symbol.annotations` both resolve through a defined
  `AnnotationId`/`AnnotationArena`.
- `Scope` supports overloads (`SmallVec`/`Vec` bucket per name) and keeps
  term/type namespaces distinct via `Name`.
- `Symbol` has an `owner`, may have `SymbolInfo::Deferred`, has no
  `declarations` field of its own, and `ClassInfo` is the sole authority for
  a class's `ScopeId`.
- `Symbol`'s companion/module relationship is an explicit `SymbolLinks`
  field, never a bare `Option<SymbolId>` with unstated meaning.
- `SymbolOrigin`'s classfile/TASTy variants carry an opaque, core-owned
  origin ID rather than claiming untraceable provenance.
- Nothing in `dotty-core` depends on `dotty-tasty`, `dotty-classfile`, or any
  future parser/typer/compiler crate.
- The crate builds, is added to `[workspace.members]`, and passes the
  existing CI job (`fmt`, `test`, `clippy -D warnings`, `cargo doc`,
  `git diff --check`) with the test layout from §14 in place.

Not in scope for the foundation PR: the typer itself, subtyping, inference,
lookup through inheritance, implicit resolution, classfile → symbol
conversion, TASTy → `dotty-core` conversion, inline-call provenance on
`Inlined`, and the parser itself.

## 16. Open decisions for review

1. `bitflags` and `smallvec` as new dependencies (§13).
2. Whether `dotty::core` is re-exported from the root facade now or deferred
   until a real consumer exists (§3). This document defaults to "defer."
3. Confirm `compile_fail` doctests are an acceptable substitute for a
   `trybuild`-based phase-safety test suite (§14).
4. Naming/packaging of the classfile/TASTy adapter layer introduced in §2 —
   candidates include a single `dotty-loader` crate, or two crates
   (`dotty-classfile-sema`, `dotty-tasty-sema`) mirroring the existing
   `dotty-classfile`/`dotty-tasty` split. Only the dependency direction
   (outside the codec crates, outside the typer) is fixed by this document.
5. Whether `Annotation.tree: Option<TreeId<Typed>>` is enough for the
   foundation PR, or whether annotation *arguments* need their own
   lighter-weight representation before the typer exists to produce full
   typed trees for them (§8, `[MAJOR 3]`).

## 17. Suggested follow-up work after the foundation PR

Once the foundation lands, the following can proceed in parallel, each
depending only on `dotty-core`:

- Further Scala source grammar increments in `dotty-parser`, producing
  `AstArena<Untyped>` from today's `dotty-lexer` output and the shared
  `dotty-core::token` contracts.
- Classfile semantic adapter (`dotty-classfile` decoder + loader producing
  `Symbol`/`Type`/`ClassInfo`), in the dedicated adapter layer from §2.
- TASTy semantic adapter (`dotty-tasty` structured trees →
  `Symbol`/`Type`/`AstArena<Typed>`), in the same adapter layer.
- Namer (`Untyped AST` → symbols entered into scopes).
- Typer skeleton (`AstArena<Untyped>` → `AstArena<Typed>`), including
  finally deciding `Inlined.call`'s representation once inline expansion is
  in scope.

At that point `dotty-rs` has one stable semantic core that every compiler
component builds against, instead of renegotiating the boundary between
modules on every new component.
