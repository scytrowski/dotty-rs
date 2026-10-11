# Namer error taxonomy

This audit covers the public `dotty_namer::NamerError` variants and the
`SourceSemanticIndexError` cases converted into them by
`name_compilation_unit`. It describes the current boundary; it does not change
the error API or make the namer responsible for parser diagnostics.

## Classification

| `NamerError` | Trigger and provenance | Construction site(s) | Source position | Continuability and future diagnostic decision |
| --- | --- | --- | --- | --- |
| `RootIsNotPackage` | Caller passes a root whose kind is not `PackageDef`; this violates the compilation-unit entry contract. | `Namer::enter_package_header` in [`naming.rs`](../crates/dotty-namer/src/naming.rs). | Root `TreeId`; inspect the span through the caller's live arena. | Compiler/API invariant failure. Do not turn into a source `NamerIssue`. |
| `DuplicateSourceTreeSymbol` | A second canonical symbol is registered for one `(SourceId, TreeId)`. | `SourceSemanticIndex::record_symbol` and `SourceSemanticIndex::record_package_symbol` in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | Source/tree are retained; span is available through the caller's arena. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateDerivedSourceTreeSymbol` | A second derived symbol is registered for one `(owner, source tree)`, such as a constructor-parameter copy or extension prefix. | `SourceSemanticIndex::record_derived_symbol` in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | Source/tree are retained; owner symbol may have been allocated during this naming attempt. | Semantic-index invariant failure. No source diagnostic candidate. |
| `ConflictingSourceProvenance` | One semantic symbol is assigned different canonical/derived source definitions. | `SourceSemanticIndex::ensure_source_provenance`, called by `record_symbol` and `record_derived_symbol`, in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | Both source/tree pairs are retained; spans can be read from the arena. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateExtensionPrefixClauses` | Extension prefix-clause metadata is registered more than once for a method symbol. | `SourceSemanticIndex::record_extension_prefix_clauses` in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | No span in the error; method source definition is available only through a successful index. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateSourceExportSite` | An export tree is registered more than once in the index. | `SourceSemanticIndex::record_export_site`, called by `Namer::record_export_site`, in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs) and [`naming.rs`](../crates/dotty-namer/src/naming.rs). | Source/tree are retained; arena position is available. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateDeclarationScope` | An owner is assigned a second declaration scope. | `SourceSemanticIndex::record_scope` in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | No source span; symbol lookup is safe only if the symbol predates rollback or the call succeeds. | Semantic-index invariant failure. No source diagnostic candidate. |
| `DuplicateDeclarationContext` | A symbol is recorded with two different source contexts; re-recording the same context is idempotent. | `SourceSemanticIndex::record_declaration_context` in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | No span. Both `SourceContextId`s are index-local and the failed index is discarded. | Semantic-index invariant failure. Context IDs in the error are unusable for later lookup. No source diagnostic candidate. |
| `DuplicateFieldInitializerContext` | An initializer context is registered more than once for a field symbol. | `SourceSemanticIndex::record_field_initializer_context` in [`source_semantics.rs`](../crates/dotty-core/src/source_semantics.rs). | No source span; symbol lookup has the rollback limitation described below. | Semantic-index invariant failure. No source diagnostic candidate. |
| `MalformedAstShape` | A naming routine expects a particular shared-AST shape and encounters another kind. | `Namer::enter_package_header`, `enter_class_or_trait_header`, `enter_module_header`, `ensure_enum_companion`, `scan_template_body`, `enter_enum_case_class_header`, `enter_enum_singleton_case`, `enter_enum_case_pattern_definition`, `enter_method_header`, `enter_extension_method_headers`, `scan_extension_prefix_parameters`, `enter_secondary_constructor_header`, `scan_method_parameters`, `scan_constructor_parameters`, and `flatten_package_name_into`, all in [`naming.rs`](../crates/dotty-namer/src/naming.rs). | Offending tree index and expected shape are retained, but not `SourceSpan`; the live arena can supply it. | Compiler/AST invariant or recovered-input limitation. These sites cover package-path flattening, class/object/constructor headers, parameters, enums, extension children, and enum-case patterns. A malformed shape does not by itself prove invalid source. |
| `InvalidVisibilityQualifier` | `private[Q]` or `protected[Q]` does not resolve to an enclosing class/object/package boundary. This can be invalid source, parser-recovery fallout, or a valid-source owner-resolution gap. | `Namer::source_visibility_for_owner` calls `resolve_visibility_boundary` and constructs the error through `invalid_visibility_qualifier` in [`naming.rs`](../crates/dotty-namer/src/naming.rs). | Declaration span is included as `Option<SourceSpan>`; qualifier and protection mode are retained. | The diagnostic-aware entry point classifies this only when the AST contains no parser `ErrorNode`, the source span belongs to the requested source, and every semantic owner has source provenance. This accepts the verified non-enclosing `private[Sibling]` case while keeping recovered trees and the known synthetic enum companion owner-resolution gap as `NamerError`. The legacy entry point always retains its original `NamerError` result. |
| `UnsupportedGivenNameShape` | An anonymous given's declared type cannot be reduced by the current stable-name extractor; this includes valid Scala shapes and recovered/manually built trees. | `Namer::normalize_given_name` reports failures through `Namer::unsupported_given_name_shape` in [`naming.rs`](../crates/dotty-namer/src/naming.rs). | Given span and unsupported child tree index/kind are retained. | Not a source diagnostic by default. Scala 3.9 accepts `given Service.Token.type = Service.Token`, whose `SingletonTypeTree` is currently unsupported. Keep as an explicit compiler limitation until naming support is implemented. |
| `AnonymousGivenWithoutParents` | An anonymous structural given module has no parent type from which to derive its synthetic name; this can come from recovery or a manually invalid AST. | `Namer::normalize_given_name` in [`naming.rs`](../crates/dotty-namer/src/naming.rs). | Given span is included as `Option<SourceSpan>`. | No clean, valid Scala 3.9 example is established. `given with ...` is rejected by the Scala parser, so this is not evidence for a source diagnostic. Keep typed and unresolved pending a clean-parser witness. |

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
| `enum Color { case Red(private[Color] val value: Int) }` | Accepted by Scala 3.9.0; the dotty-rs parser reports no diagnostics. | Namer returns `InvalidVisibilityQualifier` for the case parameter. The enum case is owned under a synthetic companion module class, and `resolve_visibility_boundary` does not recover the source enum's `Color` boundary. This is a clean-AST false positive and a namer owner-resolution gap, not a source diagnostic. |

The focused integration tests in `crates/dotty-namer/tests/parsed_unit.rs`
pin the recovered unresolved qualifier, clean valid enum-case false positive,
and valid singleton given shape; both naming failures assert rollback. Existing
tests also cover clean invalid visibility, valid enclosing visibility,
hand-built malformed AST shapes, and missing-parent anonymous givens.

## Decision summary

* No current `NamerError` is safe to convert unconditionally into a source
  diagnostic. The opt-in API classifies `InvalidVisibilityQualifier` only
  when the parsed arena has no recovery `ErrorNode` and all semantic owners
  have source provenance. This deliberately leaves unresolved synthetic-owner cases
  such as enum-case parameters internal until their valid source boundary is
  represented correctly.
* Explicitly unsupported valid source: `UnsupportedGivenNameShape` for at
  least singleton-type anonymous givens.
* Not established as source diagnostics: `AnonymousGivenWithoutParents` and
  every `MalformedAstShape` case. They need a clean-parser Scala 3.9 witness
  before classification changes.
* Compiler/index invariants: the root-contract error and all eight
  `SourceSemanticIndexError` conversions.
