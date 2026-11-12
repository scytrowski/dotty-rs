Warning: truncated output (original token count: 411300)
... 596622 bytes omitted ...

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

#[cfg(test)]
type TransactionFailureObserver<'a> = std::rc::Rc<dyn Fn(&SourceTyper<'a>) + 'a>;
#[cfg(test)]
type TransactionFailureErrorObserver<'a> = std::rc::Rc<dyn Fn(&SourceTyper<'a>, &TyperError) + 'a>;

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
    scala_2_13_source_mode: Option<bool>,
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
    #[cfg(test)]
    transaction_failure_observer: Option<TransactionFailureObserver<'a>>,
    #[cfg(test)]
    transaction_failure_error_observer: Option<TransactionFailureErrorObserver<'a>>,
    #[cfg(test)]
    inject_failure_after_outermost_success: bool,
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
            scala_2_13_source_mode: None,
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
            #[cfg(test)]
            transaction_failure_observer: None,
            #[cfg(test)]
            transaction_failure_error_observer: None,
            #[cfg(test)]
            inject_failure_after_outermost_success: false,
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
                expression::FunctionLiteralTypingContext {
                    context,
                    expected_function1: None,
                    poly_binder: None,
                },
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
    // Keep the metadata corruption/recovery proof in a descendant unit module
    // so it can restore borrowed session state through private fields without
    // exposing a production mutable accessor.
    #[path = "issue_2076_product2_recovery.rs"]
    mod issue_2076_product2_recovery;
    #[path = "issue_2179_relation_observation.rs"]
    mod issue_2179_relation_observation;
    #[path = "issue_2234_source_owner_preparation.rs"]
    mod issue_2234_source_owner_preparation;
    #[path = "issue_2242_generic_product1.rs"]
    mod issue_2242_generic_product1;
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
        fn class_type_parameters(
            &self,
            store: &SemanticStore,
            class: SymbolId,
        ) -> Option<Vec<dotty_core::ClassTypeParameter>> {
            self.inner.class_type_parameters(store, class)
        }

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
        member_error: Option<ResolutionError>,
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
            if let Some(error) = &self.member_error {
                return Err(error.clone());
            }
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
 …232152 tokens truncated…ameNotFound { .. })
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

    #[test]
    fn issue_1972_failed_root_retries_with_same_typer_and_matches_clean_run() {
        let source_text = "package scala { trait Product }; package matching { case class Flagged(value: Int, enabled: Boolean); object Matcher { def select(item: Flagged): Int = item match { case Flagged(value, enabled) => if (enabled) value else 0 }; def classify(item: Flagged): Int = item match { case Flagged(0, true) => 0; case Flagged(value, _) => value } } }";
        let (clean_parsed, mut clean_store, clean_packages, definitions, clean_index, source) =
            parse_and_name(source_text);
        let method_tree =
            |parsed: &dotty_parser::ParseResult, store: &SemanticStore, name: &str| {
                parsed
                    .ast
                    .iter()
                    .find_map(|(tree, node)| match &node.kind {
                        TreeKind::DefDef(definition)
                            if store.names.resolve(definition.name.as_name().text()) == name =>
                        {
                            Some(tree)
                        }
                        _ => None,
                    })
                    .expect("requested source method")
            };
        let clean_select = method_tree(&clean_parsed, &clean_store, "select");
        let clean_classify = method_tree(&clean_parsed, &clean_store, "classify");
        let mut clean_typer = SourceTyper::new(
            &clean_parsed.ast,
            source,
            &clean_index,
            &mut clean_store,
            definitions,
            &clean_packages,
        );
        clean_typer
            .type_method_definition(clean_select)
            .expect("clean select method types");
        clean_typer
            .type_method_definition(clean_classify)
            .expect("clean sibling method types");
        let clean_projection =
            issue_1972_retry_projection(&clean_parsed, source, definitions, &clean_typer);

        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let select = method_tree(&parsed, &store, "select");
        let classify = method_tree(&parsed, &store, "classify");
        let select_symbol = index.symbol_at(source, select).unwrap();
        let class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Flagged" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("source Flagged class");
        let unapply = index
            .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .expect("generated Flagged.unapply identity");
        let before_store = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let before_typed = typer.typed_ast().checkpoint();
        let before_mappings = typer.source_typed_index().len();
        let observed_bindings = Rc::new(RefCell::new(None));
        let observation = Rc::clone(&observed_bindings);
        let parsed_ref = &parsed;
        typer.transaction_failure_observer = Some(Rc::new(move |typer| {
            let bindings = parsed_ref
                .ast
                .iter()
                .filter_map(|(tree, node)| {
                    let symbol = typer.pattern_binding_symbol_at(source, tree)?;
                    let name = match &node.kind {
                        TreeKind::Ident(ident) => typer.store.names.resolve(ident.name.text()),
                        TreeKind::Bind(binding) => typer.store.names.resolve(binding.name.text()),
                        other => {
                            panic!("pattern binder source node has unexpected shape: {other:?}")
                        }
                    };
                    Some((
                        tree.index(),
                        name.to_owned(),
                        issue_1972_symbol_type(typer.store, symbol, definitions),
                        issue_1972_symbol_owner(typer.store, symbol),
                    ))
                })
                .collect::<Vec<_>>();
            assert_eq!(
                bindings
                    .iter()
                    .map(|binding| (binding.1.as_str(), binding.2.as_str()))
                    .collect::<Vec<_>>(),
                [("value", "Int"), ("enabled", "Boolean")],
                "failure is injected after both extractor binders were entered"
            );
            assert_ne!(
                typer.typed_ast().checkpoint(),
                before_typed,
                "the failing transaction had built typed expression state"
            );
            assert!(
                typer.source_typed_index().get(source, select).is_some(),
                "the failing transaction had published the root mapping before rollback"
            );
            *observation.borrow_mut() = Some(bindings);
        }));
        typer.inject_failure_after_outermost_success = true;
        let error = typer
            .type_method_definition(select)
            .expect_err("the one-shot test failure is injected after typing the whole root");
        assert!(matches!(
            error,
            TyperError::SymbolResolution {
                error: ResolutionError::Malformed { .. },
                ..
            }
        ));
        assert!(
            observed_bindings.borrow().is_some(),
            "the rollback observer saw partial case state before cleanup"
        );
        assert!(!typer.inject_failure_after_outermost_success);
        assert_eq!(typer.store.checkpoint(), before_store);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_eq!(typer.source_typed_index().len(), before_mappings);
        assert!(typer.source_typed_index().get(source, select).is_none());
        assert_eq!(
            *typer.store.symbols.info(select_symbol),
            SymbolInfo::Missing
        );
        assert_eq!(*typer.store.symbols.info(unapply), SymbolInfo::Missing);
        for (tree, _) in parsed.ast.iter() {
            assert!(typer.pattern_binding_symbol_at(source, tree).is_none());
        }

        typer
            .type_method_definition(select)
            .expect("the same live SourceTyper retries successfully after rollback");
        typer
            .type_method_definition(classify)
            .expect("the valid sibling remains typeable on that same SourceTyper");
        assert_eq!(
            issue_1972_retry_projection(&parsed, source, definitions, &typer),
            clean_projection,
            "successful source-to-typed mappings and case bindings match a clean session"
        );
    }

    fn issue_1999_late_failure_projection(
        typer: &SourceTyper<'_>,
        parsed: &dotty_parser::ParseResult,
    ) -> Vec<(&'static str, String)> {
        let source = typer.source;
        let typed_nodes = typer
            .typed_ast()
            .iter()
            .map(|(tree, node)| (tree.index(), node.clone()))
            .collect::<Vec<_>>();
        let mappings = parsed
            .ast
            .iter()
            .map(|(tree, _)| {
                (
                    tree.index(),
                    typer.source_typed_index().get(source, tree),
                    typer.function_literal_method_at(source, tree),
                    typer.function_literal_definition_at(source, tree),
                    typer.function_literal_parameter_symbol_at(source, tree),
                )
            })
            .collect::<Vec<_>>();
        let mut local_scopes = typer
            .local_scopes
            .iter()
            .map(|((source, tree), scope)| (source.index(), tree.index(), scope.index()))
            .collect::<Vec<_>>();
        local_scopes.sort_unstable();
        let mut local_symbols = typer
            .local_symbols
            .iter()
            .map(|((source, tree), symbol)| (source.index(), tree.index(), symbol.index()))
            .collect::<Vec<_>>();
        local_symbols.sort_unstable();
        let mut pattern_bindings = typer
            .pattern_bindings
            .by_tree
            .iter()
            .map(|((source, tree), symbol)| (source.index(), tree.index(), symbol.index()))
            .collect::<Vec<_>>();
        pattern_bindings.sort_unstable();
        let mut pattern_scopes = typer
            .pattern_bindings
            .scope_by_symbol
            .iter()
            .map(|(symbol, scope)| (symbol.index(), scope.index()))
            .collect::<Vec<_>>();
        pattern_scopes.sort_unstable();
        let expression_scopes = typer
            .expression_scopes
            .iter()
            .map(|frame| {
                (
                    frame.scope.index(),
                    frame.parent.map(|parent| parent.index()),
                    frame.is_block_scope,
                    frame.is_case_scope,
                    frame
                        .imports
                        .iter()
                        .map(|(tree, context)| {
                            (
                                tree.index(),
                                context.lexical.index(),
                                context.owner.index(),
                                context.local_scopes.map(|scope| scope.index()),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let active_local_type_scopes = typer
            .active_local_type_scopes
            .iter()
            .map(|frame| {
                (
                    frame.context.index(),
                    frame.scope.index(),
                    frame
                        .body
                        .map(|span| (span.source().index(), span.span().range())),
                )
            })
            .collect::<Vec<_>>();
        let active_local_import_scopes = typer
            .active_local_import_scopes
            .iter()
            .map(|(context, scope)| (context.index(), scope.map(|scope| scope.index())))
            .collect::<Vec<_>>();
        let active_local_type_binders = typer
            .active_local_type_binders
            .iter()
            .map(|(ty, binders)| {
                (
                    ty.index(),
                    binders
                        .iter()
                        .map(|symbol| symbol.index())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let initializing_local_symbols = {
            let mut symbols = typer
                .initializing_local_symbols
                .iter()
                .map(|symbol| symbol.index())
                .collect::<Vec<_>>();
            symbols.sort_unstable();
            symbols
        };
        let inferred_local_lazy_types_in_progress = {
            let mut symbols = typer
                .inferred_local_lazy_types_in_progress
                .iter()
                .map(|symbol| symbol.index())
                .collect::<Vec<_>>();
            symbols.sort_unstable();
            symbols
        };
        let inferred_field_types_in_progress = {
            let mut symbols = typer
                .inferred_field_types_in_progress
                .iter()
                .map(|symbol| symbol.index())
                .collect::<Vec<_>>();
            symbols.sort_unstable();
            symbols
        };
        let inferred_method_results_in_progress = {
            let mut symbols = typer
                .inferred_method_results_in_progress
                .iter()
                .map(|symbol| symbol.index())
                .collect::<Vec<_>>();
            symbols.sort_unstable();
            symbols
        };
        vec![
            (
                "store checkpoint",
                format!("{:?}", typer.store.checkpoint()),
            ),
            ("symbols and infos", format!("{:?}", typer.store.symbols)),
            (
                "all scopes and buckets",
                format!("{:?}", typer.store.scopes),
            ),
            ("types", format!("{:?}", typer.store.types)),
            ("annotations", format!("{:?}", typer.store.annotations)),
            (
                "resolver checkpoint",
                format!("{:?}", typer.resolver.checkpoint()),
            ),
            (
                "typed checkpoint",
                format!("{:?}", typer.typed_ast().checkpoint()),
            ),
            ("typed nodes", format!("{typed_nodes:?}")),
            ("source typed mappings", format!("{mappings:?}")),
            ("local scopes", format!("{local_scopes:?}")),
            ("local symbols", format!("{local_symbols:?}")),
            ("pattern bindings", format!("{pattern_bindings:?}")),
            ("pattern scopes", format!("{pattern_scopes:?}")),
            ("expression scopes", format!("{expression_scopes:?}")),
            (
                "active local type scopes",
                format!("{active_local_type_scopes:?}"),
            ),
            (
                "active local import scopes",
                format!("{active_local_import_scopes:?}"),
            ),
            (
                "active local type binders",
                format!("{active_local_type_binders:?}"),
            ),
            (
                "initializing local symbols",
                format!("{initializing_local_symbols:?}"),
            ),
            (
                "inferred local lazy types",
                format!("{inferred_local_lazy_types_in_progress:?}"),
            ),
            (
                "inferred field types",
                format!("{inferred_field_types_in_progress:?}"),
            ),
            (
                "inferred method results",
                format!("{inferred_method_results_in_progress:?}"),
            ),
        ]
    }

    fn assert_issue_1999_projection_unchanged(
        before: &[(&'static str, String)],
        after: &[(&'static str, String)],
        message: &str,
    ) {
        let changed = before
            .iter()
            .zip(after)
            .filter_map(|((name, before), (_, after))| (before != after).then_some(*name))
            .collect::<Vec<_>>();
        assert!(changed.is_empty(), "{message}: changed {changed:?}");
    }

    #[test]
    fn issue_2037_reference_unwrap_result_failure_rolls_back_binder_and_retries_sibling() {
        let source_text = include_str!("../../tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala")
            .replace("def unwrap(expr: Expr): Expr", "def invalid(expr: Expr): Int")
            .replace(
                "  def eval(expr: Expr): Int =",
                "  def validSibling(expr: Expr): Expr =\n    expr match\n      case Num(_)      => expr\n      case Succ(inner) => inner\n\n  def eval(expr: Expr): Int =",
            );
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(&source_text);
        let method_tree = |name: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == name =>
                    {
                        Some(tree)
                    }
                    _ => None,
                })
                .expect("requested source method")
        };
        let invalid = method_tree("invalid");
        let sibling = method_tree("validSibling");
        let invalid_symbol = index
            .symbol_at(source, invalid)
            .expect("source invalid method");
        let expr = class_symbol(&parsed, &store, &index, source, "Expr");
        let succ = class_symbol(&parsed, &store, &index, source, "Succ");
        let succ_unapply = index
            .generated_case_class_method(succ, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .expect("canonical source-derived Succ.unapply");
        let succ_selector = index
            .derived_symbol_at(
                succ,
                source,
                parsed
                    .ast
                    .iter()
                    .find_map(|(_, node)| match &node.kind {
                        TreeKind::TypeDef(definition)
                            if store.names.resolve(definition.name.as_name().text()) == "Succ" =>
                        {
                            let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind
                            else {
                                return None;
                            };
                            let TreeKind::DefDef(constructor) =
                                &parsed.ast.get(template.constructor).kind
                            else {
                                return None;
                            };
                            Some(constructor.value_param_clauses[0][0])
                        }
                        _ => None,
                    })
                    .expect("Succ constructor field tree"),
            )
            .expect("canonical source-derived Succ._1");
        let (rhs, cases, binder_tree, body_tree) = {
            let TreeKind::DefDef(definition) = &parsed.ast.get(invalid).kind else {
                unreachable!()
            };
            let rhs = definition.rhs.expect("invalid method has a body");
            let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
                panic!("invalid method retains the original Match")
            };
            assert_eq!(matched.cases.len(), 2);
            let TreeKind::CaseDef(second_case) = &parsed.ast.get(matched.cases[1]).kind else {
                unreachable!()
            };
            let TreeKind::Apply(extractor) = &parsed.ast.get(second_case.pattern).kind else {
                panic!("Succ pattern remains an extractor application")
            };
            let binder = extractor.args[0];
            let body = match &parsed.ast.get(second_case.body).kind {
                TreeKind::Block(block) => block.expr,
                _ => second_case.body,
            };
            (rhs, matched.cases.clone(), binder, body)
        };
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../dotty-tasty/tests/fixtures/scala3-library");
        let java_home = std::env::var_os("JAVA_HOME").expect("JDK 21 is configured");
        let classpath = dotty_classloader::classloader::CompositeClassPath::new(vec![
            Box::new(dotty_classloader::classloader::DirectoryClassPath::new(
                fixture,
            )),
            Box::new(
                dotty_classloader::classloader::JdkClassPath::new(
                    std::path::PathBuf::from(java_home).join("jmods"),
                )
                .expect("JDK module path"),
            ),
        ]);
        let resolver = dotty_classloader::classloader::ClasspathSymbolResolver::new(
            classpath,
            definitions,
            dotty_classloader::classloader::LoadingSession::new(),
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let before_store = typer.store().checkpoint();
        let before_typed = typer.typed_ast().checkpoint();
        let before_projection = issue_1999_late_failure_projection(&typer, &parsed);
        let before_type_index = parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
            .collect::<Vec<_>>();
        let watched = [invalid_symbol, expr, succ, succ_unapply, succ_selector]
            .map(|symbol| (symbol, *typer.store().symbols.info(symbol)));
        let observed = Rc::new(RefCell::new(false));
        let observation = Rc::clone(&observed);
        let parsed_ref = &parsed;
        typer.transaction_failure_observer = Some(Rc::new(move |typer| {
            let typed_match = typer
                .source_typed_index()
                .get(source, rhs)
                .expect("the Match reached its typed mapping before result conformance failed");
            assert_eq!(
                type_symbol(typer.store(), typer.typed_ast().get(typed_match).ty),
                expr,
                "the Match reached its typed mapping before declared-result conformance failed"
            );
            assert_eq!(
                cases
                    .iter()
                    .filter(|case| typer.source_typed_index().get(source, **case).is_some())
                    .count(),
                2,
                "both original cases completed before result conformance"
            );
            let binder = typer
                .pattern_binding_symbol_at(source, binder_tree)
                .expect("the source-owned inner binder exists before rollback");
            assert_eq!(typer.store().symbols.get(binder).kind, SymbolKind::Local);
            assert_eq!(
                typer.store().symbols.get(binder).owner,
                Some(invalid_symbol)
            );
            assert_eq!(
                typer.store().symbols.get(binder).origin,
                SymbolOrigin::Source(source)
            );
            let SymbolInfo::Complete(binder_type) = *typer.store().symbols.info(binder) else {
                panic!("the late-failure binder already has its completed type")
            };
            assert_eq!(type_symbol(typer.store(), binder_type), expr);
            let typed_binder = typer
                .source_typed_index()
                .get(source, binder_tree)
                .expect("the source binder has a typed mapping before rollback");
            let typed_body = typer
                .source_typed_index()
                .get(source, body_tree)
                .expect("the inner body reference has a typed mapping before rollback");
            assert!(
                matches!(
                    typer.store().types.get(typer.typed_ast().get(typed_body).ty),
                    Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == binder
                ),
                "the typed Succ body refers to its case-local inner binder"
            );
            let TreeKind::Bind(binding) = &typer.typed_ast().get(typed_binder).kind else {
                panic!("typed Succ pattern retains its canonical binder")
            };
            assert_eq!(typer.store().names.resolve(binding.name.text()), "inner");
            assert_eq!(
                parsed_ref.ast.get(binder_tree).position,
                typer.typed_ast().get(typed_binder).position
            );
            assert_ne!(typer.typed_ast().checkpoint(), before_typed);
            *observation.borrow_mut() = true;
        }));

        let error = typer
            .type_method_definition(invalid)
            .expect_err("Int result mismatch fails after the reference-valued Match is typed");
        assert!(
            matches!(
                error,
                TyperError::ExpectedExpressionTypeMismatch {
                    source: error_source,
                    tree_index,
                    actual,
                    expected,
                } if error_source == source
                    && tree_index == rhs.index()
                    && expected == definitions.int
                && actual != expected
            ),
            "unexpected complete-method result diagnostic: {error:?}"
        );
        let normalized_diagnostic = match &error {
            TyperError::ExpectedExpressionTypeMismatch {
                source: error_source,
                tree_index,
                actual: _,
                expected,
            } => (*error_source, *tree_index, *expected),
            other => panic!("unexpected complete-method result diagnostic: {other:?}"),
        };
        assert!(
            *observed.borrow(),
            "observer saw the live Succ binder before rollback"
        );
        assert_eq!(typer.store().checkpoint(), before_store);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_issue_1999_projection_unchanged(
            &before_projection,
            &issue_1999_late_failure_projection(&typer, &parsed),
            "rollback removes binder scopes, typed nodes and every source mapping",
        );
        assert_eq!(
            parsed
                .ast
                .iter()
                .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
                .collect::<Vec<_>>(),
            before_type_index,
            "source type cache is restored"
        );
        assert!(typer.source_typed_index().get(source, invalid).is_none());
        assert!(typer.source_typed_index().get(source, rhs).is_none());
        assert!(
            typer
                .pattern_binding_symbol_at(source, binder_tree)
                .is_none()
        );
        for (symbol, info) in watched {
            assert_eq!(*typer.store().symbols.info(symbol), info);
        }

        let typed_sibling = typer
            .type_method_definition(sibling)
            .expect("valid same-session unwrap sibling succeeds after late failure rollback");
        assert_eq!(
            typer.source_typed_index().get(source, sibling),
            Some(typed_sibling)
        );
        let TreeKind::DefDef(sibling_definition) = &parsed.ast.get(sibling).kind else {
            unreachable!()
        };
        let sibling_rhs = sibling_definition.rhs.expect("valid sibling body");
        let typed_rhs = typer
            .source_typed_index()
            .get(source, sibling_rhs)
            .expect("same-session sibling Match is completely mapped");
        let TreeKind::Match(sibling_match) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("valid sibling remains a typed Match")
        };
        assert_eq!(sibling_match.cases.len(), 2);

        let sibling_store = typer.store().checkpoint();
        let sibling_typed = typer.typed_ast().checkpoint();
        let sibling_projection = issue_1999_late_failure_projection(&typer, &parsed);
        let sibling_type_index = parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
            .collect::<Vec<_>>();
        typer.transaction_failure_observer = None;
        let retry = typer
            .type_method_definition(invalid)
            .expect_err("repeating the invalid method reports the same diagnostic");
        let retry_diagnostic = match retry {
            TyperError::ExpectedExpressionTypeMismatch {
                source: error_source,
                tree_index,
                actual,
                expected,
            } if actual != expected => (error_source, tree_index, expected),
            other => panic!("retry changed the complete-method diagnostic: {other:?}"),
        };
        assert_eq!(retry_diagnostic, normalized_diagnostic);
        assert_eq!(typer.store().checkpoint(), sibling_store);
        assert_eq!(typer.typed_ast().checkpoint(), sibling_typed);
        assert_issue_1999_projection_unchanged(
            &sibling_projection,
            &issue_1999_late_failure_projection(&typer, &parsed),
            "repeated failure preserves the successfully typed sibling",
        );
        assert_eq!(
            parsed
                .ast
                .iter()
                .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
                .collect::<Vec<_>>(),
            sibling_type_index
        );
        assert_eq!(
            typer.source_typed_index().get(source, sibling),
            Some(typed_sibling)
        );
    }

    #[test]
    fn issue_1999_late_event_result_failure_observes_both_cases_before_atomic_rollback() {
        let source_text =
            include_str!("../../tests/fixtures/source-sealed-adt-match-3.9.0/Event.scala")
                .replace(
                    "object Matcher:\n",
                    "object Matcher:\n  def prior(event: Event): Int = 1\n\n",
                )
                .replace(
                    "def extract(event: Event): Int",
                    "def extract(event: Event): Boolean",
                );
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(&source_text);
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "extract" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .expect("mutated extract root");
        let (rhs, cases) = match &parsed.ast.get(method).kind {
            TreeKind::DefDef(definition) => {
                let rhs = definition.rhs.expect("extract has a body");
                let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
                    panic!("extract body remains a Match")
                };
                (rhs, matched.cases.clone())
            }
            _ => unreachable!(),
        };
        assert_eq!(cases.len(), 2);
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../dotty-tasty/tests/fixtures/scala3-library");
        let java_home = std::env::var_os("JAVA_HOME").expect("JDK 21 is configured");
        let classpath = dotty_classloader::classloader::CompositeClassPath::new(vec![
            Box::new(dotty_classloader::classloader::DirectoryClassPath::new(
                fixture,
            )),
            Box::new(
                dotty_classloader::classloader::JdkClassPath::new(
                    std::path::PathBuf::from(java_home).join("jmods"),
                )
                .expect("JDK module path"),
            ),
        ]);
        let resolver = dotty_classloader::classloader::ClasspathSymbolResolver::new(
            classpath,
            definitions,
            dotty_classloader::classloader::LoadingSession::new(),
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));
        let prior = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text()) == "prior" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .expect("prior successful sibling method");
        typer
            .type_method_definition(prior)
            .expect("prior successful sibling remains available during rollback");
        let before_projection = issue_1999_late_failure_projection(&typer, &parsed);
        let before_resolver = typer.resolver.checkpoint();
        let before_typed = typer.typed_ast().checkpoint();
        let before_index = typer.source_typed_index().len();
        let before_local_scopes = typer.local_scopes.len();
        let before_pattern_bindings = typer.pattern_bindings.by_tree.len();
        let before_pattern_scope_bindings = typer.pattern_bindings.scope_by_symbol.len();
        let before_expression_scopes = typer.expression_scopes.len();
        let observed = Rc::new(RefCell::new(None));
        let observation = Rc::clone(&observed);
        let parsed_ref = &parsed;
        let observed_cases = cases.clone();
        typer.transaction_failure_observer = Some(Rc::new(move |typer| {
            if typer.source_typed_index().get(source, rhs).is_none() {
                return;
            }
            let bindings = parsed_ref
                .ast
                .iter()
                .filter_map(|(tree, node)| {
                    let symbol = typer.pattern_binding_symbol_at(source, tree)?;
                    let name = match &node.kind {
                        TreeKind::Bind(binding) => typer.store.names.resolve(binding.name.text()),
                        TreeKind::Ident(ident) => typer.store.names.resolve(ident.name.text()),
                        other => {
                            panic!("Event pattern binding has unexpected source shape: {other:?}")
                        }
                    };
                    Some((
                        name.to_owned(),
                        issue_1972_symbol_type(typer.store, symbol, definitions),
                    ))
                })
                .collect::<Vec<_>>();
            assert_eq!(
                bindings,
                [
                    ("id".to_owned(), "Int".to_owned()),
                    ("code".to_owned(), "Int".to_owned())
                ]
            );
            let typed_cases = observed_cases
                .iter()
                .filter_map(|case| typer.source_typed_index().get(source, *case))
                .collect::<Vec<_>>();
            assert_eq!(
                typed_cases.len(),
                2,
                "both case roots were typed before result conformance failed"
            );
            assert!(
                typer.source_typed_index().get(source, rhs).is_some(),
                "the Match was typed before result conformance failed"
            );
            assert!(
                typer.typed_ast().checkpoint() != before_typed,
                "the failing transaction built typed nodes"
            );
            *observation.borrow_mut() = Some((bindings, typed_cases.len()));
        }));

        let error = typer
            .type_method_definition(method)
            .expect_err("the wrong declared result fails after Match typing");
        assert!(
            matches!(error, TyperError::ExpectedExpressionTypeMismatch { .. }),
            "{error:?}"
        );
        assert_eq!(observed.borrow().as_ref().map(|(_, count)| *count), Some(2));
        assert_issue_1999_projection_unchanged(
            &before_projection,
            &issue_1999_late_failure_projection(&typer, &parsed),
            "rollback restores complete semantic contents and preserves the prior root",
        );
        assert_eq!(typer.resolver.checkpoint(), before_resolver);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_eq!(typer.source_typed_index().len(), before_index);
        assert_eq!(typer.local_scopes.len(), before_local_scopes);
        assert_eq!(
            typer.pattern_bindings.by_tree.len(),
            before_pattern_bindings
        );
        assert_eq!(
            typer.pattern_bindings.scope_by_symbol.len(),
            before_pattern_scope_bindings
        );
        assert_eq!(typer.expression_scopes.len(), before_expression_scopes);
        assert!(typer.source_typed_index().get(source, method).is_none());
        assert!(typer.source_typed_index().get(source, prior).is_some());
        assert!(typer.source_typed_index().get(source, rhs).is_none());
        for case in cases {
            assert!(typer.source_typed_index().get(source, case).is_none());
        }
        for (tree, _) in parsed.ast.iter() {
            assert!(typer.pattern_binding_symbol_at(source, tree).is_none());
        }
        let retry = typer
            .type_method_definition(method)
            .expect_err("retry reports the same post-Match result mismatch");
        assert_eq!(format!("{retry:?}"), format!("{error:?}"));
        assert_issue_1999_projection_unchanged(
            &before_projection,
            &issue_1999_late_failure_projection(&typer, &parsed),
            "retry preserves every semantic value and the earlier successful root",
        );
    }

    #[test]
    fn issue_1999_structural_corruption_restores_then_recovers_in_same_typer() {
        let fixture_source =
            include_str!("../../tests/fixtures/source-sealed-adt-match-3.9.0/Event.scala");
        let source_text = fixture_source
            .replace("sealed trait Event", "sealed trait Event\ntrait Other")
            .replace(
                "object Matcher:\n",
                "object Matcher:\n  def prior(event: Event): Int = 1\n\n",
            );
        let mut receipts = Vec::new();
        for corruption in [
            "forged companion",
            "foreign _1 owner",
            "unrelated parent",
            "duplicate parent",
            "invalid parent TypeId",
            "missing parent ClassInfo",
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(&source_text);
            let tree_named = |name: &str| {
                parsed
                    .ast
                    .iter()
                    .find_map(|(tree, node)| match &node.kind {
                        TreeKind::DefDef(definition)
                            if store.names.resolve(definition.name.as_name().text()) == name =>
                        {
                            Some(tree)
                        }
                        _ => None,
                    })
                    .expect("source method")
            };
            let prior = tree_named("prior");
            let extract = tree_named("extract");
            let classify = tree_named("classify");
            let symbol_named = |name: &str| {
                parsed
                    .ast
                    .iter()
                    .find_map(|(tree, node)| match &node.kind {
                        TreeKind::TypeDef(definition)
                            if store.names.resolve(definition.name.as_name().text()) == name =>
                        {
                            index.symbol_at(source, tree)
                        }
                        _ => None,
                    })
                    .expect("source class symbol")
            };
            let event = symbol_named("Event");
            let other = symbol_named("Other");
            let created = symbol_named("Created");
            let deleted = symbol_named("Deleted");
            let created_links = store.symbols.get(created).links;
            let deleted_companion = store.symbols.get(deleted).links.companion.unwrap();
            let class_scope = index.scope_of(created).expect("Created scope");
            let selector_name = store.names.intern("_1");
            let selector = store
                .scopes
                .get(class_scope)
                .lookup(&dotty_core::Name::new(
                    selector_name,
                    dotty_core::Namespace::Term,
                ))
                .expect("Created._1 selector");
            let selector_owner = store.symbols.get(selector).owner;
            let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty/tests/fixtures/scala3-library");
            let java_home = std::env::var_os("JAVA_HOME").expect("JDK 21 is configured");
            let classpath = dotty_classloader::classloader::CompositeClassPath::new(vec![
                Box::new(dotty_classloader::classloader::DirectoryClassPath::new(
                    fixture,
                )),
                Box::new(
                    dotty_classloader::classloader::JdkClassPath::new(
                        std::path::PathBuf::from(java_home).join("jmods"),
                    )
                    .expect("JDK module path"),
                ),
            ]);
            let resolver = dotty_classloader::classloader::ClasspathSymbolResolver::new(
                classpath,
                definitions,
                dotty_classloader::classloader::LoadingSession::new(),
            );
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            )
            .with_resolver(Box::new(resolver));
            let prior_typed = typer
                .type_method_definition(prior)
                .expect("prior successful root is retained");
            typer.complete_symbol(event).expect("canonical Event");
            typer.complete_symbol(other).expect("canonical Other");
            let SymbolInfo::Complete(original_event_info) = *typer.store.symbols.info(event) else {
                panic!("Event ClassInfo precompleted")
            };
            let SymbolInfo::Complete(event_info_id) = *typer.store.symbols.info(event) else {
                panic!("Event ClassInfo is complete")
            };
            let Some(Type::ClassInfo(mut event_info)) =
                typer.store.types.try_get(event_info_id).cloned()
            else {
                panic!("Event is represented by ClassInfo")
            };

            match corruption {
                "forged companion" => {
                    typer.store.symbols.get_mut(created).links.companion = Some(deleted_companion);
                }
                "foreign _1 owner" => {
                    typer.store.symbols.get_mut(selector).owner = Some(deleted);
                }
                "unrelated parent" => {
                    let parent = typer.store.types.alloc(Type::TypeRef {
                        prefix: definitions.no_prefix,
                        target: dotty_core::types::TypeRefTarget::Symbol(other),
                    });
                    event_info.parents = vec![parent];
                }
                "duplicate parent" => {
                    event_info.parents = vec![definitions.object_type, definitions.object_type];
                }
                "invalid parent TypeId" => {
                    event_info.parents = vec![typer.store.types.reserve().id()];
                }
                "missing parent ClassInfo" => event_info.parents.clear(),
                _ => unreachable!(),
            }
            if corruption.contains("parent") || corruption == "missing parent ClassInfo" {
                let malformed = typer.store.types.alloc(Type::ClassInfo(event_info));
                typer
                    .store
                    .symbols
                    .set_info(event, SymbolInfo::Complete(malformed));
            }
            let before = issue_1999_late_failure_projection(&typer, &parsed);
            let error = typer
                .type_method_definition(extract)
                .expect_err("structural corruption rejects a whole source DefDef");
            if corruption.contains("parent") || corruption == "missing parent ClassInfo" {
                assert!(
                    matches!(&error, TyperError::MalformedClassInfo { symbol, .. } if *symbol == event),
                    "{corruption}: {error:?}"
                );
            } else {
                assert!(
                    matches!(
                        error,
                        TyperError::SymbolSourceKindMismatch { .. }
                            | TyperError::ExtractorQualifierNotFound { .. }
                            | TyperError::ExtractorQualifierNotValueLike { .. }
                            | TyperError::ExtractorUnapplyNotFound { .. }
                            | TyperError::ExtractorResultMemberNotFound { .. }
                            | TyperError::DeclarationContextMissing { .. }
                    ),
                    "{corruption}: {error:?}"
                );
            }
            assert_issue_1999_projection_unchanged(
                &before,
                &issue_1999_late_failure_projection(&typer, &parsed),
                "structural whole-method rollback preserves prior root and all state",
            );
            let retry = typer
                .type_method_definition(extract)
                .expect_err("same complete source method fails on retry");
            assert_eq!(format!("{retry:?}"), format!("{error:?}"));
            assert_issue_1999_projection_unchanged(
                &before,
                &issue_1999_late_failure_projection(&typer, &parsed),
                "structural whole-method retry leaves all state unchanged",
            );

            match corruption {
                "forged companion" => typer.store.symbols.get_mut(created).links = created_links,
                "foreign _1 owner" => typer.store.symbols.get_mut(selector).owner = selector_owner,
                _ => typer
                    .store
                    .symbols
                    .set_info(event, SymbolInfo::Complete(original_event_info)),
            }
            assert!(typer.source_typed_index().get(source, extract).is_none());
            assert!(typer.source_typed_index().get(source, classify).is_none());
            assert_eq!(
                typer.source_typed_index().get(source, prior),
                Some(prior_typed)
            );
            typer
                .type_method_definition(extract)
                .expect("restored uncached extract types in the same SourceTyper");
            typer
                .type_method_definition(classify)
                .expect("restored uncached classify types in the same SourceTyper");
            assert!(typer.source_typed_index().get(source, extract).is_some());
            assert!(typer.source_typed_index().get(source, classify).is_some());
            receipts.push(format!(
                "{corruption}\t{}\textract+classify",
                issue_1999_error_category(&error)
            ));
        }
        assert_eq!(
            format!(
                "corruption\tfirst error\trecovered roots\n{}\n",
                receipts.join("\n")
            ),
            include_str!(
                "../../tests/fixtures/source-sealed-adt-match-3.9.0/rust-structural-lifecycle.tsv"
            )
        );
    }

    fn issue_1999_error_category(error: &TyperError) -> &'static str {
        match error {
            TyperError::MalformedClassInfo { .. } => "MalformedClassInfo",
            TyperError::SymbolSourceKindMismatch { .. } => "SymbolSourceKindMismatch",
            TyperError::ExtractorQualifierNotFound { .. } => "ExtractorQualifierNotFound",
            TyperError::ExtractorQualifierNotValueLike { .. } => "ExtractorQualifierNotValueLike",
            TyperError::ExtractorUnapplyNotFound { .. } => "ExtractorUnapplyNotFound",
            TyperError::ExtractorResultMemberNotFound { .. } => "ExtractorResultMemberNotFound",
            TyperError::DeclarationContextMissing { .. } => "DeclarationContextMissing",
            other => panic!("unexpected structural lifecycle error: {other:?}"),
        }
    }

    type Issue1972RetryProjection = (
        Vec<(u32, String, String)>,
        Vec<(u32, String, String, String)>,
    );

    fn issue_1972_retry_projection(
        parsed: &dotty_parser::ParseResult,
        source: SourceId,
        definitions: Definitions,
        typer: &SourceTyper<'_>,
    ) -> Issue1972RetryProjection {
        let mappings = parsed
            .ast
            .iter()
            .filter_map(|(source_tree, _)| {
                let typed_tree = typer.source_typed_index().get(source, source_tree)?;
                let typed_node = typer.typed_ast().get(typed_tree);
                Some((
                    source_tree.index(),
                    format!("{:?}", std::mem::discriminant(&typed_node.kind)),
                    issue_1972_type_label(typer.store, typed_node.ty, definitions),
                ))
            })
            .collect();
        let bindings = parsed
            .ast
            .iter()
            .filter_map(|(source_tree, source_node)| {
                let symbol = typer.pattern_binding_symbol_at(source, source_tree)?;
                let name = match &source_node.kind {
                    TreeKind::Ident(ident) => typer.store.names.resolve(ident.name.text()),
                    TreeKind::Bind(binding) => typer.store.names.resolve(binding.name.text()),
                    other => panic!("pattern binder source node has unexpected shape: {other:?}"),
                };
                Some((
                    source_tree.index(),
                    name.to_owned(),
                    issue_1972_symbol_type(typer.store, symbol, definitions),
                    issue_1972_symbol_owner(typer.store, symbol),
                ))
            })
            .collect();
        (mappings, bindings)
    }

    fn issue_1972_type_label(
        store: &SemanticStore,
        ty: TypeId,
        definitions: Definitions,
    ) -> String {
        if ty == definitions.int {
            "Int".to_owned()
        } else if ty == definitions.boolean {
            "Boolean".to_owned()
        } else if ty == definitions.unit {
            "Unit".to_owned()
        } else {
            store
                .types
                .try_get(ty)
                .map(|ty| format!("{:?}", std::mem::discriminant(ty)))
                .unwrap_or_else(|| "missing".to_owned())
        }
    }

    fn issue_1972_symbol_type(
        store: &SemanticStore,
        symbol: SymbolId,
        definitions: Definitions,
    ) -> String {
        match *store.symbols.info(symbol) {
            SymbolInfo::Complete(ty) => issue_1972_type_label(store, ty, definitions),
            SymbolInfo::Missing => "Missing".to_owned(),
            _ => "Incomplete".to_owned(),
        }
    }

    fn issue_1972_symbol_owner(store: &SemanticStore, symbol: SymbolId) -> String {
        let entry = store.symbols.get(symbol);
        let owner = entry
            .owner
            .map(|owner| {
                store
                    .names
                    .resolve(store.symbols.get(owner).name.text())
                    .to_owned()
            })
            .unwrap_or_else(|| "<none>".to_owned());
        let origin = match entry.origin {
            SymbolOrigin::Source(_) => "Source",
            SymbolOrigin::Classfile(_) => "Classfile",
            SymbolOrigin::Tasty(_) => "Tasty",
            SymbolOrigin::Synthetic => "Synthetic",
            SymbolOrigin::Builtin => "Builtin",
        };
        format!(
            "{}|{owner}|{origin}",
            store.names.resolve(entry.name.text())
        )
    }
}
