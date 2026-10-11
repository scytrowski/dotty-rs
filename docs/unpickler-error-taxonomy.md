# TASTy unpickler error taxonomy

This is an audit of the public `dotty_tasty_unpickler::UnpickleError` in
`crates/dotty-tasty-unpickler/src/error.rs` at the time of issue #977. It records
the existing model and proposes the classification/code surface for a later
implementation issue. It does not change unpickling behavior, the enum, its
`Display` text, or any public signature.

## Classification rules

| Category | Meaning in this audit |
| --- | --- |
| `MalformedInput` | The artifact has an invalid structural shape, address, name reference, or semantic reference under the input-local contract. |
| `UnsupportedFeature` | The shape may be legal TASTy, but this unpickler does not project or interpret it. |
| `ResolutionFailure` | An external/session symbol or dependency is missing, ambiguous, or the resolver itself failed. Context may be needed to distinguish absence from a malformed local reference. |
| `Deferred` | The operation is legal but requires completion or a later pass/milestone. |
| `InternalInvariant` | A pass-order, store/index, ownership, or registration contract expected of this implementation was violated. |

These are primary categories, not claims that every enclosing file is corrupt.
In particular, nested decode errors retain their own distinctions; a
`TastyFileError` or `AstError` can report an unsupported valid version/tag as
well as malformed data. The `ResolutionFailure` entries marked conditional
must keep that context when categories are implemented.

## Variant inventory

Every public variant is listed once. Producer references point to the
implementation module/function; several variants have multiple construction
sites in the named function. The suggested code is stable, lowercase, and does
not depend on `Debug`, `Display`, IDs, or addresses. Keeping the Rust variant
name out of the eventual implementation is possible, but this explicit mapping
is the proposed contract.

