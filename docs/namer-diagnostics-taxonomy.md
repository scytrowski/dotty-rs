# Namer error taxonomy

This audit covers the public `dotty_namer::NamerError` variants and the
`SourceSemanticIndexError` cases converted into them by
`name_compilation_unit`. It describes the current boundary; it does not change
the error API or make the namer responsible for parser diagnostics.

## Classification

| `NamerError` | Trigger and provenance | Source position | Continuability and future diagnostic decision |
| --- | --- | --- | --- |
| `RootIsNotPackage` | The caller passes a root whose kind is not `PackageDef`. This violates the compilation-unit entry contract; parser-produced compilation units use a package root. | Only the root `TreeId` is retained. The caller can inspect its position while the arena is alive. | Compiler/API invariant failure. Do not turn into a source `NamerIssue`. |
| `DuplicateSourceTreeSymbol` | Registering a second canonical symbol for one `(SourceId, TreeId)` in `SourceSemanticIndex`. Can be triggered by a duplicate traversal or conflicting naming logic. | Source and tree identity are retained; the span is available through the caller's arena. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateDerivedSourceTreeSymbol` | Registering a second derived symbol for one `(owner, source tree)`; used for copied/owner-specific identities such as constructor parameters and extension prefixes. | Source/tree are retained; owner symbol may have been allocated during this naming attempt. | Semantic-index invariant failure. No source diagnostic candidate. |
| `ConflictingSourceProvenance` | One semantic symbol is assigned different canonical/derived source definitions. | Both source/tree pairs are retained; spans can be read from the arena. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateExtensionPrefixClauses` | Extension prefix-clause metadata is registered more than once for a method symbol. | No span in the error; the method's source definition can be looked up only while a successful index is available. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateSourceExportSite` | An export tree is registered more than once in the index. | Source/tree are retained; arena position is available. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateDeclarationScope` | An owner is assigned a second declaration scope. | No source span; symbol lookup is available only if the symbol predates rollback or the call succeeds. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateDeclarationContext` | A symbol is recorded with two different source contexts. Re-recording the same context is idempotent. | No span. Both `SourceContextId`s are index-local and the failed index is discarded. | Semantic-index invariant failure. Context IDs in the error are not usable for later lookup. No source diagnostic candidate. |
| `DuplicateFieldInitializerContext` | An initializer context is registered more than once for a field symbol. | No source span; symbol lookup has the rollback limitation described below. | Semantic-index invariant failure. No source diagnostic candidate. |
| `MalformedAstShape` | A naming routine expects a particular shared-AST shape and encounters another kind. There are checks in package-path flattening, class/object and constructor headers, class/method/extension parameters, enum/companion traversal, extension children, and enum-case patterns. This can mean a manually malformed AST, an internal producer mismatch, or an AST retained by parser recovery; it does not by itself prove invalid source. | The error retains the offending tree index and static expected shape, but not `SourceSpan`. The arena can still supply the span. | Compiler/AST invariant or recovered-input limitation. Keep as an error until a specific clean-source case is demonstrated; never report it as a source error solely from the variant. |
| `InvalidVisibilityQualifier` | `private[Q]` or `protected[Q]` does not resolve to an enclosing class/object/package boundary. The same result can arise from a clean tree for invalid source or a parser-recovered tree whose semantic nesting is incomplete. | The declaration span is included as `Option<SourceSpan>`; qualifier name and protection mode are retained. | The only demonstrated source-diagnostic candidate. Scala 3.9 rejects `private[Elsewhere]` in `class Outer` with E139 (“no enclosing class or object is named”). A future `NamerIssue` can use this case for clean parser output. When parsing had recovery diagnostics, the namer cannot distinguish a source mistake from recovery fallout because parser diagnostics are not part of its input; preserve the typed error until a caller can make that distinction. |
| `UnsupportedGivenNameShape` | An anonymous given's declared type cannot be reduced by the current stable-name extractor. This includes valid Scala type shapes not handled by the namer, as well as shapes retained from recovered or manually built ASTs. | Given span and unsupported child tree index/kind are retained. | Not a source diagnostic by default. Scala 3.9 accepts `given Service.Token.type = Service.Token`, while the current namer returns this error for its `SingletonTypeTree`. Keep it as an explicit unsupported compiler case until naming support is implemented. |
| `AnonymousGivenWithoutParents` | An anonymous structural given module has no parent type from which to derive its synthetic name. The parser may create this shape while recovering malformed input; a hand-built AST can also violate the namer's naming precondition. | Given span is included as `Option<SourceSpan>`. | No clean, valid Scala 3.9 example has been established. `given with ...` is rejected by Scala 3.9's parser, so this is not evidence for a source diagnostic. Keep it typed and unresolved as a diagnostic candidate pending a clean-parser example. |

