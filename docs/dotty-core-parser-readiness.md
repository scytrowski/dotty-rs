# `dotty-core` parser-readiness audit

Status: parser-readiness audit and implementation tracker. This document does
not implement `dotty-parser`.

The compatibility target is Scala 3.9.0. The audit compares the current Rust
model with the parser-facing untyped tree families in Scala 3.9.0's
`untpd.scala` and `Parsers.scala`:

- [`untpd.scala`](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/ast/untpd.scala)
- [`Parsers.scala`](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/Parsers.scala)
- [current AST phase model](../crates/dotty-core/src/ast/phase.rs)
- [current shared tree model](../crates/dotty-core/src/ast/common.rs)
- [current untyped-only model](../crates/dotty-core/src/ast/untyped.rs)
- [parser-facing lexer contract](lexer-design-3.9.0.md)

## Decision vocabulary

- **REPRESENT** — the distinction must cross the parser boundary and needs a
  lossless AST representation.
- **LOWER** — the distinction is useful while parsing but has no observable
  meaning after the parser; the parser may emit an existing core node.
- **REJECT / FEATURE-GATE** — the parser must produce a diagnostic or require
  an explicit dialect/feature before accepting the construct.
- **NOT NEEDED** — the Dotty type is an internal helper, a short-lived typer
  node, or an abstract implementation family rather than parser output.

The labels describe the intended parser boundary, not whether the current
Rust implementation already satisfies the decision.

## Current boundary

The current AST uses `TreeKind::PhaseSpecific(P::ExtraNode)` for syntax-only
nodes. `AstPhase::ExtraNode` is `UntypedNode` for `Untyped` and
`Infallible` for `Typed`. This is the right mechanism for new parser-only
nodes: a typed tree cannot contain a real untyped-only node by construction.

The current concrete untyped-only model contains:

```text
ModuleDef, Function, PolyFunction,
InfixOp, PrefixOp, PostfixOp,
Parens, Tuple,
ForYield, ForDo, GenFrom, GenAlias,
PatDef, ExtensionMethods, InterpolatedString,
ContextBounds, NumberLiteral, Throw,
Derived
```

The current shared model already contains the phase-generic forms needed by
both parser and typer, including `Try`, `Template`, type trees, definitions,
imports/exports, quotes/splices, patterns, and `ApplyKind::Using`.

The current `Template` still stores only `parents`, `self_val`, and `body`;
`derives` is currently represented as a separate `UntypedNode::Derived`.
The audit below records the intended correction: derives and uses belong to
untyped template metadata, not to an out-of-band tree convention.

## Audit of parser-facing untyped constructs

