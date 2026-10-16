# Source semantic TASTy pickler: minimal Scala 3.9 contract

This is the readiness audit for [#1189](https://github.com/scytrowski/dotty-rs/issues/1189),
not a pickler implementation. It audits repository revision `3882dbb50` and Scala
3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`, targeting TASTy
**28.9.0**. The proposed first complete output is:

```scala
package example
class Answer:
  def value: Int = 42
```

The source frontend has sufficient canonical declaration identities and can
produce this method's real typed RHS. It does **not** currently expose a complete
typed package/class/template root. Use a pickler-local input plan over those
identities and body roots; do not add a shared `TypedCompilationUnit` or make
Typer depend on a serializer. Full compiler interoperability remains the
[#1195](https://github.com/scytrowski/dotty-rs/issues/1195) gate.

## 1. Ownership and dependency boundary

Arrows below mean input/dependency flow, not new dependencies in the frontend:

```text
dotty-core (SemanticStore, Definitions, TypedAst, canonical IDs) ─┐
                                                               ├─> dotty-tasty-pickler
 dotty-tasty (names, trees, sections, file codec) ────────────────┘       │
                                                                       v
                                                               owned TASTy bytes

integration tests only:
 source -> Lexer -> Parser -> Namer -> Typer -> source-bound PicklePlan
                                                   JVM plan: comparison only
 bytes -> Rust validator / semantic unpickler / pinned Scala compiler
```

`dotty-tasty-pickler` should have only `dotty-core` and `dotty-tasty` as normal
workspace dependencies. Typer, Namer, Parser, Lexer, unpickler and JVM integration
belong in root tests or dev dependencies. It neither loads classpaths nor emits
class files. `dotty-tasty` remains format-only; `dotty-core` remains independent
of every adapter. A future codec construction helper must be wire-level, not a
second symbol/type model.

## 2. Canonical input evidence

These are current code locations, including the functions/fields that establish
the contract. File links are relative to this document.

| Fact | Authority and use |
| --- | --- |
| Phase distinction | [`ast/phase.rs`](../crates/dotty-core/src/ast/phase.rs), `AstPhase` and `impl AstPhase for Typed`: `TypeInfo = TypeId`, `DefMetadata = ()`, `TemplateMetadata = ()`. Typed metadata cannot supply modifiers. |
| Tree payload | [`ast/tree.rs`](../crates/dotty-core/src/ast/tree.rs), `Tree` has `kind`, optional `SourceSpan`, and `ty`; no declaration `SymbolId` field. `TreeKind` permits typed definitions but does not certify frontend production of them. |
| Source structure/order | [`ast/common.rs`](../crates/dotty-core/src/ast/common.rs), `PackageDef.stats`, `TypeDef.rhs`, `Template.constructor/parents/body`, `DefDef.type_params/value_param_clauses/source_param_clause_order/tpt/rhs`. A class is a `TypeDef` with a `Template`, not a separate `ClassDef`. |
| Canonical source identity | [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs), `SourceSemanticIndex::symbol_at(source, tree)`. The key includes `SourceId` because tree IDs are arena-relative. `definition_of(symbol)` supplies reverse canonical/derived provenance; packages deliberately have no unique reverse source definition. |
| Derived identity | Same file, `derived_symbol_at(owner, source, tree)`. Constructor parameter copies are distinct owner-specific symbols, never replacements for `symbol_at`. |
| Naming class/constructor | [`naming.rs`](../crates/dotty-namer/src/naming.rs), `enter_class_or_trait_header`, `scan_class_or_trait`: enter class, class scope, primary constructor from `template.constructor`, constructor scope and parameter copies. `enter_method_header` enters ordinary members and parameters. |
| Semantic metadata | [`symbols/symbol.rs`](../crates/dotty-core/src/symbols/symbol.rs), `Symbol.{owner,kind,flags,visibility,info,origin,annotations,position}`. Owners and scope membership are distinct. [`completion.rs`](../crates/dotty-core/src/symbols/completion.rs), `SymbolInfo::Complete(TypeId)` is required for emitted declarations. |
| Class membership/parents | [`types/class_info.rs`](../crates/dotty-core/src/types/class_info.rs), `ClassInfo.{class,prefix,parents,declarations,self_type}`. Its declaration scope is the semantic membership authority; do not create `Symbol.children`. |
| Class completion | [`completion/declarations.rs`](../crates/dotty-typer/src/typer/completion/declarations.rs), `complete_class_info`: projects source parents and inserts `Definitions.object_type` if none, or before a first trait parent. Publishes `ClassInfo` with `prefix = no_prefix`. |
| Method signature | [`completion/methods.rs`](../crates/dotty-typer/src/typer/completion/methods.rs), `complete_method_signature_body`: assembles clauses around declared/inferred result. No clauses means the result type itself; `def value: Int` need not have a `Type::Method` wrapper. |
| Constructor signature | [`completion/constructors.rs`](../crates/dotty-typer/src/typer/completion/constructors.rs), `complete_constructor_signature`, `normalize_constructor_clauses`, `constructor_effective_result`: normalizes an empty value clause and uses the owner instance type as effective result. |
| Ordered signature payload | [`types/method.rs`](../crates/dotty-core/src/types/method.rs), `MethodType`/`MethodParam` encode parameter names/types and clause kind, **not parameter SymbolIds**. Recover identities from source clauses/index, then cross-check the signature. |
| Typed body production | [`typer/mod.rs`](../crates/dotty-typer/src/typer/mod.rs), `type_expression`, `type_expression_expected`, `typed_ast`, `source_typed_index`; [`expression/mod.rs`](../crates/dotty-typer/src/typer/expression/mod.rs), constant typing/widening. A literal body has a constant type; its widened declared result is `Definitions.int`. |
| Source/typed mapping | [`source_typed_index.rs`](../crates/dotty-typer/src/source_typed_index.rs), `SourceTypedIndex::get/insert`: one recorded typed replacement per represented source node; insertion detects conflicting replacements. It is Typer-owned, not a core interface. |
| Real integration precedent | [`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs), `compile_source`, `find_single_source_class`, `collect_source_methods`: follows source child edges, resolves canonical symbols, completes them, types method RHSs and builds explicit ordered plans. |
| JVM comparison | [`dotty-jvm/src/lib.rs`](../crates/dotty-jvm/src/lib.rs), `JvmCompilationUnit`, `JvmClassPlan`, `JvmMethodPlan`: borrowed store/typed arena, canonical class/method/parameter IDs, optional body. JVM initialization/fields/locals policies are not pickler input. |

