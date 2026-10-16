# Source semantic TASTy pickler: minimal Scala 3.9 contract

This document began as the readiness audit for
[#1189](https://github.com/scytrowski/dotty-rs/issues/1189), auditing repository
revision `3882dbb50` and Scala
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
| Naming class/constructor | [`naming.rs`](../crates/dotty-namer/src/naming.rs), `enter_class_or_trait_header`, `scan_template_body`: enter class, class scope, primary constructor from `template.constructor`, constructor scope and parameter copies. `enter_method_header` enters ordinary members and parameters. |
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
| `SymbolFlags` (`symbols/flags.rs`) does not model Scala's constructor `STABLE` marker. The independently printed class also has a compiler-injected `SourceFile` annotation. | Treat default-constructor `STABLE` as a narrowly derived wire fact after checking constructor kind/provenance, not a new guessed semantic flag. General annotations remain unsupported. The [#1190 oracle](tasty-pickler-oracle.md) confirms the minimal file can omit the injected provenance annotation and Positions/Comments while retaining SOURCEFILEattr. |
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

## 5. Minimal output and identity-to-wire mapping

An audit experiment compiled the exact example with the cached Scala 3.9.0
compiler, inspected `-Yprint-tasty`, and compiled a standalone copy with
`-from-tasty`. The observed output is one `example/Answer.tasty`, alongside the
compiler's `example/Answer.class` in its original output directory. The standalone
copy had no class file; Scala emitted a new class in a separate directory.
This confirms the **input layout**, not interoperability of future Rust output.
Checked-in fixtures and repeatable snapshot tooling belong to #1190.

The printed AST had this shape (addresses and name indexes intentionally omitted):

```text
PACKAGE
  TERMREFpkg example
  TYPEDEF Answer
    TEMPLATE
      APPLY                              // implicit Object parent construction
        SELECTin <init>[SIGNED: () -> java.lang.Object]
          NEW
            TYPEREF Object (TERMREFpkg java.lang)
          SHAREDtype -> that Object type // selection owner
      DEFDEF <init>
        EMPTYCLAUSE
        TYPEREF Unit (TERMREFpkg scala)
        STABLE                           // modifier; no constructor RHS
      DEFDEF value                       // no EMPTYCLAUSE
        IDENTtpt Int
          TYPEREF Int (TERMREFpkg scala)
        INTconst 42
    ANNOTATION SourceFile(...)           // compiler provenance, class tail
```

The compiler also printed `Positions` and `SOURCEFILEattr` in `Attributes`; its
name table included `Comments` even though the printed file had no comment body.
Do not infer section presence merely from an interned name. The annotation
introduces additional `APPLY`, `SELECTin`, `NEW`, signed-name and string nodes.
It is compiler-generated, not evidence of a source annotation in the core store.

| Source semantic identity / edge | TASTy projection and required check |
| --- | --- |
| Class owner chain, package `SymbolId` | `PACKAGE` + `TERMREFpkg` qualified name. Walk actual package owners, validate kinds/cycles/root, render names only after identities are fixed. |
| Class `SymbolId`, complete `ClassInfo` | One `TYPEDEF` containing `TEMPLATE`; definition-address map keyed by class ID. `ClassInfo.class` and scope owner must equal the root ID. Package/class output path follows this same chain. |
| Template and source class association | Template header is not a second source class symbol. Keep its address/bookkeeping private; validate the source `TypeDef.rhs` edge and class ownership. |
| `ClassInfo.parents == [Definitions.object_type]` | Explicit implicit-parent `APPLY(SELECTin(NEW(Object type), signed Object <init>, Object owner type))` from the observed oracle. `java.lang.Object` is a language dependency name here, not a JVM descriptor. |
| Canonical primary constructor `SymbolId` | `DEFDEF <init>`, empty value clause, `scala.Unit` return tree, no RHS, `STABLE`; bind the constructor ID to the definition address. Completed constructor info must instead return the owning class after clause normalization. |
| Method `SymbolId`, source `DefDef` | `DEFDEF` name is an ordinary name, scoped by class nesting; no owner guessed from spelling. Signature comes from clauses + result tree. Parameterless `value` carries no empty-clause marker. |
| Ordered parameter `SymbolId`s | `PARAM` definitions in source clause order; method owners/types checked against semantic signature. Empty and absent clauses stay distinct. Nonempty clauses are characterization fixtures, deferred from first complete slice. |
| Method result `TypeId == Definitions.int` | Oracle uses `IDENTtpt(Int, TYPEREF(Int, TERMREFpkg(scala)))`; a semantic `TypeTree` alone would encode the type directly. #1190 must pin any allowed syntax-wrapper normalization before changing this observed form. |
| RHS `(source, untyped_rhs) -> typed_body` | Exact source binding for this method, pointing into supplied typed arena. Read `TreeKind::Literal(Constant::Int(n))` and `Tree.ty = Type::Constant(Constant::Int(n))`; emit that actual `INTconst n`, not the declared result type or a hardcoded 42. |
| Local declaration references | `TYPEREFdirect`/`TERMREFdirect` for true local no-prefix refs, or `*REFsymbol` plus prefix; targets are mapped definition addresses, including forward fixups. No raw core numeric ID goes into an address field. |
| External builtin references | Identity-specific `Definitions` mapping gives public `TYPEREF`/`TERMREFpkg` for Int/Unit/Object. The Object constructor's signed reference is a bounded oracle-defined dependency operation, not a fabricated source constructor ID. |
| Reused semantic `TypeId` | Optional deterministic `SHAREDtype` to a previously bound compatible type node. No merging of separately identified declarations by spelling. |
| Flags/visibility/annotations | Definition tails come from semantic symbol facts, plus explicitly documented default-constructor/provenance wire rules. Unsupported metadata produces an error, never silent loss. |
| Source ID, source filename and optional spans | Check `SymbolOrigin::Source`, symbol/tree source spans and plan source agree. Encode normalized source name in `Attributes`; positions use final relocated AST addresses if enabled. Never substitute lexical-context owner for semantic owner. |

### Supported and deferred surface

| Surface | First complete file (#1194/#1195) | Deferred / oracle-only |
| --- | --- | --- |
| Unit/output boundary | One ordinary top-level class in explicit `package example`; one package-aligned `.tasty` per class | Default/nested packages, multiple classes, linked class/object outputs |
| Class | Public, no type/value parameters, no self type, no fields, no explicit extends or initialization statements; exact canonical implicit Object parent | Traits, objects, companions, nesting, enums, arbitrary inheritance/constructors |
| Method | One public ordinary monomorphic **parameterless** concrete method, declared Int result | Nonempty/empty-parens/curried/contextual clauses, abstract methods: #1190 characterization only |
| Signature/types | Int result, constant Int RHS type; Unit constructor return syntax; Object parent; canonical local source reference infrastructure | Generic/applied/refined/alias/match/capture types and arbitrary external member references |
| Body | Actual typed Int literal, including its value/type agreement | Calls, blocks, if/match, locals, functions, returns and all other expressions |
| Metadata | Public/no extra source flags or annotations; required constructor marker; normalized source identity | General annotations/modifiers, full position/comment preservation, outline/best-effort/Java TASTy |

An empty class is an oracle control, not an automatic broadening of the first
end-to-end acceptance test. #1193 may expose a clearly marked partial declaration
skeleton for structural tests; it must not present a missing concrete RHS as a
complete or interoperable file.

The [#1190 oracle](tasty-pickler-oracle.md) independently compiled reduced Answer
without `Positions`, `Comments`, or Scala's injected `SourceFile` annotation,
while retaining `SOURCEFILEattr`. This freezes the omission decision for #1191:
the minimal producer may omit those sections/that compiler annotation. The
observed parent, constructor STABLE marker, Int `IDENTtpt` wrapper and RHS stay
in the first contract. Do not add general annotation support or synthetic core
symbols merely to reproduce incidental upstream bytes.

## 6. Immutable input contract

[#1191](https://github.com/scytrowski/dotty-rs/issues/1191) implements the validation
boundary in [`dotty-tasty-pickler`](../crates/dotty-tasty-pickler/src/lib.rs).
The following abbreviated interface includes source evidence: an arbitrary
in-range typed-body ID is not an authoritative method/body association.

```rust,ignore
// All semantic IDs below belong to these exact borrowed stores/arenas.
struct PicklePlan<'a> {
    store: &'a SemanticStore,
    definitions: &'a Definitions,
    typed_ast: &'a AstArena<Typed>,
    source_ast: &'a AstArena<Untyped>,
    source_index: &'a SourceSemanticIndex,
    typed_bindings: &'a dyn SourceTypedBindings,
    source: SourceMetadata,              // owned SourceId + normalized String name
    source_root: TreeId<Untyped>,         // explicit one-class PackageDef boundary
    class: SymbolId,
    class_tree: TreeId<Untyped>,
    constructor: DefaultConstructorPlan,
    methods: &'a [MethodPicklePlan],       // source declaration order
}
trait SourceTypedBindings {              // pickler-owned port; no Typer import
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>)
        -> Option<TreeId<Typed>>;
}
struct MethodPicklePlan {
    symbol: SymbolId,
    source_tree: TreeId<Untyped>,
    parameter_clauses: Vec<Vec<SymbolId>>, // [] != [[]]; v0 requires []
    body: Option<TreeId<Typed>>,
}
struct DefaultConstructorPlan {
    symbol: SymbolId,
    source_tree: TreeId<Untyped>,          // exact Template.constructor edge
    parameter_clauses: Vec<Vec<SymbolId>>, // source clauses before normalization
}
```

Owning small ordered ID vectors does not clone semantic records. The current
`PicklePlan::validate()` / `validate_with_limits(ValidationLimits)` entry points
return a `ValidatedPicklePlan` borrowing the plan and its underlying session.
`ValidatedPicklePlan::plan()` preserves those exact borrowed stores/arenas and
`package()` exposes the canonical package reached through the class owner chain.
The source root is required so extra top-level forms cannot be silently omitted.
Constructor parameter IDs describe the **source** clauses; the semantic default
constructor still has a normalized empty value clause and class-instance result.

The current accepted boundary is one ordinary public class in one explicit named
package, implicit Object parent, default constructor and one public parameterless
method with a declared Int result and an actual typed Int literal. Other valid
forms remain explicitly unsupported. Validation does not re-type source, mutate
stores/arenas/indices/symbols, or publish files. A separate `EncodingContext` now
projects names/types/references into owned AST fragments. The checked plan's
`declaration_skeleton` API assembles declaration trees into an explicitly
incomplete structural document; completed method bodies/files and a root-facade
export remain deferred.

The crate's only normal dependencies are `dotty-core` and `dotty-tasty`. A test-only
closure implements the binding port with `SourceTyper::source_typed_index().get`;
the production crate neither imports nor depends on Typer, Namer, Parser or JVM.
The source integration test consumes the unchanged #1190 Answer fixture and
asserts exact canonical class/constructor/method identities and typed RHS root,
then emits and structurally validates the provisional skeleton without mutating
the frontend state. Implicit and explicit-empty default constructor source
clauses normalize to identical wire skeletons.

The source filename is supplied by the integration layer: `SymbolOrigin::Source`
contains a SourceId, not a path, so the store cannot authenticate that filename.
Require a normalized relative name without absolute or parent-traversal segments;
use it consistently for provenance and keep it separate from the package-derived
output path. Source-text length is also not in the plan: the producer guarantees
span bounds in that source, while the pickler checks source identity and local
range invariants.

The integration adapter implements the port by delegating to its actual
`SourceTypedIndex::get`. The port remains a trusted producer assertion; the
pickler can check its returned binding and source graph, but cannot prove an
arbitrary caller implemented it honestly. This is the same-session precondition,
not a claim that bare numeric IDs authenticate their origin.

Validation must cover:

1. Live IDs, supported class kind and canonical source provenance. Cross-check
   `source_index.symbol_at(source, class_tree) == class`; follow the actual
   template/constructor/body edges. Check all available spans have this source
   and valid ranges; absent synthetic positions are allowed.
2. Completed class info with matching class/scope identity; package-only, acyclic
   full owner chain and matching `SymbolOrigin::Source(source)`. Reject malformed scopes,
   unsupported self type/parents and explicit initialization.
3. Exact constructor and member membership, kinds and owners; ordered methods
   equal the supported source body declarations; no omitted/duplicate identities
   or unrecognized extra class declarations. Use unordered scopes only to check
   set membership. Missing source semantics is not an empty class.
4. Complete method/constructor signatures and parameter clause checks. Require
   `[]` for `value`, `[[]]` for the default constructor's normalized signature;
   verify constructor effective result references this class, not Unit. The plan
   does not use a fake `Type::Method([], Int)` for a parameterless method.
5. For a source method with RHS, require a body and exact equality with
   `typed_bindings.typed_at(source, source_definition.rhs)`. Require the live
   literal node and constant type to agree, with canonical widened result Int.
   Compare source Int literals and raw `Number(Whole)` nodes with that value;
   bound and decode decimal/binary/hexadecimal spellings, including signed
   32-bit nondecimal patterns. Reject malformed whole numbers and unsupported
   noninteger numbers instead of accepting an arbitrary typed Int value.
   A source declaration without RHS is distinct from an untyped concrete RHS;
   the first complete slice rejects abstract declarations as unsupported.
6. Supported public visibility, source flags and annotation set; checked builtin
   identities, names/namespaces and external reference rules. External builtin
   `Missing` info is allowed only for the validated `Definitions` mapping.

Compared with `JvmCompilationUnit`, this adds source declaration/RHS provenance,
clause boundaries, constructor identity and source metadata. It excludes JVM
local slots, erasure, field layouts, classfile versions, internal names and JVM
initialization plans. It can be implemented without production frontend changes.

## 7. Typed failure contract and resource bounds

`PickleError` should be a structured enum with `SymbolId`, phase-correct `TreeId`,
`TypeId`, source and expected/actual roles where relevant. Keep these categories
separate; messages are presentation, not machine-readable discriminants.

| Category | Examples / required context |
| --- | --- |
| `IncompleteSemanticInfo` | Emitted source symbol `Missing`/`Deferred`/`Error`, absent source/RHS binding, missing completed class/member type; include symbol and info state. |
| `InvalidId` / `InvalidBinding` | Out-of-range symbol/type/tree/scope/name; mismatched source/typed RHS mapping, wrong AST kind or constant/type mismatch. Include relevant ID and role, expected/actual binding/type. |
| `InvalidOwner` / `InvalidDeclaration` | Foreign method or parameter, duplicate member, class/scope mismatch, owner cycle, missing/extra member, wrong constructor; include involved identities and expected/actual owners. |
| `UnsupportedConstruct` | Valid Scala outside the table: object, generic method, abstract method, annotation, source flag, or nonliteral body; include feature and source symbol/tree/span. |
| `UnsupportedTypeReference` | Structural name-designator, unapproved external or ownerless symbol, private/shadowed dependency, unsupported semantic type; include TypeId/target/prefix. A missing source type instead uses incomplete info. |
| `LimitExceeded` / `CyclicInput` | Owner/type/tree cycle, depth/fuel/name/node/byte budget, checked-address overflow; include budget and location. |
| `Encoding` | Preserve the typed `NameTableError`, `WriteError`, `TermEncodeError`, or `TastyFileError` cause as applicable, plus emission context. Never parse error strings. |

Current `ValidationLimits` defaults bound owner depth to 256, inspected items to
1,000,000 and each name/source filename to 4,096 bytes. The validator follows only
the bounded minimal declaration/type/body shape; recursive general tree/type
encoding is not implemented. Limits are checked before traversing supplied
collections, and owner cycles have explicit identity-based detection.

For the future encoder, initial proposed defaults are maximum semantic/tree/owner depth 256, total visited
nodes/edges 1,000,000, name entries 100,000, and complete encoded output 16 MiB.
Expose limits explicitly; check additions/conversions and reserve sizes before
allocation. Wire `u32` address/nat bounds still apply even with a larger caller
budget. Keep emitted tree depth within the codec's default 1024 index/tree limits
or pass a consistent stricter limit. Use iterative traversal or bounded recursion,
active-path cycle checks and total fuel (including repeated DAG edges). Do not
rely on Rust allocation panics, unchecked `get`, or stack overflow for errors.
Any failure leaves source state unchanged and yields no partial artifact.

## 8. Deterministic emission and codec handoff

Emission order is fixed: validate source/class boundary; collect constructor and
source-ordered methods; reserve local declarations; intern names dependency-first;
lower parents, constructor, methods and metadata; bind/fix references; assemble
sections and file; validate final bytes. Within a template, parent precedes the
constructor, which precedes ordinary methods. Emit modifiers in a documented
order consistent with upstream `pickleFlags`, not numeric flag/hash iteration.

Keep separate maps `SymbolId -> definition label/final address` and `TypeId ->
encoded type label/address`. Identity maps are lookup-only; ordering comes from
plan traversal. External refs never acquire fake local definition addresses.
Type sharing uses exact TypeIds and compatible wire roles; direct references and
shared-node references must target visible legal node starts of the right kind.

Reserve forward labels independently of numeric addresses. Resolve every label
exactly once; reject duplicate/unbound targets. Length-prefix/address widths can
change after fixups, so use the existing address-aware/relocated codec, or a
bounded layout pass with codec-produced sizes until stable, then revalidate all
refs. Never patch a compact variable-length reference assuming width cannot
change. Forward-reference tests must cross a nat-width boundary, not just target
a preceding definition.

The fresh-construction seam identified for #1192 is resolved by
`dotty-tasty::construction::AstBuilder`, a flat wire-only labelled node builder.
The existing
`EncodedSection::structured_asts_relocated` relocates **existing** raw/structured
node offsets; its relocation checks preserve the original visible node sequence.
It is not a fresh-label allocator. `AstBuilder` reserves labels independently of
addresses and sizes new nodes with the existing Writer until layout stabilizes,
including changes to reference Nat and enclosing length widths. It rejects
unbound/duplicate/unreachable labels, child cycles, wrong declaration-target
namespaces, forward sharing and resource/size overflow. It contains no semantic
IDs. Tests cover nested length growth and a forward target above address 127.

`dotty-tasty-pickler::EncodingContext` now provides the #1192 bounded standalone
projection. `for_plan` reserves exact class/constructor/method identities from a
validated plan; `new` accepts same-session store/Definitions/source index for
standalone projections, validating each supplied symbol/type. Local reservations
and references key SymbolIds; memoized semantic types key exact TypeIds. Wire
names intern dependency-first in caller traversal order and follow upstream's
term/type spelling sharing while reference tags keep the namespace.

Supported projection forms are canonical local/package references, validated
public package-owned dependency classes, the audited Int/Object builtins, local
or package `this`, and one plain monomorphic method clause. Unit is exposed only
for constructor wire results. Explicit Int/Unit result syntax is separate from
semantic type lowering. The Object constructor has an oracle-specific empty
SIGNED name; no dependency constructor symbol is synthesized. External methods,
private/shadowed refs, non-plain clauses, nested method-valued parameter/result
types, erased/varargs parameters and other semantic types stay unsupported.

Dependency registration checks an exact owner-scope bucket, public metadata,
ClassInfo identity and the canonical package chain. As package symbols do not
store their scope, the supplied dependency scope's provenance remains an adapter
precondition; no name-based symbol discovery occurs. Local scope checks use the
source semantic index and canonical ClassInfo declaration scopes.

`ProjectedFragment` owns names and label-resolved AST bytes only. Bind reserved
definitions through `bind_declaration` using supplied wire nodes of
the correct declaration kind. The #1193 skeleton producer now constructs the
frozen definition trees using this API.
Every label must be bound and emitted once. Caller root/child order must preserve
type projection order so SHAREDtype links point backward. Addresses are relative
to fragment byte zero, so compose one whole intended AST in its context before
finishing. The skeleton producer owns declaration payload grammar, sections and
structural file validation. A mutating-call error poisons the context, and no
failed operation can publish a partial fragment or mutate compiler state.

Projection defaults bound semantic work to 1,000,000 items, 100,000 names,
4,096 bytes/name, and owner/type depth 256. Recursive type lowering has a hard
256-depth ceiling even with larger caller limits. Wire defaults independently
bound labels/fields to 1,000,000, depth to 256, bytes to 16 MiB and layout passes
to 64. Type cycles, incomplete/unsupported types, invalid IDs/owners/namespaces
and nested codec causes remain structured typed errors. The oracle comparison
asserts reference/name meaning; it claims neither byte identity nor independent
Scala acceptance of completed files. The skeleton below remains intentionally
incomplete.

### Declaration-only provisional documents (#1193)

`ValidatedPicklePlan::declaration_skeleton()` (or the variant accepting
`ProjectionLimits`) produces an owned `DeclarationSkeleton`. It reuses the
validated one-class plan and `EncodingContext` reservations. The constructor and
methods keep their exact canonical identities, checked owners and source order;
`declarations()` exposes their final addresses within ASTs, never core numeric
IDs substituted for byte addresses. `intended_tasty_path()` comes from the
Scala-encoded canonical package/class components, independently of source
spelling in wire names and the source filename retained in
`SourceMetadata` and SOURCEFILEattr. No API publishes a file.

Artifact component encoding follows the pinned compiler's
`util/NameTransformer.scala` and `core/Names.scala` SimpleName encoding. Operators
use their `$op` expansions, `$` is retained, and non-Java-identifier UTF-16 units
use uppercase `$uNNNN`. Supplementary scalars therefore encode as two surrogate
escapes. The Java identifier-part table is frozen from the #1190 oracle's JDK 21
(Temurin 21.0.8, Unicode 15.0), without adding a JDK/runtime dependency. This path
encoding does not rewrite TASTy name-table entries or define symbol identity.

The wire shape is PACKAGE, TERMREFpkg, class TYPEDEF/TEMPLATE, the checked
ClassInfo Object parent application, default constructor, then ordinary method.
The parent is APPLY of the signed Object constructor SELECTin with NEW of the
Object type and its shared owner type. Constructor EMPTYCLAUSE, direct Unit
return syntax, absent RHS and STABLE match the oracle; STABLE derives from
validated default-constructor provenance rather than inventing a core flag.
The semantic constructor signature remains empty-clause/class-instance result.
The ordinary method remains parameterless: Int IDENTtpt, no EMPTYCLAUSE and no
extra METHODtype payload. Its source symbol remains concrete; the skeleton adds
neither an abstract flag nor a replacement RHS. Only the already validated
public/empty semantic metadata is emitted; unsupported flags and visibility
return errors before serialization.

`omitted_bodies()` records the exact canonical method/typed-RHS roots deliberately
not emitted. `provisional_bytes()` is explicitly for structural inspection.
TastyFileBuilder supplies the 28.9.0 header, ASTs and Attributes/SOURCEFILEattr,
and validates the assembled document before returning bytes. The diagnostic
header tooling string says `incomplete declaration skeleton`; its UUID is fixed,
not a completed artifact content identity. The permitted Positions, Comments and
compiler-injected SourceFile annotation omissions follow #1190. Concrete bodies
are still missing: this output is non-interoperable and must not be published as
a completed artifact. #1194 adds actual body emission and #1195 independently
compiles Rust-produced output.

The structural oracle test compares normalized package/class/template/parent,
constructor and method shapes against the unchanged Scala Answer TASTy fixture.
It preserves declaration names, nesting, parameter clauses, return syntax,
signed names and modifier tails. The only expression omission is the ordinary
method's real Int RHS; the only class annotation ignored is precisely the
compiler SourceFile annotation. Shared types normalize to their targets and
header/section layout is outside the declaration comparison. Additional tests
assert exact canonical ID/address relationships, failure isolation, normalized
constructor source clauses, Unicode names/output paths, filename provenance,
missing/ambiguous identities, unsupported metadata, limits and deterministic
output. This is a structural declaration guarantee, not byte equality with Scala
or independent compiler acceptance.

Use `NameTableBuilder`, `EncodedSection` and `TastyFileBuilder`; section order is
`ASTs`, then optional `Positions`, optional `Comments`, then `Attributes`. Target
header is magic + 28.9.0 + an honest fixed adapter tooling version + deterministic
UUID. A content-derived UUID can follow the pinned Scala name/AST and remaining
section PJW-hash rule; do not use timestamps, random IDs or absolute paths. The
UUID is a wire identifier, not semantic identity. Any future classfile linkage
must use this file's actual UUID, never an unrelated fixture class's attribute.

Normalize only documented source-path/tooling/UUID noise when comparing to Scala.
Two Rust runs with equal semantic input, source name and configuration must emit
equal bytes, including stable names, labels, sections and UUID, regardless of
HashMap layout or allocation order. Compiler-vs-Rust comparisons are structural
and semantic, not byte-for-byte. `TastyFile::encode` roundtrip of a parsed fixture
proves preservation of that fixture representation, not source semantic pickling.

## 9. Independent validation and follow-up acceptance

| Verifier | What it proves | What it cannot prove |
| --- | --- | --- |
| `TastyFile::parse_and_validate_scala_3_9` | Target header, wire grammar and reference validity within existing codec checks | Scala ownership/type correctness or interoperability |
| Structured AST assertions | Exact package/class/constructor/method clauses, result/ref edges, actual literal value | That an independent compiler accepts those choices |
| `TastyUnpickler::enter_symbols` + `complete_symbol` in a fresh store | Canonicalized declaration owners, metadata, parameter/signature/parent parity for the supported inverse | Typed-body reconstruction or independent acceptance; inverse can share a bug |
| Independently Scala-generated #1190 fixtures | Real 3.9.0 structure, synthetic facts, distinctions and external signatures | Acceptance of new Rust-generated output |
| Pinned Scala `-from-tasty`, fresh output | Independent unpickling and compiler phases generate a class from the candidate | Runtime behavior unless that emitted class is executed |
| Fresh class + Java/Scala runtime harness | `new example.Answer().value() == 42` from the independently compiled output | General Scala pickler completeness |

Semantic parity uses an explicit mapping between entered wire-definition addresses
and source class/method/constructor roles, then compares owner graphs and normalized
types across stores. Numeric SymbolIds/TypeIds are not comparable across sessions.
The unpickler does not reconstruct literal bodies: assert `INTconst` separately.

### Verified `-from-tasty` layout and commands

The pinned compiler's [`Driver.fromTastySetup`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/Driver.scala)
adds the input root to the classpath using
[`TastyFileUtil.getClassPath/getClassName`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/fromtasty/TastyFileUtil.scala).
[`TASTYRun`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/fromtasty/TASTYRun.scala)
accepts `.tasty` or `.jar`, derives class names, and
[`ReadTasty`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/fromtasty/ReadTasty.scala)
resolves them via classpath loaders. `SymbolLoaders.initializeFromClassPath`
chooses `TastyLoader` for a TASTy binary. Thus a package-aligned standalone TASTy
works here despite historical error messages referring to missing class files.
A random renamed path must not be assumed equivalent; #1190 should pin rejection.

Use an explicitly cached classpath. Existing dependency versions are enumerated
in [`module-oracle/run`](../tools/typer-classpath-corpus-audit/module-oracle/run):
Scala compiler/library/scala-library/tasty-core/interfaces **3.9.0**, scala-asm
**9.9.0-scala-1**, compiler-interface **1.12.0**, util-interface **1.11.5**.
`SCALA39_CLASSPATH` below is the colon-separated list of those JARs;
`SCALA39_ROOT` is the checkout at the audited hash and `JAVA_HOME` a recorded JDK.
The audit used cached Maven artifacts and Temurin 21.0.8, with no downloads.

```sh
test "$(git -C "$SCALA39_ROOT" rev-parse HEAD)" = \
  777528f19a58e794c9954a42f433373472ec57f8
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main -version
# Require exactly Scala 3.9.0; #1190 records artifact provenance/hashes as well.
mkdir -p oracle candidate/example compiled
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Yprint-tasty -d oracle Answer.scala
# Audit layout control only: copy solely oracle/example/Answer.tasty into candidate.
# Final #1195 substitutes actual Rust-produced bytes at this path.
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -from-tasty -d compiled \
  candidate/example/Answer.tasty
# Require compiled/example/Answer.class and verify/execute that newly emitted class.
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.decompiler.Main \
  -classpath "$SCALA39_CLASSPATH" candidate/example/Answer.tasty
```

The last command independently decompiles; it is **not** a compilation gate.
The launcher `dotty.tools.MainGenericCompiler -decompile` also dispatches to the
decompiler. Passing `-decompile` directly to `dotty.tools.dotc.Main` did not select
that driver in this audit and failed; use the explicit entry point above.
Do not confuse a successful pretty-print with successful `-from-tasty` compilation.

Keep candidate and output roots separate and initially empty, preserve package
layout, and exclude the original oracle output/source directories from classpath
and runtime. No adjacent `.class` or borrowed classfile TASTY attribute was needed
in the observed minimal case. A JAR case must preserve the same package paths;
it is supported by upstream but was not exercised in this audit. Full classfile
linkage and other backends remain outside this adapter.

### Downstream work and merge gates

1. **[#1190 — oracle/fixtures](https://github.com/scytrowski/dotty-rs/issues/1190),
   independent of this audit:** check in small Scala sources, generated TASTy and
   normalized machine-readable snapshots under `tests/fixtures/tasty_pickler/`
   until the crate exists. Cover Answer, Empty, ordered multi-parameter method
   and abstract declaration, plus parameterless vs `()`. Pin parent `SELECTin`,
   Object constructor signed result, Unit constructor syntax, no constructor RHS,
   STABLE, Int `IDENTtpt`, literal type, names/refs, section/metadata facts. Test
   reduced provenance/position omission; freeze accepted shape with this design.
   Include valid fixture success, malformed candidate rejection, bad layout,
   and requested-tool missing/version-mismatch/nonzero failure. Do not silently
   skip an explicitly requested oracle or fetch tools in ordinary CI.
2. **[#1191 — immutable plan](https://github.com/scytrowski/dotty-rs/issues/1191):**
   starts only after #1189 and #1190 merge. Implement the borrowed boundary and
   source-binding port; test live/wrong IDs, incorrect source/owner/binding,
   duplicate/omitted members, incomplete info, flags, clauses, limits and unchanged
   stores after failure. No encoder or shared typed-unit refactor.
3. **[#1192 — names/types/refs](https://github.com/scytrowski/dotty-rs/issues/1192):**
   consume the checked plan; implement identity-based builtin/reference mapping,
   deterministic name interning and bounded type projection; prove fresh-label
   codec/fixup path and nat-width changes. Test same-spelled symbols under distinct
   owners, namespace differences and non-ASCII names. No class/file serialization.
4. **[#1193 — declarations](https://github.com/scytrowski/dotty-rs/issues/1193):**
   emit frozen package/class/template/default-constructor/method structure with
   canonical metadata and owner bindings. Compare decoded skeleton to #1190;
   deliberately partial missing-RHS output remains explicitly non-interoperable.
5. **[#1194 — real body/file](https://github.com/scytrowski/dotty-rs/issues/1194):**
   bind the actual frontend literal to the correct method, validate its constant
   and declared Int result, assemble owned 28.9.0 bytes and run Rust validation.
   Test non-42 literals as well, wrong type, missing/foreign body, unsupported node,
   metadata omissions, limits and determinism; do not hardcode the canonical RHS.
6. **[#1195 — independent gate](https://github.com/scytrowski/dotty-rs/issues/1195):**
   real source pipeline, Rust wire + structural + semantic parity checks, pinned
   Scala compilation of **Rust output**, fresh emitted-class inspection and runtime
   value 42. Include malformed/unsupported candidates and a missing-tool negative
   control; run two independent source sessions for determinism. Any independent
   compiler failure is a failed gate, even if all Rust roundtrips pass.

Each implementation increment runs focused crate/boundary regressions and all
AGENTS.md checks. This audit changes documentation only; it does not regenerate
baselines, introduce production dependencies, implement the pickler, or add
modules/objects/generics/inline/macros/full source positions to the sprint.

## Implemented complete literal-body producer (#1194)

`ValidatedPicklePlan::pickle()` now returns an owned `PickledTasty` for the frozen
one-package/class/default-constructor/parameterless-Int-method slice.
`pickle_with_limits(PickleLimits)` adds a whole-file size budget (16 MiB by
default) to the existing semantic/name/type/wire budgets. It measures header,
name-table and section framing plus payloads before the full-file encoder runs.
Wire depth failures remain `PickleError::Construction`; whole-file size failures
are `PickleError::LimitExceeded { kind: FileBytes, .. }`. No partial output is
published on failure, and no compiler arena or source index is mutated.

The small expression lowerer borrows the same `SemanticStore` and `TypedAst` and
reads the exact validated method-plan `TreeId<Typed>`. The sole supported kind is
`Literal(Int(n))`; its actual `Tree::ty` must be `Type::Constant(Int(n))`.
The method's declared result remains canonical `Definitions.int`. In the pinned
`TreePickler.pickleConstant` contract this becomes a single `INTconst n` after
the method's Int return `IDENTtpt`. No expression type ascription, recursive
wrapper, fake method tree or extra `METHODtype` is required. Other expression
kinds fail explicitly without traversal or simplification, even cyclic malformed
unsupported nodes. Existing source/typed root identity and span checks apply.

Complete and incomplete output share declaration/owner/reference assembly but
remain distinct public results: `DeclarationSkeleton` still records omitted
bodies, whereas `PickledTasty::body(method_symbol)` records the exact serialized
root. Complete `bytes()` are assembled with `TastyFileBuilder`, validated against
28.9.0 and owned independently of the input session. Source provenance,
Scala-encoded output paths and canonical declaration/owner/address bindings are
retained. No disk publication or production frontend/JVM dependency is added.

The complete header uses `dotty-rs minimal semantic pickler`, with the pinned
`TastyHash.pjwHash64` algorithm and `TastyPickler.assembleParts` UUID layout:
`hash(names payload) XOR hash(ASTs payload)`, then `hash(Attributes payload)`,
each as an uncompressed big-endian 64-bit value. The existing fixed diagnostic
UUID is retained only by the explicitly incomplete skeleton. Positions, Comments
and compiler SourceFile annotation remain omitted per the reduced oracle.

Tests inspect the actual method RHS, compare it to the unchanged Scala oracle,
verify UUID hashing against that fixture, exercise signed Int boundaries and
re-encode/decode the generated file byte-for-byte. Real Lexer/Parser/Namer/Typer
sessions pass existing `SourceTypedIndex` roots directly, including non-42 values
and independent deterministic sessions. **These checks do not establish Scala
compiler interoperability**; compilation of this Rust producer's output remains
#1195. The earlier audit and sequencing sections describe historical decisions,
not a requirement to keep the complete producer unimplemented.
