# TASTy unpickler error taxonomy

This document began as the issue #977 audit of the public
`dotty_tasty_unpickler::UnpickleError`. Issue #978 implements its typed reason,
category and stable-code recommendations in
`crates/dotty-tasty-unpickler/src/error.rs`. The change preserves unpickling,
retry and rollback behavior; it does not introduce a diagnostic renderer or
change the `Result<_, UnpickleError>` API.

## Classification rules

| Category | Meaning in this audit |
| --- | --- |
| `MalformedInput` | The artifact has an invalid structural shape, address, name reference, or semantic reference under the input-local contract. |
| `UnsupportedFeature` | The shape may be legal TASTy, but this unpickler does not project or interpret it. |
| `ResolutionFailure` | An external/session symbol or dependency is missing, ambiguous, or the resolver itself failed. Context may be needed to distinguish absence from a malformed local reference. |
| `Deferred` | The operation is legal but requires completion or a later pass/milestone. |
| `InternalInvariant` | A pass-order, store/index, ownership, or registration contract expected of this implementation was violated. |
| `Conditional` | The current error payload omits evidence needed to distinguish two categories; consult the source, resolver, or store context before making a more specific claim. |

These are primary categories, not claims that every enclosing file is corrupt.
In particular, nested decode errors retain their own distinctions; a
`TastyFileError` or `AstError` can report an unsupported valid version/tag as
well as malformed data. The `ResolutionFailure` entries marked conditional
are represented as `Conditional` when the public payload cannot retain the
necessary context. This avoids mislabeling implementation depth limits or
missing external symbols as malformed bytes.

## Variant inventory

Every public variant is listed once. Producer references point to the
implementation module/function; several variants have multiple construction
sites in the named function. `UnpickleError::code()` returns explicit stable
lowercase strings; codes do not depend on `Debug`, `Display`, IDs, or addresses.
The five typed reason families return reason-specific codes under their family
prefixes, as listed below.