For `Answer`, start at the parsed root, descend `PackageDef.stats` to the class
`TypeDef`, and use `symbol_at(source, class_tree)`. Follow `TypeDef.rhs` to its
`Template`; bind the constructor with `symbol_at(source, template.constructor)`.
Walk `template.body` in source order and bind each method tree using `symbol_at`.
Require each method/constructor owner to equal the class ID, and walk the class's
owner chain to the canonical package/root boundary. Use scope membership as an
additional consistency check, never to infer owners.

For each formal clause, preserve its shape (including an empty clause) and source
parameter order. Resolve each source parameter through the canonical or explicitly
owner-derived index entry appropriate to that role, check ownership and completed
type, then compare with `MethodType` clause structure. Scope iteration is unsuitable:
[`symbols/scope.rs`](../crates/dotty-core/src/symbols/scope.rs),
`Scope::entered_symbols` enumerates a `HashMap` and has no declaration-order contract.

For the actual RHS, complete `value`, obtain `expression_context_for(method)`, and
call `type_expression_expected(rhs, context, definitions.int)` (or type then check
conformance). Record the returned `TreeId<Typed>` and check that
`source_typed_index().get(source, rhs)` returns exactly that ID. A successful
signature completion alone is insufficient: an explicitly typed method's RHS
need not have been typed by that operation.

## 3. Readiness gaps and bounded decisions

| Gap with concrete location | Required boundary decision / downstream work |
| --- | --- |
| `Tree<Typed>` has no declaration symbol field (`ast/tree.rs`); `SourceTypedIndex` is in Typer and has no compilation-unit completion/provenance guarantee. | Integration must bind source declaration and RHS IDs to canonical symbols/typed roots. A checked adapter supplies this evidence through a core-only port; reject a missing binding. No name matching. |
| `SourceTyper::type_expression_inner` (`typer/mod.rs`) handles expressions/local forms and rejects other forms as `UnsupportedExpression`; it does not build a package/class/template tree. `TypedAstBuilder` (`ast/typed.rs`) has expression/type-tree constructors, not a unit builder. | Build declaration wire structure from the explicit plan + completed semantics; consume actual typed RHS nodes. Local typed `DefDef` support is not a whole-unit API. |
| `Typed::{DefMetadata,TemplateMetadata} = ()` (`ast/phase.rs`). | Read flags, visibility and annotations from `Symbol`. Require an explicit supported set. Reject unsupported annotations/visibility/flags; absence of syntax is not evidence of public/unannotated semantics. |
| Bootstrapped `Int`, `Unit`, `Object` are `Builtin`, `owner: None`, `info: Missing` (`definitions.rs`, `bootstrap`, `builtin_symbol`). `declare_special_aliases` only installs `&`/`|` package owners. | Use exact `Definitions` identities for a bounded external-name mapping (`scala.Int`, `scala.Unit`, `java.lang.Object`). Do not turn every ownerless or same-named symbol into a builtin. Do not require external builtins to have source `ClassInfo`. |
| Source `ClassInfo.prefix` is normalized to `no_prefix` (`complete_class_info`); it does not contain the full Scala package prefix. | Reconstruct serialized package prefixes from the canonical owner chain. Never emit a local direct reference for an external builtin just because its core prefix is `NoPrefix`. |
| `ClassInfo.parents` stores types, not typed superclass constructor trees (`types/class_info.rs`); the parser's implicit parent list is empty. | The pickler must explicitly lower the supported implicit Object parent to the oracle's constructor-call shape, as a serialization rule for this exact class form. Require the canonical Object parent; no arbitrary-parent inference. |
| Source constructor semantic result is the owning class (`completion/constructors.rs`), but the wire return tree is `scala.Unit` (also documented by [`unpickler/constructor.rs`](../crates/dotty-tasty-unpickler/src/constructor.rs)). | Lower constructor return syntax separately from effective result. The canonical source constructor remains authoritative. Restrict to the validated default constructor; do not copy JVM constructor bytecode generation. |
| A source default constructor need not have a typed RHS, and the upstream template uses a synthetic local template owner (`TreePickler`, `registerDef(tree.symbol)`). | Plan must identify default-constructor provenance explicitly. The observed default constructor has **no RHS**, an `EMPTYCLAUSE`, a Unit return tree and a `STABLE` tail. Emit precisely that bounded form; template bookkeeping stays local, without fake source `SymbolId`s or a claimed typed constructor body. |
| `SymbolFlags` (`symbols/flags.rs`) does not model Scala's constructor `STABLE` marker. The independently printed class also has a compiler-injected `SourceFile` annotation. | Treat default-constructor `STABLE` as a narrowly derived wire fact after checking constructor kind/provenance, not a new guessed semantic flag. General annotations remain unsupported. #1190 must test whether compiler provenance annotation/position omission is interoperable; do not promise omission compatibility from an unchanged fixture. |
| [`unpickler`](../crates/dotty-tasty-unpickler/src/unpickler.rs) enters/completes symbols and types, not a typed AST/body. | Semantic parity checks compare normalized owners/signatures/parents; separately inspect the RHS structurally and compile it independently. |
| Core IDs are arena-relative numeric identities (`ids.rs`), without store-generation tokens. An in-range ID from another arena can alias a live slot. | Validate bounds/relationships and require one compilation session's borrowed arenas and binding provider. Do not promise detection of every same-slot foreign/stale ID. A generational-ID redesign is outside this sprint. |