The eight duplicate/index cases above are the complete set currently exposed
by `SourceSemanticIndexError` and translated by `From<SourceSemanticIndexError>
for NamerError`. Their construction is centralized in index registration
methods in `dotty-core/src/source_semantics.rs`; they are not parser syntax
diagnostics.

The existing compatibility audit also records these parser-recovered corpus
failures. They are evidence that the errors occur on recovered input, not
evidence that the original Scala source is invalid:

| Error | Count in the existing audit | Files | Taxonomy here |
| --- | ---: | --- | --- |
| `InvalidVisibilityQualifier` | 4 | `CaptureSet.scala`, `TypeComparer.scala`, `ProtoTypes.scala`, `Promise.scala` | Recovery-sensitive unresolved owner; do not emit a source diagnostic without clean-parse provenance. |
| `MalformedAstShape` | 3 | `tpd.scala`, `QuoteMatcher.scala`, `NamedTuple.scala` | Recovered extension children outside supported `DefDef`/`Export` forms; treat as recovered-input limitation or producer invariant, not a syntax diagnostic. |

These counts are the checked-in results in `docs/namer-v0.1-compatibility.md`;
the corpus runner was not rerun as part of this focused audit.

## Positions, IDs, and rollback

`name_compilation_unit` checkpoints `SemanticStore` and `Packages`. On failure
it removes scope insertions made into pre-existing scopes, rolls package
registrations back, and truncates symbols, types, scopes, and annotations
allocated since the checkpoint. Existing store tests and namer rollback tests
cover this behavior. The input `AstArena`, `SourceId`, and caller-owned source
text are not rolled back, so a `TreeId`/`SourceSpan` can still locate source
text after an error if the caller retains those inputs. `NameInterner` is also
not rolled back; its IDs remain valid.

Treat semantic IDs carried by an error as diagnostic context only after
`name_compilation_unit` returns `Err`. A `SymbolId` may refer to an allocation
that was truncated and can later be reused. `SourceContextId` values are local
to the temporary index, which is dropped on failure. The error's tree and
source identities are the stable route back to the retained input; where an
error carries only a symbol, the caller must not dereference it unless it
knows that symbol predates the checkpoint.

Failure is currently all-or-nothing at the compilation-unit boundary. It is
safe for the caller to report the error and abandon this naming attempt, but
there is no supported continuation using a partial `SourceSemanticIndex`.
Continuing after a future diagnostic will require a deliberate recovery
contract that preserves index invariants; it must not be inferred from the
fact that some errors carry a span.

## Scala 3.9.0 and parser-recovery evidence

The source reference is Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The cases below were checked with
the 3.9.0 compiler artifact and the pinned source checkout:

| Case | Scala 3.9.0 result | Namer consequence |
| --- | --- | --- |
| `class Outer: private[Elsewhere] def value: Int = 1` followed by `class Elsewhere` | Rejects the qualifier with E139; a same-file declaration is not an enclosing boundary. | On a clean AST, `InvalidVisibilityQualifier` represents a source-level error and is a future diagnostic candidate. |
| `object Service: object Token; given Service.Token.type = Service.Token` | Accepted. | Parser produces a `SingletonTypeTree`; namer currently returns `UnsupportedGivenNameShape`. This is unsupported valid input, not invalid source. |
| `given with ...` | Rejected during parsing (`identifier expected`). | A recovered AST with a structural given lacking parents is not sufficient evidence for a source diagnostic; `AnonymousGivenWithoutParents` remains unresolved. |
| `class Outer { private[Missing] val hidden = }` | The source has a parser error in the initializer. | Parser recovery retains the declaration, and namer returns `InvalidVisibilityQualifier`; this demonstrates why the error alone cannot distinguish source diagnostics from recovery effects. |

The focused integration tests in `crates/dotty-namer/tests/parsed_unit.rs`
pin the last three namer behaviors (recovered invalid qualifier, valid
singleton given shape, and rollback after both errors). Existing tests also
cover clean invalid visibility, valid enclosing visibility, hand-built
malformed AST shapes, missing-parent anonymous givens, and rollback.

## Decision summary

* Safe future source-diagnostic candidate: `InvalidVisibilityQualifier`,
  conditional on knowing the relevant syntax was not affected by parser
  recovery.
* Explicitly unsupported valid source: `UnsupportedGivenNameShape` for at
  least singleton-type anonymous givens.
* Not established as source diagnostics: `AnonymousGivenWithoutParents` and
  every `MalformedAstShape` case. They need a clean-parser Scala 3.9 witness
  before classification changes.
* Compiler/index invariants: the root-contract error and all eight
  `SourceSemanticIndexError` conversions.
