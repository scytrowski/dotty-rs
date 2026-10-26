Warning: truncated output (original token count: 338834)
... 306758 bytes omitted ...

//! Source declaration completion driver.

use std::collections::{HashMap, HashSet};

use dotty_core::ast::{
    ApplyKind, Ident, Modifier, Tree, TreeKind, TypeBoundsTree, TypedAstBuilder, UntypedNode,
};
#[cfg(test)]
use dotty_core::ast::{NumberKind, NumberLiteral};
use dotty_core::types::{
    ClassInfo, MethodKind, MethodParamSpec, MethodType, PolyType, TermRefTarget, Type,
    TypeParamSpec, TypeRefTarget, method_type_from_symbols, poly_type_from_symbols,
};
use dotty_core::{
    AstArena, Definitions, MemberRequest, MemberSelector, MemberSpace, Name, NoResolver, Packages,
    ResolutionError, ScopeId, SemanticStore, SourceContextId, SourceDefinition, SourceId,
    SourceSemanticIndex, SourceSpan, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin,
    SymbolResolver, TreeId, TypeId, Typed, Untyped, Visibility,
};

use crate::{SourceTypeIndex, SourceTypedIndex};

mod application;
pub use application::ConstructorCandidate;
mod completion;
mod context;
mod error;
mod expression;
use expression::LocalMethodIndex;
mod resolution;
use resolution::imports::ImportSelection;
mod source_annotations;
mod source_function;
mod transaction;
mod type_projection;

use application::{
    ApplicationCandidate, ApplicationRequest, InferenceLocation, InfixApplicationRequest,
    ResolvedApplicationFunction, TypedArgument,
};
pub use context::{ExpressionContext, ExpressionScopeId};
use context::{ExpressionScopeFrame, next_expression_scope_owner};
pub use error::{
    ExtractorMethodShapeIssue, ExtractorPatternArgumentIssue, ExtractorProductIssue,
    ExtractorResultMemberIssue, FieldInitializerContextIssue, PatternKind,
    TuplePatternResolutionIssue, TypeArgumentBoundSide, TyperError,
};
pub use source_function::{
    MAX_SOURCE_FUNCTION_ARITY, SourceFunctionFamily, SourceFunctionKind, SourceFunctionPurity,
};

#[path = "../lookup/mod.rs"]
mod lookup;
#[path = "../substitution.rs"]
mod substitution;
#[path = "../types/subtype.rs"]
mod subtype;

pub use lookup::{MAX_MEMBER_LOOKUP_DEPTH, MemberCandidate, MemberLookupError};
pub use subtype::{
    MAX_TYPE_RELATION_DEPTH, MAX_TYPE_RELATION_VIEWS, MAX_UNION_RELATION_COMPARISONS,
    TypeRelationError,
};

/// Why one overload was excluded from an application candidate set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverloadRejection {
    /// This call's `ApplyKind` does not consume the candidate's current clause.
    ApplicationKindMismatch {
        application_kind: ApplyKind,
        method_kind: MethodKind,
    },
    WrongArity {
        expected: usize,
        actual: usize,
    },
    ArgumentNonConformance {
        argument_index: usize,
        actual: TypeId,
        expected: TypeId,
    },
    TypeArgumentInferenceFailure {
        parameter_index: Option<usize>,
    },
    TypeArgumentBoundViolation {
        parameter_index: usize,
        side: TypeArgumentBoundSide,
    },
    IncompleteSignature,
    UnsupportedSemantics,
}

fn application_kind_accepts(application_kind: ApplyKind, method_kind: MethodKind) -> bool {
    match application_kind {
        ApplyKind::Regular => method_kind == MethodKind::Plain,
        ApplyKind::Using => matches!(method_kind, MethodKind::Contextual | MethodKind::Implicit),
    }
}

/// Completes source declaration signatures against a shared semantic session.
pub struct SourceTyper<'a> {
    arena: &'a AstArena<Untyped>,
    source: SourceId,
    index: &'a SourceSemanticIndex,
    store: &'a mut SemanticStore,
    definitions: Definitions,
    packages: &'a Packages,
    resolver: Box<dyn SymbolResolver + 'a>,
    type_index: SourceTypeIndex,
    source_annotations: HashMap<TreeId<Untyped>, dotty_core::AnnotationId>,
    source_type_projection_depth: usize,
    signature_parameter_in_progress: Option<SymbolId>,
    local_symbols: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    inferred_local_lazy_values: HashMap<SymbolId, (TreeId<Untyped>, ExpressionContext)>,
    local_scopes: HashMap<(SourceId, TreeId<Untyped>), ScopeId>,
    patdef_expansions: expression::blocks::PatDefExpansionIndex,
    patdef_typing_attempts: HashMap<(SourceId, TreeId<Untyped>), Result<(), String>>,
    pattern_bindings: PatternBindingIndex,
    local_methods: LocalMethodIndex,
    active_local_type_scopes: Vec<ActiveLocalTypeScope>,
    active_local_import_scopes: Vec<(SourceContextId, Option<ExpressionScopeId>)>,
    active_local_type_binders: Vec<(TypeId, Vec<SymbolId>)>,
    initializing_local_symbols: HashSet<SymbolId>,
    inferred_local_lazy_types_in_progress: HashSet<SymbolId>,
    inferred_field_types_in_progress: HashSet<SymbolId>,
    inferred_method_results_in_progress: HashSet<SymbolId>,
    uninitialized_parent_receiver: Option<SymbolId>,
    import_qualifier_cache: HashMap<ImportQualifierCacheKey, SymbolId>,
    import_member_cache: HashMap<ImportMemberCacheKey, SymbolId>,
    semantic_transaction_depth: usize,
    typed_arena: AstArena<Typed>,
    typed_index: SourceTypedIndex,
    function_literals: expression::FunctionLiteralIndex,
    expression_scopes: Vec<ExpressionScopeFrame>,
    expression_scope_owner: u64,
    synthetic_package_entries: Vec<(ScopeId, SymbolId)>,
    synthetic_class_member_entries: Vec<(ScopeId, SymbolId)>,
}

#[derive(Clone, Copy)]
pub(super) struct ActiveLocalTypeScope {
    context: SourceContextId,
    scope: ScopeId,
    body: Option<SourceSpan>,
}

impl ActiveLocalTypeScope {
    fn exact(context: SourceContextId, scope: ScopeId) -> Self {
        Self {
            context,
            scope,
            body: None,
        }
    }

    fn body(context: SourceContextId, scope: ScopeId, body: SourceSpan) -> Self {
        Self {
            context,
            scope,
            body: Some(body),
        }
    }

    fn applies_to(
        self,
        contexts: &[SourceContextId],
        context: SourceContextId,
        position: Option<SourceSpan>,
    ) -> bool {
        match self.body {
            None => self.context == context,
            Some(body) => {
                contexts.contains(&self.context)
                    && position.is_some_and(|position| {
                        let body_range = body.span().range();
                        let position_range = position.span().range();
                        body.source() == position.source()
                            && body_range.start() <= position_range.start()
                            && position_range.end() <= body_range.end()
                    })
            }
        }
    }
}

/// Canonical identity for one successfully typed ordinary local value.
///
/// The declaration is the exact typed `ValDef` recorded for its source tree;
/// the symbol is the local identity entered by the typer. Consumers should
/// keep this pair together rather than trying to recover the symbol from the
/// declaration's printed name or source position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedLocalBinding {
    declaration: TreeId<Typed>,
    symbol: SymbolId,
    scope: ScopeId,
}

impl TypedLocalBinding {
    /// Typed `ValDef` declaration in this typer's arena.
    pub const fn declaration(self) -> TreeId<Typed> {
        self.declaration
    }

    /// Canonical local symbol entered by this typer.
    pub const fn symbol(self) -> SymbolId {
        self.symbol
    }

    /// Block scope that contains this exact local declaration.
    pub const fn scope(self) -> ScopeId {
        self.scope
    }
}