| Variant | Primary category and evidence | Main producer(s) | Stable code |
| --- | --- | --- | --- |
| `Tasty` | `MalformedInput` or unsupported format/version, conditional on nested cause | `From<TastyFileError>`; file queries throughout unpickler | `unpickler.tasty` |
| `Ast` | `MalformedInput` or unsupported AST feature/tag, conditional on nested cause | `From<AstError>`; structured AST access throughout unpickler | `unpickler.ast` |
| `DuplicateDefinition` | `InternalInvariant`; an AST address must have one entered identity | `index::insert_symbol` | `unpickler.duplicate_definition` |
| `DuplicateScope` | `InternalInvariant`; scope registration is unique per symbol | `index::insert_scope`, `unpickler::share_owner_scope` | `unpickler.duplicate_scope` |
| `ConflictingCompanion` | `InternalInvariant`; reciprocal companion links conflict with published state | `companions::link_companions` | `unpickler.conflicting_companion` |
| `InvalidNameReference` | `MalformedInput` for an out-of-range/missing reference; a depth-limit failure is ambiguous and may be `UnsupportedFeature` for a valid long name chain | `names::{collect_segments,entry_at,split,derive,resolve}` | `unpickler.invalid_name_reference` |
| `UnsupportedName` | `UnsupportedFeature`; name-table tag/kind is not interpreted | `names::{derive,resolve}` | `unpickler.unsupported_name` |
| `UnsupportedQualifier` | `UnsupportedFeature`; access qualifier tree form is not read | `mapping::qualifier_ref`, `enter::resolve_qualifier` | `unpickler.unsupported_qualifier` |
| `InvalidReferenceTarget` | `MalformedInput` when the target is not a visible AST node; depth-bounded shared-reference cycles and over-depth chains are currently indistinguishable and may include valid input beyond the limit | `ast_view`, `enter`, `class`, `binders`, `types`, and tree/type walkers | `unpickler.invalid_reference_target` |
| `InvalidQualifier` | `MalformedInput`; qualifier does not enclose the definition or has an illegal owner relation | `enter::resolve_qualifier` | `unpickler.invalid_qualifier` |
| `UnsupportedPackagePath` | `UnsupportedFeature`; package path is not a supported direct/shared package reference | `enter::package_path_name` | `unpickler.unsupported_package_path` |
| `MissingDefinition` | `MalformedInput` when the AST walk expects a definition that is absent; pass/index mismatch would instead be an invariant | `ast_view::node`, `enter`, `completion`, `opaque` | `unpickler.missing_definition` |
| `DuplicateType` | `InternalInvariant`; type/address caches are single-assignment | `index::{insert_type,insert_type_tree,insert_term_tree}` | `unpickler.duplicate_type` |
| `UnsupportedTypeTree` | `UnsupportedFeature`; tree tag is not projected | `type_tree::type_of_tpt` | `unpickler.unsupported_type_tree` |
| `UnsupportedTermTree` | `UnsupportedFeature`; term tree has no implemented `tpe` projection | `term_type::type_of_term` | `unpickler.unsupported_term_tree` |
| `MissingClassScope` | `InternalInvariant`; pass 1 promises a scope for every entered template | `class::complete_class` | `unpickler.missing_class_scope` |
| `UnsupportedParentTree` | `UnsupportedFeature`; parent wrapper tag is not interpreted | `class::type_of_parent_in` | `unpickler.unsupported_parent_tree` |
| `MalformedParentTree` | `MalformedInput` for invalid wrapper/call shape; the nesting-limit reason is `UnsupportedFeature` unless the nested structure is independently shown to be cyclic/invalid | `class::type_of_parent_in` | `unpickler.malformed_parent_tree.*` |
| `InvalidSelfTypeTree` | `MalformedInput`; `SELFDEF` does not have its required shape | `class::type_of_self_def` | `unpickler.invalid_self_type_tree.*` |
| `MalformedDefinition` | `MalformedInput` for shape mismatch; nested opaque-parameter lambdas and alias-graph recursion-limit cases are `UnsupportedFeature` unless the graph is independently shown cyclic | `class::complete_class`, `method::{clauses_of,defdef_header}`, `opaque::{complete_opaque_alias,ensure_alias_graph_acyclic,unwrap_lambda_tpt}` | `unpickler.malformed_definition.*` |
| `UnsupportedMethodParameterSemantics` | `UnsupportedFeature`; a valid parameter modifier cannot be represented in adapted method info | `method::guard_parameter_modifiers` | `unpickler.unsupported_method_parameter_semantics` |
| `ConstructorOwnerNotClassLike` | `MalformedInput` for a constructor under a non-class-like owner; impossible owner-state can indicate an index invariant | `constructor::constructor_owner` | `unpickler.constructor_owner_not_class_like` |
| `MalformedOwnerClassInfo` | `InternalInvariant`; a completed class owner does not hold its canonical `ClassInfo` | `constructor::constructor_owner`, `opaque::apply_pending_opaque_aliases` | `unpickler.malformed_owner_class_info` |
| `SharedLambdaOwnerConflict` | `UnsupportedFeature`; shared lambda has more than one semantic owner | `type_tree::type_of_lambda_tpt` | `unpickler.shared_lambda_owner_conflict` |
| `SharedRefinementOwnerConflict` | `UnsupportedFeature`; shared refinement has more than one fixed synthetic-class owner | `refinement::type_of_refined_tpt` | `unpickler.shared_refinement_owner_conflict` |
| `UnsupportedRefinementStat` | `UnsupportedFeature`; refinement member tag is not projected | `enter::enter_refined_tpt` | `unpickler.unsupported_refinement_stat` |
| `MissingRefinementClass` | `InternalInvariant`; discovery/entry pass did not produce its synthetic class | `refinement::refinement_class` | `unpickler.missing_refinement_class` |
| `InvalidRefinementClass` | `InternalInvariant`; entry at the refinement address is not its synthetic class | `refinement::refinement_class` | `unpickler.invalid_refinement_class` |
| `MissingRefinementScope` | `InternalInvariant`; synthetic class was entered without its required scope | `refinement::refinement_class` | `unpickler.missing_refinement_scope` |
| `MalformedRefinedTypeTree` | `MalformedInput` for a missing parent; `UnsupportedFeature` when a refinement stat is outside the supported member subset | `refinement::type_of_refined_tpt` | `unpickler.malformed_refined_type_tree.*` |
| `UnsupportedRefinementOverload` | `UnsupportedFeature`; the model deliberately refuses same-name refinement overloads | `refinement::type_of_refined_tpt` | `unpickler.unsupported_refinement_overload` |
| `CloseOverThis` | Category follows its nested `TypeRebindError`; `TooDeep` is `UnsupportedFeature`, duplicate-symbol causes are `InternalInvariant`, and other causes are `Conditional` | `refinement::type_of_refined_tpt` | `unpickler.close_over_this` |
| `ParameterAbstraction` | Category follows its nested `TypeRebindError`; `TooDeep` is `UnsupportedFeature`, duplicate-symbol causes are `InternalInvariant`, and other causes are `Conditional` | `method::build_clause`, `opaque::opaque_type_lambda`, `type_tree::type_of_lambda_tpt` | `unpickler.parameter_abstraction` |
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
| `UnsupportedType` | `Conditional`; a valid later/unimplemented type tag is unsupported, while a tag that is not a type is malformed | `types::decode_type`, `term_type::type_of_term`, `annotated::annotation_class_type` | `unpickler.unsupported_type` |
| `MissingReferencedSymbol` | `ResolutionFailure` for local/cross-unit definitions absent from this unit's index; absence alone is not proof of corrupt bytes | `types::referenced_symbol` | `unpickler.missing_referenced_symbol` |
| `InvalidReferenceKind` | `MalformedInput`; reference namespace/kind and target symbol kind disagree | `types::{referenced_symbol,referenced_class,this_class}` | `unpickler.invalid_reference_kind` |
| `UnresolvedPackage` | `ResolutionFailure`; package registry and resolver both lack the named package | `types::referenced_package`, `opaque` | `unpickler.unresolved_package` |
| `UnresolvedMember` | `ResolutionFailure`; `Ok(None)` is not proof the member does not exist outside the current resolver/session | `types::resolved_named`, `opaque` | `unpickler.unresolved_member` |
| `AmbiguousMember` | `ResolutionFailure`; lookup has multiple valid candidates and no selector | `types::resolved_named` | `unpickler.ambiguous_member` |
| `UnsupportedSignedReference` | `UnsupportedFeature`; member selection by signature is not implemented | `types::resolved_named`, `refined::decode_refined_type` | `unpickler.unsupported_signed_reference` |
| `MalformedType` | `MalformedInput` for invalid child/type/reference shape; depth-limit and owner-space designator reasons are `UnsupportedFeature`, not proof of corrupt bytes | `discovery`, `enter`, `types`, `binders`, `recursive`, `refined`, `refinement`, `term_type`, `type_tree`, `annotated`, `symbol_annotations`, `reachability` | `unpickler.malformed_type.*` |
| `UnsupportedAnnotationTree` | `UnsupportedFeature`; annotation tree is retained in TASTy but not projected | `type_tree::type_of_tpt`, `annotated::decode_annotation_payload` | `unpickler.unsupported_annotation_tree` |
| `InvalidCompactAnnotationType` | `Conditional`; the payload does not distinguish a malformed compact type from a binder/pending reference that is still being decoded | `types`, `annotated::decode_annotation_payload` | `unpickler.invalid_compact_annotation_type` |
| `UnsupportedAnnotationConstructor` | `UnsupportedFeature`; constructor spine/class form is outside the annotation projection | `annotated::{decode_constructor_annotation,constructor_spine,annotation_class_type}` | `unpickler.unsupported_annotation_constructor` |
| `UnsupportedAnnotationArgument` | `UnsupportedFeature`; annotation value is not a literal/class literal supported by the model | `annotated::annotation_argument` | `unpickler.unsupported_annotation_argument` |
| `InvalidAnnotationType` | `Conditional`; unsupported but valid annotation type forms have not been ruled out by the payload | `annotated::decode_constructor_annotation` | `unpickler.invalid_annotation_type` |
| `BoundsVarianceTargetPending` | `Deferred`; variance rebinding targets a lambda still under decode | `types`, `binders::apply_declared_variances` | `unpickler.bounds_variance_target_pending` |
| `BoundsVarianceArityMismatch` | `MalformedInput`; variance count differs from binder arity | `types`, `binders::apply_declared_variances` | `unpickler.bounds_variance_arity_mismatch` |
| `RebindFailed` | `Conditional`; nested rebinding failures can indicate malformed graphs or invalid compiler-owned IDs, and the error retains the typed cause | `binders::apply_declared_variances` | `unpickler.rebind_failed` |
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
| `TypeRebindError` | `CloseOverThis`, `ParameterAbstraction`, `RebindFailed` | See `crates/dotty-core/src/types/rebind.rs`. Carries exact type/symbol IDs, arity, or graph condition. `TooDeep` is `UnsupportedFeature`; duplicate-symbol causes are `InternalInvariant`; other causes are `Conditional` because the payload alone cannot establish whether a malformed graph came from the artifact or an internal inconsistency. A caught error is returned after enclosing operation rollback. |