| Variant | Primary category and evidence | Main producer(s) | Suggested code |
| --- | --- | --- | --- |
| `Tasty` | `MalformedInput` or unsupported format/version, conditional on nested cause | `From<TastyFileError>`; file queries throughout unpickler | `unpickler.tasty` |
| `Ast` | `MalformedInput` or unsupported AST feature/tag, conditional on nested cause | `From<AstError>`; structured AST access throughout unpickler | `unpickler.ast` |
| `DuplicateDefinition` | `InternalInvariant`; an AST address must have one entered identity | `index::insert_symbol` | `unpickler.duplicate_definition` |
| `DuplicateScope` | `InternalInvariant`; scope registration is unique per symbol | `index::insert_scope`, `unpickler::share_owner_scope` | `unpickler.duplicate_scope` |
| `ConflictingCompanion` | `InternalInvariant`; reciprocal companion links conflict with published state | `companions::link_companions` | `unpickler.conflicting_companion` |
| `InvalidNameReference` | `MalformedInput`; absent, invalid, cyclic, or over-depth name-table reference | `names::{collect_segments,entry_at,split,derive,resolve}` | `unpickler.invalid_name_reference` |
| `UnsupportedName` | `UnsupportedFeature`; name-table tag/kind is not interpreted | `names::{derive,resolve}` | `unpickler.unsupported_name` |
| `UnsupportedQualifier` | `UnsupportedFeature`; access qualifier tree form is not read | `mapping::qualifier_ref`, `enter::resolve_qualifier` | `unpickler.unsupported_qualifier` |
| `InvalidReferenceTarget` | `MalformedInput`; target is not a visible AST node start | `ast_view`, `enter`, `class`, `binders`, `types`, and tree/type walkers | `unpickler.invalid_reference_target` |
| `InvalidQualifier` | `MalformedInput`; qualifier does not enclose the definition or has an illegal owner relation | `enter::resolve_qualifier` | `unpickler.invalid_qualifier` |
| `UnsupportedPackagePath` | `UnsupportedFeature`; package path is not a supported direct/shared package reference | `enter::package_path_name` | `unpickler.unsupported_package_path` |
| `MissingDefinition` | `MalformedInput` when the AST walk expects a definition that is absent; pass/index mismatch would instead be an invariant | `ast_view::node`, `enter`, `completion`, `opaque` | `unpickler.missing_definition` |
| `DuplicateType` | `InternalInvariant`; type/address caches are single-assignment | `index::{insert_type,insert_type_tree,insert_term_tree}` | `unpickler.duplicate_type` |
| `UnsupportedTypeTree` | `UnsupportedFeature`; tree tag is not projected | `type_tree::type_of_tpt` | `unpickler.unsupported_type_tree` |
| `UnsupportedTermTree` | `UnsupportedFeature`; term tree has no implemented `tpe` projection | `term_type::type_of_term` | `unpickler.unsupported_term_tree` |
| `MissingClassScope` | `InternalInvariant`; pass 1 promises a scope for every entered template | `class::complete_class` | `unpickler.missing_class_scope` |
| `UnsupportedParentTree` | `UnsupportedFeature`; parent wrapper tag is not interpreted | `class::type_of_parent_in` | `unpickler.unsupported_parent_tree` |
| `MalformedParentTree` | `MalformedInput` for invalid wrapper/call shape; the nesting-limit reason is `UnsupportedFeature` unless the nested structure is independently shown to be cyclic/invalid | `class::type_of_parent_in` | `unpickler.malformed_parent_tree` |
| `InvalidSelfTypeTree` | `MalformedInput`; `SELFDEF` does not have its required shape | `class::type_of_self_def` | `unpickler.invalid_self_type_tree` |
| `MalformedDefinition` | `MalformedInput` for shape mismatch; nested opaque-parameter lambdas and alias-graph recursion-limit cases are `UnsupportedFeature` unless the graph is independently shown cyclic | `class::complete_class`, `method::defdef_header`, `opaque::{complete_opaque_alias,ensure_alias_graph_acyclic,unwrap_lambda_tpt}` | `unpickler.malformed_definition` |
| `UnsupportedMethodParameterSemantics` | `UnsupportedFeature`; a valid parameter modifier cannot be represented in adapted method info | `method::guard_parameter_modifiers` | `unpickler.unsupported_method_parameter_semantics` |
| `ConstructorOwnerNotClassLike` | `MalformedInput` for a constructor under a non-class-like owner; impossible owner-state can indicate an index invariant | `constructor::constructor_owner` | `unpickler.constructor_owner_not_class_like` |
| `MalformedOwnerClassInfo` | `InternalInvariant`; a completed class owner does not hold its canonical `ClassInfo` | `constructor::constructor_owner`, `opaque::apply_pending_opaque_aliases` | `unpickler.malformed_owner_class_info` |
| `SharedLambdaOwnerConflict` | `UnsupportedFeature`; shared lambda has more than one semantic owner | `type_tree::type_of_lambda_tpt` | `unpickler.shared_lambda_owner_conflict` |
| `SharedRefinementOwnerConflict` | `UnsupportedFeature`; shared refinement has more than one fixed synthetic-class owner | `refinement::type_of_refined_tpt` | `unpickler.shared_refinement_owner_conflict` |
| `UnsupportedRefinementStat` | `UnsupportedFeature`; refinement member tag is not projected | `enter::enter_refined_tpt` | `unpickler.unsupported_refinement_stat` |
| `MissingRefinementClass` | `InternalInvariant`; discovery/entry pass did not produce its synthetic class | `refinement::refinement_class` | `unpickler.missing_refinement_class` |
| `InvalidRefinementClass` | `InternalInvariant`; entry at the refinement address is not its synthetic class | `refinement::refinement_class` | `unpickler.invalid_refinement_class` |
| `MissingRefinementScope` | `InternalInvariant`; synthetic class was entered without its required scope | `refinement::refinement_class` | `unpickler.missing_refinement_scope` |
| `MalformedRefinedTypeTree` | `MalformedInput`; indexed children or refined-member shape disagree with the structured tree | `refinement::type_of_refined_tpt` | `unpickler.malformed_refined_type_tree` |
| `UnsupportedRefinementOverload` | `UnsupportedFeature`; the model deliberately refuses same-name refinement overloads | `refinement::type_of_refined_tpt` | `unpickler.unsupported_refinement_overload` |
| `CloseOverThis` | `InternalInvariant` for a failed core rebinding of a constructed refinement; nested `TypeRebindError` preserves the concrete cause | `refinement::type_of_refined_tpt` | `unpickler.close_over_this` |
| `ParameterAbstraction` | `InternalInvariant` for a failed core abstraction of selected binder parameters; nested cause retained | `method::build_clause`, `opaque::opaque_type_lambda`, `type_tree::type_of_lambda_tpt` | `unpickler.parameter_abstraction` |
| `UnstableSelectQualifier` | `UnsupportedFeature`; the model has no widening operation for an otherwise understood selection | `types::select_member_type` | `unpickler.unstable_select_qualifier` |
| `InvalidSingletonTypeTree` | `MalformedInput` if the tree claims a non-singleton type; unsupported legal singleton forms need a distinct future variant if evidence arises | `type_tree::type_of_tpt` | `unpickler.invalid_singleton_type_tree` |
| `MissingEnteredSymbol` | `InternalInvariant` for an entered local definition omitted from the index; cross-unit or local unentered targets use `MissingReferencedSymbol` instead | `completion`, `method`, `refinement`, `symbol_annotations`, `type_tree`, `opaque` | `unpickler.missing_entered_symbol` |
| `UnsupportedSymbolCompletion` | `Deferred`; symbol kind belongs to a later completion milestone | `completion::complete_in` | `unpickler.unsupported_symbol_completion` |
| `UnsupportedAnnotationCompletion` | `UnsupportedFeature`; this API does not support annotation completion for kinds without an indexed annotation tail | `symbol_annotations::complete_annotations_in` | `unpickler.unsupported_annotation_completion` |
| `OpaqueAliasOwnerNotClassLike` | `MalformedInput` when the entered owner has an impossible kind; confirm against source fixture before treating foreign/unresolved owners as malformed | `opaque::complete_opaque_alias` | `unpickler.opaque_alias_owner_not_class_like` |
| `OpaqueAliasOwnerNotEntered` | `ResolutionFailure` if owner belongs to another unit; `InternalInvariant` if a same-unit owner should already have been entered | `opaque::complete_opaque_alias` | `unpickler.opaque_alias_owner_not_entered` |
| `OpaqueAliasCycle` | `MalformedInput`; alias graph/owner refinement would be cyclic | `opaque::{ensure_alias_graph_acyclic,insert_owner_alias}` | `unpickler.opaque_alias_cycle` |
| `SymbolCompletionDeferred` | `Deferred`; symbol already holds a deferred state that this pass does not force | `completion::complete_in`, `method::parameter_symbol` | `unpickler.symbol_completion_deferred` |
| `SymbolInfoError` | `InternalInvariant`; attempted completion observed an error info in the semantic store | `completion::complete_in` | `unpickler.symbol_info_error` |
| `InvalidCompletedBounds` | `MalformedInput` if the projected declaration is not bounds; an unsupported but valid bound encoding needs separate evidence | `completion::bounds_of` | `unpickler.invalid_completed_bounds` |
| `UnsupportedType` | `UnsupportedFeature` for a valid later/unimplemented type tag; a tag that is not a type is malformed and should be distinguished by the nested AST evidence | `types::decode_type`, `term_type::type_of_term`, `annotated::annotation_class_type` | `unpickler.unsupported_type` |
| `MissingReferencedSymbol` | `ResolutionFailure` for local/cross-unit definitions absent from this unit's index; absence alone is not proof of corrupt bytes | `types::referenced_symbol` | `unpickler.missing_referenced_symbol` |
| `InvalidReferenceKind` | `MalformedInput`; reference namespace/kind and target symbol kind disagree | `types::{referenced_symbol,referenced_class,this_class}` | `unpickler.invalid_reference_kind` |
| `UnresolvedPackage` | `ResolutionFailure`; package registry and resolver both lack the named package | `types::referenced_package`, `opaque` | `unpickler.unresolved_package` |
| `UnresolvedMember` | `ResolutionFailure`; `Ok(None)` is not proof the member does not exist outside the current resolver/session | `types::resolved_named`, `opaque` | `unpickler.unresolved_member` |
| `AmbiguousMember` | `ResolutionFailure`; lookup has multiple valid candidates and no selector | `types::resolved_named` | `unpickler.ambiguous_member` |
| `UnsupportedSignedReference` | `UnsupportedFeature`; member selection by signature is not implemented | `types::resolved_named`, `refined::decode_refined_type` | `unpickler.unsupported_signed_reference` |
| `MalformedType` | `MalformedInput` for invalid child/type/reference shape; recursion-limit and owner-space designator cases are ambiguous or unsupported (see below), not proof of corrupt bytes | `discovery`, `enter`, `types`, `binders`, `recursive`, `refined`, `refinement`, `term_type`, `annotated`, `symbol_annotations`, `reachability` | `unpickler.malformed_type` |
| `UnsupportedAnnotationTree` | `UnsupportedFeature`; annotation tree is retained in TASTy but not projected | `type_tree::type_of_tpt`, `annotated::decode_annotation_payload` | `unpickler.unsupported_annotation_tree` |
| `InvalidCompactAnnotationType` | `MalformedInput` if the compact form violates its type contract; a binder/pending reference may be `Deferred` pending decoding | `types`, `annotated::decode_annotation_payload` | `unpickler.invalid_compact_annotation_type` |
| `UnsupportedAnnotationConstructor` | `UnsupportedFeature`; constructor spine/class form is outside the annotation projection | `annotated::{decode_constructor_annotation,constructor_spine,annotation_class_type}` | `unpickler.unsupported_annotation_constructor` |
| `UnsupportedAnnotationArgument` | `UnsupportedFeature`; annotation value is not a literal/class literal supported by the model | `annotated::annotation_argument` | `unpickler.unsupported_annotation_argument` |
| `InvalidAnnotationType` | `MalformedInput` if the annotation's type is not an accepted annotation class/applied type; confirm valid source forms before finalizing | `annotated::decode_constructor_annotation` | `unpickler.invalid_annotation_type` |
| `BoundsVarianceTargetPending` | `Deferred`; variance rebinding targets a lambda still under decode | `types`, `binders::apply_declared_variances` | `unpickler.bounds_variance_target_pending` |
| `BoundsVarianceArityMismatch` | `MalformedInput`; variance count differs from binder arity | `types`, `binders::apply_declared_variances` | `unpickler.bounds_variance_arity_mismatch` |
| `RebindFailed` | `MalformedInput` when nested rebinding reports malformed graph/IDs; some causes may indicate `InternalInvariant`; retain exact nested cause | `binders::apply_declared_variances` | `unpickler.rebind_failed` |
| `InvalidMethodModifier` | `MalformedInput`; method-type modifier tag is invalid in this position | `types`, `binders::method_kind` | `unpickler.invalid_method_modifier` |
| `InvalidBinderReference` | `MalformedInput`; binder address is not a visible AST node | `binders::decode_param_type` | `unpickler.invalid_binder_reference` |
| `InvalidBinderKind` | `MalformedInput`; `PARAMtype` target is not a binder | `binders::{parameter_arity,decode_param_type}` | `unpickler.invalid_binder_kind` |
| `InvalidParameterIndex` | `MalformedInput`; parameter index exceeds the referenced binder's arity | `binders::decode_param_type` | `unpickler.invalid_parameter_index` |
| `InvalidTypeParameterBounds` | `MalformedInput`; parameter info is not bounds where bounds are required | `binders::check_parameter_bounds`, `types` | `unpickler.invalid_type_parameter_bounds` |
| `UnsupportedResolutionPrefix` | `UnsupportedFeature` when prefix semantics are not modeled; with a resolver returning no answer the external member remains unresolved, not corrupt | `types::resolved_named`, `types::this_class` | `unpickler.unsupported_resolution_prefix` |
| `UnsupportedResolutionSpace` | `UnsupportedFeature` when explicit owner/declaration space has no implemented lookup semantics | `types::resolved_named` | `unpickler.unsupported_resolution_space` |
| `IllegalTypePrefix` | `UnsupportedFeature`; valid-looking prefix requires `QualSkolemType`, absent from the shared model | `types::resolved_named` | `unpickler.illegal_type_prefix` |
| `ResolverFailure` | `ResolutionFailure`; preserves resolver error (ambiguous/malformed resolver state) and request address | `types::{resolved_named,referenced_package}` | `unpickler.resolver_failure` |