#[derive(Clone, Copy)]
struct SourceTreeLocation {
    tree_index: u32,
    position: Option<SourceSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ImportQualifierCacheKey {
    import_tree: u32,
    qualifier_tree: u32,
    source_context: u32,
    qualifier_context: u32,
    location_tree: u32,
    position: Option<SourceSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ImportMemberCacheKey {
    qualifier: SymbolId,
    name: Name,
}

#[derive(Clone, Default)]
struct PatternBindingIndex {
    by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    #[allow(dead_code)] // Kept for case-scope provenance checks in pattern entry.
    scope_by_symbol: HashMap<SymbolId, ScopeId>,
}

#[derive(Clone)]
enum MethodClauseSpec {
    Types(Vec<TypeParamSpec>),
    Terms(Vec<MethodParamSpec>, MethodKind),
}

impl<'a> SourceTyper<'a> {
    /// Creates a driver using caller-owned, already-bootstrapped definitions.
    pub fn new(
        arena: &'a AstArena<Untyped>,
        source: SourceId,
        index: &'a SourceSemanticIndex,
        store: &'a mut SemanticStore,
        definitions: Definitions,
        packages: &'a Packages,
    ) -> Self {
        Self {
            arena,
            source,
            index,
            store,
            definitions,
            packages,
            resolver: Box::new(NoResolver),
            type_index: SourceTypeIndex::default(),
            source_annotations: HashMap::new(),
            source_type_projection_depth: 0,
            signature_parameter_in_progress: None,
            local_symbols: HashMap::new(),
            inferred_local_lazy_values: HashMap::new(),
            local_scopes: HashMap::new(),
            patdef_expansions: expression::blocks::PatDefExpansionIndex::default(),
            patdef_typing_attempts: HashMap::new(),
            pattern_bindings: PatternBindingIndex::default(),
            local_methods: LocalMethodIndex::default(),
            active_local_type_scopes: Vec::new(),
            active_local_import_scopes: Vec::new(),
            active_local_type_binders: Vec::new(),
            initializing_local_symbols: HashSet::new(),
            inferred_local_lazy_types_in_progress: HashSet::new(),
            inferred_field_types_in_progress: HashSet::new(),
            inferred_method_results_in_progress: HashSet::new(),
            uninitialized_parent_receiver: None,
            import_qualifier_cache: HashMap::new(),
            import_member_cache: HashMap::new(),
            semantic_transaction_depth: 0,
            typed_arena: AstArena::new(),
            typed_index: SourceTypedIndex::new(),
            function_literals: expression::FunctionLiteralIndex::default(),
            expression_scopes: Vec::new(),
            expression_scope_owner: next_expression_scope_owner(),
            synthetic_package_entries: Vec::new(),
            synthetic_class_member_entries: Vec::new(),
        }
    }

    /// Uses an external semantic resolver after source and session lookup.
    ///
    /// The resolver may materialize canonical symbols in the shared store;
    /// classpath IO remains the resolver adapter's responsibility, outside the
    /// typer. Failed typer transactions restore both store and resolver state.
    pub fn with_resolver(mut self, resolver: Box<dyn SymbolResolver + 'a>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Types one supported source expression into this driver's typed arena.
    /// Repeated typing of a source tree returns its existing typed identity.
    /// Failed attempts roll back typed nodes and semantic type allocations.
    pub fn type_expression(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            self.validate_value_expression(tree, typed)?;
            self.validate_cached_parent_argument_receiver_access(tree, typed)?;
            return Ok(typed);
        }
        let method_type_scope = self.enclosing_method_type_scope(context)?;
        if let Some(scope) = method_type_scope {
            self.active_local_type_scopes.push(scope);
        }
        let result = self.run_expression_transaction(|typer, info_journal, new_mappings| {
            typer.type_value_expression_inner(tree, context, info_journal, new_mappings)
        });
        if method_type_scope.is_some() {
            self.active_local_type_scopes.pop();
        }
        result
    }

    /// Types an expression and checks its widened value type against `expected`.
    /// The returned tree preserves the expression's own semantic type.
    pub fn type_expression_expected(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        expected: TypeId,
    ) -> Result<TreeId<Typed>, TyperError> {
        let method_type_scope = self.enclosing_method_type_scope(context)?;
        if let Some(scope) = method_type_scope {
            self.active_local_type_scopes.push(scope);
        }
        let result = self.run_expression_transaction(|typer, info_journal, new_mappings| {
            typer.type_expression_expected_inner(
                tree,
                context,
                expected,
                info_journal,
                new_mappings,
            )
        });
        if method_type_scope.is_some() {
            self.active_local_type_scopes.pop();
        }
        result
    }

    /// Types one canonical source method definition, including its signature,
    /// parameter declarations, result type, and body, into this driver's typed
    /// arena. The definition must be a named class, trait, or module-class
    /// member recorded by the source semantic index. Local methods continue to
    /// be typed as part of their containing block.
    ///
    /// Failed attempts roll back typed nodes and semantic mutations. A
    /// successful attempt records the source-to-typed mapping for the method
    /// definition and each typed source child.
    pub fn type_method_definition(
        &mut self,
        tree: TreeId<Untyped>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let node = self
            .arena
            .try_get(tree)
            .ok_or(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            })?;
        let TreeKind::DefDef(definition) = &node.kind else {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition",
            });
        };
        if definition.rhs.is_none() {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition without body",
            });
        }
        let has_parameter_default = |definition: &dotty_core::ast::DefDef<Untyped>| {
            definition
                .value_param_clauses
                .iter()
                .flatten()
                .any(|parameter| {
                    self.arena.try_get(*parameter).is_some_and(|node| {
                        matches!(&node.kind, TreeKind::ValDef(parameter) if parameter.rhs.is_some())
                    })
                })
        };
        let rhs_range = definition
            .rhs
            .and_then(|rhs| self.arena.try_get(rhs))
            .and_then(|rhs| rhs.position)
            .map(|position| position.span().range());
        let has_default_in_body = rhs_range.is_some_and(|rhs_range| {
            self.arena.iter().any(|(nested_tree, nested_node)| {
                nested_tree != tree
                    && nested_node.position.is_some_and(|position| {
                        let range = position.span().range();
                        rhs_range.start() <= range.start() && range.end() <= rhs_range.end()
                    })
                    && matches!(&nested_node.kind, TreeKind::DefDef(nested) if has_parameter_default(nested))
            })
        });
        if has_parameter_default(definition) || has_default_in_body {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition containing unsupported default parameter arguments",
            });
        }
        let definition = definition.clone();
        let position = node.position;
        let method = self
            .index
            .symbol_at(self.source, tree)
            .filter(|method| {
                matches!(
                    self.index.definition_of(*method),
                    Some(SourceDefinition::Canonical { source, tree: definition_tree })
                        if source == self.source && definition_tree == tree
                )
            })
            .ok_or(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition without a canonical source symbol",
            })?;
        let symbol = self.store.symbols.get(method);
        if symbol.kind != SymbolKind::Method
            || symbol.flags.contains(SymbolFlags::EXTENSION)
            || !symbol.owner.is_some_and(|owner| {
                self.store.symbols.contains(owner)
                    && matches!(
                        self.store.symbols.get(owner).kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    )
            })
        {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "unsupported source method owner or shape",
            });
        }
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return match &self.typed_arena.get(typed).kind {
                TreeKind::DefDef(_) => Ok(typed),
                _ => Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: tree.index(),
                    kind: "existing source mapping is not a typed method definition",
                }),
            };
        }

        self.run_expression_transaction(|typer, info_journal, new_mappings| {
            typer.type_source_method_definition(
                tree,
                &definition,
                position,
                info_journal,
                new_mappings,
            )
        })
    }

    fn enclosing_method_type_scope(
        &self,
        context: ExpressionContext,
    ) -> Result<Option<ActiveLocalTypeScope>, TyperError> {
        if !self.store.symbols.contains(context.owner)
            || !matches!(
                self.store.symbols.get(context.owner).kind,
                SymbolKind::Method | SymbolKind::Constructor
            )
        {
            return Ok(None);
        }
        let scope = self.method_scope(context.owner)?;
        let has_type_parameters = self
            .store
            .scopes
            .get(scope)
            .entered_symbols()
            .any(|symbol| self.store.symbols.get(symbol).kind == SymbolKind::TypeParameter);
        if !has_type_parameters {
            return Ok(None);
        }
        let definition = self.local_methods.definition(context.owner).or_else(|| {
            match self.index.definition_of(context.owner) {
                Some(SourceDefinition::Canonical { source, tree }) if source == self.source => {
                    Some((source, tree))
                }
                _ => None,
            }
        });
        let Some((source, method_tree)) = definition else {
            return Ok(None);
        };
        let Some(TreeKind::DefDef(definition)) =
            self.arena.try_get(method_tree).map(|node| &node.kind)
        else {
            return Ok(None);
        };
        let Some(body) = definition
            .rhs
            .and_then(|rhs| self.arena.try_get(rhs))
            .and_then(|node| node.position)
        else {
            return Ok(None);
        };
        if source != self.source {
            return Ok(None);
        }
        Ok(Some(ActiveLocalTypeScope::body(
            context.lexical,
            scope,
            body,
        )))
    }

    /// Returns the value type represented by a typed expression's own type.
    ///
    /// Literal expressions preserve their exact `Type::Constant` type in the
    /// typed AST; consumers use this operation when they need the canonical
    /// underlying value type. A member reference is adapted to its receiver view.
    /// Adapting a reference may complete source symbols and allocate semantic
    /// types; failures roll those changes back.
    pub fn widen_expression_type(&mut self, ty: TypeId) -> Result<TypeId, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let package_entry_checkpoint = self.synthetic_package_entries.len();
        let class_member_entry_checkpoint = self.synthetic_class_member_entries.len();
        let type_index_checkpoint = self.type_index.checkpoint();
        let inferred_field_types_checkpoint = self.inferred_field_types_in_progress.clone();
        let mut info_journal = Vec::new();
        let result = self.widen_expression_type_journaled(ty, &mut info_journal, 0);
        if result.is_err() {
            for (symbol, previous) in info_journal.into_iter().rev() {
                if self.store.symbols.contains(symbol) {
                    self.store.symbols.set_info(symbol, previous);
                }
            }
            self.rollback_synthetic_package_entries(package_entry_checkpoint);
            self.rollback_synthetic_class_member_entries(class_member_entry_checkpoint);
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(store_checkpoint);
            self.type_index.restore(type_index_checkpoint);
            self.inferred_field_types_in_progress = inferred_field_types_checkpoint;
        }
        result
    }

    fn type_expression_inner(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            self.validate_value_expression(tree, typed)?;
            self.validate_cached_parent_argument_receiver_access(tree, typed)?;
            return Ok(typed);
        }
        let Some(source_tree) = self.arena.try_get(tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        if matches!(source_tree.kind, TreeKind::Apply(_)) {
            self.prepare_raw_generic_constructor_chain(tree, context, info_journal, new_mappings)?;
        }
        let typed = match source_tree.kind {
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => self
                .type_parenthesized_expression(parens.inner, context, info_journal, new_mappings),
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => self.type_infix_expression(
                tree,
                infix,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) => self.type_prefix_expression(
                tree,
                prefix,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::New(new) => self.type_new_expression(
                tree,
                new,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::PhaseSpecific(UntypedNode::Function(function)) => self.type_function_literal(
                tree,
                function,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Literal(literal) => {
                self.type_literal_expression(tree, literal, source_tree.position)
            }
            TreeKind::PhaseSpecific(UntypedNode::Number(number)) => {
                self.type_number_literal_expression(tree, number, source_tree.position)
            }
            TreeKind::This(this) => {
                self.type_this_expression(tree, this, source_tree.position, context)
            }
            TreeKind::Super(super_tree) => self.type_super_expression(
                tree,
                super_tree,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Typed(ascription) => self.type_ascription_expression(
                tree,
                ascription,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Annotated(annotated) => self.type_annotated_expression(
                tree,
                annotated,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::If(if_expr) => self.type_if_expression(
                tree,
                if_expr,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Match(matched) => self.type_match_expression(
                tree,
                matched,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::While(while_expr) => self.type_while_expression(
                tree,
                while_expr,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Return(return_expr) => self.type_return_expression(
                tree,
                return_expr,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Assign(assignment) => self.type_assignment_expression(
                tree,
                assignment,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Ident(ident) => self.type_identifier_expression(
                tree,
                ident,
                source_tree.position,
                context,
                info_journal,
            ),
            TreeKind::Apply(application) => self.type_application(
                tree,
                application,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::TypeApply(application) => self.type_type_application(
                tree,
                application,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::ValDef(definition) => self.type_local_value(
                tree,
                &definition,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Block(block) => self.type_block_expression(
                tree,
                block,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            TreeKind::Select(selection) => self.type_selection_expression(
                tree,
                selection,
                source_tree.position,
                context,
                info_journal,
                new_mappings,
            ),
            _ => Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: tree.index(),
                expression_kind: tree_kind_name(&source_tree.kind),
            }),
        }?;
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(typed)
    }

    /// Returns this driver's source type cache.
    pub fn source_type_index(&self) -> &SourceTypeIndex {
        &self.type_index
    }

    /// Exposes the shared semantic store after the driver is no longer needed.
    pub fn store(&self) -> &SemanticStore {
        self.store
    }

    /// Typed expression trees produced by this typer instance.
    pub fn typed_ast(&self) -> &AstArena<Typed> {
        &self.typed_arena
    }

    /// Source-to-typed mappings produced by this typer instance.
    pub fn source_typed_index(&self) -> &SourceTypedIndex {
        &self.typed_index
    }

    /// Returns the independent outcome of attempting to type one source
    /// pattern definition. This observation intentionally survives rollback
    /// of a containing expression transaction, unlike `source_typed_index`.
    pub fn patdef_typing_attempt_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<&Result<(), String>> {
        self.patdef_typing_attempts.get(&(source, tree))
    }

    /// The typer-owned identities assigned to successfully typed block locals.
    pub fn local_symbol_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<SymbolId> {
        self.local_symbols.get(&(source, tree)).copied()
    }

    /// Returns the exact typed declaration and canonical symbol for a
    /// successfully typed ordinary local `val` or `var`.
    ///
    /// The typed identity comes from [`SourceTypedIndex`], while the symbol
    /// comes from the typer's local declaration index. Unsupported or failed
    /// local declarations have no binding. Both indexes participate in the
    /// same expression transaction and are rolled back together.
    pub fn typed_local_binding_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<TypedLocalBinding> {
        let declaration = self.typed_index.get(source, tree)?;
        if !matches!(
            self.typed_arena.try_get(declaration)?.kind,
            TreeKind::ValDef(_)
        ) {
            return None;
        }
        let symbol = self.local_symbol_at(source, tree)?;
        let scope = *self.local_scopes.get(&(source, tree))?;
        if !self
            .store
            .symbols
            .get(symbol)
            .flags
            .intersection(
                SymbolFlags::LAZY
                    .union(SymbolFlags::GIVEN)
                    .union(SymbolFlags::IMPLICIT)
                    .union(SymbolFlags::ERASED),
            )
            .is_empty()
        {
            return None;
        }
        Some(TypedLocalBinding {
            declaration,
            symbol,
            scope,
        })
    }

    /// Returns the typer-owned symbol introduced by a source pattern binding.
    pub fn pattern_binding_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.pattern_bindings.by_tree.get(&(source, tree)).copied()
    }

    #[allow(dead_code)] // Tests and later pattern typing inspect case ownership.
    pub(super) fn pattern_binding_scope(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.pattern_bindings.scope_by_symbol.get(&symbol).copied()
    }

    /// Returns the typer-owned method identity entered for a local `DefDef`.
    pub fn local_method_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.local_methods.symbol_at(source, tree)
    }

    /// Returns the source declaration recorded for a typer-owned local method.
    pub fn local_method_definition(&self, method: SymbolId) -> Option<(SourceId, TreeId<Untyped>)> {
        self.local_methods.definition(method)
    }

    /// Returns the method-owned scope reserved for a local method's parameters.
    pub fn local_method_scope(&self, method: SymbolId) -> Option<ScopeId> {
        self.local_methods.scope(method)
    }

    /// Returns the enclosing block context captured when a local method was entered.
    pub fn local_method_declaration_context(&self, method: SymbolId) -> Option<ExpressionContext> {
        self.local_methods.declaration_context(method)
    }

    /// Returns the typer-owned parameter identity for a local method parameter.
    pub fn local_method_parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.local_methods.parameter_symbol_at(source, tree)
    }

    /// Returns the typer-owned method identity assigned to a source function literal.
    pub fn function_literal_method_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.function_literals.method_at(source, tree)
    }

    /// Returns the method-owned scope for a successfully typed function literal.
    pub fn function_literal_method_scope(&self, method: SymbolId) -> Option<ScopeId> {
        self.function_literals.scope_of(method)
    }

    /// Returns the separate typed definition referenced by a source closure.
    pub fn function_literal_definition_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<TreeId<Typed>> {
        self.function_literals.definition_at(source, tree)
    }

    /// Returns the typer-owned parameter identity for a function-literal parameter.
    pub fn function_literal_parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.function_literals.parameter_at(source, tree)
    }

    /// Returns the typer-owned type parameter symbol for a local method tree.
    pub fn local_method_type_parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.local_methods.type_parameter_symbol_at(source, tree)
    }

    pub(super) fn extension_prefix_clauses(
        &self,
        method: SymbolId,
    ) -> Option<&[Vec<TreeId<Untyped>>]> {
        self.index
            .extension_prefix_clauses(method)
            .or_else(|| self.local_methods.extension_prefix_clauses(method))
    }

    /// Exposes the package registry used by this driver.
    pub fn packages(&self) -> &Packages {
        self.packages
    }
}

fn insert_scope(scopes: &mut Vec<dotty_core::ScopeId>, scope: dotty_core::ScopeId) {
    if !scopes.contains(&scope) {
        scopes.push(scope);
    }
}

pub(in crate::typer) fn tree_kind_name(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "identifier",
        TreeKind::TypeTree(_) => "type tree",
        TreeKind::AppliedTypeTree(_) => "applied type tree",
        TreeKind::SingletonTypeTree(_) => "singleton type tree",
        TreeKind::RefinedTypeTree(_) => "refined type tree",
        TreeKind::LambdaTypeTree(_) => "lambda type tree",
        TreeKind::MatchTypeTree(_) => "match type tree",
        TreeKind::ByNameTypeTree(_) => "by-name type tree",
        TreeKind::TypeBoundsTree(_) => "type bounds tree",
        _ => "expression or declaration tree",
    }
}

pub(in crate::typer) fn local_block_declaration_kind(
    kind: &TreeKind<Untyped>,
) -> Option<&'static str> {
    match kind {
        TreeKind::ValDef(_) => Some("val/var definition"),
        TreeKind::DefDef(_) => Some("method definition"),
        TreeKind::TypeDef(_) => Some("type definition"),
        TreeKind::PackageDef(_) => Some("package definition"),
        TreeKind::Import(_) => Some("import"),
        TreeKind::Export(_) => Some("export"),
        TreeKind::PhaseSpecific(UntypedNode::PatDef(_)) => Some("pattern definition"),
        TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(_)) => Some("extension methods"),
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => Some("module definition"),
        _ => None,
    }
}

pub(in crate::typer) fn source_method_flags(modifiers: &[Modifier]) -> SymbolFlags {
    modifiers
        .iter()
        .fold(SymbolFlags::EMPTY, |flags, modifier| {
            let flag = match modifier {
                Modifier::Abstract
                | Modifier::Final
                | Modifier::Implicit
                | Modifier::Given
                | Modifier::Override
                | Modifier::Inline
                | Modifier::Transparent
                | Modifier::Extension
                | Modifier::Erased => dotty_core::source_modifier_flag(*modifier),
                _ => SymbolFlags::EMPTY,
            };
            flags | flag
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_classloader::classloader::{
        ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
    };
    use dotty_core::{
        Name, Namespace, SourceText, SymbolFlags, SymbolLinks, SymbolOrigin, TermName, Visibility,
        ast::VisibilitySyntax,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Copy)]
    struct JavaObjectStubPath;

    #[derive(Clone, Copy)]
    struct ContextsPackagePath;

    #[derive(Clone, Copy)]
    struct TypesPackagePath;

    struct ProfiledCompilerResolver {
        inner: dotty_classloader::classloader::ClasspathSymbolResolver<
            dotty_classloader::classloader::CompositeClassPath,
        >,
        profile: Rc<RefCell<dotty_classloader::classloader::ModuleResolutionProfile>>,
    }

    impl SymbolResolver for ProfiledCompilerResolver {
        fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
            self.inner.checkpoint()
        }

        fn rollback_to(
            &mut self,
            store: &mut SemanticStore,
            checkpoint: dotty_core::ResolverCheckpoint,
        ) {
            self.inner.rollback_to(store, checkpoint);
        }

        fn module_class_of_object(
            &self,
            store: &SemanticStore,
            object: SymbolId,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.inner.module_class_of_object(store, object)
        }

        fn complete_module_class(
            &mut self,
            store: &mut SemanticStore,
            module_class: SymbolId,
        ) -> Result<bool, ResolutionError> {
            self.inner.complete_module_class(store, module_class)
        }

        fn complete_class(
            &mut self,
            store: &mut SemanticStore,
            symbol: SymbolId,
        ) -> Result<bool, ResolutionError> {
            self.inner.complete_class(store, symbol)
        }

        fn resolve_member(
            &mut self,
            store: &mut SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            let result = self.inner.resolve_member(store, request);
            let observed = self.inner.take_module_resolution_profile();
            let mut profile = self.profile.borrow_mut();
            profile.package_term_requests += observed.package_term_requests;
            profile.package_term_successes += observed.package_term_successes;
            profile.package_term_unresolved += observed.package_term_unresolved;
            profile.package_term_non_object_results += observed.package_term_non_object_results;
            profile.package_term_errors += observed.package_term_errors;
            profile.new_object_identities += observed.new_object_identities;
            profile.reused_object_identities += observed.reused_object_identities;
            profile.module_member_successes += observed.module_member_successes;
            profile
                .resolved_object_targets
                .extend(observed.resolved_object_targets);
            profile
                .resolved_member_targets
                .extend(observed.resolved_member_targets);
            profile
                .package_term_targets
                .extend(observed.package_term_targets);
            profile.module_targets.extend(observed.module_targets);
            profile.member_targets.extend(observed.member_targets);
            profile
                .completed_module_classes
                .extend(observed.completed_module_classes);
            result
        }

        fn resolve_package(
            &mut self,
            store: &mut SemanticStore,
            path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.inner.resolve_package(store, path)
        }
    }

    impl ClassPathEntry for JavaObjectStubPath {
        fn find_class(
            &self,
            name: &dotty_classloader::classloader::BinaryName,
        ) -> Result<Option<ClassResource>, ClassPathError> {
            if name.as_internal() != "java/lang/Object" {
                return Ok(None);
            }
            let name = b"java/lang/Object";
            let mut bytes = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61, 0, 3, 1];
            bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
            bytes.extend_from_slice(name);
            bytes.extend_from_slice(&[7, 0, 1, 0, 0x21, 0, 2, 0, 0]);
            bytes.extend_from_slice(&[0; 8]);
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(std::path::PathBuf::from("<java.lang.Object stub>")),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"])
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(["java", "lang"].starts_with(package))
        }
    }

    impl ClassPathEntry for ContextsPackagePath {
        fn find_class(
            &self,
            _name: &dotty_classloader::classloader::BinaryName,
        ) -> Result<Option<ClassResource>, ClassPathError> {
            Ok(None)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["dotty", "tools", "dotc", "core", "Contexts"])
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(["dotty", "tools", "dotc", "core", "Contexts"].starts_with(package))
        }
    }

    impl ClassPathEntry for TypesPackagePath {
        fn find_class(
            &self,
            _name: &dotty_classloader::classloader::BinaryName,
        ) -> Result<Option<ClassResource>, ClassPathError> {
            Ok(None)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["dotty", "tools", "dotc", "core", "Types"])
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(["dotty", "tools", "dotc", "core", "Types"].starts_with(package))
        }
    }

    const LOCAL_EXTENSION_CALL_SOURCE: &str =
        include_str!("../../tests/fixtures/local-extension-calls/LocalExtensionCalls.scala");
    const LOCAL_LAZY_RECURSION_ORACLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/local-lazy-values/Recursive.scala");
    const LOCAL_LAZY_EXPLICIT_SELF_ORACLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/local-lazy-values/ExplicitSelf.scala");
    const LOCAL_LAZY_FORWARD_ORACLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/local-lazy-values/Forward.scala");
    const CLASS_LAZY_FIELDS_ORACLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/class-lazy-fields/ClassLazyFields.scala");

    struct ScriptedResolver {
        member: Option<SymbolId>,
        package: Option<SymbolId>,
        member_requests: Rc<RefCell<Vec<dotty_core::Name>>>,
        package_requests: Rc<RefCell<Vec<Vec<String>>>>,
    }

    #[derive(Default)]
    struct JournalingResolver {
        member: Option<SymbolId>,
        journal: Vec<Option<SymbolId>>,
    }

    impl SymbolResolver for JournalingResolver {
        fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
            dotty_core::ResolverCheckpoint::new(self.journal.len() as u64)
        }

        fn rollback_to(
            &mut self,
            _store: &mut SemanticStore,
            checkpoint: dotty_core::ResolverCheckpoint,
        ) {
            while self.journal.len() > checkpoint.token() as usize {
                self.member = self.journal.pop().unwrap_or(None);
            }
        }

        fn resolve_member(
            &mut self,
            store: &mut SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            if let Some(member) = self.member.filter(|member| store.symbols.contains(*member)) {
                return Ok(Some(member));
            }
            let member = store.symbols.alloc(dotty_core::Symbol {
                name: request.name,
                owner: None,
                kind: SymbolKind::Field,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: dotty_core::SymbolLinks::default(),
            });
            self.journal.push(self.member.replace(member));
            Ok(Some(member))
        }

        fn resolve_package(
            &mut self,
            _store: &mut SemanticStore,
            _path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            Ok(None)
        }
    }

    impl SymbolResolver for ScriptedResolver {
        fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
            dotty_core::ResolverCheckpoint::new(0)
        }

        fn rollback_to(
            &mut self,
            _store: &mut SemanticStore,
            _checkpoint: dotty_core::ResolverCheckpoint,
        ) {
        }

        fn resolve_member(
            &mut self,
            _store: &mut SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.member_requests.borrow_mut().push(request.name);
            Ok(self.member)
        }

        fn resolve_package(
            &mut self,
            _store: &mut SemanticStore,
            path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.package_requests
                .borrow_mut()
                .push(path.iter().map(|segment| (*segment).to_owned()).collect());
            Ok(self.package)
        }
    }

    fn setup() -> (AstArena<Untyped>, SemanticStore, Packages, Definitions) {
        let arena = AstArena::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        (arena, store, Packages::new(), definitions)
    }

    fn symbol(store: &mut SemanticStore, kind: SymbolKind, info: SymbolInfo) -> SymbolId {
        let name = store.names.intern("member");
        store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(name, Namespace::Term),
            owner: None,
            kind,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn parse_and_name(
        text: &str,
    ) -> (
        dotty_parser::ParseResult,
        SemanticStore,
        Packages,
        Definitions,
        SourceSemanticIndex,
        SourceId,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(11);
        let mut packages = Packages::new();
        let (parsed, index) = parse_and_name_unit(text, source, &mut store, &mut packages);
        (parsed, store, packages, definitions, index, source)
    }

    fn parse_capture_checked_and_name(
        text: &str,
    ) -> (
        dotty_parser::ParseResult,
        SemanticStore,
        Packages,
        Definitions,
        SourceSemanticIndex,
        SourceId,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(12);
        let mut packages = Packages::new();
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::Parser::new(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        )
        .with_features(dotty_parser::ParserFeatures {
            capture_checking: true,
            ..dotty_parser::ParserFeatures::default()
        })
        .source_compilation_unit();
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "TyperCaptureChecking.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        (parsed, store, packages, definitions, index, source)
    }

    fn publish_empty_class_info(store: &mut SemanticStore, class_type: TypeId, no_prefix: TypeId) {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) = store.types.try_get(class_type)
        else {
            panic!("class type should refer to a symbol");
        };
        let class = *class;
        let declarations = store.scopes.alloc(dotty_core::Scope::new(Some(class)));
        let info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: no_prefix,
            class,
            parents: Vec::new(),
            declarations,
            self_type: None,
        }));
        store.symbols.set_info(class, SymbolInfo::Complete(info));
    }

    fn parse_and_name_unit(
        text: &str,
        source: SourceId,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> (dotty_parser::ParseResult, SourceSemanticIndex) {
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Typer.scala",
            store,
            packages,
        )
        .unwrap();
        (parsed, index)
    }

    fn val_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::ValDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (index.symbol_at(source, tree).unwrap(), definition.tpt);
            }
        }
        panic!("source val `{target}` not found");
    }

    fn val_definition_and_rhs(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::ValDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (
                    index.symbol_at(source, tree).unwrap(),
                    tree,
                    definition.rhs.expect("test val should have an rhs"),
                );
            }
        }
        panic!("source val `{target}` not found");
    }

    fn method_definition_and_rhs(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::DefDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.expect("test method should have an rhs"),
                );
            }
        }
        panic!("source method `{target}` not found");
    }

    fn method_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> SymbolId {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::DefDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return index.symbol_at(source, tree).unwrap();
            }
        }
        panic!("source method `{target}` not found");
    }

    fn preindex_block_for_test(
        typer: &mut SourceTyper<'_>,
        block_tree: TreeId<Untyped>,
        parent_context: ExpressionContext,
    ) -> (ScopeId, ExpressionContext) {
        let TreeKind::Block(block) = &typer.arena.get(block_tree).kind else {
            panic!("source tree should be a block")
        };
        let stats = block.stats.clone();
        let scope = typer
            .store
            .scopes
            .alloc(dotty_core::Scope::new(Some(parent_context.owner)));
        let context = typer.push_local_scope(parent_context, scope).unwrap();
        if let Some(frame) = context
            .local_scopes
            .and_then(|stack| typer.expression_scopes.get_mut(stack.index()))
        {
            frame.is_block_scope = true;
        }
        typer
            .preindex_local_methods(&stats, context, scope)
            .unwrap();
        (scope, context)
    }

    fn method_parameter_context(
        parsed: &dotty_parser::ParseResult,
        index: &SourceSemanticIndex,
        source: SourceId,
        method: SymbolId,
        parameter_index: usize,
    ) -> SourceContextId {
        let parameter = method_parameter_symbol(parsed, index, source, method, parameter_index);
        index.declaration_context_of(parameter).unwrap()
    }

    fn method_parameter_symbol(
        parsed: &dotty_parser::ParseResult,
        index: &SourceSemanticIndex,
        source: SourceId,
        method: SymbolId,
        parameter_index: usize,
    ) -> SymbolId {
        for (tree, node) in parsed.ast.iter() {
            let TreeKind::DefDef(definition) = &node.kind else {
                continue;
            };
            if index.symbol_at(source, tree) != Some(method) {
                continue;
            }
            let parameter_tree = definition.value_param_clauses[0][parameter_index];
            return index.symbol_at(source, parameter_tree).unwrap();
        }
        panic!("method parameter was not found");
    }

    fn type_value_rhs(source_text: &str) -> (dotty_core::Constant, Type, TypeId, Definitions) {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Literal(literal) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed literal node")
        };
        let value = literal.value.clone();
        let own_type = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed).ty)
            .clone();
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        (value, own_type, widened, definitions)
    }

    fn type_synthetic_literal(
        value: dotty_core::Constant,
    ) -> (dotty_core::Constant, Type, TypeId, Definitions) {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 0 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        parsed.ast.get_mut(rhs).kind = TreeKind::Literal(dotty_core::ast::Literal {
            value: value.clone(),
        });
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Literal(literal) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed literal node")
        };
        let literal_value = literal.value.clone();
        let own_type = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed).ty)
            .clone();
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        (literal_value, own_type, widened, definitions)
    }

    fn type_method_rhs(
        source_text: &str,
        method_name: &str,
    ) -> Result<(TreeKind<Typed>, Type, TypeId, Type, Definitions), TyperError> {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method_name);
        let parameter_context = parsed.ast.iter().find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            if store.names.resolve(definition.name.as_name().text()) == method_name {
                return None;
            }
            let symbol = index.symbol_at(source, tree)?;
            (store.symbols.get(symbol).kind == SymbolKind::Parameter
                && store.symbols.get(symbol).owner == Some(method))
            .then(|| index.declaration_context_of(symbol))
            .flatten()
        });
        let context = ExpressionContext {
            lexical: parameter_context.unwrap_or_else(|| {
                index
                    .declaration_context_of(method)
             …232152 tokens truncated…unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationArityMismatch {
                expected: 1,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn polymorphic_method_application_infers_from_argument() {
        let source_text = "class C { def id[A](x: A): A = x; def use: Int = id(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let int = definitions.int;
        let identity = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "id" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(typer.typed_ast().get(typed).ty, int);
        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Apply(_)
        ));
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            unreachable!()
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == identity
        ));
        let SymbolInfo::Complete(poly_id) = typer.store().symbols.get(identity).info else {
            panic!("the original polymorphic method should remain complete")
        };
        assert!(matches!(typer.store().types.get(poly_id), Type::Poly(_)));
        assert!(
            typer
                .type_contains_param_ref(poly_result(typer.store(), poly_id), poly_id)
                .unwrap()
        );
    }

    fn poly_result(store: &SemanticStore, poly: TypeId) -> TypeId {
        match store.types.get(poly) {
            Type::Poly(poly) => poly.result,
            _ => panic!("expected original method info to remain polymorphic"),
        }
    }

    #[test]
    fn polymorphic_application_infers_multiple_parameters_by_binder_index() {
        let source_text = "class C { def second[A, B](a: A, b: B): B = b; def use(a: Int, b: Boolean): Boolean = second(a, b) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let boolean = definitions.boolean;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(typer.typed_ast().get(typed).ty, boolean);
    }

    #[test]
    fn polymorphic_application_infers_nested_applied_type_arguments() {
        let source_text = "class Text; class Box[A]; class Use { def unbox[A](box: Box[A]): A = ???; def use(box: Box[Text]): Text = unbox(box) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == text
        ));
    }

    #[test]
    fn polymorphic_application_rejects_conflicting_constraints_without_lub() {
        let source_text = "class C { def pair[A](x: A, y: A): A = x; def use(x: Int, y: Boolean): Any = pair(x, y) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConflictingInferenceConstraints {
                parameter_index: 0,
                ..
            })
        ));
    }

    #[test]
    fn polymorphic_application_accepts_equivalent_repeated_constraints() {
        let source_text = "class C { def pair[A](x: A, y: A): A = x; def use: Int = pair(1, 2) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let int = definitions.int;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(typer.typed_ast().get(typed).ty, int);
    }

    #[test]
    fn polymorphic_application_still_checks_concrete_formal_fragments() {
        let source_text = "class C { def accept[A](value: A, count: Int): A = value; def use: Any = accept(1, true) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let int = definitions.int;
        let boolean = definitions.boolean;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 1,
                actual,
                expected,
                ..
            }) if actual == boolean && expected == int
        ));
    }

    #[test]
    fn polymorphic_application_reports_unconstrained_parameters() {
        let source_text = "class C { def make[A](): A = ???; def use: Any = make() }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnconstrainedTypeParameter {
                parameter_index: 0,
                ..
            })
        ));
    }

    #[test]
    fn polymorphic_application_checks_inferred_type_argument_bounds() {
        let source_text = "class Base; class Other; class Use { def accept[T <: Base](value: T): T = value; def use(value: Other): Other = accept(value) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let store_before = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::InferredTypeArgumentBoundViolation {
                parameter_index: 0,
                side: TypeArgumentBoundSide::Upper,
                ..
            })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.store().checkpoint(), store_before);
    }

    #[test]
    fn regular_application_rejects_a_contextual_clause() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationMethodKindMismatch {
                application_kind: ApplyKind::Regular,
                method_kind: MethodKind::Contextual,
                ..
            })
        ));
    }

    #[test]
    fn contextual_function_projection_does_not_insert_a_contextual_application() {
        let source_text = "class C { def use(function: Int ?=> Int): Int = function(1) }";
        let (parsed, mut store, mut packages, definitions, index, source) =
            parse_and_name(source_text);
        enter_scala_function_classes(&mut store, &mut packages, [1]);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let context = typer.expression_context_for(method).unwrap();
        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(&result, Err(TyperError::ApplicationCalleeNotMethod { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn regular_application_rejects_an_implicit_clause() {
        let source_text = "class C { def f(implicit x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationMethodKindMismatch {
                application_kind: ApplyKind::Regular,
                method_kind: MethodKind::Implicit,
                ..
            })
        ));
    }

    #[test]
    fn explicit_using_application_types_arguments_and_preserves_its_kind() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed using application")
        };
        assert_eq!(application.kind, ApplyKind::Using);
        assert_eq!(application.args.len(), 1);
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn explicit_using_application_types_a_contextual_constructor_clause() {
        let source_text = "class C(using x: Int); class Use { def make: C = new C(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed contextual constructor application")
        };
        assert_eq!(application.kind, ApplyKind::Using);
        assert_eq!(application.args.len(), 1);
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("expected the constructor selection")
        };
        assert!(matches!(
            typer.typed_ast().get(selection.qualifier).kind,
            TreeKind::New(_)
        ));
    }

    #[test]
    fn explicit_using_application_types_multiple_arguments() {
        let source_text = "class C { def f(using x: Int, flag: Boolean): Int = x; def use: Int = f(using 1, true) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed using application")
        };
        assert_eq!(application.kind, ApplyKind::Using);
        assert_eq!(application.args.len(), 2);
    }

    #[test]
    fn explicit_using_application_checks_arity() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(using 1, 2) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationArityMismatch {
                expected: 1,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn generic_plain_then_using_application_preserves_both_clauses() {
        let source_text = "class C { class Ctx[A]; def f[A](x: A)(using ctx: Ctx[A]): A = x; def use(using ctx: Ctx[Int]): Int = f(1)(using ctx) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(using_application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected the outer using application")
        };
        assert_eq!(using_application.kind, ApplyKind::Using);
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let TreeKind::Apply(regular_application) =
            &typer.typed_ast().get(using_application.function).kind
        else {
            panic!("expected the inner regular application")
        };
        assert_eq!(regular_application.kind, ApplyKind::Regular);
    }

    #[test]
    fn generic_explicit_using_infers_from_its_current_contextual_clause() {
        let source_text = "class C { class Ctx[A]; def f[A](using ctx: Ctx[A]): Ctx[A] = ctx; def use(using ctx: Ctx[Int]): Ctx[Int] = f(using ctx) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed generic using application")
        };
        assert_eq!(application.kind, ApplyKind::Using);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::Applied { args, .. } if args == &[definitions.int]
        ));
    }

    #[test]
    fn generic_contextual_overload_infers_from_the_current_using_clause() {
        let source_text = "class C { class Ctx[A]; def f[A](using ctx: Ctx[A]): Ctx[A] = ctx; def f(using ctx: Ctx[Boolean]): Ctx[Boolean] = ctx; def use(using ctx: Ctx[Int]): Ctx[Int] = f(using ctx) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let method_name = Name::new(store.names.intern("f"), Namespace::Term);
        let methods = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&method_name)
            .to_vec();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let generic_method = methods
            .into_iter()
            .find(|symbol| matches!(typer.complete_symbol(*symbol), Ok(callable) if matches!(typer.store().types.get(callable), Type::Poly(_))))
            .unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed generic using overload application")
        };
        assert_eq!(application.kind, ApplyKind::Using);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::Applied { args, .. } if args == &[definitions.int]
        ));
        let function = typer.typed_ast().get(application.function);
        assert!(matches!(
            typer.store().types.get(function.ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == generic_method
        ));
    }

    #[test]
    fn generic_explicit_using_infers_from_a_legacy_implicit_clause() {
        let source_text = "class C { class Ctx[A]; def f[A](implicit ctx: Ctx[A]): Ctx[A] = ctx; def use(implicit ctx: Ctx[Int]): Ctx[Int] = f(using ctx) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: method_parameter_context(&parsed, &index, source, method, 0),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed generic using application")
        };
        assert_eq!(application.kind, ApplyKind::Using);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::Applied { args, .. } if args == &[definitions.int]
        ));
    }

    #[test]
    fn overload_resolution_selects_by_application_clause_kind() {
        let source_text = "class C { def f(x: Int): Int = x; def f(using x: Int): Int = x; def plain: Int = f(1); def contextual: Int = f(using 2) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let method_name = Name::new(store.names.intern("f"), Namespace::Term);
        let methods = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&method_name)
            .to_vec();
        let (plain_method, plain_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "plain");
        let (using_method, using_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "contextual");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let plain_context = typer.expression_context_for(plain_method).unwrap();
        let using_context = typer.expression_context_for(using_method).unwrap();
        let mut expected_plain = None;
        let mut expected_using = None;
        for method in methods {
            let callable = typer.complete_symbol(method).unwrap();
            let Some(Type::Method(signature)) = typer.store().types.try_get(callable) else {
                panic!("expected monomorphic overload signatures")
            };
            match signature.kind {
                MethodKind::Plain => expected_plain = Some(method),
                MethodKind::Contextual => expected_using = Some(method),
                MethodKind::Implicit => panic!("unexpected implicit overload"),
            }
        }

        let plain_typed = typer.type_expression(plain_rhs, plain_context).unwrap();
        let using_typed = typer.type_expression(using_rhs, using_context).unwrap();

        for (typed, expected) in [
            (plain_typed, expected_plain.unwrap()),
            (using_typed, expected_using.unwrap()),
        ] {
            let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
                panic!("expected a typed overloaded application")
            };
            let function = typer.typed_ast().get(application.function);
            assert!(matches!(
                typer.store().types.get(function.ty),
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } if *symbol == expected
            ));
        }
        assert!(matches!(
            typer.typed_ast().get(using_typed).kind,
            TreeKind::Apply(ref application) if application.kind == ApplyKind::Using
        ));
    }

    #[test]
    fn equally_applicable_using_overloads_remain_ambiguous_and_atomic() {
        let source_text = "class C { def f(using x: Int): Int = x; def f(using y: Int): Int = y; def use: Int = f(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let store_checkpoint = typer.store().checkpoint();
        let typed_tree_count = typer.typed_ast().iter().count();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::AmbiguousOverloadApplication { candidates, .. })
                if candidates.len() == 2
        ));
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), typed_tree_count);
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn explicit_using_application_consumes_legacy_implicit_clause() {
        let source_text = "class C { def f(implicit x: Int): Int = x; def use: Int = f(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Apply(ref application) if application.kind == ApplyKind::Using
        ));
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn using_application_rejects_a_plain_clause() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationMethodKindMismatch {
                application_kind: ApplyKind::Using,
                method_kind: MethodKind::Plain,
                ..
            })
        ));
    }

    #[test]
    fn explicit_using_application_reports_argument_type_mismatch_atomically() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(using true) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 0,
                actual,
                expected,
                ..
            }) if actual == definitions.boolean && expected == definitions.int
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn local_by_name_application_uses_result_type_and_preserves_signature() {
        let source_text = "class C { def outer: Int = { def use(prefix: Int, value: => Int): Int = value; use(4, 2) } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (outer, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("outer body should be a block")
        };
        let local_tree = source_block.stats[0];
        let TreeKind::Apply(source_application) = &parsed.ast.get(source_block.expr).kind else {
            panic!("block result should be the local call")
        };
        let source_argument = source_application.args[1];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("outer body should remain a typed block")
        };
        let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed_block.expr).kind
        else {
            panic!("local call should remain a typed Apply")
        };
        assert_eq!(typed_application.kind, ApplyKind::Regular);
        assert_eq!(typed_application.args.len(), 2);
        let typed_function = typed_application.function;
        let typed_arguments = typed_application.args.clone();
        assert_eq!(
            typer.source_typed_index().get(source, source_argument),
            Some(typed_arguments[1]),
            "typed Apply should retain the exact typed source argument"
        );
        let local = typer
            .local_method_symbol_at(source, local_tree)
            .expect("local method should have been preindexed");
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == local
        ));
        let signature = typer.complete_symbol(local).unwrap();
        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("local method should have a method signature")
        };
        assert_eq!(method_type.params.len(), 2);
        assert_eq!(method_type.params[0].ty, definitions.int);
        let Type::ByName { result } = typer.store().types.get(method_type.params[1].ty) else {
            panic!("method signature should retain its by-name formal")
        };
        assert_eq!(*result, definitions.int);
        assert_eq!(
            typer.typed_ast().get(typed_arguments[1]).position,
            parsed.ast.get(source_argument).position
        );
    }

    #[test]
    fn by_name_application_accepts_nontrivial_and_nonlocal_arguments() {
        for source_text in [
            "class C { def outer: Int = { def use(value: => Int): Int = value; use(if true then 1 else 2) } }",
            "class C { def use(value: => Int): Int = value; def outer: Int = use(1) }",
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
            let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "outer");
            let context = ExpressionContext {
                lexical: index.declaration_context_of(method).unwrap(),
                owner: method,
                local_scopes: None,
            };
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );

            let typed = typer.type_expression(rhs, context).unwrap();

            assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        }
    }

    #[test]
    fn by_name_application_mismatch_rolls_back_and_retry_is_deterministic() {
        let source_text =
            "class C { def outer: Int = { def use(value: => Int): Int = value; use(true) } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (outer, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(outer).unwrap(),
            owner: outer,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let checkpoint = typer.store().checkpoint();

        let first = typer.type_expression(rhs, context).unwrap_err();
        assert!(matches!(
            &first,
            TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 0,
                actual,
                expected,
                ..
            } if *actual == definitions.boolean && *expected == definitions.int
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().get(source, rhs).is_none());

        let retry = typer.type_expression(rhs, context).unwrap_err();
        assert_eq!(format!("{retry:?}"), format!("{first:?}"));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn by_name_overload_candidates_remain_deferred() {
        let source_text = "class C { def choose(value: => Int): Int = value; def choose(value: Boolean): Int = 0; def outer: Int = choose(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate { .. })
            ),
            "unexpected by-name overload result: {result:?}"
        );
    }

    #[test]
    fn polymorphic_by_name_application_remains_deferred() {
        let source_text = "class C { def use[A](value: => A): A = value; def outer: Int = use(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnsupportedPolymorphicApplicationShape { .. })
        ));
    }

    #[test]
    fn erased_application_parameter_is_deferred() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    definition
                        .value_param_clauses
                        .first()
                        .and_then(|clause| clause.first())
                        .and_then(|tree| index.symbol_at(source, *tree))
                }
                _ => None,
            })
            .unwrap();
        let flags = store.symbols.get(parameter).flags;
        store.symbols.get_mut(parameter).flags = flags | SymbolFlags::ERASED;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ErasedApplicationParameterDeferred {
                parameter_index: 0,
                ..
            })
        ));
    }

    #[test]
    fn varargs_application_parameter_is_deferred() {
        let source_text = "class C { def f(xs: Int*): Int = 1; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::VarargsApplicationParameterDeferred {
                    parameter_index: 0,
                    ..
                })
            ),
            "unexpected varargs result: {result:?}"
        );
    }

    #[test]
    fn varargs_parameter_reference_is_deferred_without_sequence_type() {
        let source_text = "class C { def f(xs: Int*): Int = xs }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
            .unwrap();
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            panic!("method symbol should point to a DefDef")
        };
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index.symbol_at(source, parameter_tree).unwrap();
        let TreeKind::ValDef(parameter_definition) = &parsed.ast.get(parameter_tree).kind else {
            panic!("method parameter should be a ValDef");
        };
        let parameter_tpt = parameter_definition.tpt;
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let before = typer.store().checkpoint();
        assert!(matches!(
            typer.complete_symbol(parameter),
            Err(TyperError::RepeatedParameterSignatureContextMissing {
                parameter: found_parameter,
                parameter_tree_index,
            }) if found_parameter == parameter && parameter_tree_index == parameter_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);

        typer.complete_symbol(method).unwrap();
        let method_type = typer.store().symbols.info(method);
        let SymbolInfo::Complete(method_type) = *method_type else {
            panic!("method signature should be complete")
        };
        let Type::Method(method_type) = typer.store().types.get(method_type) else {
            panic!("method symbol should contain a method type")
        };
        assert!(method_type.params[0].varargs);
        assert_eq!(method_type.params[0].ty, definitions.int);

        let parameter_info = typer.store().symbols.info(parameter);
        let SymbolInfo::Complete(parameter_value_type) = *parameter_info else {
            panic!("varargs parameter symbol should retain its repeated type")
        };
        assert!(matches!(
            typer.store().types.get(parameter_value_type),
            Type::Repeated { element } if *element == definitions.int
        ));
        assert_eq!(
            typer.source_type_index().type_at(source, parameter_tpt),
            Some(parameter_value_type)
        );
        assert_eq!(
            typer.complete_symbol(parameter).unwrap(),
            parameter_value_type
        );
        assert_eq!(
            typer.source_type_index().type_at(source, parameter_tpt),
            Some(parameter_value_type)
        );
        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::VarargsParameterReferenceDeferred {
                source: found_source,
                tree_index,
                symbol: found_symbol,
            }) if found_source == source && tree_index == rhs.index() && found_symbol == parameter
        ));
        assert!(matches!(
            typer.widen_expression_type(parameter_value_type),
            Err(TyperError::ExpressionTypeCannotBeWidened { ty }) if ty == parameter_value_type
        ));
    }

    #[test]
    fn repeated_applied_parameter_caches_the_marker_and_keeps_its_element_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]; class C { def f(xs: Box[Int]*): Int = 1 }");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let method_tree = match index.definition_of(method).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => panic!("canonical method definition expected"),
        };
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            panic!("method symbol should point to a DefDef");
        };
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index.symbol_at(source, parameter_tree).unwrap();
        let TreeKind::ValDef(parameter_definition) = &parsed.ast.get(parameter_tree).kind else {
            panic!("method parameter should be a ValDef");
        };
        let parameter_tpt = parameter_definition.tpt;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(method).unwrap();
        let SymbolInfo::Complete(parameter_info) = *typer.store().symbols.info(parameter) else {
            panic!("parameter symbol should be complete");
        };
        let Type::Repeated { element } = typer.store().types.get(parameter_info) else {
            panic!("parameter symbol should retain Repeated");
        };
        let element = *element;
        let Type::Applied { tycon, args } = typer.store().types.get(element) else {
            panic!("repeated element type should preserve its application");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), box_class);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], definitions.int);
        assert_eq!(
            typer.source_type_index().type_at(source, parameter_tpt),
            Some(parameter_info)
        );
    }

    #[test]
    fn postfix_star_outside_parameter_type_position_stays_unsupported() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def f(xs: Int*): Int = 1 }");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "xs" =>
                {
                    index
                        .symbol_at(source, tree)
                        .map(|symbol| (symbol, definition.tpt))
                }
                _ => None,
            })
            .unwrap();
        let (symbol, tpt) = parameter;
        let context = index.declaration_context_of(symbol).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(tpt, context),
            Err(TyperError::UnsupportedTypeTree { .. })
        ));
        assert_eq!(typer.source_type_index().type_at(source, tpt), None);
    }

    #[test]
    fn repeated_parameter_element_failure_rolls_back_the_marker_and_cache() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def f(xs: Missing*): Int = 1 }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let method_tree = match index.definition_of(method).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => panic!("canonical method definition expected"),
        };
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            panic!("method symbol should point to a DefDef");
        };
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index.symbol_at(source, parameter_tree).unwrap();
        let TreeKind::ValDef(parameter_definition) = &parsed.ast.get(parameter_tree).kind else {
            panic!("method parameter should be a ValDef");
        };
        let parameter_tpt = parameter_definition.tpt;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::TypeNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.store().symbols.info(parameter), &SymbolInfo::Missing);
        assert_eq!(
            typer.source_type_index().type_at(source, parameter_tpt),
            None
        );
    }

    #[test]
    fn repeated_parameter_that_is_not_final_is_rejected_with_position_error() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def f(xs: Int*): Int = 1; def g(y: Int): Int = 1 }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let (other_method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "g");
        let method_tree = match index.definition_of(method).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => panic!("canonical method definition expected"),
        };
        let other_method_tree = match index.definition_of(other_method).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => panic!("canonical method definition expected"),
        };
        let other_parameter_tree = match &parsed.ast.get(other_method_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("method symbol should point to a DefDef"),
        };
        if let TreeKind::DefDef(definition) = &mut parsed.ast.get_mut(method_tree).kind {
            definition.value_param_clauses[0].push(other_parameter_tree);
        } else {
            panic!("method symbol should point to a DefDef");
        }
        let repeated_parameter_tree = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("method symbol should point to a DefDef"),
        };
        let repeated_parameter = index.symbol_at(source, repeated_parameter_tree).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::RepeatedParameterNotFinal {
                method: error_method,
                method_tree_index,
                clause_index: 0,
                parameter_index: 0,
            }) if error_method == method && method_tree_index == method_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(repeated_parameter),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn repeated_parameter_in_contextual_clause_is_rejected_with_clause_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def f(xs: Int*): Int = 1 }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let method_tree = match index.definition_of(method).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => panic!("canonical method definition expected"),
        };
        let parameter_tree = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("method symbol should point to a DefDef"),
        };
        let parameter = index.symbol_at(source, parameter_tree).unwrap();
        let flags = store.symbols.get(parameter).flags;
        store.symbols.get_mut(parameter).flags = flags | SymbolFlags::GIVEN;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::RepeatedParameterClauseUnsupported {
                method: error_method,
                method_tree_index,
                clause_index: 0,
                parameter_index: 0,
                kind: MethodKind::Contextual,
            }) if error_method == method && method_tree_index == method_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn curried_monomorphic_application_types_each_clause() {
        let source_text =
            "class C { def f(x: Int)(y: Boolean): Boolean = y; def use: Boolean = f(1)(true) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.boolean);
        let TreeKind::Apply(outer) = &typer.typed_ast().get(typed).kind else {
            panic!("outer clause should be an Apply")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(outer.function).ty),
            Type::Method(method) if method.params.len() == 1 && method.params[0].ty == definitions.boolean
        ));
    }

    #[test]
    fn selected_generic_receiver_method_can_be_applied() {
        let source_text = "class Box[A] { def accept(value: A): A = value }; class Use { def use(box: Box[Int]): Int = box.accept(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = val_symbol(&parsed, &store, &index, source, "box").0;
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn dependent_method_result_is_deferred() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let callee = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let reserved = typer.store.types.reserve();
        let dependent = typer.store.types.alloc(Type::ParamRef {
            binder: reserved.id(),
            index: 0,
        });
        let callable = typer.store.types.fill(
            reserved,
            Type::Method(dotty_core::types::MethodType {
                params: vec![dotty_core::types::MethodParam {
                    name: dotty_core::TermName::new(typer.store.names.intern("x")),
                    ty: definitions.int,
                    erased: false,
                    varargs: false,
                }],
                result: dependent,
                kind: MethodKind::Plain,
            }),
        );
        typer
            .store
            .symbols
            .set_info(callee, SymbolInfo::Complete(callable));

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::DependentMethodApplicationDeferred { binder, result, .. })
                if binder == callable && result == dependent
        ));
    }
}