`UnpickleError::source()` currently exposes only `Tasty`, `Ast`, and
`RebindFailed`; the first two expose their wrapped source, while close/abstraction
errors include `TypeRebindError` in their `Display` only. `ResolutionError`
does not currently implement `source()` through `UnpickleError`. Preserve this
existing behavior in #977; any error-chain API change belongs to a later issue.

## Legacy reason text and typed families

There are five reason-bearing variants. The list below records every legacy
reason literal and the typed enum variant now used at its producer. Each typed
reason has its own stable code under the family prefix in the inventory table. The
finite reason enums deliberately have no `Other(String)` fallback.

| Error variant | Legacy `reason` text | Producer | Typed reason |
| --- | --- | --- | --- |
| `MalformedParentTree` | `a parent constructor call nests too deeply` | `class::type_of_parent_in` | `ParentTreeReason::NestingLimit` (`unpickler.malformed_parent_tree.nesting_limit`) |
| `MalformedParentTree` | `a parent constructor call has a function` | `class::type_of_parent_in` | `ParentTreeReason::ConstructorHasFunction` (`unpickler.malformed_parent_tree.constructor_has_function`) |
| `MalformedParentTree` | `a parent constructor selection does not select <init>` | `class::type_of_parent_in` | `ParentTreeReason::SelectsConstructor` (`unpickler.malformed_parent_tree.selects_constructor`) |
| `MalformedParentTree` | `a parent constructor selection is not on NEW` | `class::type_of_parent_in` | `ParentTreeReason::SelectsNew` (`unpickler.malformed_parent_tree.selects_new`) |
| `MalformedParentTree` | `NEW has one type tree` | `class::type_of_parent_in` | `ParentTreeReason::NewArity` (`unpickler.malformed_parent_tree.new_arity`) |
| `InvalidSelfTypeTree` | `a self definition has one type tree` | `class::type_of_self_def` | `SelfTypeTreeReason::TypeTreeArity` (`unpickler.invalid_self_type_tree.type_tree_arity`) |
| `MalformedDefinition` | `a class definition's first child is its template` | `class::complete_class` | `DefinitionReason::ClassTemplatePosition` (`unpickler.malformed_definition.class_template_position`) |
| `MalformedDefinition` | `a method has a result type after its parameters` | `method::defdef_header` | `DefinitionReason::ResultTypePosition` (`unpickler.malformed_definition.result_type_position`) |
| `MalformedDefinition` | `more header parameters than parameter nodes` | `method::clauses_of` | `DefinitionReason::HeaderParametersExceedNodes` (`unpickler.malformed_definition.header_parameters_exceed_nodes`) |
| `MalformedDefinition` | `a header parameter disagrees with its node` | `method::clauses_of` | `DefinitionReason::HeaderParameterTagMismatch` (`unpickler.malformed_definition.header_parameter_tag_mismatch`) |
| `MalformedDefinition` | `more parameter nodes than header parameters` | `method::clauses_of` | `DefinitionReason::ParameterNodesExceedHeader` (`unpickler.malformed_definition.parameter_nodes_exceed_header`) |
| `MalformedDefinition` | `an opaque type alias has one right-hand-side tree` | `opaque::complete_opaque_alias` | `DefinitionReason::OpaqueAliasRhsArity` (`unpickler.malformed_definition.opaque_alias_rhs_arity`) |
| `MalformedDefinition` | `opaque bounds children do not match their encoded shape` | `opaque::complete_opaque_alias` | `DefinitionReason::OpaqueBoundsShape` (`unpickler.malformed_definition.opaque_bounds_shape`) |
| `MalformedDefinition` | `an opaque alias dependency graph exceeds the recursion limit` | `opaque::ensure_alias_graph_acyclic` | `DefinitionReason::OpaqueAliasDepthLimit` (`unpickler.malformed_definition.opaque_alias_depth_limit`) |
| `MalformedDefinition` | `a type alias has one right-hand-side tree` | `opaque::ensure_alias_graph_acyclic` | `DefinitionReason::TypeAliasRhsArity` (`unpickler.malformed_definition.type_alias_rhs_arity`) |
| `MalformedDefinition` | `a generic opaque alias lambda has parameters and a body` | `opaque::unwrap_lambda_tpt` | `DefinitionReason::OpaqueLambdaShape` (`unpickler.malformed_definition.opaque_lambda_shape`) |
| `MalformedDefinition` | `nested opaque alias parameter lambdas are unsupported` | `opaque::unwrap_lambda_tpt` | `DefinitionReason::NestedOpaqueParameterLambdasUnsupported` (`unpickler.malformed_definition.nested_opaque_parameter_lambdas`); categorized `UnsupportedFeature`. |
| `MalformedRefinedTypeTree` | `a refined type tree has a parent` | `enter::enter_refined_tpt`, `refinement::type_of_refined_tpt` | `RefinedTypeTreeReason::UnexpectedParent` (`unpickler.malformed_refined_type_tree.unexpected_parent`) |
| `MalformedRefinedTypeTree` | `a refinement stat is not a supported member definition` | `refinement::type_of_refined_tpt` | `RefinedTypeTreeReason::UnsupportedMemberStat` (`unpickler.malformed_refined_type_tree.unsupported_member_stat`); categorized `UnsupportedFeature`. |
| `MalformedType` | `a match type has a bound and a scrutinee` | `types::decode_type` | `TypeReason::MatchTypeShape` (`unpickler.malformed_type.match_type_shape`) |
| `MalformedType` | `the node's children do not match its shape` | `types::decode_children`, `type_tree::type_of_tpt` | `TypeReason::ChildShapeMismatch` (`unpickler.malformed_type.child_shape_mismatch`) |
| `MalformedType` | `an owner-space reference resolved to a name designator` | `types::resolved_declaration` | `TypeReason::OwnerSpaceNameDesignator` (`unpickler.malformed_type.owner_space_name_designator`); categorized `UnsupportedFeature`, since a structural name designator does not establish malformed input |
| `MalformedType` | `a refined type has a parent and a refinement` | `refined::decode_refined_type` | `TypeReason::RefinedTypeShape` (`unpickler.malformed_type.refined_type_shape`) |
| `MalformedType` | `not a direct or symbol reference` | `discovery::reference_target` | `TypeReason::InvalidReferenceTargetShape` (`unpickler.malformed_type.invalid_reference_target_shape`) |
| `MalformedType` | `the binder's children disagree with its parameters` | `binders::decode_methodic` | `TypeReason::BinderChildMismatch` (`unpickler.malformed_type.binder_child_mismatch`) |
| `MalformedType` | `the binder has no parameters` | `binders::decode_methodic` | `TypeReason::EmptyBinder` (`unpickler.malformed_type.empty_binder`) |
| `MalformedType` | `a bounds tail holds a marker that is not a variance` | `binders::declared_variances` | `TypeReason::InvalidVarianceMarker` (`unpickler.malformed_type.invalid_variance_marker`) |
| `MalformedType` | `an ANNOTATION tail entry has a tycon and a full annotation` | `symbol_annotations::full_annotation_address` | `TypeReason::AnnotationTailShape` (`unpickler.malformed_type.annotation_tail_shape`) |
| `MalformedType` | `the annotation of an annotated type is not a node` | `annotated::annotation_tree_root` | `TypeReason::AnnotationOfAnnotatedTypeIsNotNode` (`unpickler.malformed_type.annotation_of_annotated_type_is_not_node`) |
| `MalformedType` | `an annotated type has a parent and an annotation` | `annotated::decode_annotated_type` | `TypeReason::AnnotatedTypeShape` (`unpickler.malformed_type.annotated_type_shape`) |
| `MalformedType` | `the annotation is not a node` | `annotated::decode_annotation_payload` | `TypeReason::AnnotationPayloadIsNotNode` (`unpickler.malformed_type.annotation_payload_is_not_node`) |
| `MalformedType` | `an identifier type tree has one type` | `annotated::annotation_class_type` | `TypeReason::IdentifierTypeTreeArity` (`unpickler.malformed_type.identifier_type_tree_arity`) |
| `MalformedType` | `an application has a function` | `annotated::constructor_spine` | `TypeReason::AnnotationApplyFunctionMissing` (`unpickler.malformed_type.annotation_apply_function_missing`) |
| `MalformedType` | `an application's arguments do not match its shape` | `annotated::constructor_spine` | `TypeReason::AnnotationApplyArityMismatch` (`unpickler.malformed_type.annotation_apply_arity_mismatch`) |
| `MalformedType` | `a type application has a function` | `annotated::constructor_spine` | `TypeReason::AnnotationTypeApplyFunctionMissing` (`unpickler.malformed_type.annotation_type_apply_function_missing`) |
| `MalformedType` | `a type application's arguments do not match its shape` | `annotated::constructor_spine` | `TypeReason::AnnotationTypeApplyArityMismatch` (`unpickler.malformed_type.annotation_type_apply_arity_mismatch`) |
| `MalformedType` | `a selection has a qualifier and an owner` | `annotated::constructor_spine` | `TypeReason::AnnotationSelectionShape` (`unpickler.malformed_type.annotation_selection_shape`) |
| `MalformedType` | a \`new\` has one class | `annotated::constructor_spine` | `TypeReason::AnnotationNewClassArity` (`unpickler.malformed_type.annotation_new_class_arity`) |
| `MalformedType` | `a named argument has one value` | `annotated::annotation_argument` | `TypeReason::NamedArgumentArity` (`unpickler.malformed_type.named_argument_arity`) |
| `MalformedType` | `an annotation argument literal is not a constant` | `annotated::annotation_argument` | `TypeReason::AnnotationLiteralNotConstant` (`unpickler.malformed_type.annotation_literal_not_constant`) |
| `MalformedType` | `a recursive type has exactly one child` | `recursive::decode_rec_type` | `TypeReason::RecursiveTypeArity` (`unpickler.malformed_type.recursive_type_arity`) |
| `MalformedType` | `a parent tree nests too deeply` | `enter::enter_parent_lambdas`, `reachability::walk_parent` | `TypeReason::ParentTreeDepthLimit` (`unpickler.malformed_type.parent_tree_depth_limit`); `UnsupportedFeature` for an input exceeding the implementation's depth limit, absent evidence of a cycle |
| `MalformedType` | `a type tree nests too deeply` | `discovery::discover_type_tree`, `reachability::walk` | `TypeReason::TypeTreeDepthLimit` (`unpickler.malformed_type.type_tree_depth_limit`); `UnsupportedFeature` for input beyond the configured limit |
| `MalformedType` | `a lambda type tree has type parameters and a body` | `enter::enter_lambda_tpt`, `type_tree::type_of_lambda_tpt` | `TypeReason::LambdaTreeShape` (`unpickler.malformed_type.lambda_tree_shape`) |
| `MalformedType` | `a lambda type tree parameter is not a type parameter` | `enter::enter_lambda_tpt` | `TypeReason::LambdaParameterKind` (`unpickler.malformed_type.lambda_parameter_kind`) |
| `MalformedType` | `a refined type tree has a parent` | `enter::enter_refined_tpt` | `TypeReason::RefinedTreeUnexpectedParent` (`unpickler.malformed_type.refined_tree_unexpected_parent`) |
| `MalformedType` | `an identifier has one type` | `term_type::type_of_term` | `TypeReason::IdentifierTermTypeArity` (`unpickler.malformed_type.identifier_term_type_arity`) |
| `MalformedType` | `a selection has one qualifier` | `term_type::type_of_term` | `TypeReason::TermSelectionQualifierArity` (`unpickler.malformed_type.term_selection_qualifier_arity`) |
| `MalformedType` | `a qualified this has one qualifier` | `term_type::qualified_this_type` | `TypeReason::QualifiedThisQualifierArity` (`unpickler.malformed_type.qualified_this_qualifier_arity`) |
| `MalformedType` | `the qualifier of a qualified this is not a type identifier` | `term_type::qualified_this_type` | `TypeReason::QualifiedThisQualifierKind` (`unpickler.malformed_type.qualified_this_qualifier_kind`) |
| `MalformedType` | `an explicit type tree has one child` | `type_tree::type_of_tpt` | `TypeReason::ExplicitTypeTreeArity` (`unpickler.malformed_type.explicit_type_tree_arity`) |
| `MalformedType` | `a by-name type tree has one child` | `type_tree::type_of_tpt` | `TypeReason::ByNameTypeTreeArity` (`unpickler.malformed_type.by_name_type_tree_arity`) |
| `MalformedType` | `an applied type tree has a constructor` | `type_tree::type_of_tpt` | `TypeReason::AppliedTypeConstructorMissing` (`unpickler.malformed_type.applied_type_constructor_missing`) |
| `MalformedType` | `a bounds type tree has one to three children` | `type_tree::type_of_tpt` | `TypeReason::BoundsTypeTreeArity` (`unpickler.malformed_type.bounds_type_tree_arity`) |
| `MalformedType` | `a type selection has one qualifier` | `type_tree::type_of_tpt` | `TypeReason::TypeSelectionQualifierArity` (`unpickler.malformed_type.type_selection_qualifier_arity`) |
| `MalformedType` | `a singleton type tree has one reference` | `type_tree::type_of_tpt` | `TypeReason::SingletonTreeReferenceArity` (`unpickler.malformed_type.singleton_tree_reference_arity`) |
| `MalformedType` | `an annotated type tree has a type tree and an annotation` | `type_tree::type_of_tpt` | `TypeReason::AnnotatedTypeTreeShape` (`unpickler.malformed_type.annotated_type_tree_shape`) |

The `MalformedDefinition`, `MalformedParentTree`, `MalformedRefinedTypeTree`,
and `MalformedType` variants still contain some valid-but-unsupported cases.
Their `category()` implementation consults the typed reason and reports those
cases as `UnsupportedFeature`; the outer variant remains unchanged for API
compatibility. A later cleanup can split those outer variants if callers need
a narrower match shape.

There was no open-ended `reason` producer in the audited source. The current
helpers accept typed reason values, and all audited literals map to finite enum
variants. Dynamic resolver causes remain nested `ResolutionError` payloads and
are not folded into a catch-all reason string.


### Depth limits and conditional categories

Several existing variants also report implementation limits through the same
shape used for malformed references. The distinction must be represented
before an exhaustive `category()` is added:

* `names.rs` caps name-table traversal at `MAX_NAME_DEPTH = 64` and returns
  `InvalidNameReference { reference }` both for missing references and for an
  over-depth chain. The cycle is not separately identified. A reviewer
  reproduced a valid Scala 3.9.0 class in a package with 70 segments
  (`p0.p1.…p69`): parsing succeeded and `enter_symbols` returned
  `InvalidNameReference { reference: 139 }`. This is valid input beyond the
  implementation limit, so that outcome is not evidence of malformed TASTy.
  `category()` therefore returns `Conditional` until a future error-shape
  change adds separate missing/cycle/depth reasons.
* `ast_view::{next_unshared_tag,resolve_shared_term,resolve_shared_type}` cap
  shared-link following at `MAX_SHARED_DEPTH = 16` and return
  `InvalidReferenceTarget { from, to }` for both non-node targets and a chain
  that exceeds the bound. Cycles are also bounded by the same limit rather
  than reported as a distinct reason. The same depth-only result is produced
  by shared-link guards in `enter`, `type_tree`, `term_type`, `types`, and
  `recursive`. A target that is not a node is `MalformedInput`; a valid
  over-depth link chain could be `UnsupportedFeature` until evidence proves
  that TASTy forbids it. `category()` therefore returns `Conditional` for the
  current combined variant; a future error-shape change should distinguish
  invalid target, cycle, and depth limit.
* The other depth guards were audited too: `enter::MAX_TREE_DEPTH` errors use
  malformed-reason variants in type discovery/reachability, and the opaque
  alias graph limit uses `MalformedDefinition`. Treat those as unsupported
  limits unless the specific input also proves a cycle or invalid shape.

The stable codes remain variant-level (`unpickler.invalid_name_reference` and
`unpickler.invalid_reference_target`), while `category()` returns `Conditional`
for these combined outcomes.

## Locations and transaction safety

* `address`, `from`, `to`, `definition`, and `annotation_address` are AST
  addresses; `reference` in `InvalidNameReference`/`UnsupportedName` is a name
  table reference. `InvalidBinderReference.binder` is an AST address, while
  `InvalidBinderKind.binder`, `InvalidParameterIndex.binder`, and
  `InvalidTypeParameterBounds.binder` are semantic `TypeId`s. AST addresses
  are coordinates in the decoded AST section; they are **not**
  `dotty_core::SourceSpan` values. Nested `AstError` byte offsets are TASTy
  payload offsets too.
* `UnresolvedPackage` retains the package text and use-site AST address;
  `UnresolvedMember` retains the name, namespace, prefix, optional explicit
  declaration space, and use-site address; `AmbiguousMember` retains candidate
  count and name. `ResolverFailure` retains the resolver cause and address.
  `Tasty`/`Ast` retain their structural source errors.
* `SymbolId` payload variants are `DuplicateScope`, `ConflictingCompanion`,
  `MissingClassScope`, `InvalidRefinementClass`, `MissingRefinementScope`,
  `ConstructorOwnerNotClassLike`, `MalformedOwnerClassInfo`,
  `OpaqueAliasOwnerNotClassLike`, and `OpaqueAliasOwnerNotEntered`.
  `TypeId` payload variants are `MalformedOwnerClassInfo`,
  `UnstableSelectQualifier`, `InvalidSingletonTypeTree`,
  `InvalidCompletedBounds`, `UnresolvedMember`, `AmbiguousMember`,
  `InvalidCompactAnnotationType`, `InvalidAnnotationType`,
  `BoundsVarianceTargetPending`, `BoundsVarianceArityMismatch`,
  `RebindFailed`, `InvalidBinderKind`, `InvalidParameterIndex`,
  `InvalidTypeParameterBounds`, `UnsupportedResolutionPrefix`,
  `UnsupportedResolutionSpace`, and `IllegalTypePrefix`.
  Nested `TypeRebindError` payloads in `CloseOverThis`, `ParameterAbstraction`,
  and `RebindFailed` can also carry `SymbolId` fields
  (`DuplicateParameterSymbol`/`DuplicateSubstitutionSymbol`); those nested IDs
  have the same rollback warning as direct payloads.
* `enter_symbols` rolls back its store allocations, unit index, package registry
  writes, and shared-scope declarations on failure. Public type/tree/term
  projection and completion calls roll back the store and resolver checkpoints,
  address caches, pending aliases, recursive-this entries, annotations, and
  pre-existing symbol-info writes. See `unpickler.rs::enter_symbols`,
  `unpickler.rs::finish_transaction`, and `docs/tasty-semantic-unpickler.md`.
* Consequently, an ID stored in an error may name an allocation removed by the
  failed transaction. It is safe to preserve, compare, clone, debug-format, or
  render the typed ID itself. It must not be dereferenced through the rolled
  back store, resolver cache, or index. The implemented `category()` and
  `code()` methods are pure matches over the error variant and nested cause;
  they do not look up `SymbolId`/`TypeId`.
* Audit found no payload that stores a reference into a transaction-owned
  temporary buffer. The static reason payloads are static literals; strings
  (`name`, `package`) are owned. `ResolutionError::Malformed.reason` is owned.
  No unstable payload risk was found.

## Existing evidence and scope for follow-up

Focused tests now assert unique codes for every typed reason, representative
results for every category (including nested TASTy/AST causes), and safe code
and category access after a `TypeId` has been rolled back. Existing fixture and
transaction tests remain in place. No corpus expectations were regenerated,
and classification tests do not parse `Display` text.

This implementation adds no renderer, `Diagnostic`, cross-crate error redesign,
or speculative TASTy acceptance change.