## Nested error sources

| Nested source | Variants/causes | Classification notes and location |
| --- | --- | --- |
| `TastyFileError` | `Tasty(TastyFileError)` via `From<TastyFileError>` | See `crates/dotty-tasty/src/file.rs`. Header version/magic, invalid references, missing/duplicate sections and section payload errors have different meanings. Preserve the nested variant; some version failures are unsupported format, not malformed content. File errors use file/section-relative offsets or name references, not Scala source spans. |
| `AstError` | `Ast(AstError)` via `From<AstError>` | See `crates/dotty-tasty/src/ast.rs`. `InvalidTag`, bad references/counts and read errors are malformed; `UnsupportedCategory`, `UnsupportedRelocation`, and recursion/resource limits may instead mean valid input beyond current support. Offsets are AST payload byte offsets; `address` is an AST coordinate. |
| `ResolutionError` | `ResolverFailure { address, error }` | See `crates/dotty-core/src/resolution.rs`. `Ambiguous` is resolution ambiguity; `Malformed { reason }` is inconsistent resolver state (`InternalInvariant` for a trusted resolver, otherwise a resolver failure with untrusted cause). `Ok(None)` remains `UnresolvedMember`/`UnresolvedPackage`, not this variant. |
| `TypeRebindError` | `CloseOverThis`, `ParameterAbstraction`, `RebindFailed` | See `crates/dotty-core/src/types/rebind.rs`. Carries exact type/symbol IDs, arity, or graph condition. Invalid/unfilled/cyclic/deep graphs are malformed input only if all referenced IDs came from the artifact; otherwise invariant. A caught error is returned after enclosing operation rollback. |

