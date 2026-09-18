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
FunctionWithMods,
ContextBounds, ContextBoundTypeTree, NumberLiteral, Throw,
ParsedTry,
ErrorNode
```

The current shared model already contains the phase-generic forms needed by
both parser and typer, including `Try`, `Template`, type trees, definitions,
imports/exports, quotes/splices, patterns, and `ApplyKind::Using`.

`Template<Untyped>` now stores parser-only `derives` and ordered `uses`
metadata through `UntypedTemplateMetadata`; `Template<Typed>` carries `()`.
Each `UseRef` preserves its `initially` bit, so the parser does not need an
out-of-band tree convention or a lossy lowering step.

## Audit of parser-facing untyped constructs

| Scala/Dotty construct | Current Rust representation | Decision | Audit result |
| --- | --- | --- | --- |
| `ModuleDef` | `UntypedNode::ModuleDef` | REPRESENT | Correct parser-only representation; later lowering creates the synthetic module definition/type definition pair. |
| `Function` | `UntypedNode::Function` | REPRESENT | Preserves lambda parameters and body. |
| `WildcardFunction` | None | LOWER | Placeholder bookkeeping belongs inside the parser. Emit `Function` with generated parameters once the enclosing expression is complete; do not expose Dotty's overlap-testing subclass. |
| `PolyFunction` | `UntypedNode::PolyFunction` | REPRESENT | The parser must preserve the polymorphic function-literal shape before typing. |
| `FunctionWithMods` | `UntypedNode::FunctionWithMods` | REPRESENT | Preserves the result type, whole-function modifiers, and one erased flag per parameter. Scala 3.9 parser creates this for contextual/pure/erased function types. |
| `InLambdaTypeTree` | None | NOT NEEDED | Dotty's node carries a compiler callback for short-lived typing of lambda definitions; it is not source information that should cross a Rust parser boundary. |
| `InfixOp` | `UntypedNode::InfixOp` | REPRESENT | Operator spelling and operand grouping must survive until later precedence/desugaring work. |
| `PrefixOp` | `UntypedNode::PrefixOp` | REPRESENT | Operator spelling must be retained. |
| `PostfixOp` | `UntypedNode::PostfixOp` | REPRESENT | Legacy postfix syntax is source-visible and must not be silently discarded. |
| `Parens` | `UntypedNode::Parens` | REPRESENT | Parentheses can affect parsing, positions, and later diagnostics. |
| `Tuple` | `UntypedNode::Tuple` | REPRESENT | Tuple grouping is parser output, not an early application rewrite. |
| `ForYield` / `ForDo` | `UntypedNode::ForYield` / `ForDo` | REPRESENT | Yield-versus-do is syntactically observable before for-comprehension lowering. |
| `GenFrom` / `GenAlias` | `UntypedNode::GenFrom` / `GenAlias` | REPRESENT | Generator and alias forms remain distinct; `GenFrom.check_mode` preserves the parser's source-version and `case`-pattern policy until for-comprehension lowering. |
| `PatDef` | `UntypedNode::PatDef` | REPRESENT | Pattern definitions retain modifiers, patterns, type ascription, and RHS. |
| `ExtMethods` | `UntypedNode::ExtensionMethods` | REPRESENT | The local name differs, but the parser-facing information is present. |
| `InterpolatedString` | `UntypedNode::InterpolatedString` | REPRESENT | Interpolator name and interpolation parts remain available to later lowering. |
| `ContextBounds` | `UntypedNode::ContextBounds` | REPRESENT | Multiple bounds and their order are preserved. Individual Scala 3.9 aliases are represented by `ContextBoundTypeTree` entries. |
| `ContextBoundTypeTree` | `UntypedNode::ContextBoundTypeTree` | REPRESENT | Preserves the bound tree, its type parameter, and the optional Scala 3.9 `as` term name. `ContextBounds` keeps these entries in source order. |
| `Number` / `NumberKind` | `UntypedNode::Number(NumberLiteral)` | REPRESENT | `NumberLiteral` retains exact source-backed text plus `Whole(radix)`, `Decimal`, or `Floating`. Suffixed literals follow Scala's parser path into semantic `Literal` nodes rather than being mislabeled as `Number`. |
| `Throw` | `UntypedNode::Throw` | REPRESENT | The parser emits a distinct throw expression. |
| `ErrorNode` (local recovery placeholder) | `UntypedNode::Error` | REPRESENT | Required for parser recovery. It remains untyped-only, carries only a small kind enum, and keeps diagnostics outside the AST. |
| `ParsedTry` | `UntypedNode::ParsedTry` | REPRESENT | Preserves a catch handler that is either an expression or case clause before conversion to `Vec<CaseDef>`. |
| `DerivingTemplate` | `Template<Untyped>::metadata.derives` | REPRESENT | Phase-indexed `UntypedTemplateMetadata` retains derives without copying Dotty's subclass or keeping an unattached node. |
| `UseRef` | `Template<Untyped>::metadata.uses` | REPRESENT | `reference` and `initially` are retained in wire/source order. |
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
| `GenCheckMode` | `GenFrom::check_mode` | REPRESENT | The parser computes this mode from source version and generator syntax. All six Scala 3.9 variants are retained for compatibility with later lowering. |
| `Mod` / `Modifiers` | `Modifier` / `Modifiers` | REPRESENT / FEATURE-GATE | Default Scala 3.9 source modifiers include `Var`, `Into`, `Given`, `Implicit`, `Erased`, and `Impure`; `Tracked` and `Update` are retained for the capture-checking dialect, while `Private`/`Protected` remain `VisibilitySyntax`. |

## Findings requiring follow-up

The concrete parser-facing gaps identified by this audit are now represented
in the untyped AST. The remaining items below are policy decisions, not
missing AST payloads.

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