## 4. Pinned Scala emission evidence

All upstream paths below are relative to Scala revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
The local checkout was verified at that exact hash before inspection.

- [`transform/Pickler.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/transform/Pickler.scala),
  `run`: slices the typed unit per top-level class (linked companions are grouped),
  pickles trees, compacts references, writes positions/comments/attributes, then
  assembles bytes. This is pre-backend typed Scala, before JVM erasure. The Rust
  adapter consumes source semantics at this logical point, not JVM descriptors.
- [`core/tasty/TreePickler.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/tasty/TreePickler.scala),
  `pickleDef`, `pickleTree`: `PACKAGE` writes package type then stats; `TYPEDEF`
  writes name then `TEMPLATE`; template writes header parameters, parents, optional
  self, then constructor and body stats. Definitions write parameters, return/type
  tree, optional RHS and modifier/annotation tail. There is no serialized owner-ID
  field: nesting and reference prefixes encode ownership.
- Same file, `pickleParamss`: **no clause** writes no marker; **one empty clause**
  writes `EMPTYCLAUSE`; adjacent clauses of the same category use `SPLITCLAUSE`.
  A declaration's signature is reconstructed from its parameter definitions and
  result type, not an extra `METHODtype` payload on every `DEFDEF`.
- Same file, `pickleNewType`: package references are `TERMREFpkg`/`TYPEREFpkg`;
  local `NoPrefix` references use `*REFdirect`; prefixed local symbols use
  `*REFsymbol`; ordinary external refs use `*REF` with name/signature and prefix.
  Private/shadowed external refs use `*REFin` and are deferred here.
- Same file, `preRegister`, `registerDef`, `pickleForwardSymRef`, `pickleStats`:
  register identities before emitting siblings, reserve unresolved references,
  bind at definition addresses, patch then compact. Wire addresses are offsets
  within `ASTs`, not `SymbolId`, `TreeId`, or whole-file offsets.
- [`core/tasty/NameBuffer.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/tasty/NameBuffer.scala),
  `nameIndex`: dependency-first insertion into a `LinkedHashMap`, zero-based refs,
  with signature-aware names where required by referenced methods. Equal spelling
  does not make two declarations the same semantic symbol.
- [`core/tasty/TastyPickler.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/tasty/TastyPickler.scala),
  `assembleParts`: header/version/tooling/UUID, length-prefixed names, then ordered
  named sections. Scala derives UUID halves from name/tree and other-section
  hashes. Byte equality with Scala output is not the Rust adapter's contract.

The reusable Rust wire layer is already present in
[`file.rs`](../crates/dotty-tasty/src/file.rs) (`TastyFileBuilder`),
[`name_table.rs`](../crates/dotty-tasty/src/name_table.rs) (`NameTableBuilder`),
[`ast.rs`](../crates/dotty-tasty/src/ast.rs) (raw/structured definitions and refs),
[`term.rs`](../crates/dotty-tasty/src/term.rs) (constants), and
[`section.rs`](../crates/dotty-tasty/src/section.rs) (`EncodedSection`, positions,
attributes, address-aware/relocated structured encoding). These APIs encode wire
structure; they do not choose source symbols or infer missing declaration edges.
See [TASTy API](tasty-api.md), [format](tasty-format-3.9.0.md), and
[semantic unpickler](tasty-semantic-unpickler.md) for existing guarantees.