`UnpickleError::source()` currently exposes only `Tasty`, `Ast`, and
`RebindFailed`; the first two expose their wrapped source, while close/abstraction
errors include `TypeRebindError` in their `Display` only. `ResolutionError`
does not currently implement `source()` through `UnpickleError`. Preserve this
existing behavior in #977; any error-chain API change belongs to a later issue.

## Static `reason` inventory and proposed typed families

There are five reason-bearing variants. The strings are finite at present; do
not replace them with `Other(String)`. The list below is the complete set of
literal producer values found under `crates/dotty-tasty-unpickler/src/`.

| Existing variant | Existing `reason` text | Producer | Suggested bounded reason |
| --- | --- | --- | --- |
| `MalformedParentTree` | `a parent constructor call nests too deeply` | `class::type_of_parent_in` | `ParentReason::NestingLimit` |
| `MalformedParentTree` | `a parent constructor call has a function` | `class::type_of_parent_in` | `ParentReason::ConstructorHasFunction` |
| `MalformedParentTree` | `a parent constructor selection selects <init>` | `class::type_of_parent_in` | `ParentReason::SelectsConstructor` |
| `MalformedParentTree` | `a parent constructor selection is on NEW` | `class::type_of_parent_in` | `ParentReason::SelectsNew` |
| `MalformedParentTree` | `NEW has one type tree` | `class::type_of_parent_in` | `ParentReason::NewArity` |
| `InvalidSelfTypeTree` | `a self definition has one type tree` | `class::type_of_self_def` | `SelfTypeReason::TypeTreeArity` |
| `MalformedDefinition` | `a class definition's first child is its template` | `class::complete_class` | `DefinitionReason::ClassTemplatePosition` |
| `MalformedDefinition` | `a method has a result type after its parameters` | `method::defdef_header` | `DefinitionReason::ResultTypePosition` |
| `MalformedDefinition` | `an opaque type alias has one right-hand-side tree` | `opaque::complete_opaque_alias` | `DefinitionReason::OpaqueAliasRhsArity` |
| `MalformedDefinition` | `opaque bounds children do not match their encoded shape` | `opaque::complete_opaque_alias` | `DefinitionReason::OpaqueBoundsShape` |
| `MalformedDefinition` | `an opaque alias dependency graph exceeds the recursion limit` | `opaque::ensure_alias_graph_acyclic` | `DefinitionReason::OpaqueAliasDepthLimit` |
| `MalformedDefinition` | `a type alias has one right-hand-side tree` | `opaque::ensure_alias_graph_acyclic` | `DefinitionReason::TypeAliasRhsArity` |
| `MalformedDefinition` | `a generic opaque alias lambda has parameters and a body` | `opaque::unwrap_lambda_tpt` | `DefinitionReason::OpaqueLambdaShape` |
| `MalformedDefinition` | `nested opaque alias parameter lambdas are unsupported` | `opaque::unwrap_lambda_tpt` | **Move to `UnsupportedFeature` in a later API migration**; the TASTy may be valid. |
| `MalformedRefinedTypeTree` | `a refined type tree has a parent` | `enter::enter_refined_tpt`, `refinement::type_of_refined_tpt` | `RefinedTreeReason::UnexpectedParent` |
| `MalformedRefinedTypeTree` | `a refinement stat is not a supported member definition` | `refinement::type_of_refined_tpt` | This is actually `UnsupportedFeature`; use `RefinedTreeReason::UnsupportedMemberStat` only if the variant remains a mixed family, otherwise split it in a later issue. |
| `MalformedType` | `a match type has a bound and a scrutinee` | `types::decode_type` | `TypeReason::MatchTypeShape` |
| `MalformedType` | `the node's children do not match its shape` | `types::decode_children` | `TypeReason::ChildShapeMismatch` |
| `MalformedType` | `an owner-space reference resolved to a name designator` | `types::resolved_declaration` | `TypeReason::OwnerSpaceNameDesignator`; likely `UnsupportedFeature` or `ResolutionFailure`, since a structural name designator does not establish malformed input |
| `MalformedType` | `a refined type has a parent and a refinement` | `refined::decode_refined_type` | `TypeReason::RefinedTypeShape` |
| `MalformedType` | `not a direct or symbol reference` | `discovery::reference_target` | `TypeReason::InvalidReferenceTargetShape` |
| `MalformedType` | `the binder's children disagree with its parameters` | `binders::decode_methodic` | `TypeReason::BinderChildMismatch` |
| `MalformedType` | `the binder has no parameters` | `binders::decode_methodic` | `TypeReason::EmptyBinder` |
| `MalformedType` | `a bounds tail holds a marker that is not a variance` | `binders::declared_variances` | `TypeReason::InvalidVarianceMarker` |
| `MalformedType` | `an ANNOTATION tail entry has a tycon and a full annotation` | `symbol_annotations::full_annotation_address` | `TypeReason::AnnotationTailShape` |
| `MalformedType` | `the annotation of an annotated type is not a node` | `annotated::annotation_tree_root` | `TypeReason::AnnotationAddressIsNotNode` |
| `MalformedType` | `an annotated type has a parent and an annotation` | `annotated::decode_annotated_type` | `TypeReason::AnnotatedTypeShape` |
| `MalformedType` | `the annotation is not a node` | `annotated::decode_annotation_payload` | `TypeReason::AnnotationAddressIsNotNode` |
| `MalformedType` | `an identifier type tree has one type` | `annotated::annotation_class_type` | `TypeReason::IdentifierTypeTreeArity` |
| `MalformedType` | `a named argument has one value` | `annotated::annotation_argument` | `TypeReason::NamedArgumentArity` |
| `MalformedType` | `an annotation argument literal is not a constant` | `annotated::annotation_argument` | `TypeReason::AnnotationLiteralNotConstant` |
| `MalformedType` | `a recursive type has exactly one child` | `recursive::decode_rec_type` | `TypeReason::RecursiveTypeArity` |
| `MalformedType` | `a parent tree nests too deeply` | `enter::enter_parent_lambdas` | `TypeReason::ParentTreeDepthLimit`; `UnsupportedFeature` for an input exceeding the implementation's depth limit, absent evidence of a cycle |
| `MalformedType` | `a lambda type tree has type parameters and a body` | `enter::enter_lambda_tpt` | `TypeReason::LambdaTreeShape` |
| `MalformedType` | `a lambda type tree parameter is not a type parameter` | `enter::enter_lambda_tpt` | `TypeReason::LambdaParameterKind` |
| `MalformedType` | `a refined type tree has a parent` | `enter::enter_refined_tpt` | `TypeReason::RefinedTreeUnexpectedParent` |