| Scala/Dotty construct | Current Rust representation | Decision | Audit result |
| --- | --- | --- | --- |
| `ModuleDef` | `UntypedNode::ModuleDef` | REPRESENT | Correct parser-only representation; later lowering creates the synthetic module definition/type definition pair. |
| `Function` | `UntypedNode::Function` | REPRESENT | Preserves lambda parameters and body. |
| `WildcardFunction` | None | LOWER | Placeholder bookkeeping belongs inside the parser. Emit `Function` with generated parameters once the enclosing expression is complete; do not expose Dotty's overlap-testing subclass. |
| `PolyFunction` | `UntypedNode::PolyFunction` | REPRESENT | The parser must preserve the polymorphic function-literal shape before typing. |
| `FunctionWithMods` | None | REPRESENT | Current `Function` loses function-type modifiers and erased-parameter information. Add the smallest untyped representation before parser implementation. Scala 3.9 parser creates this for `given`/`implicit`/`erased` function types. |
| `InLambdaTypeTree` | None | NOT NEEDED | Dotty's node carries a compiler callback for short-lived typing of lambda definitions; it is not source information that should cross a Rust parser boundary. |
| `InfixOp` | `UntypedNode::InfixOp` | REPRESENT | Operator spelling and operand grouping must survive until later precedence/desugaring work. |
| `PrefixOp` | `UntypedNode::PrefixOp` | REPRESENT | Operator spelling must be retained. |
| `PostfixOp` | `UntypedNode::PostfixOp` | REPRESENT | Legacy postfix syntax is source-visible and must not be silently discarded. |
| `Parens` | `UntypedNode::Parens` | REPRESENT | Parentheses can affect parsing, positions, and later diagnostics. |
| `Tuple` | `UntypedNode::Tuple` | REPRESENT | Tuple grouping is parser output, not an early application rewrite. |
| `ForYield` / `ForDo` | `UntypedNode::ForYield` / `ForDo` | REPRESENT | Yield-versus-do is syntactically observable before for-comprehension lowering. |
| `GenFrom` / `GenAlias` | `UntypedNode::GenFrom` / `GenAlias` | REPRESENT | Generator and alias forms must remain distinct. `GenCheckMode` is a lowering concern and is intentionally not part of the first core model; this must be revisited when for-comprehension desugaring is implemented. |
| `PatDef` | `UntypedNode::PatDef` | REPRESENT | Pattern definitions retain modifiers, patterns, type ascription, and RHS. |
| `ExtMethods` | `UntypedNode::ExtensionMethods` | REPRESENT | The local name differs, but the parser-facing information is present. |
| `InterpolatedString` | `UntypedNode::InterpolatedString` | REPRESENT | Interpolator name and interpolation parts remain available to later lowering. |
| `ContextBounds` | `UntypedNode::ContextBounds` | REPRESENT | Multiple bounds and their order are preserved. The current shape is incomplete for Scala 3.9 context-bound aliases; see the gap list below. |
| `ContextBoundTypeTree` | None | REPRESENT | Scala 3.9 syntax can carry an `as` name on an individual context bound. That name cannot be reconstructed from the current `ContextBounds` fields and requires a small untyped node or equivalent payload. |
| `Number` / `NumberKind` | `UntypedNode::Number(NumberLiteral)` | REPRESENT | Exact spelling is retained through `NameId`. The parser contract must document whether numeric kind comes from the token stream or must be added to `NumberLiteral`; it must not rely on semantic numeric conversion. |
| `Throw` | `UntypedNode::Throw` | REPRESENT | The parser emits a distinct throw expression. |
| `ErrorNode` (local recovery placeholder) | None | REPRESENT | Required for parser recovery. It remains untyped-only, carries only a small kind enum, and keeps diagnostics outside the AST. |
| `ParsedTry` | None; only shared `Try` exists | REPRESENT | Required to preserve a catch handler that is either an expression or case clause before conversion to `Vec<CaseDef>`. |
| `DerivingTemplate` | `Template` plus `UntypedNode::Derived` | REPRESENT | Use phase-indexed `UntypedTemplateMetadata` containing `derives`; do not copy Dotty's subclass or keep an unattached `Derived` node. |
| `UseRef` | None | REPRESENT | Store `reference` and `initially` in untyped template metadata. The `initially` bit is parser information and must not be dropped. |
| `ImportSelector` | Phase-generic `ImportSelector<P>` | REPRESENT | The local representation intentionally uses `TreeId<P>` to avoid cross-arena references. It preserves parser data even though Dotty keeps selectors untyped-only. |
| `SymbolLit` | None | REJECT / FEATURE-GATE | Default Scala 3.9 policy should diagnose legacy symbol literals. If the deprecated compatibility dialect is supported, add a distinct untyped node or define an explicit, tested lowering; do not conflate it with a string literal. |
| `MacroTree` | None | REJECT / FEATURE-GATE | Keep macro syntax outside the first parser contract until its accepted dialect and downstream semantics are defined. Do not silently turn it into an ordinary expression. |
| `CapturesAndResult` | None | REJECT / FEATURE-GATE | Capture checking is not part of the default parser contract. A future capture-checking dialect may add this representation together with capture-set nodes. |
| `XMLBlock` | None | REJECT / FEATURE-GATE | The lexer preserves XML-related lexical structure, but XML parsing is outside the first parser milestone. Accept only under an explicit XML policy once an XML AST/lowering contract exists. |
| `TypedSplice` | Existing `Splice` / `SplicePattern` | LOWER | Quote/splice source syntax is represented by the existing nodes; Dotty's typed-splice helper is not a separate parser contract. |
| `DerivedTypeTree` | None | NOT NEEDED | Internal marker used after symbol/type derivation, not source parser output. |
| `DependentTypeTree` | None | NOT NEEDED | Short-lived typer representation containing semantic callbacks/symbol state. |
| `WildcardTypeBoundsTree` | None | NOT NEEDED | Dotty helper/extractor for wildcard bounds, not a required standalone source node. |
| `OpTree` | `InfixOp` / `PrefixOp` / `PostfixOp` | NOT NEEDED | Abstract implementation base; the concrete operator nodes carry the required source information. |
| `TermTree`, `TypTree`, `PatternTree`, `NameTree`, `Tree` | Shared `TreeKind` families | NOT NEEDED | Abstract Dotty inheritance families, not independent parser payloads. |
| `GenCheckMode` | None | LOWER | Desugaring metadata rather than source syntax. Its eventual owner must be decided before implementing for-comprehension lowering. |
| `Mod` / `Modifiers` | `Modifier` / `Modifiers` | REPRESENT | Source modifiers belong to untyped definitions and function types. The modifier inventory itself needs a Scala 3.9 completeness check before `FunctionWithMods` is added. |

## Findings requiring follow-up

The audit produces four concrete follow-up items before the parser starts:

1. Add a phase-indexed template metadata slot. For `Untyped`, the metadata
   should retain `derives` and `uses`; for `Typed`, it should be `()`.
2. Add an untyped `ParsedTry` representation. The existing shared `Try` is
   the later lowered/semantic form and must remain unchanged.
3. Add an untyped recovery node with no diagnostic text or `TypeId`.
4. Add the smallest representations for `ContextBoundTypeTree` and
   `FunctionWithMods`, or explicitly narrow the parser dialect so those forms
   are rejected. Silent loss is not an acceptable option.

The following are policy decisions, not immediate AST additions:

- placeholders lower to ordinary `Function` nodes inside the parser;
- capture checking, XML, macros, and legacy symbol literals are feature-gated
  until their parser contracts are defined;
- token/source handling remains in `dotty-core::token` and
  `dotty-core::source`, while concrete lexer implementation remains in
  `dotty-lexer`.

## Parser boundary

The intended dependency remains:

```text
SourceText + TokenSource + NameInterner
                    ↓
              dotty-parser
                    ↓
             AstArena<Untyped>
                    ↓
                  Namer
                    ↓
             AstArena<Typed>
```

The parser should consume `dotty-core::token::TokenSource` and observe
`ScannerEvent` values. It should recover lexemes by slicing `SourceText` at a
token span and intern names through `NameInterner`; token values should not
become an owned `String` cache. The concrete `dotty-lexer` remains replaceable
behind that contract.

## Explicit non-goals of this audit

This document does not add AST nodes, parser grammar, recovery algorithms,
feature configuration, XML parsing, macro handling, capture checking, namer,
typer, or desugaring passes. Those are follow-up implementation increments
after the decisions above are reviewed.