The `MalformedDefinition`, `MalformedParentTree`, `MalformedRefinedTypeTree`,
and `MalformedType` families currently mix malformed structure, unsupported
valid constructs, and resource-limit conditions. A typed-reason migration should preserve the
distinction in category; it should not infer category from reason enum alone
unless the family is first split. The `MalformedRefinedTypeTree` unsupported
member case and nested opaque parameter lambdas are direct candidates for a
future error-shape correction, but changing them is outside this audit issue.

There is no open-ended `reason` producer in the audited source. Helpers accept
`&'static str` (`term_type::malformed`, `types::malformed`, and the other
construction sites), and current callers pass the finite literal set above.
This is evidence for bounded enums, not evidence to alter the API in #977.

## Locations and transaction safety

* Every `address`, `from`, `to`, `binder`, `reference`, and
  `annotation_address` in this enum is a TASTy name-table reference or AST
  address as its field documentation specifies. AST addresses are coordinates
  in the decoded AST section; they are **not** `dotty_core::SourceSpan` values.
  Nested `AstError` byte offsets are likewise TASTy payload offsets.
* `UnresolvedPackage` retains the package text and use-site AST address;
  `UnresolvedMember` retains the name, namespace, prefix, optional explicit
  declaration space, and use-site address; `AmbiguousMember` retains candidate
  count and name. `ResolverFailure` retains the resolver cause and address.
  `Tasty`/`Ast` retain their structural source errors.
* `SymbolId` payload variants are `DuplicateScope`, `ConflictingCompanion`,
  `MissingClassScope`, `InvalidRefinementClass`, `MissingRefinementScope`,
  `ConstructorOwnerNotClassLike`, `MalformedOwnerClassInfo`,
  `OpaqueAliasOwnerNotClassLike`, `OpaqueAliasOwnerNotEntered`,
  `InvalidBinderKind`, and `InvalidTypeParameterBounds`.
  `TypeId` payload variants are `MalformedOwnerClassInfo`,
  `UnstableSelectQualifier`, `InvalidSingletonTypeTree`,
  `InvalidCompletedBounds`, `UnresolvedMember`, `AmbiguousMember`,
  `InvalidCompactAnnotationType`, `InvalidAnnotationType`,
  `BoundsVarianceTargetPending`, `BoundsVarianceArityMismatch`,
  `RebindFailed`, `InvalidBinderKind`, `InvalidParameterIndex`,
  `InvalidTypeParameterBounds`, `UnsupportedResolutionPrefix`,
  `UnsupportedResolutionSpace`, and `IllegalTypePrefix`.
* `enter_symbols` rolls back its store allocations, unit index, package registry
  writes, and shared-scope declarations on failure. Public type/tree/term
  projection and completion calls roll back the store and resolver checkpoints,
  address caches, pending aliases, recursive-this entries, annotations, and
  pre-existing symbol-info writes. See `unpickler.rs::enter_symbols`,
  `unpickler.rs::finish_transaction`, and `docs/tasty-semantic-unpickler.md`.
* Consequently, an ID stored in an error may name an allocation removed by the
  failed transaction. It is safe to preserve, compare, clone, debug-format, or
  render the typed ID itself. It must not be dereferenced through the rolled
  back store, resolver cache, or index. In particular, future `category()` and
  `code()` implementations must be pure matches over the error variant and
  nested cause; they must not look up `SymbolId`/`TypeId`.
* Audit found no payload that stores a reference into a transaction-owned
  temporary buffer. The static reason payloads are static literals; strings
  (`name`, `package`) are owned. `ResolutionError::Malformed.reason` is owned.
  No unstable payload risk was found.

## Existing evidence and scope for follow-up

The current tests assert exact existing variants and payloads for malformed
addresses/names, unsupported tags, resolution ambiguity/failure, and selected
reason values; the full unpickler suite also contains rollback, retry, and
Scala 3.9 semantic-fixture coverage. The implementation issue should add a
focused test only if a proposed category/reason mapping has no direct evidence.
This audit does not regenerate corpus expectations or add a test that merely
parses `Display`.

The next implementation issue can use this inventory to add exhaustive
`category()`/`code()` methods and migrate the five finite reason families. It
must first split or otherwise faithfully represent the mixed-category cases
listed above. No renderer, `Diagnostic`, cross-crate error redesign, or
unpickling acceptance change is implied here.
