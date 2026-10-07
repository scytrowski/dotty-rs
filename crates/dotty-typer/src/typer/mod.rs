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
    SymbolResolver, TreeId, TypeId, Typed, Untyped,
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
    ExtractorResultMemberIssue, PatternKind, TuplePatternResolutionIssue, TypeArgumentBoundSide,
    TyperError,
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
    patdef_expansions: expression::blocks::PatDefExpansionIndex,
    patdef_typing_attempts: HashMap<(SourceId, TreeId<Untyped>), Result<(), String>>,
    pattern_bindings: PatternBindingIndex,
    local_methods: LocalMethodIndex,
    active_local_type_scopes: Vec<(SourceContextId, ScopeId)>,
    active_local_import_scopes: Vec<(SourceContextId, Option<ExpressionScopeId>)>,
    active_local_type_binders: Vec<(TypeId, Vec<SymbolId>)>,
    initializing_local_symbols: HashSet<SymbolId>,
    inferred_method_results_in_progress: HashSet<SymbolId>,
    typed_arena: AstArena<Typed>,
    typed_index: SourceTypedIndex,
    expression_scopes: Vec<ExpressionScopeFrame>,
    expression_scope_owner: u64,
}

#[derive(Clone, Copy)]
struct SourceTreeLocation {
    tree_index: u32,
    position: Option<SourceSpan>,
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
            patdef_expansions: expression::blocks::PatDefExpansionIndex::default(),
            patdef_typing_attempts: HashMap::new(),
            pattern_bindings: PatternBindingIndex::default(),
            local_methods: LocalMethodIndex::default(),
            active_local_type_scopes: Vec::new(),
            active_local_import_scopes: Vec::new(),
            active_local_type_binders: Vec::new(),
            initializing_local_symbols: HashSet::new(),
            inferred_method_results_in_progress: HashSet::new(),
            typed_arena: AstArena::new(),
            typed_index: SourceTypedIndex::new(),
            expression_scopes: Vec::new(),
            expression_scope_owner: next_expression_scope_owner(),
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
            return Ok(typed);
        }
        self.run_expression_transaction(|typer, info_journal, new_mappings| {
            typer.type_value_expression_inner(tree, context, info_journal, new_mappings)
        })
    }

    /// Types an expression and checks its widened value type against `expected`.
    /// The returned tree preserves the expression's own semantic type.
    pub fn type_expression_expected(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        expected: TypeId,
    ) -> Result<TreeId<Typed>, TyperError> {
        self.run_expression_transaction(|typer, info_journal, new_mappings| {
            typer.type_expression_expected_inner(
                tree,
                context,
                expected,
                info_journal,
                new_mappings,
            )
        })
    }

    /// Returns the value type represented by a typed expression's own type.
    ///
    /// Literal expressions preserve their exact `Type::Constant` type in the
    /// typed AST; consumers use this operation when they need the canonical
    /// builtin value type. A member reference is adapted to its receiver view.
    /// Adapting a reference may complete source symbols and allocate semantic
    /// types; failures roll those changes back.
    pub fn widen_expression_type(&mut self, ty: TypeId) -> Result<TypeId, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let mut info_journal = Vec::new();
        let result = self.widen_expression_type_journaled(ty, &mut info_journal, 0);
        if result.is_err() {
            for (symbol, previous) in info_journal.into_iter().rev() {
                if self.store.symbols.contains(symbol) {
                    self.store.symbols.set_info(symbol, previous);
                }
            }
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(store_checkpoint);
            self.type_index.restore(type_index_checkpoint);
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
            TreeKind::Literal(literal) => {
                self.type_literal_expression(tree, literal, source_tree.position)
            }
            TreeKind::PhaseSpecific(UntypedNode::Number(number)) => {
                self.type_number_literal_expression(tree, number, source_tree.position)
            }
            TreeKind::This(this) => {
                self.type_this_expression(tree, this, source_tree.position, context)
            }
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

    /// Returns the typer-owned type parameter symbol for a local method tree.
    pub fn local_method_type_parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.local_methods.type_parameter_symbol_at(source, tree)
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
                Modifier::Abstract => SymbolFlags::ABSTRACT,
                Modifier::Final => SymbolFlags::FINAL,
                Modifier::Implicit => SymbolFlags::IMPLICIT,
                Modifier::Given => SymbolFlags::GIVEN,
                Modifier::Override => SymbolFlags::OVERRIDE,
                Modifier::Inline => SymbolFlags::INLINE,
                Modifier::Transparent => SymbolFlags::TRANSPARENT,
                Modifier::Extension => SymbolFlags::EXTENSION,
                Modifier::Erased => SymbolFlags::ERASED,
                _ => SymbolFlags::EMPTY,
            };
            flags | flag
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{
        Name, Namespace, SourceText, SymbolFlags, SymbolLinks, SymbolOrigin, TermName, Visibility,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;
    use std::{cell::RefCell, rc::Rc};

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
                    .expect("test method should have a declaration context")
            }),
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
        let typed = typer.type_expression(rhs, context)?;
        let node = typer.typed_ast().get(typed);
        let kind = node.kind.clone();
        let own_type_id = node.ty;
        let widened_id = typer.widen_expression_type(own_type_id)?;
        Ok((
            kind,
            typer.store().types.get(own_type_id).clone(),
            widened_id,
            typer.store().types.get(widened_id).clone(),
            definitions,
        ))
    }

    fn type_symbol(store: &SemanticStore, ty: TypeId) -> SymbolId {
        match store.types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a symbol TypeRef, got {other:?}"),
        }
    }

    fn applied_class_type(
        store: &mut SemanticStore,
        definitions: Definitions,
        class: SymbolId,
        arguments: &[TypeId],
    ) -> TypeId {
        applied_class_type_with_prefix(store, class, definitions.no_prefix, arguments)
    }

    fn applied_class_type_with_prefix(
        store: &mut SemanticStore,
        class: SymbolId,
        prefix: TypeId,
        arguments: &[TypeId],
    ) -> TypeId {
        let tycon = store.types.alloc(Type::type_ref(prefix, class));
        store.types.alloc(Type::Applied {
            tycon,
            args: arguments.to_vec(),
        })
    }

    fn parent_prefix_for(
        store: &SemanticStore,
        class: SymbolId,
        expected_parent: SymbolId,
    ) -> TypeId {
        let SymbolInfo::Complete(info) = *store.symbols.info(class) else {
            panic!("class info for {class:?} is not complete");
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            panic!("class info for {class:?} does not reference ClassInfo");
        };
        let Some(parent) = info.parents.iter().find(|parent| {
            let tycon = match store.types.get(**parent) {
                Type::Applied { tycon, .. } => *tycon,
                _ => **parent,
            };
            matches!(
                store.types.get(tycon),
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } if *symbol == expected_parent
            )
        }) else {
            panic!("class {class:?} has no parent view");
        };
        let tycon = match store.types.get(*parent) {
            Type::Applied { tycon, .. } => *tycon,
            _ => *parent,
        };
        match store.types.get(tycon) {
            Type::TypeRef { prefix, .. } => *prefix,
            other => panic!("parent view has unexpected type constructor {other:?}"),
        }
    }

    fn nominal_type_ref(
        store: &mut SemanticStore,
        definitions: Definitions,
        class: SymbolId,
    ) -> TypeId {
        nominal_type_ref_with_prefix(store, class, definitions.no_prefix)
    }

    fn nominal_type_ref_with_prefix(
        store: &mut SemanticStore,
        class: SymbolId,
        prefix: TypeId,
    ) -> TypeId {
        store.types.alloc(Type::type_ref(prefix, class))
    }

    fn candidate_for(
        typer: &mut SourceTyper<'_>,
        receiver: TypeId,
        class: SymbolId,
        name: Name,
    ) -> MemberCandidate {
        typer
            .lookup_members(receiver, name)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.declaring_class == class)
            .unwrap()
    }

    fn type_parameter_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        name: &str,
    ) -> SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source type parameter `{name}` not found"))
    }

    fn class_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        name: &str,
    ) -> SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    index.symbol_at(source, tree).filter(|symbol| {
                        matches!(
                            store.symbols.get(*symbol).kind,
                            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                        )
                    })
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source class-like symbol `{name}` not found"))
    }

    fn type_alias_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        name: &str,
    ) -> SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeAlias
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source type alias `{name}` not found"))
    }

    fn complete_builtin_annotation(name: &str) -> (TypeId, Definitions) {
        let text = format!("val x: {name} = 1");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (symbol, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let ty = typer.complete_symbol(symbol).unwrap();
        (ty, definitions)
    }

    #[test]
    fn constructor_uses_caller_bootstrapped_definitions() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);

        let typer = SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert_eq!(typer.definitions.int, definitions.int);
    }

    #[test]
    fn subtype_relation_is_reflexive_for_canonical_primitive_types() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(typer.is_subtype(definitions.int, definitions.int).unwrap());
        assert!(typer.conforms(definitions.int, definitions.int).unwrap());
    }

    #[test]
    fn distinct_builtin_primitive_types_do_not_conform() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(
            !typer
                .conforms(definitions.boolean, definitions.int)
                .unwrap()
        );
    }

    #[test]
    fn semantic_type_refs_equivalent_across_distinct_type_ids() {
        let (arena, mut store, packages, definitions) = setup();
        let symbol = type_symbol(&store, definitions.int);
        let prefix = store.types.alloc(Type::NoPrefix);
        let duplicate = store.types.alloc(Type::type_ref(prefix, symbol));
        assert_ne!(definitions.int, duplicate);
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(typer.is_subtype(definitions.int, duplicate).unwrap());
    }

    #[test]
    fn same_class_references_with_distinct_prefixes_are_not_equivalent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C; class First; class Second");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let first = class_symbol(&parsed, &store, &index, source, "First");
        let second = class_symbol(&parsed, &store, &index, source, "Second");
        let first_prefix = store.types.alloc(Type::ThisType { class: first });
        let second_prefix = store.types.alloc(Type::ThisType { class: second });
        let first_reference = store.types.alloc(Type::type_ref(first_prefix, class));
        let second_reference = store.types.alloc(Type::type_ref(second_prefix, class));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(!typer.is_subtype(first_reference, second_reference).unwrap());
    }

    #[test]
    fn bottom_and_top_rules_use_canonical_definitions() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(
            typer
                .is_subtype(definitions.nothing_type, definitions.int)
                .unwrap()
        );
        assert!(
            typer
                .is_subtype(definitions.int, definitions.any_type)
                .unwrap()
        );
    }

    #[test]
    fn unrelated_source_classes_do_not_conform_without_inheritance() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A; class B");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let b_type = nominal_type_ref(&mut store, definitions, b);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        assert!(!typer.is_subtype(a_type, b_type).unwrap());
    }

    #[test]
    fn source_class_conforms_to_direct_parent_after_explicit_completion() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Parent; class Child extends Parent");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = nominal_type_ref(&mut store, definitions, child);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        let parent_prefix = parent_prefix_for(typer.store, child, parent);
        let parent_type = nominal_type_ref_with_prefix(typer.store, parent, parent_prefix);
        assert!(typer.is_subtype(child_type, parent_type).unwrap());
    }

    #[test]
    fn source_class_conforms_to_trait_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait Parent; class Child extends Parent");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = nominal_type_ref(&mut store, definitions, child);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        let parent_prefix = parent_prefix_for(typer.store, child, parent);
        let parent_type = nominal_type_ref_with_prefix(typer.store, parent, parent_prefix);
        assert!(typer.is_subtype(child_type, parent_type).unwrap());
    }

    #[test]
    fn source_class_conforms_to_multilevel_parent() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Grandparent; class Parent extends Grandparent; class Child extends Parent",
        );
        let grandparent = class_symbol(&parsed, &store, &index, source, "Grandparent");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = nominal_type_ref(&mut store, definitions, child);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        typer.complete_symbol(grandparent).unwrap();
        let grandparent_prefix = parent_prefix_for(typer.store, parent, grandparent);
        let grandparent_type =
            nominal_type_ref_with_prefix(typer.store, grandparent, grandparent_prefix);
        assert!(typer.is_subtype(child_type, grandparent_type).unwrap());
    }

    #[test]
    fn instantiated_parent_view_preserves_generic_arguments() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Parent[A]; class Child[B] extends Parent[B]");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = applied_class_type(&mut store, definitions, child, &[definitions.int]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        let parent_prefix = parent_prefix_for(typer.store, child, parent);
        let parent_type =
            applied_class_type_with_prefix(typer.store, parent, parent_prefix, &[definitions.int]);
        let wrong_parent_type = applied_class_type_with_prefix(
            typer.store,
            parent,
            parent_prefix,
            &[definitions.boolean],
        );
        assert!(typer.is_subtype(child_type, parent_type).unwrap());
        assert!(!typer.is_subtype(child_type, wrong_parent_type).unwrap());
    }

    #[test]
    fn missing_class_info_is_reported_without_implicit_completion() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A; class B");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let b_type = nominal_type_ref(&mut store, definitions, b);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.is_subtype(a_type, b_type),
            Err(TypeRelationError::ClassInfoUnavailable {
                symbol,
                state: crate::types::SymbolInfoState::Missing,
            }) if symbol == a
        ));
        assert!(matches!(*typer.store.symbols.info(a), SymbolInfo::Missing));
        assert!(matches!(*typer.store.symbols.info(b), SymbolInfo::Missing));
    }

    #[test]
    fn inheritance_cycles_are_reported_for_unrelated_targets() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A extends B; class B extends A; class C");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let c = class_symbol(&parsed, &store, &index, source, "C");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let c_type = nominal_type_ref(&mut store, definitions, c);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        typer.complete_symbol(c).unwrap();
        assert!(matches!(
            typer.is_subtype(a_type, c_type),
            Err(TypeRelationError::InheritanceCycle { symbol }) if symbol == a
        ));
    }

    #[test]
    fn matching_parent_does_not_hide_a_cycle_closing_through_that_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A extends B; class B extends A");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let b_type = nominal_type_ref(&mut store, definitions, b);
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        assert!(matches!(
            typer.is_subtype(b_type, a_type),
            Err(TypeRelationError::InheritanceCycle { symbol }) if symbol == b
        ));
    }

    #[test]
    fn cycle_across_deduplicated_parent_branches_is_reported() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A extends B, C; class B extends C; class C extends B; class Unrelated",
        );
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let c = class_symbol(&parsed, &store, &index, source, "C");
        let unrelated = class_symbol(&parsed, &store, &index, source, "Unrelated");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let unrelated_type = nominal_type_ref(&mut store, definitions, unrelated);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        typer.complete_symbol(c).unwrap();
        typer.complete_symbol(unrelated).unwrap();
        assert!(matches!(
            typer.is_subtype(a_type, unrelated_type),
            Err(TypeRelationError::InheritanceCycle { symbol }) if symbol == b || symbol == c
        ));
    }

    #[test]
    fn this_type_conforms_to_its_own_class_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let this = store.types.alloc(Type::ThisType { class });
        let class_type = nominal_type_ref(&mut store, definitions, class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(typer.is_subtype(this, class_type).unwrap());
    }

    #[test]
    fn this_type_conforms_to_its_class_with_a_canonical_package_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("package p { class C; val value: C = 1 }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let this = store.types.alloc(Type::ThisType { class });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let class_type = typer.complete_symbol(value).unwrap();
        assert!(typer.is_subtype(this, class_type).unwrap());
    }

    #[test]
    fn nested_this_type_does_not_conform_through_a_different_outer_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }; class Other");
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let inner = class_symbol(&parsed, &store, &index, source, "Inner");
        let this_inner = store.types.alloc(Type::ThisType { class: inner });
        let other_prefix = store.types.alloc(Type::ThisType { class: other });
        let inner_through_other = store.types.alloc(Type::type_ref(other_prefix, inner));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(!typer.is_subtype(this_inner, inner_through_other).unwrap());
        assert_ne!(typer.store.symbols.get(inner).owner, Some(other));
        assert_eq!(typer.store.symbols.get(inner).owner, Some(outer));
    }

    #[test]
    fn nested_this_type_conforms_through_its_canonical_owner_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner; val value: Inner = null }");
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let inner = class_symbol(&parsed, &store, &index, source, "Inner");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let this_inner = store.types.alloc(Type::ThisType { class: inner });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let canonical_inner = typer.complete_symbol(value).unwrap();
        let Type::TypeRef { prefix, .. } = typer.store().types.get(canonical_inner) else {
            panic!("expected a qualified nested class reference")
        };
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: outer }
        );
        assert!(typer.is_subtype(this_inner, canonical_inner).unwrap());
    }

    #[test]
    fn class_reference_does_not_conform_to_its_this_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let this = store.types.alloc(Type::ThisType { class });
        let class_type = nominal_type_ref(&mut store, definitions, class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(!typer.is_subtype(class_type, this).unwrap());
    }

    #[test]
    fn invariant_applied_types_accept_equivalent_arguments_only() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let int_box = applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let same_int_box =
            applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let any_box =
            applied_class_type(&mut store, definitions, box_class, &[definitions.any_type]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(typer.is_subtype(int_box, same_int_box).unwrap());
        assert!(!typer.is_subtype(int_box, any_box).unwrap());
    }

    #[test]
    fn external_applied_generic_subtyping_is_explicitly_deferred() {
        let (arena, mut store, packages, definitions) = setup();
        let external = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let applied = applied_class_type(&mut store, definitions, external, &[definitions.int]);
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.is_subtype(applied, definitions.int),
            Err(TypeRelationError::ExternalGenericInstantiationDeferred { class })
                if class == external
        ));
    }

    #[test]
    fn invariant_applied_types_reject_malformed_arity_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let int_box = applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let raw_box = applied_class_type(&mut store, definitions, box_class, &[]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.is_subtype(int_box, raw_box),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
    }

    #[test]
    fn minimal_union_relations_cover_left_and_right_union_rules() {
        let (arena, mut store, packages, definitions) = setup();
        let union = store.types.alloc(Type::Or {
            left: definitions.int,
            right: definitions.boolean,
        });
        let nested_union = store.types.alloc(Type::Or {
            left: union,
            right: definitions.boolean,
        });
        let intersection = store.types.alloc(Type::And {
            left: definitions.int,
            right: definitions.boolean,
        });
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(!typer.is_subtype(union, definitions.int).unwrap());
        assert!(typer.is_subtype(union, definitions.any_type).unwrap());
        assert!(typer.is_subtype(definitions.int, union).unwrap());
        assert!(typer.is_subtype(definitions.boolean, union).unwrap());
        assert!(matches!(
            typer.is_subtype(intersection, definitions.any_type),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
        assert!(
            typer
                .is_subtype(nested_union, definitions.any_type)
                .unwrap()
        );
        assert!(typer.is_subtype(definitions.int, nested_union).unwrap());
    }

    #[test]
    fn union_relation_rejects_cycles_and_excessive_nesting() {
        let (arena, mut store, packages, definitions) = setup();
        let reserved = store.types.reserve();
        let recursive_union = reserved.id();
        store.types.fill(
            reserved,
            Type::Or {
                left: recursive_union,
                right: definitions.int,
            },
        );
        let mut nested = definitions.int;
        for _ in 0..MAX_TYPE_RELATION_DEPTH {
            nested = store.types.alloc(Type::Or {
                left: nested,
                right: definitions.boolean,
            });
        }
        let mut wide = store.types.alloc(Type::Or {
            left: definitions.int,
            right: definitions.boolean,
        });
        for _ in 0..14 {
            wide = store.types.alloc(Type::Or {
                left: wide,
                right: wide,
            });
        }
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.is_subtype(recursive_union, definitions.any_type),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
        assert!(matches!(
            typer.is_subtype(nested, definitions.any_type),
            Err(TypeRelationError::TooDeep)
        ));
        assert!(matches!(
            typer.is_subtype(wide, definitions.any_type),
            Err(TypeRelationError::TooManyUnionRelations)
        ));
    }

    #[test]
    fn non_equivalent_by_name_relations_are_explicitly_unsupported() {
        let (arena, mut store, packages, definitions) = setup();
        let int_by_name = store.types.alloc(Type::ByName {
            result: definitions.int,
        });
        let boolean_by_name = store.types.alloc(Type::ByName {
            result: definitions.boolean,
        });
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.is_subtype(int_by_name, boolean_by_name),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
        assert!(matches!(
            typer.is_subtype(int_by_name, definitions.any_type),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
    }

    #[test]
    fn empty_class_publishes_class_info_with_its_existing_scope_and_object_parent() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let scope = index.scope_of(class).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.class, class);
        assert_eq!(info.prefix, definitions.no_prefix);
        assert_eq!(info.declarations, scope);
        assert_eq!(info.parents, vec![definitions.object_type]);
        assert_eq!(info.self_type, None);
    }

    #[test]
    fn member_lookup_completes_a_missing_class_from_the_current_source() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1 }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        assert_eq!(*store.symbols.info(class), SymbolInfo::Missing);
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let scope = index.scope_of(class).unwrap();
        let field = store.scopes.get(scope).lookup(&name).unwrap();
        let receiver = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidates = typer.lookup_members(receiver, name).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol, field);
        assert_eq!(candidates[0].declaring_class, class);
        assert!(matches!(
            *typer.store().symbols.info(class),
            SymbolInfo::Complete(_)
        ));
    }

    #[test]
    fn generic_method_result_is_adapted_to_the_receiver_argument() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def get: A = null }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let canonical_parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        assert_eq!(
            store.symbols.get(canonical_parameter).owner,
            Some(box_class)
        );
        let (class_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Box" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(index.symbol_at(source, class_tree), Some(box_class));
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected the generic class template")
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("expected the synthetic primary constructor")
        };
        let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
        let constructor_parameter = index
            .derived_symbol_at(constructor_symbol, source, constructor.type_params[0])
            .unwrap();
        assert_ne!(constructor_parameter, canonical_parameter);
        let name = Name::new(store.names.intern("get"), Namespace::Term);
        let get = store
            .scopes
            .get(index.scope_of(box_class).unwrap())
            .lookup(&name)
            .unwrap();
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        assert_eq!(type_symbol(typer.store(), adapted), text_class);
        let SymbolInfo::Complete(declaration) = *typer.store().symbols.info(get) else {
            panic!("member signature should be completed")
        };
        assert_eq!(type_symbol(typer.store(), declaration), canonical_parameter);
    }

    #[test]
    fn generic_field_type_is_adapted_without_changing_its_declaration() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = null }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let value = store
            .scopes
            .get(index.scope_of(box_class).unwrap())
            .lookup(&name)
            .unwrap();
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        assert_eq!(type_symbol(typer.store(), adapted), text_class);
        let SymbolInfo::Complete(declaration) = *typer.store().symbols.info(value) else {
            panic!("field signature should be completed")
        };
        assert_eq!(type_symbol(typer.store(), declaration), parameter);
        assert_ne!(adapted, declaration);
    }

    #[test]
    fn generic_member_arguments_follow_primary_constructor_source_order() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B] { val second: B = null }; class Text; class NumberText",
        );
        let pair = class_symbol(&parsed, &store, &index, source, "Pair");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let number_text = class_symbol(&parsed, &store, &index, source, "NumberText");
        let b = type_parameter_symbol(&parsed, &store, &index, source, "B");
        assert_eq!(store.symbols.get(b).owner, Some(pair));
        let name = Name::new(store.names.intern("second"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let number_ty = nominal_type_ref(&mut store, definitions, number_text);
        let receiver = applied_class_type(&mut store, definitions, pair, &[text_ty, number_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, pair, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        assert_eq!(type_symbol(typer.store(), adapted), number_text);
    }

    #[test]
    fn nested_applied_member_types_are_substituted_recursively() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A] { val values: List[A] = null }; class List[X]; class Text",
        );
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let list_class = class_symbol(&parsed, &store, &index, source, "List");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("values"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(adapted) else {
            panic!("expected the substituted List application")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), list_class);
        assert_eq!(args.len(), 1);
        assert_eq!(type_symbol(typer.store(), args[0]), text_class);
    }

    #[test]
    fn class_substitution_keeps_a_generic_methods_own_binder_intact() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def convert[B](value: B): A = value }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("convert"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        let Type::Poly(poly) = typer.store().types.get(adapted) else {
            panic!("expected the method's type-parameter binder")
        };
        let Type::Method(method) = typer.store().types.get(poly.result) else {
            panic!("expected the method parameter clause")
        };
        let Type::ParamRef {
            binder: parameter_binder,
            index: 0,
        } = typer.store().types.get(method.params[0].ty)
        else {
            panic!("method-owned B must remain a ParamRef")
        };
        assert_eq!(*parameter_binder, adapted);
        assert_eq!(type_symbol(typer.store(), method.result), text_class);
    }

    #[test]
    fn generic_source_receivers_must_supply_exact_arity() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Pair[A, B] { val second: B = null }; class Text");
        let pair = class_symbol(&parsed, &store, &index, source, "Pair");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("second"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, pair, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, pair, name);
        assert!(matches!(
            typer.member_type_on(&candidate),
            Err(TyperError::ReceiverGenericArityMismatch {
                class,
                expected: 2,
                actual: 1,
            }) if class == pair
        ));
    }

    #[test]
    fn raw_source_generic_receivers_are_rejected_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = null }");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let receiver = nominal_type_ref(&mut store, definitions, box_class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        assert!(matches!(
            typer.member_type_on(&candidate),
            Err(TyperError::RawGenericSourceReceiverUnsupported {
                class,
                expected: 1,
            }) if class == box_class
        ));
    }

    #[test]
    fn by_name_member_parameters_are_substituted() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def consume(value: => A): A = value }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("consume"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Method(method) = typer.store().types.get(adapted) else {
            panic!("expected a term method")
        };
        let Type::ByName { result } = typer.store().types.get(method.params[0].ty) else {
            panic!("expected a by-name parameter")
        };
        assert_eq!(type_symbol(typer.store(), *result), text);
        assert_eq!(type_symbol(typer.store(), method.result), text);
    }

    #[test]
    fn member_type_bounds_are_substituted() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { type T >: A <: A }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("T"), Namespace::Type);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Bounds { low, high } = typer.store().types.get(adapted) else {
            panic!("expected member type bounds")
        };
        assert_eq!(type_symbol(typer.store(), *low), text);
        assert_eq!(type_symbol(typer.store(), *high), text);
    }

    #[test]
    fn external_applied_generic_member_adaptation_is_deferred() {
        let (arena, mut store, packages, definitions) = setup();
        let external_class = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let external_member = symbol(
            &mut store,
            SymbolKind::Method,
            SymbolInfo::Complete(definitions.int),
        );
        let receiver =
            applied_class_type(&mut store, definitions, external_class, &[definitions.int]);
        let candidate = MemberCandidate {
            symbol: external_member,
            declaring_class: external_class,
            receiver_view: receiver,
            inheritance_depth: 0,
        };
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.member_type_on(&candidate),
            Err(TyperError::ExternalGenericInstantiationDeferred { class })
                if class == external_class
        ));
    }

    #[test]
    fn member_lookup_completes_a_missing_current_source_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base { val value: Int = 1 }; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let base_scope = index.scope_of(base).unwrap();
        let field = store.scopes.get(base_scope).lookup(&name).unwrap();
        let receiver = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(child),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidates = typer.lookup_members(receiver, name).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol, field);
        assert_eq!(candidates[0].declaring_class, base);
        assert!(matches!(
            *typer.store().symbols.info(base),
            SymbolInfo::Complete(_)
        ));
    }

    #[test]
    fn inherited_parent_arguments_are_instantiated_from_the_child_receiver() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent[A] { val value: A = null }; class Child[B] extends Parent[B]; class Text",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, child, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, parent, name);
        let Type::Applied { tycon, args } = typer.store().types.get(candidate.receiver_view) else {
            panic!("expected an instantiated parent view")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), parent);
        assert_eq!(type_symbol(typer.store(), args[0]), text);
        let adapted = typer.member_type_on(&candidate).unwrap();
        assert_eq!(type_symbol(typer.store(), adapted), text);
    }

    #[test]
    fn inherited_nested_parent_arguments_are_instantiated_before_member_lookup() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent[A] { val value: A = null }; class List[X]; class Child[B] extends Parent[List[B]]; class Text",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let list = class_symbol(&parsed, &store, &index, source, "List");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, child, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, parent, name);
        let Type::Applied { args, .. } = typer.store().types.get(candidate.receiver_view) else {
            panic!("expected the applied parent view")
        };
        let Type::Applied {
            tycon: nested_tycon,
            args: nested_args,
        } = typer.store().types.get(args[0])
        else {
            panic!("expected a nested List argument")
        };
        assert_eq!(type_symbol(typer.store(), *nested_tycon), list);
        assert_eq!(type_symbol(typer.store(), nested_args[0]), text);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Applied { args, .. } = typer.store().types.get(adapted) else {
            panic!("expected the inherited member's List result")
        };
        assert_eq!(type_symbol(typer.store(), args[0]), text);
    }

    #[test]
    fn multi_level_parent_views_compose_class_substitutions() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent[A] { val value: A = null }; class List[X]; class Middle[C] extends Parent[List[C]]; class Child[B] extends Middle[B]; class Text",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let list = class_symbol(&parsed, &store, &index, source, "List");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, child, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, parent, name);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Applied { tycon, args } = typer.store().types.get(adapted) else {
            panic!("expected the composed List member type")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), list);
        assert_eq!(type_symbol(typer.store(), args[0]), text);
    }

    #[test]
    fn explicit_class_parent_is_preserved() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let child_info = typer.complete_symbol(child).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(child_info) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 1);
        assert_eq!(type_symbol(typer.store(), info.parents[0]), base);
    }

    #[test]
    fn class_with_trait_only_parent_gets_object_as_its_real_class_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T; class C extends T");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 2);
        assert_eq!(info.parents[0], definitions.object_type);
        assert_eq!(type_symbol(typer.store(), info.parents[1]), trait_symbol);
    }

    #[test]
    fn trait_alias_parent_gets_object_before_the_alias_reference() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T\nobject O { type Alias = T; class C extends Alias }");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        assert!(matches!(
            *typer.store().symbols.info(alias),
            SymbolInfo::Complete(info)
                if matches!(typer.store().types.get(info), Type::AliasingBounds { alias: target }
                    if type_symbol(typer.store(), *target) == trait_symbol)
        ));
        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 2);
        assert_eq!(info.parents[0], definitions.object_type);
        assert_eq!(type_symbol(typer.store(), info.parents[1]), alias);
    }

    #[test]
    fn class_alias_parent_does_not_get_an_unneeded_object_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base\nobject O { type Alias = Base; class C extends Alias }");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 1);
        assert_eq!(type_symbol(typer.store(), info.parents[0]), alias);
    }

    #[test]
    fn type_parameter_parent_with_unknown_class_kind_is_deferred_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A] extends A");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let parent_tree_index = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "A" =>
                {
                    Some(tree.index())
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

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::UnresolvedParentClassKind {
                source: error_source,
                tree_index,
                symbol: Some(error_symbol),
            }) if error_source == source
                && tree_index == parent_tree_index
                && error_symbol == parameter
        ));
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn class_type_parameters_complete_before_generic_parent_projection() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base[A]; class Child[A] extends Base[A]");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A"
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                                && store.symbols.get(symbol).owner == Some(child)
                        }) =>
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

        let info_id = typer.complete_symbol(child).unwrap();

        assert!(matches!(
            *typer.store().symbols.info(child_parameter),
            SymbolInfo::Complete(_)
        ));
        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        let Type::Applied { tycon, args } = typer.store().types.get(info.parents[0]) else {
            panic!("expected applied generic parent");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), base);
        assert_eq!(args.len(), 1);
        assert_eq!(type_symbol(typer.store(), args[0]), child_parameter);
    }

    #[test]
    fn constructor_arguments_do_not_change_class_parent_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base(x: Int); class Child extends Base(1)");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(child).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 1);
        assert_eq!(type_symbol(typer.store(), info.parents[0]), base);
    }

    #[test]
    fn outer_parent_type_apply_keeps_its_arguments_after_an_applied_tpt() {
        // Synthetic source AST: the parser represents ordinary nested type
        // applications as AppliedTypeTree, while this explicit TypeApply
        // exercises the distinct constructor-call wrapper shape.
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base; val seed: Int = 0; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let context = index.declaration_context_of(child).unwrap();
        let base_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Base" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let int_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Int" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let inner = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                tpt: base_tpt,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let outer = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::TypeApply(dotty_core::ast::TypeApply {
                function: inner,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer
            .project_parent_type(outer, context, 0, &mut Vec::new())
            .unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("the outer TypeApply must be preserved");
        };
        assert_eq!(args.len(), 1);
        let Type::Applied {
            tycon: inner_tycon,
            args: inner_args,
        } = typer.store().types.get(*tycon)
        else {
            panic!("the inner applied type tree must be preserved");
        };
        assert_eq!(args[0], inner_args[0]);
        assert_eq!(type_symbol(typer.store(), *inner_tycon), base);
    }

    #[test]
    fn parent_constructor_type_apply_does_not_repeat_applied_tpt_arguments() {
        // Synthetic source AST mirroring Dotty's constructor-call wrapper:
        // `TypeApply(Apply(Select(New(Base[Int]), <init>), ...), [Int])`.
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base[A]; val seed: Int = 0; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let context = index.declaration_context_of(child).unwrap();
        let base_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Base" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let int_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Int" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let applied_tpt = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                tpt: base_tpt,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let new_tree = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::New(dotty_core::ast::New { tpt: applied_tpt }),
            position: None,
            ty: (),
        });
        let constructor_name = *TermName::new(store.names.intern("<init>")).as_name();
        let selected_constructor = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::Select(dotty_core::ast::Select {
                qualifier: new_tree,
                name: constructor_name,
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let constructor_call = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::Apply(dotty_core::ast::Apply {
                function: selected_constructor,
                args: vec![int_tpt],
                kind: dotty_core::ast::ApplyKind::Regular,
            }),
            position: None,
            ty: (),
        });
        let parent = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::TypeApply(dotty_core::ast::TypeApply {
                function: constructor_call,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer
            .project_parent_type(parent, context, 0, &mut Vec::new())
            .unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("expected one application from the New type tree");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), base);
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn explicit_self_type_is_projected_without_adding_a_self_member() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait SelfType\n\nclass C:\n  self: SelfType =>\n");
        let self_type = class_symbol(&parsed, &store, &index, source, "SelfType");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let declaration_scope = index.scope_of(class).unwrap();
        let self_name = Name::new(store.names.intern("self"), Namespace::Term);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(
            type_symbol(typer.store(), info.self_type.unwrap()),
            self_type
        );
        assert!(
            typer
                .store()
                .scopes
                .get(declaration_scope)
                .lookup_all(&self_name)
                .is_empty()
        );
    }

    #[test]
    fn self_alias_without_a_type_has_no_additional_self_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { self => }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.self_type, None);
    }

    #[test]
    fn nested_class_uses_normalized_no_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }");
        let inner = class_symbol(&parsed, &store, &index, source, "Inner");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(inner).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.prefix, definitions.no_prefix);
    }

    #[test]
    fn trait_without_source_parent_gets_scala_390_object_parent() {
        // The Scala 3.9.0 ClassInfos.scala oracle fixture declares
        // `trait InfoBase[A]` without a parent; its TASTy ClassInfo has one
        // parent entry, java.lang.Object. Source completion preserves it.
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T[A]");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(trait_symbol).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents, vec![definitions.object_type]);
    }

    #[test]
    fn class_completion_leaves_ordinary_members_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val field: Int = 1; def method: Int = 1 }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let field = val_symbol(&parsed, &store, &index, source, "field").0;
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method" =>
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

        typer.complete_symbol(class).unwrap();

        assert_eq!(*typer.store().symbols.info(field), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
    }

    #[test]
    fn source_object_module_class_uses_its_template_parents() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T\nobject O extends T");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let (object_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                    if store.names.resolve(module.name.as_name().text()) == "O" =>
                {
                    Some((tree, index.symbol_at(source, tree).unwrap()))
                }
                _ => None,
            })
            .unwrap();
        let module_class = index
            .derived_symbol_at(
                store.symbols.get(object).owner.unwrap(),
                source,
                object_tree,
            )
            .unwrap();
        let module_template = match &parsed.ast.get(object_tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) => module.template,
            _ => unreachable!(),
        };
        let scope = index.scope_of(module_class).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(module_class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ModuleClass ClassInfo");
        };
        assert_eq!(info.class, module_class);
        assert_eq!(info.declarations, scope);
        assert_eq!(info.parents.len(), 2);
        assert_eq!(info.parents[0], definitions.object_type);
        assert_eq!(type_symbol(typer.store(), info.parents[1]), trait_symbol);
        assert_eq!(*typer.store().symbols.info(object), SymbolInfo::Missing);
        assert!(matches!(
            parsed.ast.get(module_template).kind,
            TreeKind::Template(_)
        ));
    }

    #[test]
    fn failed_class_parent_projection_rolls_back_header_parameters() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A] extends MissingParent");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A"
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let parent_name = Name::new(store.names.intern("MissingParent"), Namespace::Type);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let error = typer.complete_symbol(class).unwrap_err();

        assert!(matches!(
            error,
            TyperError::TypeNameNotFound { name, .. } if name == parent_name
        ));
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn failed_self_type_projection_leaves_class_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C:\n  self: MissingSelf =>\n");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let self_name = Name::new(store.names.intern("MissingSelf"), Namespace::Type);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let error = typer.complete_symbol(class).unwrap_err();

        assert!(matches!(
            error,
            TyperError::TypeNameNotFound { name, .. } if name == self_name
        ));
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn repeated_class_completion_reuses_its_published_info() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.complete_symbol(class).unwrap();
        let second = typer.complete_symbol(class).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn class_completion_rejects_non_class_existing_info() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let malformed = store.types.alloc(Type::NoType);
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn class_completion_rejects_existing_info_for_another_class() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C; class Other");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let scope = index.scope_of(class).unwrap();
        let malformed = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class: other,
            parents: vec![definitions.object_type],
            declarations: scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn class_completion_rejects_existing_info_with_noncanonical_prefix() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let scope = index.scope_of(class).unwrap();
        let malformed = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.object_type,
            class,
            parents: vec![definitions.object_type],
            declarations: scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn class_completion_rejects_existing_info_with_another_declaration_scope() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let malformed_scope = store.scopes.alloc(dotty_core::Scope::new(Some(class)));
        let malformed = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: vec![definitions.object_type],
            declarations: malformed_scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn unbounded_type_parameter_uses_canonical_nothing_and_any() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
    }

    #[test]
    fn type_parameter_completes_explicit_lower_and_upper_bounds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class L; class H; def id[A >: L <: H](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let class_symbol = |wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == wanted =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let expected_low = class_symbol("L");
        let expected_high = class_symbol("H");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if type_symbol(typer.store(), *low) == expected_low
                    && type_symbol(typer.store(), *high) == expected_high
        ));
    }

    #[test]
    fn type_parameter_with_only_upper_bound_uses_nothing_as_lower_bound() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class H; def id[A <: H](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let expected_high = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "H" =>
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

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type
                    && type_symbol(typer.store(), *high) == expected_high
        ));
    }

    #[test]
    fn type_parameter_with_only_lower_bound_uses_any_as_upper_bound() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class L; def id[A >: L](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let expected_low = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "L" =>
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

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if type_symbol(typer.store(), *low) == expected_low
                    && *high == definitions.any_type
        ));
    }

    #[test]
    fn context_bound_type_parameter_completes_ordinary_bounds_only() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A: Evidence](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let parameter_tree = index.definition_of(parameter).unwrap();
        let SourceDefinition::Canonical { tree, .. } = parameter_tree else {
            panic!("canonical source type parameter expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type parameter must be represented by a TypeDef");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(wrapper)) =
            &parsed.ast.get(definition.rhs).kind
        else {
            panic!("parser should preserve the context-bound wrapper");
        };
        assert_eq!(wrapper.context_bounds.len(), 1);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
    }

    #[test]
    fn higher_kinded_type_parameter_completion_is_deferred_without_mutation() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[F[_]](x: F[Int]): Int = 1");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "F");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(parameter).unwrap()
        else {
            panic!("canonical source type parameter expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type parameter must be represented by a TypeDef");
        };
        let rhs = definition.rhs;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(parameter),
            Err(TyperError::HigherKindedTypeParameterDeferred { symbol, tree_index })
                if symbol == parameter && tree_index == rhs.index()
        ));
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn type_parameter_completion_is_idempotent_and_reuses_its_info_id() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](value: A): A = value");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.complete_symbol(parameter).unwrap();
        let after_first = typer.store().checkpoint();
        let second = typer.complete_symbol(parameter).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.store().checkpoint(), after_first);
    }

    #[test]
    fn failed_type_parameter_bounds_roll_back_allocations_and_leave_info_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A <: Missing](value: A): A = value");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(parameter).unwrap()
        else {
            panic!("canonical source type parameter expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type parameter must be represented by a TypeDef");
        };
        let TreeKind::TypeBoundsTree(bounds) = &parsed.ast.get(definition.rhs).kind else {
            panic!("type parameter bounds must be represented by TypeBoundsTree");
        };
        let missing_type = bounds.high.unwrap();
        let missing_position = parsed.ast.get(missing_type).position;
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
            typer.complete_symbol(parameter),
            Err(TyperError::TypeNameNotFound {
                source: found_source,
                tree_index,
                name,
                position,
            }) if found_source == source
                && tree_index == missing_type.index()
                && typer.store().names.resolve(name.text()) == "Missing"
                && position == missing_position
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn failed_alias_bounds_roll_back_singleton_completion_in_outer_journal() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val x: Int = 1; type T >: x.type <: Missing }");
        let x = val_symbol(&parsed, &store, &index, source, "x").0;
        let alias = type_alias_symbol(&parsed, &store, &index, source, "T");
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
            typer.complete_symbol(alias),
            Err(TyperError::TypeNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.store().symbols.info(x), &SymbolInfo::Missing);
    }

    #[test]
    fn failed_alias_bounds_roll_back_nested_stable_field_completions() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val x: Int = 1; val y: x.type = x; type T >: y.type <: Missing }",
        );
        let x = val_symbol(&parsed, &store, &index, source, "x").0;
        let y = val_symbol(&parsed, &store, &index, source, "y").0;
        let alias = type_alias_symbol(&parsed, &store, &index, source, "T");
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
            typer.complete_symbol(alias),
            Err(TyperError::TypeNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.store().symbols.info(x), &SymbolInfo::Missing);
        assert_eq!(typer.store().symbols.info(y), &SymbolInfo::Missing);
    }

    #[test]
    fn source_type_alias_completes_to_aliasing_bounds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("type Alias = Int");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(alias).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::AliasingBounds { alias } if *alias == definitions.int
        ));
    }

    #[test]
    fn source_abstract_type_completes_to_plain_bounds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class L; class H; type Abstract >: L <: H");
        let abstract_type = type_alias_symbol(&parsed, &store, &index, source, "Abstract");
        let bound_symbol = |wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == wanted =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let expected_low = bound_symbol("L");
        let expected_high = bound_symbol("H");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(abstract_type).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if type_symbol(typer.store(), *low) == expected_low
                    && type_symbol(typer.store(), *high) == expected_high
        ));
    }

    #[test]
    fn opaque_source_type_alias_is_deferred_and_stays_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("opaque type Secret = Int");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Secret");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(alias).unwrap() else {
            panic!("canonical source type alias expected");
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
            typer.complete_symbol(alias),
            Err(TyperError::OpaqueAliasDeferred { symbol, tree_index })
                if symbol == alias && tree_index == tree.index()
        ));
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
    }

    #[test]
    fn higher_kinded_source_type_alias_is_deferred_and_stays_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("type F[A] = Option[A]");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "F");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(alias).unwrap() else {
            panic!("canonical source type alias expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type alias must be represented by a TypeDef");
        };
        let rhs = definition.rhs;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(alias),
            Err(TyperError::HigherKindedTypeAliasDeferred { symbol, tree_index })
                if symbol == alias && tree_index == rhs.index()
        ));
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
    }

    #[test]
    fn to_bounds_preserves_existing_aliasing_bounds_id() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let existing = store.types.alloc(Type::AliasingBounds {
            alias: definitions.int,
        });
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let actual = typer.alias_bounds_for_type(existing, 14).unwrap();

        assert_eq!(actual, existing);
    }

    #[test]
    fn to_bounds_rejects_by_name_types_with_exact_error() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let by_name = store.types.alloc(Type::ByName {
            result: definitions.int,
        });
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.alias_bounds_for_type(by_name, 14),
            Err(TyperError::InvalidCompletedBounds { tree_index: 14, ty, .. })
                if ty == by_name
        ));
    }

    #[test]
    fn to_bounds_rejects_method_types_with_exact_error() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let method = store
            .types
            .alloc(Type::Method(dotty_core::types::MethodType {
                params: Vec::new(),
                result: definitions.int,
                kind: dotty_core::types::MethodKind::Plain,
            }));
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.alias_bounds_for_type(method, 19),
            Err(TyperError::InvalidCompletedBounds { tree_index: 19, ty, .. })
                if ty == method
        ));
    }

    #[test]
    fn to_bounds_rejects_polymorphic_types_with_exact_error() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let poly = store.types.alloc(Type::Poly(dotty_core::types::PolyType {
            params: Vec::new(),
            result: definitions.int,
        }));
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.alias_bounds_for_type(poly, 23),
            Err(TyperError::InvalidCompletedBounds { tree_index: 23, ty, .. })
                if ty == poly
        ));
    }

    #[test]
    fn complete_returns_an_existing_type_without_requiring_provenance() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let symbol = symbol(
            &mut store,
            SymbolKind::Value,
            SymbolInfo::Complete(definitions.int),
        );
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Ok(ty) if ty == definitions.int
        ));
    }

    #[test]
    fn complete_class_info_without_source_scope_is_reused_for_external_symbols() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let class = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let declarations = store.scopes.alloc(dotty_core::Scope::new(Some(class)));
        let info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: vec![definitions.object_type],
            declarations,
            self_type: None,
        }));
        store.symbols.set_info(class, SymbolInfo::Complete(info));
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(typer.complete_symbol(class), Ok(completed) if completed == info));
    }

    #[test]
    fn missing_provenance_is_reported_without_mutation() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let symbol = symbol(&mut store, SymbolKind::Value, SymbolInfo::Missing);
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::SourceProvenanceMissing { symbol: found }) if found == symbol
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(symbol), SymbolInfo::Missing);
    }

    #[test]
    fn named_value_completion_projects_and_caches_its_type_tree() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1");
        let (symbol, tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let completed = typer.complete_symbol(symbol).unwrap();
        let context = index.declaration_context_of(symbol).unwrap();
        let projected_again = typer.type_of_tpt(tpt, context).unwrap();

        assert_eq!(completed, projected_again);
        assert_eq!(
            typer.source_type_index().type_at(source, tpt),
            Some(completed)
        );
        assert_eq!(
            *typer.store().symbols.info(symbol),
            SymbolInfo::Complete(completed)
        );
    }

    #[test]
    fn class_type_parameter_is_resolved_in_a_field_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let type_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), type_parameter);
    }

    #[test]
    fn method_parameter_completion_uses_its_method_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](value: A): A = value");
        let method = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "id" =>
                {
                    Some(definition)
                }
                _ => None,
            })
            .unwrap();
        let type_parameter = index.symbol_at(source, method.type_params[0]).unwrap();
        let parameter = index
            .symbol_at(source, method.value_param_clauses[0][0])
            .unwrap();
        assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert_eq!(type_symbol(typer.store(), info), type_parameter);
    }

    #[test]
    fn method_signature_binds_type_and_value_parameters_by_symbol() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](value: A): A = value");
        let (method_tree, method_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "id" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected the method type parameter binder");
        };
        assert_eq!(poly.params.len(), 1);
        let poly_binder = signature;
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("expected the value parameter clause");
        };
        assert_eq!(method_type.kind, MethodKind::Plain);
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == poly_binder
        ));
        assert!(matches!(
            typer.store().types.get(method_type.result),
            Type::ParamRef { binder, index: 0 } if *binder == poly_binder
        ));
        let TreeKind::ValDef(parameter) = &parsed
            .ast
            .get(method_definition.value_param_clauses[0][0])
            .kind
        else {
            panic!("expected a value parameter definition");
        };
        assert_eq!(
            typer
                .store()
                .names
                .resolve(method_type.params[0].name.as_name().text()),
            typer.store().names.resolve(parameter.name.as_name().text())
        );
    }

    #[test]
    fn method_signature_preserves_curried_clause_kinds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(a: Int)(using b: Int)(implicit c: Boolean): Unit = ()");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let mut signature = typer.complete_symbol(method).unwrap();
        let expected_kinds = [
            MethodKind::Plain,
            MethodKind::Contextual,
            MethodKind::Implicit,
        ];
        for (clause_index, expected_kind) in expected_kinds.into_iter().enumerate() {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("expected method clause {clause_index}");
            };
            assert_eq!(method_type.kind, expected_kind);
            assert_eq!(method_type.params.len(), 1);
            assert!(!method_type.params[0].erased);
            signature = method_type.result;
        }
        assert_eq!(signature, definitions.unit);
    }

    #[test]
    fn inconsistent_parameter_flags_fail_method_completion_atomically() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(using first: Int, second: Int): Int = first");
        let (method_tree, method_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let first = index
            .symbol_at(source, method_definition.value_param_clauses[0][0])
            .unwrap();
        let second = index
            .symbol_at(source, method_definition.value_param_clauses[0][1])
            .unwrap();
        let second_flags = store.symbols.get(second).flags;
        store.symbols.get_mut(second).flags =
            second_flags.difference(SymbolFlags::GIVEN) | SymbolFlags::IMPLICIT;
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
            Err(TyperError::MalformedMethodClause {
                method: error_method,
                method_tree_index,
                clause_index: 0
            }) if error_method == method && method_tree_index == method_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(first), SymbolInfo::Missing);
    }

    #[test]
    fn explicit_parameterless_method_signature_is_its_result_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def answer: Int = 42");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "answer" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();
        assert_eq!(typer.complete_symbol(method).unwrap(), signature);

        assert_eq!(signature, definitions.int);
    }

    #[test]
    fn method_signature_preserves_erased_parameter_flags() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(value: Int): Int = value");
        let (method_tree, method_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let parameter = index
            .symbol_at(source, method_definition.value_param_clauses[0][0])
            .unwrap();
        let parameter_flags = store.symbols.get(parameter).flags;
        store.symbols.get_mut(parameter).flags = parameter_flags | SymbolFlags::ERASED;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected a method clause");
        };
        assert!(method_type.params[0].erased);
    }

    #[test]
    fn primary_constructor_without_arguments_constructs_its_missing_owner() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree.0).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_tree.1).kind else {
            panic!("expected a class template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        assert_eq!(store.symbols.get(constructor).kind, SymbolKind::Constructor);
        assert_eq!(*store.symbols.info(class), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected normalized empty constructor clause");
        };
        assert_eq!(method_type.kind, MethodKind::Plain);
        assert!(method_type.params.is_empty());
        assert_eq!(type_symbol(typer.store(), method_type.result), class);
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn repeated_primary_constructor_completion_returns_the_same_type_id() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.complete_symbol(constructor).unwrap();
        let second = typer.complete_symbol(constructor).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn implicit_first_constructor_clause_gets_an_empty_plain_clause_before_it() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(implicit context: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(plain) = typer.store().types.get(signature) else {
            panic!("expected the inserted leading plain clause");
        };
        assert_eq!(plain.kind, MethodKind::Plain);
        assert!(plain.params.is_empty());
        let Type::Method(implicit) = typer.store().types.get(plain.result) else {
            panic!("expected the source implicit clause after the inserted clause");
        };
        assert_eq!(implicit.kind, MethodKind::Implicit);
        assert_eq!(implicit.params.len(), 1);
        assert_eq!(
            typer
                .store()
                .names
                .resolve(implicit.params[0].name.as_name().text()),
            "context"
        );
    }

    #[test]
    fn all_contextual_constructor_clauses_get_a_trailing_empty_plain_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(using context: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(contextual) = typer.store().types.get(signature) else {
            panic!("expected the source contextual clause");
        };
        assert_eq!(contextual.kind, MethodKind::Contextual);
        assert_eq!(contextual.params.len(), 1);
        let Type::Method(plain) = typer.store().types.get(contextual.result) else {
            panic!("expected the normalized trailing plain clause");
        };
        assert_eq!(plain.kind, MethodKind::Plain);
        assert!(plain.params.is_empty());
    }

    #[test]
    fn explicit_empty_constructor_clause_prevents_a_second_normalization_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(using context: Int)()");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let class = store.symbols.get(constructor).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(contextual) = typer.store().types.get(signature) else {
            panic!("expected the source contextual clause");
        };
        assert_eq!(contextual.kind, MethodKind::Contextual);
        let Type::Method(empty) = typer.store().types.get(contextual.result) else {
            panic!("expected the explicit empty clause");
        };
        assert_eq!(empty.kind, MethodKind::Plain);
        assert!(empty.params.is_empty());
        assert_eq!(type_symbol(typer.store(), empty.result), class);
    }

    #[test]
    fn primary_constructor_uses_derived_parameter_separate_from_val_field() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(val value: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let parameter_tree = match &parsed.ast.get(template.constructor).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("expected a primary constructor definition"),
        };
        let field = index.symbol_at(source, parameter_tree).unwrap();
        let parameter = index
            .derived_symbol_at(constructor, source, parameter_tree)
            .unwrap();
        let class = store.symbols.get(field).owner.unwrap();
        assert_ne!(field, parameter);
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected a constructor value clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.params[0].ty, definitions.int);
        assert_eq!(type_symbol(typer.store(), method_type.result), class);
    }

    #[test]
    fn generic_primary_constructor_applies_owner_to_its_derived_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A](val value: A)");
        let (class_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected a class template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("expected a primary constructor definition");
        };
        let class_parameter = index
            .symbol_at(source, constructor_definition.type_params[0])
            .unwrap();
        let constructor_parameter = index
            .derived_symbol_at(constructor, source, constructor_definition.type_params[0])
            .unwrap();
        let value_parameter_tree = constructor_definition.value_param_clauses[0][0];
        let value_field = index.symbol_at(source, value_parameter_tree).unwrap();
        let constructor_value = index
            .derived_symbol_at(constructor, source, value_parameter_tree)
            .unwrap();
        assert_ne!(class_parameter, constructor_parameter);
        assert_ne!(value_field, constructor_value);
        assert_eq!(*store.symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*store.symbols.info(value_field), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected the constructor-owned Poly binder");
        };
        assert_eq!(poly.params.len(), 1);
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("expected the primary constructor value clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        let Type::Applied { tycon, args } = typer.store().types.get(method_type.result) else {
            panic!("expected an applied constructed class result");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        assert_eq!(args.len(), 1);
        assert!(matches!(
            typer.store().types.get(args[0]),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        let SymbolInfo::Complete(constructor_parameter_info) =
            *typer.store().symbols.info(constructor_parameter)
        else {
            panic!("expected the constructor type parameter to be completed");
        };
        assert!(matches!(
            typer.store().types.get(constructor_parameter_info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
        let SymbolInfo::Complete(constructor_value_info) =
            *typer.store().symbols.info(constructor_value)
        else {
            panic!("expected the constructor value parameter to be completed");
        };
        assert_eq!(
            type_symbol(typer.store(), constructor_value_info),
            constructor_parameter
        );
        assert_eq!(
            *typer.store().symbols.info(value_field),
            SymbolInfo::Missing
        );
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn type_clause_is_kept_outer_when_contextual_constructor_clause_is_normalized() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A](using context: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected the constructor type parameter binder");
        };
        let Type::Method(contextual) = typer.store().types.get(poly.result) else {
            panic!("expected the contextual clause after the type clause");
        };
        assert_eq!(contextual.kind, MethodKind::Contextual);
        let Type::Method(plain) = typer.store().types.get(contextual.result) else {
            panic!("expected normalization to add an empty plain clause");
        };
        assert_eq!(plain.kind, MethodKind::Plain);
        assert!(plain.params.is_empty());
    }

    #[test]
    fn object_primary_constructor_result_targets_its_module_class() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("object O");
        let (object_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                    Some((tree, definition.template))
                }
                _ => None,
            })
            .unwrap();
        let object = index.symbol_at(source, object_tree).unwrap();
        let object_owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(object_owner, source, object_tree)
            .unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected an object template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        assert_eq!(store.symbols.get(constructor).owner, Some(module_class));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected the normalized empty object constructor clause");
        };
        assert_eq!(type_symbol(typer.store(), method_type.result), module_class);
    }

    #[test]
    fn constructor_with_non_class_like_owner_fails_without_mutation() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        store.symbols.get_mut(constructor).owner = None;
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
            typer.complete_symbol(constructor),
            Err(TyperError::ConstructorOwnerNotClassLike {
                constructor: error_constructor,
                owner: None
            }) if error_constructor == constructor
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn failed_primary_constructor_parameter_type_rolls_back_everything() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Missing)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let parameter_tree = match &parsed.ast.get(template.constructor).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("expected a primary constructor definition"),
        };
        let parameter = index
            .derived_symbol_at(constructor, source, parameter_tree)
            .unwrap();
        let type_tree = match &parsed.ast.get(parameter_tree).kind {
            TreeKind::ValDef(definition) => definition.tpt,
            _ => panic!("expected a constructor parameter definition"),
        };
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
            typer.complete_symbol(constructor),
            Err(TyperError::TypeNameNotFound {
                source: error_source,
                tree_index,
                ..
            }) if error_source == source && tree_index == type_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn constructor_rejects_malformed_precompleted_owner_info() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let (class_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected a class template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let malformed_info = store
            .types
            .alloc(Type::ClassInfo(dotty_core::types::ClassInfo {
                prefix: definitions.int,
                class,
                parents: Vec::new(),
                declarations: index.scope_of(class).unwrap(),
                self_type: None,
            }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed_info));
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
            typer.complete_symbol(constructor),
            Err(TyperError::MalformedConstructorOwnerInfo {
                constructor: error_constructor,
                owner: error_owner,
                info
            }) if error_constructor == constructor && error_owner == class && info == malformed_info
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn secondary_constructor_completes_its_own_parameter_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int) { def this(other: Int) = this(other) }");
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
        let class = store.symbols.get(constructor).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected the secondary constructor value clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.params[0].ty, definitions.int);
        assert_eq!(type_symbol(typer.store(), method_type.result), class);
    }

    #[test]
    fn secondary_constructor_of_generic_owner_returns_the_applied_owner_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A](value: A) { def this(other: A) = this(other) }");
        let class_parameter_tree = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                        panic!("expected a class template");
                    };
                    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
                    else {
                        panic!("expected the primary constructor");
                    };
                    Some(constructor.type_params[0])
                }
                _ => None,
            })
            .unwrap();
        let class_parameter = index.symbol_at(source, class_parameter_tree).unwrap();
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
        let owner = store.symbols.get(constructor).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("a valid secondary constructor has no constructor type parameters");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(
            type_symbol(typer.store(), method_type.params[0].ty),
            class_parameter
        );
        let Type::Applied { tycon, args } = typer.store().types.get(method_type.result) else {
            panic!("expected the owner type applied to its class parameter");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), owner);
        assert_eq!(args.len(), 1);
        assert_eq!(type_symbol(typer.store(), args[0]), class_parameter);
        assert!(matches!(
            *typer.store().symbols.info(class_parameter),
            SymbolInfo::Complete(_)
        ));
    }

    #[test]
    fn secondary_constructor_owner_parameter_failure_rolls_back_completion() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A <: Missing](value: A) { def this(other: A) = this(other) }");
        let class_parameter_tree = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                        panic!("expected a class template");
                    };
                    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
                    else {
                        panic!("expected the primary constructor");
                    };
                    Some(constructor.type_params[0])
                }
                _ => None,
            })
            .unwrap();
        let class_parameter = index.symbol_at(source, class_parameter_tree).unwrap();
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
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
            typer.complete_symbol(constructor),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(class_parameter),
            SymbolInfo::Missing
        );
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn secondary_constructor_type_parameters_are_explicitly_unsupported() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int) { def this[A](other: A) = this(0) }");
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
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
            typer.complete_symbol(constructor),
            Err(TyperError::SecondaryConstructorTypeParametersUnsupported {
                constructor: error_constructor
            }) if error_constructor == constructor
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn class_field_completes_its_declared_int_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1 }");
        let (field, _) = val_symbol(&parsed, &store, &index, source, "value");
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(field).unwrap();

        assert_eq!(info, definitions.int);
    }

    #[test]
    fn constructor_field_and_derived_parameter_complete_from_the_same_valdef() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(val value: Int)");
        let (class_tree, class_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let class_symbol = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("class definition must own a template");
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("class template must own a primary constructor");
        };
        let parameter_tree = constructor.value_param_clauses[0][0];
        let field = index.symbol_at(source, parameter_tree).unwrap();
        let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
        let derived_parameter = index
            .derived_symbol_at(constructor_symbol, source, parameter_tree)
            .unwrap();
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(
            store.symbols.get(derived_parameter).kind,
            SymbolKind::Parameter
        );
        assert_ne!(field, derived_parameter);
        assert_eq!(store.symbols.get(field).owner, Some(class_symbol));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let field_info = typer.complete_symbol(field).unwrap();
        let parameter_info = typer.complete_symbol(derived_parameter).unwrap();

        assert_eq!(field_info, definitions.int);
        assert_eq!(parameter_info, definitions.int);
        assert_eq!(
            *typer.store().symbols.info(field),
            SymbolInfo::Complete(field_info)
        );
        assert_eq!(
            *typer.store().symbols.info(derived_parameter),
            SymbolInfo::Complete(parameter_info)
        );
    }

    #[test]
    fn extension_receiver_parameter_completes_from_its_source_valdef() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension (value: Int) def identity: Int = value");
        let extension = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some((tree, extension))
                }
                _ => None,
            })
            .unwrap();
        let parameter_tree = extension.1.param_clauses[0][0];
        let method_tree = extension.1.methods[0];
        let method = index.symbol_at(source, method_tree).unwrap();
        let parameter = index
            .derived_symbol_at(method, source, parameter_tree)
            .unwrap();
        assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert_eq!(info, definitions.int);
    }

    #[test]
    fn extension_signature_prepends_contextual_and_receiver_clauses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "extension (using context: Int) (value: Int) def identity(argument: Int): Int = value",
        );
        let extension = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some((tree, extension))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, extension.1.methods[0]).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let mut signature = typer.complete_symbol(method).unwrap();

        for expected_kind in [MethodKind::Contextual, MethodKind::Plain, MethodKind::Plain] {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("expected the next extension method clause");
            };
            assert_eq!(method_type.kind, expected_kind);
            assert_eq!(method_type.params.len(), 1);
            assert_eq!(method_type.params[0].ty, definitions.int);
            signature = method_type.result;
        }
        assert_eq!(signature, definitions.int);
    }

    #[test]
    fn extension_signature_binds_prefix_type_parameters_before_value_clauses() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension [A](value: A) def identity: A = value");
        let extension = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some(extension)
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, extension.methods[0]).unwrap();
        let type_parameter = index
            .derived_symbol_at(method, source, extension.param_clauses[0][0])
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected an extension type parameter binder");
        };
        let Type::Method(receiver) = typer.store().types.get(poly.result) else {
            panic!("expected an extension receiver clause after the type binder");
        };
        assert_eq!(receiver.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(receiver.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(receiver.result),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(poly.params[0].bounds),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
        assert_eq!(
            typer.store().symbols.get(type_parameter).kind,
            SymbolKind::TypeParameter
        );
    }

    #[test]
    fn extension_methods_get_owner_specific_parameter_binders() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "extension (value: Int) { def first: Int = value; def second: Int = value }",
        );
        let extension = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some(extension)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(extension.methods.len(), 2);
        let first = index.symbol_at(source, extension.methods[0]).unwrap();
        let second = index.symbol_at(source, extension.methods[1]).unwrap();
        let receiver_tree = extension.param_clauses[0][0];
        let first_receiver = index
            .derived_symbol_at(first, source, receiver_tree)
            .unwrap();
        let second_receiver = index
            .derived_symbol_at(second, source, receiver_tree)
            .unwrap();
        assert_ne!(first_receiver, second_receiver);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first_signature = typer.complete_symbol(first).unwrap();
        let second_signature = typer.complete_symbol(second).unwrap();

        for signature in [first_signature, second_signature] {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("expected an extension receiver clause");
            };
            assert_eq!(method_type.params.len(), 1);
            assert_eq!(method_type.params[0].ty, definitions.int);
            assert_eq!(method_type.result, definitions.int);
        }
    }

    #[test]
    fn right_associative_extension_signature_is_deferred_without_mutation() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension (value: Int) def +::(other: Int): Int = value");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(_) => Some(tree),
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
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
            Err(TyperError::RightAssociativeExtensionDeferred {
                symbol,
                tree_index
            }) if symbol == method && tree_index == method_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
    }

    #[test]
    fn by_name_parameter_completion_preserves_its_by_name_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(value: => Int): Unit = ()");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "value");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::ByName { result } if *result == definitions.int
        ));
    }

    #[test]
    fn method_type_parameter_shadows_a_class_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def f[A](value: A): A = value }");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            unreachable!()
        };
        let method_type_parameter = index.symbol_at(source, definition.type_params[0]).unwrap();
        let value_parameter = index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .unwrap();
        let context = index.declaration_context_of(value_parameter).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(definition.tpt, context).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), method_type_parameter);
    }

    #[test]
    fn unresolved_type_name_error_has_position_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: MissingType = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
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
            typer.complete_symbol(value),
            Err(TyperError::TypeNameNotFound {
                source: found_source,
                tree_index,
                name,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
                && typer.store().names.resolve(name.text()) == "MissingType"
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(value), SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn duplicate_type_candidates_return_a_positioned_ambiguity_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class T; val x: T = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                source: found_source,
                tree_index,
                name,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
                && typer.store().names.resolve(name.text()) == "T"
        ));
    }

    #[test]
    fn object_name_alone_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O\nval x: O = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let object = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
                )
                .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::WrongTypeNameKind {
                source: found_source,
                tree_index,
                name,
                symbol: found_symbol,
                kind: SymbolKind::Object,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_symbol == object
                && found_position == position
                && typer.store().names.resolve(name.text()) == "O"
                && name.is_type()
        ));
    }

    #[test]
    fn imported_object_alias_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object Lib { object O }\nimport Lib.O as Alias\nval x: Alias = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let object = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) =
                    &node.kind
                else {
                    return None;
                };
                (store.names.resolve(module.name.as_name().text()) == "O")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::WrongTypeNameKind {
                source: found_source,
                tree_index,
                name,
                symbol: found_symbol,
                kind: SymbolKind::Object,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_symbol == object
                && found_position == position
                && typer.store().names.resolve(name.text()) == "Alias"
                && name.is_type()
        ));
    }

    #[test]
    fn parenthesized_type_projects_the_inner_type_and_caches_both_trees() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: (Int) = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(parens)) =
            &parsed.ast.get(type_tree).kind
        else {
            panic!("expected the parser to preserve parentheses around the type")
        };
        let inner = parens.inner;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(symbol).unwrap();

        assert_eq!(projected, definitions.int);
        assert_eq!(
            typer.source_type_index().type_at(source, type_tree),
            Some(projected)
        );
        assert_eq!(
            typer.source_type_index().type_at(source, inner),
            Some(projected)
        );
    }

    #[test]
    fn nested_parenthesized_type_projects_and_caches_each_layer() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: ((Int)) = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let mut parenthesized = vec![type_tree];
        let mut inner = type_tree;
        while let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(parens)) =
            parsed.ast.get(inner).kind
        {
            inner = parens.inner;
            if matches!(
                parsed.ast.get(inner).kind,
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(_))
            ) {
                parenthesized.push(inner);
            }
        }
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(symbol).unwrap();

        assert_eq!(projected, definitions.int);
        for tree in parenthesized {
            assert_eq!(
                typer.source_type_index().type_at(source, tree),
                Some(projected)
            );
        }
    }

    #[test]
    fn applied_type_projects_constructor_and_arguments_in_source_order() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A, B]; class A; class B; val x: F[A, B] = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut expected = Vec::new();
        for (tree, node) in parsed.ast.iter() {
            let TreeKind::TypeDef(definition) = &node.kind else {
                continue;
            };
            let name = store.names.resolve(definition.name.as_name().text());
            if matches!(name, "F" | "A" | "B") {
                let symbol = index.symbol_at(source, tree).unwrap();
                if store.symbols.get(symbol).kind == SymbolKind::Class {
                    expected.push((name.to_owned(), symbol));
                }
            }
        }
        let find_symbol = |name| expected.iter().find(|(found, _)| found == name).unwrap().1;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("expected an applied type")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), find_symbol("F"));
        assert_eq!(args.len(), 2);
        assert_eq!(type_symbol(typer.store(), args[0]), find_symbol("A"));
        assert_eq!(type_symbol(typer.store(), args[1]), find_symbol("B"));
    }

    #[test]
    fn wildcard_arguments_project_to_wildcards_with_ordered_bounds_and_cached_children() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            include_str!("../../tests/fixtures/wildcard-types/WildcardTypes.scala"),
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "value");
        let TreeKind::AppliedTypeTree(applied_tree) = &parsed.ast.get(type_tree).kind else {
            panic!("expected an applied type tree");
        };
        let wildcard_trees = applied_tree.args.clone();
        let type_bounds = wildcard_trees
            .iter()
            .map(|tree| match &parsed.ast.get(*tree).kind {
                TreeKind::TypeBoundsTree(bounds) => *bounds,
                kind => panic!("expected wildcard bounds, got {kind:?}"),
            })
            .collect::<Vec<_>>();
        let context = index.declaration_context_of(value).unwrap();
        let symbols = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition) => {
                    let name = store.names.resolve(definition.name.as_name().text());
                    matches!(name, "F" | "Foo" | "Bar" | "Container" | "Outer" | "Bound")
                        .then(|| (name.to_owned(), index.symbol_at(source, tree).unwrap()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let find_symbol = |name: &str| {
            symbols
                .iter()
                .find_map(|(found, symbol)| (found == name).then_some(*symbol))
                .unwrap()
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
            typer.type_of_tpt(wildcard_trees[0], context),
            Err(TyperError::UnsupportedTypeTree { tree_index, .. })
                if tree_index == wildcard_trees[0].index()
        ));
        let projected = typer.type_of_tpt(type_tree, context).unwrap();
        assert_eq!(typer.type_of_tpt(type_tree, context).unwrap(), projected);
        let Type::Applied {
            tycon,
            args: applied_args,
        } = typer.store().types.get(projected)
        else {
            panic!("expected an applied type");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), find_symbol("F"));
        assert_eq!(applied_args.len(), wildcard_trees.len());

        let bounds_of = |ty: TypeId| {
            let Type::Wildcard { bounds } = typer.store().types.get(ty) else {
                panic!("expected a wildcard argument");
            };
            let Type::Bounds { low, high } = typer.store().types.get(*bounds) else {
                panic!("expected wildcard bounds");
            };
            (*low, *high)
        };
        let (low, high) = bounds_of(applied_args[0]);
        assert_eq!(low, definitions.nothing_type);
        assert_eq!(high, definitions.any_type);
        let (low, high) = bounds_of(applied_args[1]);
        assert_eq!(low, definitions.nothing_type);
        assert_eq!(type_symbol(typer.store(), high), find_symbol("Foo"));
        let (low, high) = bounds_of(applied_args[2]);
        assert_eq!(type_symbol(typer.store(), low), find_symbol("Bar"));
        assert_eq!(high, definitions.any_type);
        let (low, high) = bounds_of(applied_args[3]);
        assert_eq!(type_symbol(typer.store(), low), find_symbol("Bar"));
        assert_eq!(type_symbol(typer.store(), high), find_symbol("Foo"));
        let (low, high) = bounds_of(applied_args[4]);
        assert_eq!(low, definitions.nothing_type);
        let Type::Applied {
            tycon,
            args: nested_args,
        } = typer.store().types.get(high)
        else {
            panic!("expected the nested applied upper bound");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), find_symbol("Container"));
        assert_eq!(nested_args.len(), 1);
        assert_eq!(
            type_symbol(typer.store(), nested_args[0]),
            find_symbol("Bound")
        );
        let (low, high) = bounds_of(applied_args[5]);
        assert_eq!(type_symbol(typer.store(), low), find_symbol("Bound"));
        assert_eq!(high, definitions.any_type);

        for (tree, ty) in wildcard_trees.iter().zip(applied_args.iter().copied()) {
            assert_eq!(typer.source_type_index().type_at(source, *tree), Some(ty));
        }
        for bounds in type_bounds.iter().skip(1) {
            for child in [bounds.low, bounds.high].into_iter().flatten() {
                assert!(typer.source_type_index().type_at(source, child).is_some());
            }
        }
        let nested_bound = type_bounds[4].high.unwrap();
        let TreeKind::AppliedTypeTree(nested_applied) = &parsed.ast.get(nested_bound).kind else {
            panic!("expected an applied nested bound");
        };
        assert!(
            typer
                .source_type_index()
                .type_at(source, nested_applied.tpt)
                .is_some()
        );
        assert!(
            typer
                .source_type_index()
                .type_at(source, nested_applied.args[0])
                .is_some()
        );
    }

    #[test]
    fn source_union_and_intersection_types_preserve_parser_grouping_and_cache_identity() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A; class B; class C; val union: A | B = null; val intersection: A & B = null; val precedence: A | B & C = null; val parenthesized: (A | B) & C = null",
        );
        let class = |name| class_symbol(&parsed, &store, &index, source, name);
        let a = class("A");
        let b = class("B");
        let c = class("C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let project = |typer: &mut SourceTyper<'_>, name| {
            let (symbol, tree) = val_symbol(&parsed, typer.store(), &index, source, name);
            let context = index.declaration_context_of(symbol).unwrap();
            let ty = typer.type_of_tpt(tree, context).unwrap();
            assert_eq!(typer.type_of_tpt(tree, context).unwrap(), ty);
            assert_eq!(typer.source_type_index().type_at(source, tree), Some(ty));
            ty
        };

        let union = project(&mut typer, "union");
        let intersection = project(&mut typer, "intersection");
        let precedence = project(&mut typer, "precedence");
        let parenthesized = project(&mut typer, "parenthesized");
        let Type::Or { left, right } = typer.store().types.get(union) else {
            panic!("expected union type");
        };
        assert_eq!(type_symbol(typer.store(), *left), a);
        assert_eq!(type_symbol(typer.store(), *right), b);
        let Type::And { left, right } = typer.store().types.get(intersection) else {
            panic!("expected intersection type");
        };
        assert_eq!(type_symbol(typer.store(), *left), a);
        assert_eq!(type_symbol(typer.store(), *right), b);
        let Type::Or { left, right } = typer.store().types.get(precedence) else {
            panic!("expected parser's union root");
        };
        assert_eq!(type_symbol(typer.store(), *left), a);
        let precedence_intersection = *right;
        let Type::And { left, right } = typer.store().types.get(*right) else {
            panic!("expected parser's nested intersection");
        };
        assert_eq!(type_symbol(typer.store(), *left), b);
        assert_eq!(type_symbol(typer.store(), *right), c);
        let (_, precedence_tree) = val_symbol(&parsed, typer.store(), &index, source, "precedence");
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(precedence_infix)) =
            &parsed.ast.get(precedence_tree).kind
        else {
            panic!("expected source precedence infix tree");
        };
        assert_eq!(
            typer
                .source_type_index()
                .type_at(source, precedence_infix.right),
            Some(precedence_intersection)
        );
        let Type::And {
            left,
            right: intersection_right,
        } = typer.store().types.get(parenthesized)
        else {
            panic!("expected parenthesized intersection root");
        };
        let parenthesized_union = *left;
        let Type::Or { left, right } = typer.store().types.get(*left) else {
            panic!("expected parenthesized union child");
        };
        assert_eq!(type_symbol(typer.store(), *left), a);
        assert_eq!(type_symbol(typer.store(), *right), b);
        assert_eq!(type_symbol(typer.store(), *intersection_right), c);
        let (_, parenthesized_tree) =
            val_symbol(&parsed, typer.store(), &index, source, "parenthesized");
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(parenthesized_infix)) =
            &parsed.ast.get(parenthesized_tree).kind
        else {
            panic!("expected parenthesized source infix tree");
        };
        assert_eq!(
            typer
                .source_type_index()
                .type_at(source, parenthesized_infix.left),
            Some(parenthesized_union)
        );
    }

    #[test]
    fn singleton_type_preserves_stable_field_symbol_and_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1; val alias: value.type = value }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let (alias, tree) = val_symbol(&parsed, &store, &index, source, "alias");
        let context = index.declaration_context_of(alias).unwrap();
        let singleton_tree = tree;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(singleton_tree, context).unwrap();
        let Type::TermRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("singleton type must retain a term reference");
        };
        assert_eq!(*target, TermRefTarget::Symbol(value));
        assert!(matches!(
            typer.store().types.get(*prefix),
            Type::ThisType { .. }
        ));
        assert_eq!(
            typer.type_of_tpt(singleton_tree, context).unwrap(),
            projected
        );
        assert!(
            typer
                .source_type_index()
                .type_at(source, singleton_tree)
                .is_some()
        );
    }

    #[test]
    fn singleton_type_rejects_mutable_and_method_references() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { var mutable: Int = 1; def method: Int = 1; val bad1: mutable.type = mutable; val bad2: method.type = 1 }",
        );
        let bad1 = val_symbol(&parsed, &store, &index, source, "bad1").0;
        let bad2 = val_symbol(&parsed, &store, &index, source, "bad2").0;
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let bad1_tpt = val_symbol(&parsed, &store, &index, source, "bad1").1;
        let bad2_tpt = val_symbol(&parsed, &store, &index, source, "bad2").1;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        for (symbol, tree) in [(bad1, bad1_tpt), (bad2, bad2_tpt)] {
            let context = index.declaration_context_of(symbol).unwrap();
            assert!(matches!(
                typer.type_of_tpt(tree, context),
                Err(TyperError::UnstableSelectionPrefix { .. })
                    | Err(TyperError::UnsupportedTermReference { .. })
                    | Err(TyperError::UnsupportedSingletonReference { .. })
            ));
        }
        assert_eq!(typer.store().symbols.info(method), &SymbolInfo::Missing);
    }

    #[test]
    fn singleton_type_supports_this_and_stable_field_paths() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Inner { val value: Int = 1 }; class Outer { class Nested { val outer: Outer.this.type = Outer.this } }; class C { val inner: Inner = new Inner; val self: this.type = this; val selected: inner.value.type = inner.value }",
        );
        let (self_symbol, self_tpt) = val_symbol(&parsed, &store, &index, source, "self");
        let (outer_symbol, outer_tpt) = val_symbol(&parsed, &store, &index, source, "outer");
        let (selected_symbol, selected_tpt) =
            val_symbol(&parsed, &store, &index, source, "selected");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let self_type = typer
            .type_of_tpt(self_tpt, index.declaration_context_of(self_symbol).unwrap())
            .unwrap();
        assert!(matches!(
            typer.store().types.get(self_type),
            Type::ThisType { class: actual } if *actual == class
        ));

        let outer_type = typer
            .type_of_tpt(
                outer_tpt,
                index.declaration_context_of(outer_symbol).unwrap(),
            )
            .unwrap();
        let outer_class = class_symbol(&parsed, typer.store(), &index, source, "Outer");
        assert!(matches!(
            typer.store().types.get(outer_type),
            Type::ThisType { class: actual } if *actual == outer_class
        ));

        let selected_type = typer
            .type_of_tpt(
                selected_tpt,
                index.declaration_context_of(selected_symbol).unwrap(),
            )
            .unwrap();
        let Type::TermRef {
            target: TermRefTarget::Symbol(value),
            prefix,
        } = typer.store().types.get(selected_type)
        else {
            panic!("selected singleton must retain its field reference");
        };
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(*value).name.text()),
            "value"
        );
        assert!(matches!(
            typer.store().types.get(*prefix),
            Type::TermRef { .. }
        ));
    }

    #[test]
    fn failed_singleton_selection_rolls_back_completed_prefix_and_type_cache() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Inner; class C { val inner: Inner = new Inner; val missing: inner.absent.type = inner }",
        );
        let (inner, _) = val_symbol(&parsed, &store, &index, source, "inner");
        let (missing, tpt) = val_symbol(&parsed, &store, &index, source, "missing");
        let context = index.declaration_context_of(missing).unwrap();
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
            Err(TyperError::MemberNotFound { .. })
        ));
        assert_eq!(typer.store().symbols.info(inner), &SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, tpt), None);
    }

    #[test]
    fn deeply_nested_singleton_path_returns_projection_depth_error() {
        let path = format!("a{}", ".missing".repeat(300));
        let source_text = format!("class C {{ val a: C = this; val deep: {path}.type = a }}");
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(&source_text);
        let (a, _) = val_symbol(&parsed, &store, &index, source, "a");
        let (deep, tpt) = val_symbol(&parsed, &store, &index, source, "deep");
        let context = index.declaration_context_of(deep).unwrap();
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
            Err(TyperError::SourceTypeProjectionDepthExceeded {
                max_depth,
                ..
            }) if max_depth == type_projection::MAX_SOURCE_TYPE_PROJECTION_DEPTH
        ));
        assert_eq!(typer.store().symbols.info(a), &SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, tpt), None);
    }

    #[test]
    fn source_union_types_compose_with_applied_qualified_and_alias_types() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class F[A]; class A; class B; object Outer { class Nested }; type Alias = Outer.Nested & A; val value: F[A | B] = null",
        );
        let (value, applied_tree) = val_symbol(&parsed, &store, &index, source, "value");
        let applied_context = index.declaration_context_of(value).unwrap();
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let alias_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition) if index.symbol_at(source, tree) == Some(alias) => {
                    Some(definition.rhs)
                }
                _ => None,
            })
            .unwrap();
        let alias_context = index.declaration_context_of(alias).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let applied = typer.type_of_tpt(applied_tree, applied_context).unwrap();
        let Type::Applied { args, .. } = typer.store().types.get(applied) else {
            panic!("expected applied source type");
        };
        assert_eq!(args.len(), 1);
        let Type::Or { left, right } = typer.store().types.get(args[0]) else {
            panic!("expected applied union type argument");
        };
        assert!(matches!(
            typer.store().symbols.get(type_symbol(typer.store(), *left)).name.text(),
            name if typer.store().names.resolve(name) == "A"
        ));
        assert_eq!(
            typer.store().names.resolve(
                typer
                    .store()
                    .symbols
                    .get(type_symbol(typer.store(), *right))
                    .name
                    .text()
            ),
            "B"
        );

        let alias_type = typer.type_of_tpt(alias_tree, alias_context).unwrap();
        let Type::And { left, right } = typer.store().types.get(alias_type) else {
            panic!("expected alias intersection type");
        };
        assert_eq!(
            typer.store().names.resolve(
                typer
                    .store()
                    .symbols
                    .get(type_symbol(typer.store(), *left))
                    .name
                    .text()
            ),
            "Nested"
        );
        assert_eq!(
            typer.store().names.resolve(
                typer
                    .store()
                    .symbols
                    .get(type_symbol(typer.store(), *right))
                    .name
                    .text()
            ),
            "A"
        );
    }

    #[test]
    fn unsupported_source_type_operator_is_explicit_and_failed_union_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A; class B; val value: A + B = null");
        let (value, tree) = val_symbol(&parsed, &store, &index, source, "value");
        let context = index.declaration_context_of(value).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        assert!(matches!(
            typer.type_of_tpt(tree, context),
            Err(TyperError::UnsupportedTypeTree { tree_index, tree_kind, .. })
                if tree_index == tree.index() && tree_kind == "infix type operator"
        ));

        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A; val value: A | Missing = null");
        let (value, tree) = val_symbol(&parsed, &store, &index, source, "value");
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &parsed.ast.get(tree).kind
        else {
            panic!("expected union type tree");
        };
        let left = infix.left;
        let context = index.declaration_context_of(value).unwrap();
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
            typer.type_of_tpt(tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, tree), None);
        assert_eq!(typer.source_type_index().type_at(source, left), None);
    }

    #[test]
    fn long_source_infix_type_projection_returns_a_bounded_error_and_rolls_back() {
        let operands = vec!["A"; type_projection::MAX_SOURCE_TYPE_PROJECTION_DEPTH * 4].join(" | ");
        let source_text = format!("class A; val value: {operands} = null");
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(&source_text);
        let (value, tree) = val_symbol(&parsed, &store, &index, source, "value");
        let context = index.declaration_context_of(value).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for _ in 0..2 {
            assert!(matches!(
                typer.type_of_tpt(tree, context),
                Err(TyperError::SourceTypeProjectionDepthExceeded {
                    max_depth: type_projection::MAX_SOURCE_TYPE_PROJECTION_DEPTH,
                    ..
                })
            ));
            assert_eq!(typer.store().checkpoint(), before);
            assert_eq!(typer.source_type_index().type_at(source, tree), None);
            for (nested, node) in parsed.ast.iter() {
                if matches!(node.kind, TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))) {
                    assert_eq!(typer.source_type_index().type_at(source, nested), None);
                }
            }
        }
    }

    #[test]
    fn by_name_type_projects_its_result_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(x: => Int): Unit = ()");
        let (parameter, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(parameter).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(type_tree, context).unwrap();

        let Type::ByName { result } = typer.store().types.get(projected) else {
            panic!("expected a by-name type")
        };
        assert_eq!(*result, definitions.int);
    }

    #[test]
    fn failed_applied_type_projection_rolls_back_types_and_cache_entries() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A]; class A; val x: F[A, Missing] = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::AppliedTypeTree(applied) = &parsed.ast.get(type_tree).kind else {
            panic!("expected an applied type tree")
        };
        let constructor = applied.tpt;
        let first_argument = applied.args[0];
        let context = index.declaration_context_of(value).unwrap();
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
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
        assert_eq!(typer.source_type_index().type_at(source, constructor), None);
        assert_eq!(
            typer.source_type_index().type_at(source, first_argument),
            None
        );
    }

    #[test]
    fn failed_wildcard_bound_projection_rolls_back_wildcard_bounds_and_cache_entries() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A, B]; class Known; val x: F[? <: Known, ? <: Missing] = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::AppliedTypeTree(applied) = &parsed.ast.get(type_tree).kind else {
            panic!("expected an applied type tree");
        };
        let constructor = applied.tpt;
        let first_wildcard = applied.args[0];
        let first_bound = match &parsed.ast.get(first_wildcard).kind {
            TreeKind::TypeBoundsTree(bounds) => bounds.high.unwrap(),
            kind => panic!("expected wildcard bounds, got {kind:?}"),
        };
        let context = index.declaration_context_of(value).unwrap();
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
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
        assert_eq!(typer.source_type_index().type_at(source, constructor), None);
        assert_eq!(
            typer.source_type_index().type_at(source, first_wildcard),
            None
        );
        assert_eq!(typer.source_type_index().type_at(source, first_bound), None);
    }

    #[test]
    fn missing_declared_type_is_not_inferred_from_the_value_rhs() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("val x = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::MissingDeclaredType {
                source: found_source,
                tree_index,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
        ));
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn nested_type_lookup_walks_to_the_enclosing_declaration_context() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner { val x: Outer = 1 } }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let outer = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Outer" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), outer);
    }

    #[test]
    fn member_type_reference_uses_the_enclosing_class_this_type_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner; val x: Inner = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let inner = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Inner" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let owner = store.symbols.get(inner).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::TypeRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("expected a TypeRef for the member type")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(inner));
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: owner }
        );
    }

    #[test]
    fn member_type_reference_in_an_object_uses_its_module_class_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O { class Inner; val x: Inner = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let inner = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Inner" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let owner = store.symbols.get(inner).owner.unwrap();
        assert_eq!(store.symbols.get(owner).kind, SymbolKind::ModuleClass);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::TypeRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("expected a TypeRef for the member type")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(inner));
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: owner }
        );
    }

    #[test]
    fn qualified_inner_type_resolves_through_source_class_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }; val x: Outer.Inner = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut outer = None;
        let mut inner = None;
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::TypeDef(definition) = &node.kind {
                match store.names.resolve(definition.name.as_name().text()) {
                    "Outer" => outer = index.symbol_at(source, tree),
                    "Inner" => inner = index.symbol_at(source, tree),
                    _ => {}
                }
            }
        }
        let outer = outer.unwrap();
        let inner = inner.unwrap();
        assert_eq!(*store.symbols.info(outer), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), inner);
        let Type::TypeRef { prefix, .. } = typer.store().types.get(projected) else {
            panic!("expected a qualified type reference")
        };
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: outer }
        );
        assert_eq!(*typer.store().symbols.info(outer), SymbolInfo::Missing);
    }

    #[test]
    fn package_qualified_type_uses_the_canonical_package_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("package p { class C }; package use { val x: p.C = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let package = packages.symbol(&["p"]).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), class);
        let Type::TypeRef { prefix, .. } = typer.store().types.get(projected) else {
            panic!("expected a package-qualified type reference")
        };
        let Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package_target),
        } = typer.store().types.get(*prefix)
        else {
            panic!("expected the canonical package reference prefix")
        };
        assert_eq!(*package_prefix, definitions.no_prefix);
        assert_eq!(*package_target, package);
    }

    #[test]
    fn external_member_resolver_runs_only_after_source_lookup_misses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Outer { class Local }; val local: Outer.Local = 1; val ext: Outer.External = 1",
        );
        let (local, _) = val_symbol(&parsed, &store, &index, source, "local");
        let (external, _) = val_symbol(&parsed, &store, &index, source, "ext");
        let local_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Local" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let external_type = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let package_requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = ScriptedResolver {
            member: Some(external_type),
            package: None,
            member_requests: Rc::clone(&requests),
            package_requests,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let local_projected = typer.complete_symbol(local).unwrap();
        let external_projected = typer.complete_symbol(external).unwrap();

        assert_eq!(type_symbol(typer.store(), local_projected), local_type);
        assert_eq!(
            type_symbol(typer.store(), external_projected),
            external_type
        );
        assert_eq!(requests.borrow().len(), 1);
        assert_eq!(
            typer.store().names.resolve(requests.borrow()[0].text()),
            "External"
        );
    }

    #[test]
    fn external_package_and_member_resolvers_supply_unknown_qualifiers() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("import p.C\nval x: C = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let package_name = store.names.intern("p");
        let package = store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(package_name, Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let external_type = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        store.symbols.get_mut(external_type).owner = Some(package);
        let member_requests = Rc::new(RefCell::new(Vec::new()));
        let package_requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = ScriptedResolver {
            member: Some(external_type),
            package: Some(package),
            member_requests: Rc::clone(&member_requests),
            package_requests: Rc::clone(&package_requests),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), external_type);
        assert_eq!(*package_requests.borrow(), vec![vec!["p".to_owned()]]);
        assert_eq!(member_requests.borrow().len(), 1);
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(found_package),
            ..
        } = typer
            .store()
            .types
            .get(match typer.store().types.get(projected) {
                Type::TypeRef { prefix, .. } => *prefix,
                _ => panic!("expected a type reference"),
            })
        else {
            panic!("expected the external package prefix")
        };
        assert_eq!(*found_package, package);
    }

    #[test]
    fn innermost_type_declaration_shadows_an_enclosing_name() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class Outer { class T; val x: T = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let outer = store.symbols.get(value).owner.unwrap();
        let inner_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    let symbol = index.symbol_at(source, tree)?;
                    (store.symbols.get(symbol).owner == Some(outer)).then_some(symbol)
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), inner_type);
    }

    #[test]
    fn inner_term_name_does_not_shadow_an_enclosing_type_name() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class Outer { object T; val x: T = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let enclosing_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), enclosing_type);
    }

    #[test]
    fn explicit_package_import_resolves_a_type_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.Imported; val x: Imported = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn wildcard_package_import_resolves_a_type_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.*; val x: Imported = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn renamed_package_import_resolves_the_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.{Imported as Alias}; val x: Alias = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn imported_type_alias_resolves_without_completing_the_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { type Alias = Int }; package app { import lib.Alias; val x: Alias = 1; val y: lib.Alias = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let (qualified_value, _) = val_symbol(&parsed, &store, &index, source, "y");
        let alias = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Alias" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(store.symbols.get(alias).kind, SymbolKind::TypeAlias);
        assert_eq!(*store.symbols.info(alias), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), alias);
        let qualified = typer.complete_symbol(qualified_value).unwrap();
        assert_eq!(type_symbol(typer.store(), qualified), alias);
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
    }

    #[test]
    fn renamed_import_does_not_expose_the_original_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }; package app { import lib.{Imported as Alias}; val alias: Alias = 1; val original: Imported = 1 }",
        );
        let (alias, _) = val_symbol(&parsed, &store, &index, source, "alias");
        let (original, original_tpt) = val_symbol(&parsed, &store, &index, source, "original");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let original_position = parsed.ast.get(original_tpt).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(alias).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
        assert!(matches!(
            typer.complete_symbol(original),
            Err(TyperError::TypeNameNotFound {
                name,
                tree_index,
                position,
                ..
            }) if typer.store().names.resolve(name.text()) == "Imported"
                && tree_index == original_tpt.index()
                && position == original_position
        ));
    }

    #[test]
    fn renamed_selector_hides_its_original_from_the_same_wildcard_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }; package app { import lib.{Imported as Alias, *}; val alias: Alias = 1; val original: Imported = 1 }",
        );
        let (alias, _) = val_symbol(&parsed, &store, &index, source, "alias");
        let (original, original_tpt) = val_symbol(&parsed, &store, &index, source, "original");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
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

        let projected = typer.complete_symbol(alias).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
        assert!(matches!(
            typer.complete_symbol(original),
            Err(TyperError::TypeNameNotFound { tree_index, .. })
                if tree_index == original_tpt.index()
        ));
    }

    #[test]
    fn hide_selector_excludes_a_name_from_the_same_wildcard_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Hidden; class Visible }; package app { import lib.{Hidden as _, *}; val x: Hidden = 1 }",
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::TypeNameNotFound {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "Hidden"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn explicit_import_precedes_a_later_wildcard_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class C }; package second { class C }; package app { import first.C; import second.*; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["first"]) =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn conflicting_wildcard_imports_in_one_scope_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class C }; package second { class C }; package app { import first.*; import second.*; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let (_, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "C"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn conflicting_explicit_imports_in_one_scope_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class C }; package second { class C }; package app { import first.C; import second.C; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let (_, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "C"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn repeated_import_of_the_same_type_in_one_scope_is_not_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C }; package app { import lib.C; import lib.C; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn wildcard_imports_of_aliases_to_the_same_type_are_not_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package base { class C }; package first { type T = base.C }; package second { type T = base.C }; package app { import first.*; import second.*; val x: T = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let aliases: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .collect();
        assert_eq!(aliases.len(), 2);
        let base_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        for alias in aliases.iter().copied() {
            let underlying = store.types.alloc(Type::TypeRef {
                prefix: definitions.no_prefix,
                target: TypeRefTarget::Symbol(base_type),
            });
            let info = store
                .types
                .alloc(Type::AliasingBounds { alias: underlying });
            store.symbols.set_info(alias, SymbolInfo::Complete(info));
        }
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert!(aliases.contains(&type_symbol(typer.store(), projected)));
    }

    #[test]
    fn explicit_import_precedes_package_type_from_another_source_unit() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let (package_file, package_index) = parse_and_name_unit(
            "package p { class T }",
            SourceId::from_index(21),
            &mut store,
            &mut packages,
        );
        let source = SourceId::from_index(22);
        let (parsed, index) = parse_and_name_unit(
            "package q { class T }; package p { import q.T; val x: T = 1 }",
            source,
            &mut store,
            &mut packages,
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["q"]) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let package_type = package_file
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    package_index.symbol_at(SourceId::from_index(21), tree)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            store.symbols.get(package_type).origin,
            SymbolOrigin::Source(SourceId::from_index(21))
        );
        assert_eq!(
            store.symbols.get(package_type).owner,
            packages.symbol(&["p"])
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn current_unit_package_type_precedes_an_explicit_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class T }; package second { class T }; package first { import second.T; val x: T = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["first"]) =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn duplicate_alias_candidates_in_one_import_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C; class D }; package app { import lib.{C as Alias, D as Alias}; val x: Alias = 1 }",
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "Alias"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn lexical_type_declaration_shadows_an_explicit_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C }; package app { import lib.C; class C; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let app_class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["app"]) =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), app_class);
    }

    #[test]
    fn imports_apply_only_to_declarations_after_the_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C }; package app { val before: C = 1; import lib.C; val after: C = 1 }",
        );
        let (before, before_tpt) = val_symbol(&parsed, &store, &index, source, "before");
        let (after, _) = val_symbol(&parsed, &store, &index, source, "after");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(before),
            Err(TyperError::TypeNameNotFound { name, tree_index, .. })
                if typer.store().names.resolve(name.text()) == "C"
                    && tree_index == before_tpt.index()
        ));
        assert!(typer.complete_symbol(after).is_ok());
    }

    #[test]
    fn failed_qualified_type_projection_rolls_back_prefix_and_cache() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer; val x: Outer.Missing = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(value).unwrap();
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
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn failed_imported_type_projection_rolls_back_prefix_and_cache() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Existing }; package app { import lib.*; val x: Missing = 1 }",
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(value).unwrap();
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
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn later_import_qualifier_can_use_an_earlier_type_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported { class Nested } }\npackage app { import lib.Imported as Alias; import Alias.Nested; val x: Nested = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let nested_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Nested" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), nested_type);
    }

    #[test]
    fn later_import_qualifier_can_use_an_earlier_object_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "object Lib { object Nested { class X } }\nimport Lib.Nested as Alias\nimport Alias.X\nval x: X = 1",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let nested_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "X" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), nested_type);
    }

    #[test]
    fn wildcard_import_from_an_object_uses_its_namer_module_class_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object Lib { class Imported }\nimport Lib.*\nval x: Imported = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn primitive_int_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Int");

        assert_eq!(ty, definitions.int);
    }

    #[test]
    fn primitive_boolean_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Boolean");

        assert_eq!(ty, definitions.boolean);
    }

    #[test]
    fn lexical_type_named_int_shadows_the_builtin() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Int; val x: Int = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let local_int = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Int" =>
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

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), local_int);
    }

    #[test]
    fn unit_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Unit");

        assert_eq!(ty, definitions.unit);
    }

    #[test]
    fn any_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Any");

        assert_eq!(ty, definitions.any_type);
    }

    #[test]
    fn nothing_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Nothing");

        assert_eq!(ty, definitions.nothing_type);
    }

    #[test]
    fn method_without_result_type_infers_its_body_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def method = 1");
        let method = index
            .symbol_at(
                source,
                parsed
                    .ast
                    .iter()
                    .find_map(|(id, node)| matches!(node.kind, TreeKind::DefDef(_)).then_some(id))
                    .unwrap(),
            )
            .unwrap();
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(method).unwrap() else {
            panic!("source method should be canonical");
        };
        let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("source method should use a DefDef");
        };
        let rhs = definition.rhs.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        assert_eq!(signature, definitions.int);
        assert!(typer.source_typed_index().get(source, rhs).is_some());
    }

    #[test]
    fn inferred_method_result_uses_typed_parameter_references() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def identity(value: Int) = value");
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(node.kind, TreeKind::DefDef(_)).then(|| index.symbol_at(source, tree))
            })
            .flatten()
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("parameterized inferred method should retain its method type");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.params[0].ty, definitions.int);
        assert_eq!(method_type.result, definitions.int);
    }

    #[test]
    fn inferred_generic_method_result_keeps_its_type_parameter_reference() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def identity[A](value: A) = value");
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(node.kind, TreeKind::DefDef(_)).then(|| index.symbol_at(source, tree))
            })
            .flatten()
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("inferred generic method should retain its polymorphic binder");
        };
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("generic method should retain its parameter clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.result),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
    }

    #[test]
    fn inferred_curried_method_result_keeps_each_parameter_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def choose(first: Int)(second: Boolean) = first");
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(node.kind, TreeKind::DefDef(_)).then(|| index.symbol_at(source, tree))
            })
            .flatten()
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(first_clause) = typer.store().types.get(signature) else {
            panic!("curried method should retain its first parameter clause");
        };
        let Type::Method(second_clause) = typer.store().types.get(first_clause.result) else {
            panic!("curried method should retain its second parameter clause");
        };
        assert_eq!(first_clause.params[0].ty, definitions.int);
        assert_eq!(second_clause.params[0].ty, definitions.boolean);
        assert_eq!(second_clause.result, definitions.int);
    }

    #[test]
    fn inferred_extension_method_result_uses_its_receiver_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension (value: Int) def identity = value");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    extension.methods.first().copied()
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(receiver_clause) = typer.store().types.get(signature) else {
            panic!("extension signature should retain its receiver clause");
        };
        assert_eq!(receiver_clause.params[0].ty, definitions.int);
        assert_eq!(receiver_clause.result, definitions.int);
    }

    #[test]
    fn recursive_inferred_method_result_fails_and_rolls_back_typed_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def loop = loop");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::DefDef(_)).then_some(tree))
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let completion = typer.complete_symbol(method);
        assert!(
            matches!(completion, Err(TyperError::RecursiveInferredMethodResult { symbol }) if symbol == method),
            "unexpected completion result: {completion:?}"
        );
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.inferred_method_results_in_progress.is_empty());
    }

    #[test]
    fn mutually_recursive_inferred_methods_fail_and_roll_back_typed_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def first = second\ndef second = first");
        let mut methods = HashMap::new();
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::DefDef(definition) = &node.kind {
                let method = index.symbol_at(source, tree).unwrap();
                methods.insert(
                    store
                        .names
                        .resolve(definition.name.as_name().text())
                        .to_owned(),
                    method,
                );
            }
        }
        let first = methods["first"];
        let second = methods["second"];
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
            typer.complete_symbol(first),
            Err(TyperError::RecursiveInferredMethodResult { symbol }) if symbol == first
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(first), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(second), SymbolInfo::Missing);
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.inferred_method_results_in_progress.is_empty());
    }

    #[test]
    fn top_level_method_beats_same_named_import_in_term_resolution() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "object Other { def value: Boolean = true }\nimport Other.value\ndef value = 2\ndef use = value",
        );
        let mut method_trees = HashMap::new();
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::DefDef(definition) = &node.kind {
                let name = store.names.resolve(definition.name.as_name().text());
                method_trees.insert(
                    name.to_owned(),
                    (tree, index.symbol_at(source, tree).unwrap()),
                );
            }
        }
        let (use_tree, use_method) = method_trees["use"];
        let local_value = method_trees["value"].1;
        let TreeKind::DefDef(definition) = &parsed.ast.get(use_tree).kind else {
            unreachable!();
        };
        let rhs = definition.rhs.unwrap();
        let position = parsed.ast.get(rhs).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(use_method).unwrap();
        let name = *dotty_core::TermName::new(typer.store.names.intern("value")).as_name();

        let resolved = typer
            .resolve_expression_term(name, context, rhs.index(), position)
            .unwrap();

        assert_eq!(resolved, local_value);
    }

    #[test]
    fn failed_inferred_method_body_rolls_back_partial_typed_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def failed = { val local = 1; missing }");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::DefDef(_)).then_some(tree))
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
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
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.initializing_local_symbols.is_empty());
        assert!(typer.inferred_method_results_in_progress.is_empty());
    }

    #[test]
    fn type_projection_rejects_a_context_from_another_naming_index() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1");
        let (symbol, tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(symbol).unwrap();
        let foreign_index = SourceSemanticIndex::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &foreign_index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(tpt, context),
            Err(TyperError::SourceContextMissing { tree_index, .. }) if tree_index == tpt.index()
        ));
    }

    #[test]
    fn completion_does_not_find_a_definition_by_scanning_the_ast() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: Int = 1");
        let (symbol, _) = val_symbol(&parsed, &store, &index, source, "x");
        let empty_index = SourceSemanticIndex::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &empty_index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::SourceProvenanceMissing { symbol: found }) if found == symbol
        ));
    }

    #[test]
    fn a_later_failed_completion_keeps_an_earlier_success() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1\ndef method: Missing = 1");
        let (value, value_tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(id, node)| {
                matches!(&node.kind, TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method")
                .then_some(id)
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let value_type = typer.complete_symbol(value).unwrap();
        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(
            *typer.store().symbols.info(value),
            SymbolInfo::Complete(value_type)
        );
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert_eq!(
            typer.source_type_index().type_at(source, value_tpt),
            Some(value_type)
        );
    }

    #[test]
    fn object_term_and_derived_module_class_keep_distinct_dispatch() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("object O");
        let (tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index.derived_symbol_at(owner, source, tree).unwrap();
        assert_eq!(store.symbols.get(object).kind, SymbolKind::Object);
        assert_eq!(
            store.symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(object),
            Err(TyperError::UnsupportedSymbolCompletion {
                kind: SymbolKind::Object,
                ..
            })
        ));
        let scope = index.scope_of(module_class).unwrap();
        let info_id = typer.complete_symbol(module_class).unwrap();
        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ModuleClass ClassInfo");
        };
        assert_eq!(info.class, module_class);
        assert_eq!(info.declarations, scope);
        assert_eq!(info.parents, vec![definitions.object_type]);
    }

    #[test]
    fn failed_transaction_restores_store_symbol_info_and_type_cache() {
        let (_unused_arena, mut store, packages, definitions) = setup();
        let source = SourceId::from_index(7);
        let tree = {
            let mut arena = AstArena::<Untyped>::new();
            let tree = arena.alloc(dotty_core::Tree {
                kind: TreeKind::TypeTree(dotty_core::ast::TypeTree),
                position: None,
                ty: (),
            });
            // The transaction only indexes an ID; the caller-owned arena
            // below must outlive the driver, so return the arena with the ID.
            (arena, tree)
        };
        let (arena, tree_id) = tree;
        let index = SourceSemanticIndex::new();
        let symbol = symbol(&mut store, SymbolKind::Value, SymbolInfo::Missing);
        let before = store.checkpoint();
        let attempted_type = store.types.alloc(Type::NoType);
        store.rollback_to(before);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        let result: Result<(), TyperError> = typer.run_atomic(|typer, journal| {
            let new_type = typer.store.types.alloc(Type::Error(dotty_core::ErrorType {
                message: typer.store.names.intern("temporary"),
            }));
            journal.push((symbol, *typer.store.symbols.info(symbol)));
            typer
                .store
                .symbols
                .set_info(symbol, SymbolInfo::Complete(new_type));
            typer.type_index.insert(source, tree_id, new_type).unwrap();
            Err(TyperError::UnsupportedSymbolCompletion {
                symbol,
                kind: SymbolKind::Value,
            })
        });

        assert!(matches!(
            result,
            Err(TyperError::UnsupportedSymbolCompletion { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(symbol), SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, tree_id), None);
        drop(typer);
        assert_eq!(store.types.alloc(Type::NoType), attempted_type);
    }

    #[test]
    fn failed_expression_transaction_restores_resolver_state_with_the_store() {
        let arena = AstArena::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let packages = Packages::new();
        let source = SourceId::from_index(8);
        let index = SourceSemanticIndex::new();
        let member_name = Name::new(store.names.intern("external"), Namespace::Term);
        let request = MemberRequest {
            prefix: definitions.no_prefix,
            name: member_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages)
                .with_resolver(Box::new(JournalingResolver::default()));
        let before = typer.store().checkpoint();

        let result: Result<(), TyperError> = typer.run_expression_transaction(|typer, _, _| {
            typer
                .resolver
                .resolve_member(typer.store, &request)
                .unwrap();
            Err(TyperError::TreeOutsideArena {
                source,
                tree_index: 0,
            })
        });

        assert!(matches!(result, Err(TyperError::TreeOutsideArena { .. })));
        assert_eq!(typer.store().checkpoint(), before);

        // Reuse the freed slot for an unrelated symbol. A stale resolver cache
        // would now appear live and return this wrong identity.
        let other_name = Name::new(typer.store.names.intern("other"), Namespace::Term);
        typer.store.symbols.alloc(dotty_core::Symbol {
            name: other_name,
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
        let resolved = typer
            .resolver
            .resolve_member(typer.store, &request)
            .unwrap()
            .unwrap();
        assert_eq!(typer.store.symbols.get(resolved).name, member_name);
    }

    #[test]
    fn failed_expression_transaction_restores_nested_typed_mappings() {
        let arena = AstArena::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let packages = Packages::new();
        let source = SourceId::from_index(9);
        let mut untyped_arena = AstArena::<Untyped>::new();
        let untyped = untyped_arena.alloc(dotty_core::Tree {
            kind: TreeKind::TypeTree(dotty_core::ast::TypeTree),
            position: None,
            ty: (),
        });
        let index = SourceSemanticIndex::new();
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        let result: Result<(), TyperError> = typer.run_expression_transaction(|typer, _, _| {
            let typed = typer.typed_arena.alloc(dotty_core::Tree {
                kind: TreeKind::TypeTree(dotty_core::ast::TypeTree),
                position: None,
                ty: definitions.any_type,
            });
            // Model a nested completion that commits mappings using its own
            // journal rather than the outer expression transaction's journal.
            typer.typed_index.insert(source, untyped, typed).unwrap();
            Err(TyperError::TreeOutsideArena {
                source,
                tree_index: untyped.index(),
            })
        });

        assert!(matches!(result, Err(TyperError::TreeOutsideArena { .. })));
        assert!(typer.typed_arena.iter().next().is_none());
        assert_eq!(typer.source_typed_index().get(source, untyped), None);
    }

    #[test]
    fn boolean_literal_becomes_a_typed_literal_with_the_boolean_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Boolean = true }");
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

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Boolean(true)
            })
        );
        let own_type = typer.typed_ast().get(typed).ty;
        assert!(matches!(
            typer.store().types.get(own_type),
            Type::Constant(dotty_core::Constant::Boolean(true))
        ));
        assert_eq!(
            typer.widen_expression_type(own_type).unwrap(),
            definitions.boolean
        );
    }

    #[test]
    fn character_long_float_double_and_unit_literals_use_canonical_types() {
        let cases = [
            (
                "class C { val value = 'x' }",
                dotty_core::Constant::Char('x' as u16),
                0_u8,
            ),
            (
                "class C { val value = 42L }",
                dotty_core::Constant::Long(42),
                1,
            ),
            (
                "class C { val value = 1.25f }",
                dotty_core::Constant::float(1.25),
                2,
            ),
            (
                "class C { val value = 2.5d }",
                dotty_core::Constant::double(2.5),
                3,
            ),
            ("class C { val value = () }", dotty_core::Constant::Unit, 4),
        ];

        for (source_text, expected_value, type_case) in cases {
            let (value, own_type, ty, definitions) = type_value_rhs(source_text);
            let expected_type = match type_case {
                0 => definitions.char,
                1 => definitions.long,
                2 => definitions.float,
                3 => definitions.double,
                _ => definitions.unit,
            };
            assert_eq!(value, expected_value, "{source_text}");
            assert_eq!(own_type, Type::Constant(value.clone()), "{source_text}");
            assert_eq!(ty, expected_type, "{source_text}");
        }
    }

    #[test]
    fn byte_constant_uses_the_byte_type() {
        let (value, own_type, ty, definitions) =
            type_synthetic_literal(dotty_core::Constant::Byte(7));

        assert_eq!(value, dotty_core::Constant::Byte(7));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.byte);
    }

    #[test]
    fn short_constant_uses_the_short_type() {
        let (value, own_type, ty, definitions) =
            type_synthetic_literal(dotty_core::Constant::Short(7));

        assert_eq!(value, dotty_core::Constant::Short(7));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.short);
    }

    #[test]
    fn bare_this_gets_the_nearest_enclosing_class_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = this }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: symbol,
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

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::This(dotty_core::ast::This { qual: None })
        ));
        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::ThisType { class }
        );
    }

    #[test]
    fn qualified_this_resolves_an_enclosing_class_owner() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner { val value = this } }");
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let qualifier = store.symbols.get(outer).name;
        parsed.ast.get_mut(rhs).kind = TreeKind::This(dotty_core::ast::This {
            qual: Some(qualifier),
        });
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: symbol,
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

        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::ThisType { class: outer }
        );
    }

    #[test]
    fn qualified_this_rejects_a_non_enclosing_class() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Other; class C { val value = this }");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let qualifier = store.symbols.get(other).name;
        parsed.ast.get_mut(rhs).kind = TreeKind::This(dotty_core::ast::This {
            qual: Some(qualifier),
        });
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: symbol,
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
            Err(TyperError::ThisOwnerNotEnclosing { .. })
        ));
    }

    #[test]
    fn method_parameter_identifier_gets_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def method(param: Int): Int = param }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "param");
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

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Ident(_)
        ));
        let Type::TermRef { prefix, target } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("parameter identifier should retain a term reference")
        };
        assert_eq!(*prefix, definitions.no_prefix);
        assert_eq!(*target, TermRefTarget::Symbol(parameter));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn new_expression_is_typed_with_its_projected_class_type_and_stable_mapping() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C; class Use { def make: C = new C }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let (rhs, new) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match node.kind {
                TreeKind::New(new) => Some((tree, new)),
                _ => None,
            })
            .expect("expected a source New expression");
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
            TreeKind::New(_)
        ));
        assert_eq!(
            type_symbol(typer.store(), typer.typed_ast().get(typed).ty),
            class
        );
        let typed_tpt = typer.source_typed_index().get(source, new.tpt).unwrap();
        assert!(matches!(
            typer.typed_ast().get(typed_tpt).kind,
            TreeKind::TypeTree(_)
        ));
        assert_eq!(
            typer.typed_ast().get(typed_tpt).ty,
            typer.typed_ast().get(typed).ty
        );
        assert_eq!(typer.type_expression(rhs, context).unwrap(), typed);
    }

    #[test]
    fn new_expression_preserves_generic_instance_arguments() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]; class Use { def make: Box[Int] = new Box[Int] }");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let rhs = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
            .expect("expected a source New expression");
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

        let Type::Applied { tycon, args } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("generic new must retain its applied class type");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), box_class);
        assert_eq!(args, &[definitions.int]);
    }

    #[test]
    fn primary_constructor_application_reuses_plain_application_and_preserves_identity() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Point(x: Int, y: Int); class Use { def make: Point = new Point(1, 2) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Point");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
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
            panic!("constructor call should remain an Apply")
        };
        assert_eq!(application.args.len(), 2);
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("constructor callee should remain a Select")
        };
        assert_eq!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty),
            &Type::TermRef {
                prefix: typer.typed_ast().get(selection.qualifier).ty,
                target: TermRefTarget::Symbol(constructor),
            }
        );
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[0]).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[1]).ty),
            Type::Constant(dotty_core::Constant::Int(2))
        ));
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(result_class),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("constructor result should be the completed owner type")
        };
        assert_eq!(*result_class, class);
    }

    #[test]
    fn generic_primary_constructor_infers_arguments_and_finalizes_new_after_inference() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A](value: A); class Use { def make = new Box(1) }");
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
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
        let original_callable = typer.complete_symbol(constructor).unwrap();
        let original_signature = typer.store().types.get(original_callable).clone();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("generic constructor call should be an Apply");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("generic constructor callee should be a Select");
        };
        let new_type = typer.typed_ast().get(selection.qualifier).ty;
        for actual_type in [new_type, typer.typed_ast().get(typed).ty] {
            let Type::Applied { tycon, args } = typer.store().types.get(actual_type) else {
                panic!("inferred constructor type should be applied");
            };
            assert_eq!(type_symbol(typer.store(), *tycon), class);
            assert_eq!(args, &[definitions.int]);
        }
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[0]).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert_eq!(
            typer.store().types.get(original_callable),
            &original_signature
        );
        assert!(matches!(
            typer.store().types.get(original_callable),
            Type::Poly(_)
        ));
    }

    #[test]
    fn generic_primary_constructor_infers_each_parameter_by_binder_index() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B](first: A, second: B); class Use { def make = new Pair(1, true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Pair");
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
            panic!("generic constructor call should be an Apply");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("generic constructor callee should be a Select");
        };
        for actual_type in [
            typer.typed_ast().get(selection.qualifier).ty,
            typer.typed_ast().get(typed).ty,
        ] {
            let Type::Applied { tycon, args } = typer.store().types.get(actual_type) else {
                panic!("inferred constructor type should be applied");
            };
            assert_eq!(type_symbol(typer.store(), *tycon), class);
            assert_eq!(args, &[definitions.int, definitions.boolean]);
        }
    }

    #[test]
    fn generic_primary_constructor_infers_widened_reference_argument_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Text; class Box[A](value: A); class Use { def make(value: Text) = new Box(value) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let parameter = store
            .scopes
            .get(index.scope_of(method).unwrap())
            .lookup(&Name::new(store.names.intern("value"), Namespace::Term))
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let parameter_type = typer.complete_symbol(parameter).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("generic constructor call should be an Apply");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("generic constructor callee should be a Select");
        };
        let Type::Applied { tycon, args } = typer
            .store()
            .types
            .get(typer.typed_ast().get(selection.qualifier).ty)
        else {
            panic!("inferred New type should be applied")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        assert_eq!(args, &[parameter_type]);
    }

    #[test]
    fn generic_primary_constructor_infers_nested_applied_formal_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Inner[A](value: A); class Wrap[A](value: Inner[A]); class Use { def make = new Wrap(new Inner(1)) }",
        );
        let inner_class = class_symbol(&parsed, &store, &index, source, "Inner");
        let wrap_class = class_symbol(&parsed, &store, &index, source, "Wrap");
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

        let Type::Applied { tycon, args } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("outer constructor result should be applied")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), wrap_class);
        assert_eq!(args, &[definitions.int]);
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            unreachable!()
        };
        let Type::Applied { tycon, args } = typer
            .store()
            .types
            .get(typer.typed_ast().get(application.args[0]).ty)
        else {
            panic!("constructor argument should retain its applied type")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), inner_class);
        assert_eq!(args, &[definitions.int]);
    }

    #[test]
    fn generic_primary_constructor_inference_errors_are_focused_and_atomic() {
        let cases = [
            (
                "class Pair[A](first: A, second: A); class Use { def make = new Pair(1, true) }",
                0_u8,
            ),
            (
                "class Pair[A, B](first: A); class Use { def make = new Pair(1) }",
                1,
            ),
            (
                "class Box[A <: Int](value: A); class Use { def make = new Box(true) }",
                2,
            ),
        ];
        for (source_text, expected_error) in cases {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
            let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
            let raw_new = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
                .unwrap();
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let context = typer.expression_context_for(method).unwrap();
            let store_checkpoint = typer.store().checkpoint();
            let typed_count = typer.typed_ast().iter().count();
            let error = typer.type_expression(rhs, context).unwrap_err();
            match (expected_error, error) {
                (
                    0,
                    TyperError::ConflictingConstructorInferenceConstraints {
                        parameter_index: 0,
                        ..
                    },
                ) => {}
                (
                    1,
                    TyperError::UnconstrainedConstructorTypeParameter {
                        parameter_index: 1, ..
                    },
                ) => {}
                (
                    2,
                    TyperError::ConstructorTypeArgumentBoundViolation {
                        side: TypeArgumentBoundSide::Upper,
                        ..
                    },
                ) => {}
                (expected, actual) => {
                    panic!("case {expected} returned unexpected error: {actual:?}")
                }
            }
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
            assert_eq!(typer.typed_ast().iter().count(), typed_count);
            assert!(typer.source_typed_index().get(source, raw_new).is_none());
        }
    }

    #[test]
    fn failed_generic_constructor_inference_rolls_back_alias_completion() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A](first: A, second: A); class Use { type RawPair = Pair; def make = new RawPair(1, true) }",
        );
        let alias = type_alias_symbol(&parsed, &store, &index, source, "RawPair");
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
        let semantic_checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConflictingConstructorInferenceConstraints { .. })
        ));

        assert_eq!(typer.store().checkpoint(), semantic_checkpoint);
        assert!(matches!(
            typer.store().symbols.info(alias),
            SymbolInfo::Missing
        ));
    }

    #[test]
    fn generic_primary_constructor_infers_across_curried_clauses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B](first: A)(second: B); class Use { def make = new Pair(1)(true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Pair");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let new_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
            .unwrap();
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

        let TreeKind::Apply(outer) = &typer.typed_ast().get(typed).kind else {
            panic!("curried generic constructor should produce an outer Apply")
        };
        let TreeKind::Apply(inner) = &typer.typed_ast().get(outer.function).kind else {
            panic!("curried generic constructor should produce an inner Apply")
        };
        assert_eq!(inner.args.len(), 1);
        assert_eq!(outer.args.len(), 1);
        let Type::Method(next_clause) = typer
            .store()
            .types
            .get(typer.typed_ast().get(outer.function).ty)
        else {
            panic!("first constructor clause should leave the second clause")
        };
        assert_eq!(next_clause.params[0].ty, definitions.boolean);
        let TreeKind::Select(selection) = &typer.typed_ast().get(inner.function).kind else {
            panic!("constructor function should remain a Select")
        };
        for actual_type in [
            typer.typed_ast().get(selection.qualifier).ty,
            typer.typed_ast().get(typed).ty,
        ] {
            let Type::Applied { tycon, args } = typer.store().types.get(actual_type) else {
                panic!("curried generic constructor result should be applied")
            };
            assert_eq!(type_symbol(typer.store(), *tycon), class);
            assert_eq!(args, &[definitions.int, definitions.boolean]);
        }
        assert!(typer.source_typed_index().get(source, new_tree).is_some());
    }

    #[test]
    fn generic_primary_constructor_accepts_repeated_constraints_and_inferred_bounds() {
        for source_text in [
            "class Pair[A](first: A, second: A); class Use { def make = new Pair(1, 2) }",
            "class Box[A <: Int](value: A); class Use { def make = new Box(1) }",
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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

            let Type::Applied { args, .. } =
                typer.store().types.get(typer.typed_ast().get(typed).ty)
            else {
                panic!("inferred result should preserve its applied type")
            };
            assert_eq!(args, &[definitions.int]);
        }
    }

    #[test]
    fn explicit_generic_primary_constructor_uses_new_type_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A](value: A); class Use { def make: Box[Int] = new Box[Int](1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let original_constructor_signature = typer.complete_symbol(constructor).unwrap();
        let original_constructor_type = typer
            .store()
            .types
            .get(original_constructor_signature)
            .clone();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("generic constructor call should remain an Apply");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("constructor callee should remain a Select");
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == constructor
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::Applied { tycon, args }
                if type_symbol(typer.store(), *tycon) == class && args == &[definitions.int]
        ));
        let instance_type = typer.typed_ast().get(selection.qualifier).ty;
        let Type::Applied {
            tycon: result_tycon,
            args: result_args,
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("constructor result should retain the applied class type");
        };
        let Type::Applied {
            tycon: instance_tycon,
            args: instance_args,
        } = typer.store().types.get(instance_type)
        else {
            panic!("typed New should retain the applied class type");
        };
        assert_eq!(type_symbol(typer.store(), *result_tycon), class);
        assert_eq!(type_symbol(typer.store(), *instance_tycon), class);
        assert_eq!(result_args, instance_args);
        assert_eq!(application.args.len(), 1);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[0]).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        let SymbolInfo::Complete(callable) = *typer.store().symbols.info(constructor) else {
            panic!("completed constructor should retain its semantic callable");
        };
        assert!(matches!(typer.store().types.get(callable), Type::Poly(_)));
        assert_eq!(callable, original_constructor_signature);
        assert_eq!(
            typer.store().types.get(callable),
            &original_constructor_type
        );
    }

    #[test]
    fn generic_primary_constructor_instantiates_type_parameters_in_binder_order() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B](first: A, second: B); class Use { def make: Pair[Int, Boolean] = new Pair[Int, Boolean](1, true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Pair");
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

        let Type::Applied { tycon, args } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("generic constructor result should be applied");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        assert_eq!(args, &[definitions.int, definitions.boolean]);
    }

    #[test]
    fn generic_primary_constructor_preserves_nested_explicit_type_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A](value: A); class Use { def make(value: Box[Int]): Box[Box[Int]] = new Box[Box[Int]](value) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Box");
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

        let Type::Applied { tycon, args } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("outer constructor result should be applied");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        let [inner] = args.as_slice() else {
            panic!("outer constructor should have one explicit type argument");
        };
        let Type::Applied { tycon, args } = typer.store().types.get(*inner) else {
            panic!("nested type argument should remain applied");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        assert_eq!(args, &[definitions.int]);
    }

    #[test]
    fn explicit_generic_primary_constructor_instantiates_once_for_curried_clauses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class PairBox[A](first: A)(second: A); class Use { def make: PairBox[Int] = new PairBox[Int](1)(2) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "PairBox");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
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

        let TreeKind::Apply(outer) = &typer.typed_ast().get(typed).kind else {
            panic!("curried generic constructor should produce an outer Apply");
        };
        let TreeKind::Apply(inner) = &typer.typed_ast().get(outer.function).kind else {
            panic!("curried generic constructor should produce an inner Apply");
        };
        assert_eq!(inner.args.len(), 1);
        assert_eq!(outer.args.len(), 1);
        let Type::Method(next_clause) = typer
            .store()
            .types
            .get(typer.typed_ast().get(outer.function).ty)
        else {
            panic!("first clause should leave the instantiated second clause");
        };
        assert_eq!(next_clause.params[0].ty, definitions.int);
        let TreeKind::Select(selection) = &typer.typed_ast().get(inner.function).kind else {
            panic!("generic constructor function should remain a direct Select");
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(inner.function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == constructor
        ));
        let Type::Applied { tycon, args } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("curried constructor result should retain PairBox[Int]");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        assert_eq!(args, &[definitions.int]);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(selection.qualifier).ty),
            Type::Applied { tycon, args }
                if type_symbol(typer.store(), *tycon) == class && args == &[definitions.int]
        ));
    }

    #[test]
    fn explicit_generic_primary_constructor_accepts_upper_and_lower_bounds() {
        for source_text in [
            "class Box[A <: Int](value: A); class Use { def make: Box[Int] = new Box[Int](1) }",
            "class Box[A >: Int](value: A); class Use { def make: Box[Any] = new Box[Any](1) }",
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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

            typer.type_expression(rhs, context).unwrap();
        }
    }

    #[test]
    fn explicit_generic_primary_constructor_rejects_upper_and_lower_bound_violations() {
        for (source_text, expected_side) in [
            (
                "class Box[A <: Int](value: A); class Use { def make: Box[Boolean] = new Box[Boolean](true) }",
                TypeArgumentBoundSide::Upper,
            ),
            (
                "class Box[A >: Int](value: A); class Use { def make: Box[Boolean] = new Box[Boolean](true) }",
                TypeArgumentBoundSide::Lower,
            ),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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
            let store_checkpoint = typer.store().checkpoint();
            let typed_count = typer.typed_ast().iter().count();

            assert!(matches!(
                typer.type_expression(rhs, context),
                Err(TyperError::ConstructorTypeArgumentBoundViolation {
                    parameter_index: 0,
                    side,
                    ..
                }) if side == expected_side
            ));
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
            assert_eq!(typer.typed_ast().iter().count(), typed_count);
            assert!(typer.source_typed_index().get(source, rhs).is_none());
        }
    }

    #[test]
    fn generic_primary_constructor_poly_arity_mismatch_is_focused_and_atomic() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B](first: A, second: B); class Use { def make: Pair[Int, Boolean] = new Pair[Int, Boolean](1, true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Pair");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let callable = typer.complete_symbol(constructor).unwrap();
        let Type::Poly(mut poly) = typer.store().types.get(callable).clone() else {
            panic!("generic primary constructor should complete to a Poly");
        };
        poly.params.pop();
        let malformed_callable = typer.store.types.alloc(Type::Poly(poly));
        typer
            .store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(malformed_callable));
        let context = typer.expression_context_for(method).unwrap();
        let store_checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConstructorTypeArgumentArityMismatch {
                constructor: actual_constructor,
                expected: 1,
                actual: 2,
                ..
            }) if actual_constructor == constructor
        ));
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn generic_primary_constructor_rejects_too_many_poly_parameters() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B](first: A, second: B); class Use { def make: Pair[Int, Boolean] = new Pair[Int, Boolean](1, true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Pair");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let callable = typer.complete_symbol(constructor).unwrap();
        let Type::Poly(mut poly) = typer.store().types.get(callable).clone() else {
            panic!("generic primary constructor should complete to a Poly");
        };
        poly.params.push(poly.params[0]);
        let malformed_callable = typer.store.types.alloc(Type::Poly(poly));
        typer
            .store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(malformed_callable));
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConstructorTypeArgumentArityMismatch {
                constructor: actual_constructor,
                expected: 3,
                actual: 2,
                ..
            }) if actual_constructor == constructor
        ));
    }

    #[test]
    fn explicit_type_arguments_require_a_polymorphic_primary_constructor() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A](value: A); class Use { def make: Box[Int] = new Box[Int](1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let callable = typer.complete_symbol(constructor).unwrap();
        let Type::Poly(poly) = typer.store().types.get(callable).clone() else {
            panic!("generic primary constructor should complete to a Poly");
        };
        typer
            .store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(poly.result));
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConstructorCallableNotPolymorphic {
                constructor: actual_constructor,
                ..
            }) if actual_constructor == constructor
        ));
    }

    #[test]
    fn generic_primary_constructor_rejects_unsupported_poly_bounds() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A](value: A); class Use { def make: Box[Int] = new Box[Int](1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let callable = typer.complete_symbol(constructor).unwrap();
        let Type::Poly(mut poly) = typer.store().types.get(callable).clone() else {
            panic!("generic primary constructor should complete to a Poly");
        };
        poly.params[0].bounds = definitions.any_type;
        let malformed_callable = typer.store.types.alloc(Type::Poly(poly));
        typer
            .store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(malformed_callable));
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnsupportedConstructorTypeArgumentBounds {
                constructor: actual_constructor,
                parameter_index: 0,
                ..
            }) if actual_constructor == constructor
        ));
    }

    #[test]
    fn generic_primary_constructor_result_must_match_the_new_instance_class() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A](value: A); class Other; class Use { def make: Box[Int] = new Box[Int](1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let callable = typer.complete_symbol(constructor).unwrap();
        let Type::Poly(mut poly) = typer.store().types.get(callable).clone() else {
            panic!("generic primary constructor should complete to a Poly");
        };
        let Type::Method(mut method_type) = typer.store().types.get(poly.result).clone() else {
            panic!("constructor Poly should contain a Method");
        };
        method_type.result = nominal_type_ref(typer.store, definitions, other);
        poly.result = typer.store.types.alloc(Type::Method(method_type));
        let malformed_callable = typer.store.types.alloc(Type::Poly(poly));
        typer
            .store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(malformed_callable));
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConstructorResultTypeMismatch {
                constructor: actual_constructor,
                ..
            }) if actual_constructor == constructor
        ));
    }

    #[test]
    fn backquoted_init_selection_is_not_the_constructor_marker() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int); class Use { def make: C = new C(1) }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let TreeKind::Apply(application) = &parsed.ast.get(rhs).kind else {
            panic!("constructor call should be an Apply")
        };
        let function = application.function;
        let TreeKind::Select(mut selection) = parsed.ast.get(function).kind.clone() else {
            panic!("constructor callee should be a Select")
        };
        // Model a backquoted source selection with the same New qualifier as
        // the compiler marker. The parser-generated marker is not backquoted.
        selection.backquoted = true;
        parsed.ast.get_mut(function).kind = TreeKind::Select(selection);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        assert!(!typer.is_constructor_selection(function));

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnstableSelectionPrefix { .. })
        ));
    }

    #[test]
    fn zero_argument_and_parameter_argument_constructors_share_application_typing() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Empty(); class Box(value: Int); class Use { def make(value: Int): Box = new Box(value); def empty: Empty = new Empty() }",
        );
        let (make, make_rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let (empty, empty_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "empty");
        let parameter = method_parameter_symbol(&parsed, &index, source, make, 0);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let make_context = typer.expression_context_for(make).unwrap();
        let typed_make = typer.type_expression(make_rhs, make_context).unwrap();
        let TreeKind::Apply(make_application) = &typer.typed_ast().get(typed_make).kind else {
            panic!("one-argument constructor call should be an Apply")
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(make_application.args[0]).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == parameter
        ));

        let empty_context = typer.expression_context_for(empty).unwrap();
        let typed_empty = typer.type_expression(empty_rhs, empty_context).unwrap();
        let TreeKind::Apply(empty_application) = &typer.typed_ast().get(typed_empty).kind else {
            panic!("zero-argument constructor call should retain its Apply")
        };
        assert!(empty_application.args.is_empty());
    }

    #[test]
    fn constructor_application_reports_argument_errors_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair(first: Int, second: Int); class Use { def wrongArity: Pair = new Pair(1); def wrongType: Pair = new Pair(1, true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Pair");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let (wrong_arity_method, wrong_arity_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "wrongArity");
        let (wrong_type_method, wrong_type_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "wrongType");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (method, rhs, error_check) in [
            (wrong_arity_method, wrong_arity_rhs, 0usize),
            (wrong_type_method, wrong_type_rhs, 1usize),
        ] {
            let context = typer.expression_context_for(method).unwrap();
            let store_checkpoint = typer.store().checkpoint();
            let typed_count = typer.typed_ast().iter().count();
            let error = typer.type_expression(rhs, context).unwrap_err();
            if error_check == 0 {
                assert!(matches!(
                    error,
                    TyperError::ApplicationArityMismatch {
                        expected: 2,
                        actual: 1,
                        ..
                    }
                ));
            } else {
                assert!(matches!(
                    error,
                    TyperError::ApplicationArgumentTypeMismatch {
                        argument_index: 1,
                        ..
                    }
                ));
            }
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
            assert_eq!(typer.typed_ast().iter().count(), typed_count);
            assert!(typer.source_typed_index().get(source, rhs).is_none());
            assert!(matches!(
                typer.store().symbols.info(class),
                SymbolInfo::Missing
            ));
            assert!(matches!(
                typer.store().symbols.info(constructor),
                SymbolInfo::Missing
            ));
        }
    }

    #[test]
    fn curried_constructor_applications_consume_nested_plain_methods() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Curried(first: Int)(second: Int); class Use { def make: Curried = new Curried(1)(2) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "Curried");
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

        let TreeKind::Apply(outer) = &typer.typed_ast().get(typed).kind else {
            panic!("curried constructor call should have an outer Apply")
        };
        let TreeKind::Apply(inner) = &typer.typed_ast().get(outer.function).kind else {
            panic!("curried constructor call should have an inner Apply")
        };
        assert_eq!(inner.args.len(), 1);
        assert_eq!(outer.args.len(), 1);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(outer.function).ty),
            Type::Method(_)
        ));
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(result_class),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("curried constructor result should be the owner class")
        };
        assert_eq!(*result_class, class);
    }

    #[test]
    fn overloaded_constructor_application_selects_matching_secondary_constructor() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this(flag: Boolean) = this(1) }; class Use { def make: C = new C(true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        assert_eq!(constructors.len(), 2);
        let secondary_constructor = constructors[1];
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
        let TreeKind::Apply(application) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("constructor call should remain an Apply")
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(selected),
            ..
        } = typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        else {
            panic!("constructor selection should preserve its selected symbol")
        };
        assert_eq!(*selected, secondary_constructor);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed).ty),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(result_class),
                ..
            } if *result_class == class
        ));
    }

    #[test]
    fn constructor_overload_selects_the_more_specific_parameter() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Any) { def this(value: Int) = this(value) }; class Use { def make: C = new C(1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        assert_eq!(constructors.len(), 2);
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

        let signatures: Vec<_> = constructors
            .iter()
            .map(
                |constructor| match *typer.store().symbols.info(*constructor) {
                    SymbolInfo::Complete(callable) => callable,
                    _ => panic!("constructor signature should have been completed"),
                },
            )
            .collect();
        let more_specific = constructors
            .iter()
            .zip(&signatures)
            .find_map(|(symbol, signature)| {
                let Type::Method(method) = typer.store().types.get(*signature) else {
                    return None;
                };
                (method.params[0].ty == definitions.int).then_some(*symbol)
            })
            .expect("the secondary constructor should take Int");

        let TreeKind::Apply(application) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("constructor call should remain an Apply")
        };
        assert!(
            matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(application.function).ty),
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } if *symbol == more_specific
            ),
            "selected constructor differs from expected {more_specific:?}; candidates {constructors:?}"
        );
    }

    #[test]
    fn generic_primary_constructor_competes_with_explicit_owner_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C[A](value: A) { def this(value: Int) = this(value) }; class Use { def make: C[Boolean] = new C[Boolean](true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let mut primary = None;
        let mut secondary = None;
        for constructor in constructors.iter().copied() {
            let callable = typer.complete_symbol(constructor).unwrap();
            if matches!(typer.store().types.get(callable), Type::Poly(_)) {
                primary = Some(constructor);
            } else {
                secondary = Some(constructor);
            }
        }
        let primary = primary.unwrap();
        let secondary = secondary.unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("constructor call should remain an Apply")
        };
        assert!(
            matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(application.function).ty),
                Type::TermRef {
                    target: TermRefTarget::Symbol(selected),
                    ..
                } if *selected == primary
            ),
            "generic primary constructor should win over the monomorphic secondary {secondary:?}"
        );
    }

    #[test]
    fn monomorphic_secondary_constructor_competes_with_explicit_owner_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C[A](value: A) { def this(flag: Boolean) = this(1) }; class Use { def make: C[Int] = new C[Int](true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let secondary = constructors
            .iter()
            .copied()
            .find(|constructor| {
                let callable = typer.complete_symbol(*constructor).unwrap();
                matches!(typer.store().types.get(callable), Type::Method(_))
            })
            .unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("constructor call should remain an Apply")
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(selected),
                ..
            } if *selected == secondary
        ));
    }

    #[test]
    fn raw_generic_constructor_overload_infers_the_winning_instance_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C[A](value: A) { def this(value: Int) = this(value) }; class Use { def make = new C(true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let mut primary = None;
        for constructor in constructors.iter().copied() {
            let callable = typer.complete_symbol(constructor).unwrap();
            if matches!(typer.store().types.get(callable), Type::Poly(_)) {
                primary = Some(constructor);
            }
        }
        let primary = primary.unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(application) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("constructor call should remain an Apply")
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(selected),
                ..
            } if *selected == primary
        ));
        let TreeKind::Select(selection) = typer.typed_ast().get(application.function).kind.clone()
        else {
            panic!("constructor application should select its winner")
        };
        assert!(matches!(
            typer.typed_ast().get(selection.qualifier).kind,
            TreeKind::New(_)
        ));
        let Type::Applied { args, .. } = typer
            .store()
            .types
            .get(typer.typed_ast().get(selection.qualifier).ty)
        else {
            panic!("raw generic New should retain its inferred class type arguments")
        };
        assert_eq!(args, &[definitions.boolean]);
    }

    #[test]
    fn raw_generic_constructor_infers_from_an_explicit_using_clause() {
        let source_text = "class C { class Ctx[A]; class Box[A](value: A)(using ctx: Ctx[A]); def make(using ctx: Ctx[Int]): Box[Int] = new Box(1)(using ctx) }";
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

        let TreeKind::Apply(using_application) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("constructor call should retain its outer using application")
        };
        assert_eq!(using_application.kind, ApplyKind::Using);
        let TreeKind::Apply(regular_application) = typer
            .typed_ast()
            .get(using_application.function)
            .kind
            .clone()
        else {
            panic!("constructor call should retain its regular first clause")
        };
        assert_eq!(regular_application.kind, ApplyKind::Regular);
        let TreeKind::Select(selection) = typer
            .typed_ast()
            .get(regular_application.function)
            .kind
            .clone()
        else {
            panic!("constructor application should select its constructor")
        };
        let new_tree = typer.typed_ast().get(selection.qualifier);
        assert!(matches!(new_tree.kind, TreeKind::New(_)));
        assert!(matches!(
            typer.store().types.get(new_tree.ty),
            Type::Applied { args, .. } if args == &[definitions.int]
        ));
    }

    #[test]
    fn raw_generic_secondary_constructor_rejects_uninferred_owner_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C[A](value: A, count: Int) { def this(flag: Boolean) = this(1, 1) }; class Use { def make = new C(true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let secondary = constructors
            .into_iter()
            .find(|constructor| {
                let callable = typer.complete_symbol(*constructor).unwrap();
                matches!(typer.store().types.get(callable), Type::Method(_))
            })
            .unwrap();

        let error = typer.type_expression(rhs, context).unwrap_err();

        assert!(
            matches!(
                error,
                TyperError::UnsupportedRawGenericSecondaryConstructorInference {
                    source: found_source,
                    tree_index,
                    constructor,
                    owner,
                } if found_source == source
                    && tree_index == rhs.index()
                    && constructor == secondary
                    && owner == class
            ),
            "unexpected error: {error:?}"
        );
        assert!(typer.typed_index.get(source, rhs).is_none());
    }

    #[test]
    fn curried_raw_constructor_keeps_the_first_clause_winner() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C[A](value: A)(other: Int) { def this(value: Int)(other: Int) = this(value)(other) }; class Use { def make = new C(true)(1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let mut primary = None;
        for constructor in constructors.iter().copied() {
            let callable = typer.complete_symbol(constructor).unwrap();
            if matches!(typer.store().types.get(callable), Type::Poly(_)) {
                primary = Some(constructor);
            }
        }
        let primary = primary.unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Apply(outer) = typer.typed_ast().get(typed).kind.clone() else {
            panic!("curried constructor call should retain its outer Apply")
        };
        let TreeKind::Apply(inner) = typer.typed_ast().get(outer.function).kind.clone() else {
            panic!("curried constructor call should retain its constructor Apply")
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(inner.function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(selected),
                ..
            } if *selected == primary
        ));
    }

    #[test]
    fn overloaded_constructor_no_applicable_reports_each_rejection_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this(flag: Boolean) = this(1) }; class Use { def make: C = new C() }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
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
        let semantic_checkpoint = typer.store().checkpoint();
        let typed_tree_count = typer.typed_ast().iter().count();

        let error = typer.type_expression(rhs, context).unwrap_err();

        let TyperError::ConstructorApplicationNoApplicable {
            class: error_class,
            candidates,
            ..
        } = error
        else {
            panic!("expected per-candidate constructor rejections, got {error:?}")
        };
        assert_eq!(error_class, class);
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|(_, rejection)| matches!(
            rejection,
            OverloadRejection::WrongArity {
                expected: 1,
                actual: 0
            }
        )));
        assert_eq!(typer.store().checkpoint(), semantic_checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), typed_tree_count);
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn constructor_no_applicable_retains_application_kind_mismatch() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this(using flag: Boolean) = this(1) }; class Use { def make: C = new C(true) }",
        );
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

        let error = typer.type_expression(rhs, context).unwrap_err();

        assert!(
            matches!(
                error,
                TyperError::ConstructorApplicationNoApplicable { ref candidates, .. }
                    if candidates.len() == 2
                        && candidates.iter().any(|(_, reason)| matches!(
                            reason,
                            OverloadRejection::ApplicationKindMismatch {
                                application_kind: ApplyKind::Regular,
                                method_kind: MethodKind::Contextual,
                            }
                        ))
                        && candidates.iter().any(|(_, reason)| matches!(
                            reason,
                            OverloadRejection::ArgumentNonConformance { .. }
                        ))
            ),
            "{error:?}"
        );
    }

    #[test]
    fn constructor_no_applicable_retains_incomplete_signature_reason() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this(flag: Boolean) = this(1) }; class Use { def make: C = new C(true) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructors = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        assert_eq!(constructors.len(), 2);
        let incomplete = constructors[1];
        store.symbols.set_info(incomplete, SymbolInfo::Error);
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

        let error = typer.type_expression(rhs, context).unwrap_err();

        assert!(
            matches!(
                error,
                TyperError::ConstructorApplicationNoApplicable { ref candidates, .. }
                    if candidates.len() == 2
                        && candidates.iter().any(|(symbol, reason)| {
                            *symbol == incomplete
                                && matches!(reason, OverloadRejection::IncompleteSignature)
                        })
                        && candidates.iter().any(|(_, reason)| matches!(
                            reason,
                            OverloadRejection::ArgumentNonConformance { .. }
                        ))
            ),
            "{error:?}"
        );
    }

    #[test]
    fn equally_specific_constructors_are_ambiguous_and_roll_back() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this(value: Int) = this(1) }; class Use { def make: C = new C(1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
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
        let semantic_checkpoint = typer.store().checkpoint();
        let typed_tree_count = typer.typed_ast().iter().count();

        let error = typer.type_expression(rhs, context).unwrap_err();

        assert!(
            matches!(
                error,
                TyperError::AmbiguousConstructorApplication {
                    class: error_class,
                    ref candidates,
                    ..
                } if error_class == class && candidates.len() == 2
            ),
            "{error:?}"
        );
        assert_eq!(typer.store().checkpoint(), semantic_checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), typed_tree_count);
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn unsupported_generic_constructor_competitor_blocks_a_monomorphic_winner() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this[A](using value: A) = this(1) }; class Use { def make: C = new C(1) }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
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
        let store_checkpoint = typer.store().checkpoint();
        let typed_count = typer.typed_ast().iter().count();

        let error = typer.type_expression(rhs, context).unwrap_err();

        assert!(matches!(
            error,
            TyperError::ConstructorOverloadResolutionRequiresUnsupportedCandidate {
                class: error_class,
                ..
            } if error_class == class
        ));
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), typed_count);
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn regular_constructor_application_rejects_a_contextual_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(using value: Int); class Use { def make: C = new C(1) }");
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
    fn polymorphic_constructor_application_is_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int); class Use { def make: C = new C(1) }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
        let constructor = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup(&constructor_name)
            .unwrap();
        let class_type = nominal_type_ref(&mut store, definitions, class);
        let method = store.types.alloc(Type::Method(dotty_core::MethodType {
            params: Vec::new(),
            result: class_type,
            kind: MethodKind::Plain,
        }));
        let polymorphic = store.types.alloc(Type::Poly(dotty_core::PolyType {
            params: Vec::new(),
            result: method,
        }));
        store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(polymorphic));
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ConstructorPolymorphicApplicationDeferred { .. })
        ));
    }

    #[test]
    fn new_rejects_traits_and_abstract_classes_with_specific_errors() {
        for (source_text, class_name, expected_trait) in [
            ("trait T; class Use { def make: T = new T }", "T", true),
            (
                "abstract class C; class Use { def make: C = new C }",
                "C",
                false,
            ),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
            let class = class_symbol(&parsed, &store, &index, source, class_name);
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
            let semantic_checkpoint = typer.store().checkpoint();
            let typed_tree_count = typer.typed_ast().iter().count();

            let error = typer.type_expression(rhs, context).unwrap_err();

            if expected_trait {
                assert!(
                    matches!(error, TyperError::TraitInstantiation { symbol } if symbol == class)
                );
            } else {
                assert!(
                    matches!(error, TyperError::AbstractClassInstantiation { symbol } if symbol == class)
                );
            }
            assert_eq!(typer.store().checkpoint(), semantic_checkpoint);
            assert_eq!(typer.typed_ast().iter().count(), typed_tree_count);
            assert!(typer.source_typed_index().get(source, rhs).is_none());
        }
    }

    #[test]
    fn constructor_discovery_keeps_every_direct_overload_in_scope_order() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C(value: Int) { def this(flag: Boolean) = this(0) }; class Use { def make: C = new C }",
        );
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let instance_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        assert!(matches!(store.symbols.info(class), SymbolInfo::Missing));
        let constructor_name = dotty_core::Name::new(store.names.intern("<init>"), Namespace::Term);
        let expected_order = store
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&constructor_name)
            .to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let candidates = typer.constructors_of(instance_type).unwrap();

        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.symbol)
                .collect::<Vec<_>>(),
            expected_order
        );
        assert!(matches!(
            typer.store().symbols.info(class),
            SymbolInfo::Complete(_)
        ));
        assert!(candidates.iter().all(|candidate| candidate.owner == class));
        assert!(candidates.iter().all(|candidate| {
            typer.store().symbols.get(candidate.symbol).kind == SymbolKind::Constructor
        }));
        assert!(candidates.iter().all(|candidate| {
            matches!(typer.store().types.get(candidate.callable), Type::Method(_))
        }));
    }

    #[test]
    fn constructor_discovery_validates_source_class_type_argument_arity() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]; class Plain");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let plain_class = class_symbol(&parsed, &store, &index, source, "Plain");
        let wrong_box_arity = applied_class_type(
            &mut store,
            definitions,
            box_class,
            &[definitions.int, definitions.boolean],
        );
        let wrong_plain_arity =
            applied_class_type(&mut store, definitions, plain_class, &[definitions.int]);
        let raw_box = nominal_type_ref(&mut store, definitions, box_class);
        let valid_box = applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (target, expected_class, expected, actual) in [
            (wrong_box_arity, box_class, 1, 2),
            (raw_box, box_class, 1, 0),
            (wrong_plain_arity, plain_class, 0, 1),
        ] {
            assert!(matches!(
                typer.constructors_of(target),
                Err(TyperError::ConstructorTargetGenericArityMismatch {
                    class,
                    expected: error_expected,
                    actual: error_actual,
                }) if class == expected_class
                    && error_expected == expected
                    && error_actual == actual
            ));
        }
        assert_eq!(typer.constructors_of(valid_box).unwrap().len(), 1);
    }

    #[test]
    fn new_through_simple_alias_preserves_the_projected_alias_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { type Alias = C; def make: Alias = new Alias }");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let new_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(new_tree, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == alias
        ));
        let candidates = typer
            .constructors_of(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn rejected_alias_target_rolls_back_alias_completion_and_typed_allocations() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T; class Use { type Alias = T; def make: Alias = new Alias }");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let new_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let semantic_checkpoint = typer.store().checkpoint();
        let typed_tree_count = typer.typed_ast().iter().count();

        assert!(matches!(
            typer.type_expression(new_tree, context),
            Err(TyperError::TraitInstantiation { .. })
        ));

        assert_eq!(typer.store().checkpoint(), semantic_checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), typed_tree_count);
        assert!(matches!(
            typer.store().symbols.info(alias),
            SymbolInfo::Missing
        ));
        assert!(typer.source_typed_index().get(source, new_tree).is_none());
    }

    #[test]
    fn new_module_class_is_rejected_as_a_non_instantiable_target() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("object O");
        let (module_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => {
                    Some((tree, index.symbol_at(source, tree).unwrap()))
                }
                _ => None,
            })
            .unwrap();
        let object_owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(object_owner, source, module_tree)
            .unwrap();
        let instance_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(module_class),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        assert!(matches!(
            typer.constructors_of(instance_type),
            Err(TyperError::ModuleInstantiation { symbol }) if symbol == module_class
        ));
    }

    #[test]
    fn anonymous_template_new_is_explicitly_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C; class Use { def make: C = new C { val value: Int = 1 } }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let new_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(new_tree, context),
            Err(TyperError::AnonymousClassInstantiationDeferred { .. })
        ));
    }

    #[test]
    fn constructor_discovery_does_not_inherit_parent_constructors() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent(value: Int); class Child extends Parent(0); class Use { def make: Child = new Child }",
        );
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let new_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match node.kind {
                TreeKind::New(new)
                    if matches!(parsed.ast.try_get(new.tpt).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if store.names.resolve(ident.name.text()) == "Child") =>
                {
                    Some(tree)
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
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(new_tree, context).unwrap();

        let constructors = typer
            .constructors_of(typer.typed_ast().get(typed).ty)
            .unwrap();

        assert_eq!(constructors.len(), 1);
        assert_eq!(constructors[0].owner, child);
    }

    #[test]
    fn malformed_constructor_bucket_is_reported_instead_of_filtered() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let scope = index.scope_of(class).unwrap();
        let constructor_name = dotty_core::Name::new(store.names.intern("<init>"), Namespace::Term);
        let malformed = store.symbols.alloc(dotty_core::Symbol {
            name: constructor_name,
            owner: Some(class),
            kind: SymbolKind::Method,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(scope)
            .enter(constructor_name, malformed);
        let instance_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.constructors_of(instance_type),
            Err(TyperError::MalformedConstructorBucket { class: error_class, symbol })
                if error_class == class && symbol == malformed
        ));
    }

    #[test]
    fn new_type_parameter_is_rejected_without_assuming_concreteness() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Use { def make[A]: A = new A }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "make");
        let new_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::New(_)).then_some(tree))
            .unwrap();
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A" =>
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
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(new_tree, context),
            Err(TyperError::TypeParameterInstantiation { symbol }) if symbol == parameter
        ));
    }

    #[test]
    fn constructor_target_reports_unavailable_and_malformed_external_class_info() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(17);
        let missing = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let missing_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(missing),
        });
        let malformed = symbol(
            &mut store,
            SymbolKind::Class,
            SymbolInfo::Complete(definitions.int),
        );
        let malformed_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(malformed),
        });
        let wrong_scope_owner = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let wrong_scope = store
            .scopes
            .alloc(dotty_core::Scope::new(Some(wrong_scope_owner)));
        let wrong_scope_class = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let wrong_scope_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class: wrong_scope_class,
            parents: vec![definitions.object_type],
            declarations: wrong_scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(wrong_scope_class, SymbolInfo::Complete(wrong_scope_info));
        let wrong_scope_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(wrong_scope_class),
        });
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.constructors_of(missing_type),
            Err(TyperError::NewClassInfoUnavailable { symbol }) if symbol == missing
        ));
        assert!(matches!(
            typer.constructors_of(malformed_type),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == malformed && info == definitions.int
        ));
        assert!(matches!(
            typer.constructors_of(wrong_scope_type),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == wrong_scope_class && info == wrong_scope_info
        ));
    }

    #[test]
    fn malformed_completed_constructor_type_is_rejected() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let primary = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => index.symbol_at(source, template.constructor),
                _ => None,
            })
            .unwrap();
        let malformed_callable = definitions.unit;
        store
            .symbols
            .set_info(primary, SymbolInfo::Complete(malformed_callable));
        let instance_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.constructors_of(instance_type),
            Err(TyperError::MalformedConstructorCandidate { symbol, callable })
                if symbol == primary && callable == malformed_callable
        ));
    }

    #[test]
    fn constructor_target_alias_normalization_is_bounded() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(19);
        let class = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let mut target_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        for _ in 0..=crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            let alias = symbol(&mut store, SymbolKind::TypeAlias, SymbolInfo::Missing);
            let alias_reference = store.types.alloc(Type::TypeRef {
                prefix: definitions.no_prefix,
                target: TypeRefTarget::Symbol(alias),
            });
            let alias_info = store
                .types
                .alloc(Type::AliasingBounds { alias: target_type });
            store
                .symbols
                .set_info(alias, SymbolInfo::Complete(alias_info));
            target_type = alias_reference;
        }
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.constructors_of(target_type),
            Err(TyperError::ConstructorLookupNormalization {
                error: crate::types::TypeNormalizeError::TooDeep,
                ..
            })
        ));
    }

    #[test]
    fn method_body_context_resolves_existing_parameter_without_mutating_scopes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def id(x: Int): Int = x }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "id");
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
        let scope = index.scope_of(method).unwrap();
        let name = store.symbols.get(parameter).name;
        let bucket_before = store.scopes.get(scope).lookup_all(&name).to_vec();
        let store_before = store.checkpoint();
        assert_eq!(bucket_before, vec![parameter]);

        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        assert!(context.local_scopes.is_some());
        assert_eq!(typer.store().checkpoint(), store_before);
        assert_eq!(
            typer.store().scopes.get(scope).lookup_all(&name),
            bucket_before
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
    }

    #[test]
    fn expression_block_types_stats_and_preserves_its_final_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def block(x: Int): Int = { x; true; 1 } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "block");
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
        let source_position = parsed.ast.get(rhs).position;
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("method RHS should be a source block");
        };
        assert_eq!(source_block.stats.len(), 2);
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

        let typed_node = typer.typed_ast().get(typed);
        let TreeKind::Block(typed_block) = &typed_node.kind else {
            panic!("source block should produce a typed block");
        };
        assert_eq!(typed_block.stats.len(), 2);
        assert_eq!(typed_node.position, source_position);
        assert_eq!(
            typed_node.ty,
            typer.typed_ast().get(typed_block.expr).ty,
            "block type must be the final expression's own type"
        );
        assert!(matches!(
            typer.store().types.get(typed_node.ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_block.stats[0]).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == parameter
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_block.stats[1]).ty),
            Type::Constant(dotty_core::Constant::Boolean(true))
        ));
        assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
        for source_stat in &source_block.stats {
            assert!(
                typer
                    .source_typed_index()
                    .get(source, *source_stat)
                    .is_some()
            );
        }
        assert!(
            typer
                .source_typed_index()
                .get(source, source_block.expr)
                .is_some()
        );
        assert_eq!(typer.expression_scopes.len(), 2);
        assert_ne!(
            typer.expression_scopes[0].scope,
            typer.expression_scopes[1].scope
        );
        assert_eq!(
            typer
                .store()
                .scopes
                .get(typer.expression_scopes[1].scope)
                .owner,
            Some(method)
        );
        assert_eq!(typer.type_expression(rhs, context).unwrap(), typed);
        assert_eq!(typer.expression_scopes.len(), 2);
    }

    #[test]
    fn empty_unit_style_block_keeps_its_block_shape() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def empty: Unit = {} }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "empty");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("empty braced expression should remain a source block");
        };
        assert!(source_block.stats.is_empty());
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

        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("empty source block should still produce a typed block");
        };
        assert!(block.stats.is_empty());
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::Constant(dotty_core::Constant::Unit)
        ));
        assert_eq!(
            typer.typed_ast().get(typed).ty,
            typer.typed_ast().get(block.expr).ty
        );
    }

    #[test]
    fn expression_block_keeps_a_final_term_reference_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def block(x: Int): Int = { 1; x } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "block");
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
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
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("source block should produce a typed block");
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == parameter
        ));
        assert_eq!(
            typer.typed_ast().get(typed).ty,
            typer.typed_ast().get(block.expr).ty
        );
    }

    #[test]
    fn nested_expression_blocks_push_nested_scopes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def block: Int = { { 1 }; 2 } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "block");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let method_scope = context.local_scopes.unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("outer source block should produce a typed block");
        };
        assert!(matches!(
            typer.typed_ast().get(block.stats[0]).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(typer.expression_scopes.len(), 3);
        assert_eq!(typer.expression_scopes[1].parent, Some(method_scope));
        assert_eq!(
            typer.expression_scopes[2]
                .parent
                .map(ExpressionScopeId::index),
            Some(1)
        );
        assert_ne!(
            typer.expression_scopes[1].scope,
            typer.expression_scopes[2].scope
        );
    }

    #[test]
    fn failed_block_typing_rolls_back_typed_nodes_mappings_and_scopes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def block: Int = { 1; missing } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "block");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let store_checkpoint = typer.store().checkpoint();
        let expression_scope_checkpoint = typer.expression_scopes.len();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert!(typer.source_typed_index().is_empty());
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.expression_scopes.len(), expression_scope_checkpoint);
    }

    #[test]
    fn unsupported_type_declaration_statements_are_deferred_explicitly() {
        let sources = [(
            "class C { def use: Int = { 1; type Local = Int; 3 } }",
            "type definition",
        )];

        for (source_text, expected_kind) in sources {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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
            let store_checkpoint = typer.store().checkpoint();
            let expression_scope_checkpoint = typer.expression_scopes.len();

            assert!(
                matches!(
                    typer.type_expression(rhs, context),
                    Err(TyperError::LocalBlockDeclarationDeferred { kind, .. })
                        if kind == expected_kind
                ),
                "expected `{expected_kind}` to be deferred for `{source_text}`"
            );
            assert_eq!(typer.typed_ast().iter().count(), 0);
            assert!(typer.source_typed_index().is_empty());
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
            assert_eq!(typer.expression_scopes.len(), expression_scope_checkpoint);
        }
    }

    #[test]
    fn block_local_imports_type_following_terms_and_type_trees() {
        let source_text = include_str!("../../tests/fixtures/local-imports/LocalImports.scala");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (term_method, term_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "term");
        let (type_method, type_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "localType");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let term_context = typer.expression_context_for(term_method).unwrap();
        let TreeKind::Block(source_term_block) = &parsed.ast.get(term_rhs).kind else {
            panic!("term method should retain its source block")
        };
        let source_import_tree = source_term_block.stats[0];
        let TreeKind::Import(source_import) = &parsed.ast.get(source_import_tree).kind else {
            panic!("first source statement should be an import")
        };
        let typed_term = typer.type_expression(term_rhs, term_context).unwrap();
        let TreeKind::Block(term_block) = &typer.typed_ast().get(typed_term).kind else {
            panic!("local term import should remain in its typed block")
        };
        let TreeKind::Import(typed_import) = &typer.typed_ast().get(term_block.stats[0]).kind
        else {
            panic!("the first block statement should be a typed import")
        };
        assert_eq!(typed_import.selectors.len(), 1);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(term_block.stats[0]).ty),
            Type::NoType
        ));
        assert_eq!(
            typer.source_typed_index().get(source, source_import_tree),
            Some(term_block.stats[0])
        );
        assert!(
            typer
                .source_typed_index()
                .get(source, source_import.expr)
                .is_some()
        );
        assert!(matches!(
            typer.typed_ast().get(term_block.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(term_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "member"
        ));

        let type_context = typer.expression_context_for(type_method).unwrap();
        let typed_type = typer.type_expression(type_rhs, type_context).unwrap();
        let TreeKind::Block(type_block) = &typer.typed_ast().get(typed_type).kind else {
            panic!("local type import should remain in its typed block")
        };
        assert!(matches!(
            typer.typed_ast().get(type_block.stats[0]).kind,
            TreeKind::Import(_)
        ));
        let TreeKind::ValDef(local_value) = &typer.typed_ast().get(type_block.stats[1]).kind else {
            panic!("local value should remain in the typed block")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(local_value.tpt).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == class_symbol(&parsed, typer.store(), &index, source, "Box")
        ));

        let oracle = include_str!("../../tests/fixtures/local-imports/LocalImports.typed-tree.txt");
        assert!(oracle.contains("import lib.Owner.member"));
        assert!(oracle.contains("val box: lib.Owner.Box = value"));
    }

    #[test]
    fn local_import_selectors_follow_statement_and_nested_scope_rules() {
        let source_text = "package lib { object First { val item: Int = 1; val other: Int = 2 }; object Second { val item: Int = 3 } }; package app { object Use { def renamed: Int = { import lib.First.{item as alias}; alias }; def wildcard: Int = { import lib.First.*; other }; def hidden: Int = { import lib.First.{item as _, *}; item }; def ambiguous: Int = { import lib.First.*; import lib.Second.*; item }; def before: Int = { val earlier: Int = item; import lib.First.item; item }; def direct: Int = { val item: Int = 4; import lib.First.item; item }; def parameter(item: Int): Int = { import lib.First.item; item }; def nested: Int = { import lib.First.item; { item } }; def leak: Int = { { import lib.First.item; item }; item } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for name in ["renamed", "wildcard", "nested"] {
            let (method, rhs) =
                method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
            let context = typer.expression_context_for(method).unwrap();
            typer.type_expression(rhs, context).unwrap_or_else(|error| {
                panic!("local import method `{name}` should type: {error:?}")
            });
        }

        let (_, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "renamed");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("renamed import method should have a block")
        };
        let source_import_tree = source_block.stats[0];
        let TreeKind::Import(source_import) = &parsed.ast.get(source_import_tree).kind else {
            panic!("first source statement should be the import")
        };
        let renamed_source_tree = source_import.selectors[0]
            .renamed
            .expect("renamed selector should have a name tree");
        let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("renamed import method should have a typed block")
        };
        let TreeKind::Import(typed_import) = &typer.typed_ast().get(typed_block.stats[0]).kind
        else {
            panic!("first typed statement should be the import")
        };
        let typed_renamed_tree = typed_import.selectors[0]
            .renamed
            .expect("typed renamed selector should retain its name tree");
        assert!(matches!(
            typer.typed_ast().get(typed_renamed_tree).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            typer.source_typed_index().get(source, renamed_source_tree),
            Some(typed_renamed_tree)
        );

        for name in ["hidden", "before", "leak"] {
            let (method, rhs) =
                method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
            let context = typer.expression_context_for(method).unwrap();
            assert!(
                matches!(
                    typer.type_expression(rhs, context),
                    Err(TyperError::TermNameNotFound { name, .. })
                        if typer.store().names.resolve(name.text()) == "item"
                ),
                "`{name}` should not resolve an inactive or hidden local import"
            );
        }

        for name in ["ambiguous", "parameter"] {
            let (method, rhs) =
                method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
            let context = typer.expression_context_for(method).unwrap();
            assert!(
                matches!(
                    typer.type_expression(rhs, context),
                    Err(TyperError::AmbiguousTermReference { name, .. })
                        if typer.store().names.resolve(name.text()) == "item"
                ),
                "`{name}` should report a same-depth local import ambiguity"
            );
        }

        let (method, rhs) =
            method_definition_and_rhs(&parsed, typer.store(), &index, source, "direct");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("expected direct-shadowing method body to be a block")
        };
        let source_local = source_block.stats[0];
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        let local_symbol = typer.local_symbol_at(source, source_local).unwrap();
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed direct-shadowing block")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local_symbol
        ));
    }

    #[test]
    fn failed_later_local_import_block_rolls_back_import_state_for_retry() {
        let source_text = "package lib { object Owner { val item: Int = 1 } }; package app { object Use { def broken: Int = { import lib.Owner.item; missing } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "broken");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let scope_count = typer.expression_scopes.len();
        let store_checkpoint = typer.store().checkpoint();

        for _ in 0..2 {
            assert!(matches!(
                typer.type_expression(rhs, context),
                Err(TyperError::TermNameNotFound { name, .. })
                    if typer.store().names.resolve(name.text()) == "missing"
            ));
            assert_eq!(typer.expression_scopes.len(), scope_count);
            assert!(typer.source_typed_index().is_empty());
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
        }
    }

    #[test]
    fn local_imports_precede_source_imports_but_conflict_with_enclosing_members() {
        let source_text = "package lib { object First { val shared: Int = 1 }; object Second { val shared: Int = 2 } }; package app { import lib.First.*; object Use { def sourceImport: Int = { import lib.Second.*; shared } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "sourceImport");
        let shared_symbols = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "shared" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let second = *shared_symbols
            .get(1)
            .expect("both imported members should be indexed");
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
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == second
        ));

        let source_text = "package lib { object Owner { val shared: Int = 1 } }; package app { object Use { val shared: Int = 2; def conflict: Int = { import lib.Owner.shared; shared } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "conflict");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::AmbiguousTermReference { name, .. })
                if typer.store().names.resolve(name.text()) == "shared"
        ));
    }

    #[test]
    fn local_type_import_conflicts_with_enclosing_type_member() {
        let source_text = "package lib { object Owner { class Box } }; package app { class Use { class Box; def conflict(value: lib.Owner.Box): Box = { import lib.Owner.Box; val local: Box = value; local } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "conflict");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::AmbiguousTypeName { name, .. })
                if typer.store().names.resolve(name.text()) == "Box"
        ));
    }

    #[test]
    fn local_import_qualifiers_resolve_prior_locals_and_import_aliases() {
        let source_text = "package lib { object Owner { val member: Int = 1 } }; package app { import lib.Owner; object Use { def viaLocalValue: Int = { val local = Owner; import local.member; member }; def viaImportedAlias: Int = { import lib.{Owner as O}; import O.member; member } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for name in ["viaLocalValue", "viaImportedAlias"] {
            let (method, rhs) =
                method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
            let context = typer.expression_context_for(method).unwrap();
            typer.type_expression(rhs, context).unwrap_or_else(|error| {
                panic!("local import qualifier in `{name}` should resolve: {error:?}")
            });
        }
    }

    #[test]
    fn explicitly_typed_local_value_is_entered_after_its_initializer() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local: Int = 1; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("method body should be a source block");
        };
        let local_tree = block.stats[0];
        let local_position = parsed.ast.get(local_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block_id = typer.type_expression(rhs, context).unwrap();

        let local = typer.local_symbol_at(source, local_tree).unwrap();
        let declaration = typer.store().symbols.get(local);
        assert_eq!(declaration.kind, SymbolKind::Local);
        assert_eq!(declaration.owner, Some(method));
        assert_eq!(declaration.visibility, dotty_core::Visibility::Public);
        assert_eq!(declaration.origin, SymbolOrigin::Source(source));
        assert_eq!(declaration.position, local_position);
        assert_eq!(declaration.info, SymbolInfo::Complete(definitions.int));
        assert!(!declaration.flags.contains(SymbolFlags::MUTABLE));

        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_block_id).kind else {
            panic!("source block should produce a typed block");
        };
        let TreeKind::ValDef(typed_local) = &typer.typed_ast().get(typed_block.stats[0]).kind
        else {
            panic!("local declaration should remain a typed ValDef");
        };
        assert_eq!(
            typed_local.name,
            match &parsed.ast.get(local_tree).kind {
                TreeKind::ValDef(definition) => definition.name,
                _ => unreachable!(),
            }
        );
        assert_eq!(
            typer.typed_ast().get(typed_block.stats[0]).position,
            local_position
        );
        assert_eq!(typer.typed_ast().get(typed_local.tpt).ty, definitions.int);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_local.rhs.unwrap()).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert_eq!(
            typer.typed_ast().get(typed_block.stats[0]).ty,
            definitions.int
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local
        ));
    }

    #[test]
    fn explicitly_typed_local_var_is_mutable_but_widens_to_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { var local: Int = 1; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("method body should be a source block");
        };
        let local_tree = block.stats[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block = typer.type_expression(rhs, context).unwrap();

        let local = typer.local_symbol_at(source, local_tree).unwrap();
        assert_eq!(typer.store().symbols.get(local).kind, SymbolKind::Local);
        assert!(
            typer
                .store()
                .symbols
                .get(local)
                .flags
                .contains(SymbolFlags::MUTABLE)
        );
        let TreeKind::Block(block) = &typer.typed_ast().get(typed_block).kind else {
            panic!("source block should produce a typed block");
        };
        let reference = block.expr;
        let reference_type = typer.typed_ast().get(reference).ty;
        assert!(matches!(
            typer.store().types.get(reference_type),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local
        ));
        assert_eq!(
            typer.widen_expression_type(reference_type).unwrap(),
            definitions.int
        );
    }

    #[test]
    fn local_initializer_cannot_resolve_the_definition_being_entered() {
        // In Scala 3.9 commit 777528f19a58e794c9954a42f433373472ec57f8,
        // typedBlockStats indexes definitions for the statement sequence before
        // typedStats types each RHS, and typedValDef then types the RHS. The
        // local therefore shadows this same-typed class field in its RHS, where
        // the compiler reports a recursive/forward reference.
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val local: Int = 1; def use: Int = { val local: Int = local; local } }",
        );
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
        let store_checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::RecursiveLocalValueInitializer { source: actual, .. })
                if actual == source
        ));
        assert!(typer.local_symbols.is_empty());
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
    }

    #[test]
    fn local_method_flags_preserve_supported_source_modifiers() {
        let cases = [
            (Modifier::Abstract, SymbolFlags::ABSTRACT),
            (Modifier::Final, SymbolFlags::FINAL),
            (Modifier::Implicit, SymbolFlags::IMPLICIT),
            (Modifier::Given, SymbolFlags::GIVEN),
            (Modifier::Override, SymbolFlags::OVERRIDE),
            (Modifier::Inline, SymbolFlags::INLINE),
            (Modifier::Transparent, SymbolFlags::TRANSPARENT),
            (Modifier::Extension, SymbolFlags::EXTENSION),
            (Modifier::Erased, SymbolFlags::ERASED),
        ];
        for (modifier, expected) in cases {
            assert_eq!(source_method_flags(&[modifier]), expected, "{modifier:?}");
        }
    }

    #[test]
    fn local_method_headers_are_preentered_and_forward_calls_resolve_their_identity() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def outer: Int = { foo(); inline def foo(): Int = 1; 0 } }");
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let method_tree = block.stats[1];
        let call_tree = block.stats[0];
        let TreeKind::Apply(call) = &parsed.ast.get(call_tree).kind else {
            panic!("first block stat should call the forward method")
        };
        let TreeKind::Ident(callee) = &parsed.ast.get(call.function).kind else {
            panic!("forward method call should have an identifier callee")
        };
        let call_name = callee.name;
        let call_position = parsed.ast.get(call.function).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let outer_context = typer.expression_context_for(outer).unwrap();
        let (block_scope, block_context) =
            preindex_block_for_test(&mut typer, block_tree, outer_context);

        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let method_definition = typer.local_method_definition(method).unwrap();
        assert_eq!(method_definition, (source, method_tree));
        assert_eq!(typer.store().symbols.get(method).kind, SymbolKind::Method);
        assert_eq!(typer.store().symbols.get(method).owner, Some(outer));
        assert_eq!(typer.store().symbols.get(method).name, call_name);
        assert_eq!(
            typer.store().symbols.get(method).position,
            parsed.ast.get(method_tree).position
        );
        assert!(
            typer
                .store()
                .symbols
                .get(method)
                .flags
                .contains(SymbolFlags::INLINE)
        );
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert_eq!(typer.store().scopes.get(block_scope).owner, Some(outer));
        assert_eq!(
            typer.store().scopes.get(block_scope).lookup_all(&call_name),
            &[method]
        );
        assert_eq!(
            typer
                .expression_term_candidates(
                    call_name,
                    block_context,
                    call.function.index(),
                    call_position
                )
                .unwrap(),
            vec![method]
        );

        let method_scope = typer.local_method_scope(method).unwrap();
        assert_ne!(method_scope, block_scope);
        assert_eq!(typer.store().scopes.get(method_scope).owner, Some(method));
        assert_eq!(
            typer.local_method_declaration_context(method),
            Some(block_context)
        );
        assert!(index.definition_of(method).is_none());
        assert!(index.scope_of(method).is_none());
        assert!(index.declaration_context_of(method).is_none());
        let method_context = typer.expression_context_for(method).unwrap();
        assert_eq!(method_context.owner, method);
        let method_frame = &typer.expression_scopes[method_context.local_scopes.unwrap().index()];
        assert_eq!(method_frame.scope, method_scope);
        assert_eq!(method_frame.parent, block_context.local_scopes);
    }

    #[test]
    fn same_name_local_methods_share_an_overload_bucket_in_source_order() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def same(value: Int): Int = value; def same(value: Boolean): Int = 1; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let method_trees = block.stats[..2].to_vec();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let outer_context = typer.expression_context_for(outer).unwrap();
        let (block_scope, _) = preindex_block_for_test(&mut typer, block_tree, outer_context);
        let methods = method_trees
            .iter()
            .map(|tree| typer.local_method_symbol_at(source, *tree).unwrap())
            .collect::<Vec<_>>();
        let same_name = *match &parsed.ast.get(method_trees[0]).kind {
            TreeKind::DefDef(definition) => definition.name.as_name(),
            _ => panic!("local method tree should be a DefDef"),
        };

        assert_eq!(
            typer.store().scopes.get(block_scope).lookup_all(&same_name),
            methods
        );
        let scopes = methods
            .iter()
            .map(|method| typer.local_method_scope(*method).unwrap())
            .collect::<Vec<_>>();
        assert_ne!(scopes[0], scopes[1]);
        for (method, scope) in methods.iter().zip(scopes) {
            assert_eq!(typer.store().scopes.get(scope).owner, Some(*method));
        }
    }

    #[test]
    fn nested_block_methods_are_indexed_only_in_their_own_block_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def outer: Int = { { def hidden(): Int = 1; 0 }; 2 } }");
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(outer_block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let TreeKind::Block(nested_block) = &parsed.ast.get(outer_block.stats[0]).kind else {
            panic!("first outer stat should be a nested block")
        };
        let nested_tree = outer_block.stats[0];
        let hidden_tree = nested_block.stats[0];
        let hidden_name = match &parsed.ast.get(hidden_tree).kind {
            TreeKind::DefDef(definition) => *definition.name.as_name(),
            _ => panic!("nested method tree should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let outer_context = typer.expression_context_for(outer).unwrap();
        let (outer_scope, outer_context) =
            preindex_block_for_test(&mut typer, block_tree, outer_context);

        assert!(
            typer
                .store()
                .scopes
                .get(outer_scope)
                .lookup_all(&hidden_name)
                .is_empty()
        );
        assert!(typer.local_method_symbol_at(source, hidden_tree).is_none());

        let (nested_scope, _) = preindex_block_for_test(&mut typer, nested_tree, outer_context);
        let hidden = typer.local_method_symbol_at(source, hidden_tree).unwrap();
        assert_eq!(
            typer
                .store()
                .scopes
                .get(nested_scope)
                .lookup_all(&hidden_name),
            &[hidden]
        );
        assert!(
            typer
                .store()
                .scopes
                .get(outer_scope)
                .lookup_all(&hidden_name)
                .is_empty()
        );
    }

    #[test]
    fn plain_local_method_signatures_complete_with_owned_parameter_symbols() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def zero(): Int = 1; def pair(left: Int, right: Boolean): Int = 1; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let zero_tree = block.stats[0];
        let pair_tree = block.stats[1];
        let pair_parameters = match &parsed.ast.get(pair_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0].clone(),
            _ => panic!("pair declaration should be a DefDef"),
        };
        let zero_parameters = match &parsed.ast.get(zero_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0].clone(),
            _ => panic!("zero declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let outer_context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, outer_context);

        let zero = typer.local_method_symbol_at(source, zero_tree).unwrap();
        let zero_info = typer.complete_symbol(zero).unwrap();
        let Type::Method(zero_signature) = typer.store().types.get(zero_info) else {
            panic!("zero-arg local method should have a Method signature")
        };
        assert!(zero_signature.params.is_empty());
        assert_eq!(zero_signature.kind, MethodKind::Plain);
        assert_eq!(zero_signature.result, definitions.int);

        let pair = typer.local_method_symbol_at(source, pair_tree).unwrap();
        let pair_info = typer.complete_symbol(pair).unwrap();
        let Type::Method(pair_signature) = typer.store().types.get(pair_info) else {
            panic!("pair local method should have a Method signature")
        };
        assert_eq!(pair_signature.params.len(), 2);
        assert_eq!(
            pair_signature.params[0].name,
            match &parsed.ast.get(pair_parameters[0]).kind {
                TreeKind::ValDef(parameter) => parameter.name,
                _ => panic!("first parameter should be a ValDef"),
            }
        );
        assert_eq!(pair_signature.params[0].ty, definitions.int);
        assert_eq!(pair_signature.params[1].ty, definitions.boolean);
        assert_eq!(pair_signature.kind, MethodKind::Plain);
        assert_eq!(pair_signature.result, definitions.int);
        for (parameter_index, parameter_tree) in pair_parameters.iter().copied().enumerate() {
            let parameter = typer
                .local_method_parameter_symbol_at(source, parameter_tree)
                .unwrap();
            assert_eq!(typer.store().symbols.get(parameter).owner, Some(pair));
            assert_eq!(
                typer.store().symbols.get(parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer
                    .store()
                    .scopes
                    .get(typer.local_method_scope(pair).unwrap())
                    .lookup_all(&typer.store().symbols.get(parameter).name),
                &[parameter]
            );
            assert_eq!(
                *typer.store().symbols.info(parameter),
                SymbolInfo::Complete([definitions.int, definitions.boolean][parameter_index])
            );
        }
        let checkpoint = typer.store().checkpoint();
        assert_eq!(typer.complete_symbol(pair).unwrap(), pair_info);
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(zero_parameters.is_empty());
        assert_eq!(
            typer.local_method_scope(zero).unwrap(),
            typer.method_scope(zero).unwrap()
        );
    }

    #[test]
    fn generic_local_method_signature_uses_owned_type_parameter_binders() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def identity[A](value: A): A = value; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let method_tree = match &parsed.ast.get(block_tree).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("outer body should be a block"),
        };
        let (type_parameter_tree, value_parameter_tree) = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => (
                definition.type_params[0],
                definition.value_param_clauses[0][0],
            ),
            _ => panic!("local declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();

        let signature = typer.complete_symbol(method).unwrap();

        let type_parameter = typer
            .local_method_type_parameter_symbol_at(source, type_parameter_tree)
            .unwrap();
        let value_parameter = typer
            .local_method_parameter_symbol_at(source, value_parameter_tree)
            .unwrap();
        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("generic local method should have a Poly signature");
        };
        assert_eq!(poly.params.len(), 1);
        assert_eq!(
            poly.params[0].name,
            match &parsed.ast.get(type_parameter_tree).kind {
                TreeKind::TypeDef(definition) => definition.name,
                _ => unreachable!(),
            }
        );
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("generic local method should have a term clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(method_type.result),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert_eq!(
            typer.store().symbols.get(type_parameter).owner,
            Some(method)
        );
        assert_eq!(
            typer.store().symbols.get(value_parameter).owner,
            Some(method)
        );
        let SymbolInfo::Complete(parameter_info) = *typer.store().symbols.info(value_parameter)
        else {
            panic!("local parameter should have a completed type");
        };
        assert!(matches!(
            typer.store().types.get(parameter_info),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == type_parameter
        ));
    }

    #[test]
    fn two_local_type_parameters_keep_distinct_poly_binder_indices() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def second[A, B](first: A)(last: B): B = last; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let method_tree = match &parsed.ast.get(block_tree).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("outer body should be a block"),
        };
        let (type_parameter_trees, first_parameter_tree, last_parameter_tree) =
            match &parsed.ast.get(method_tree).kind {
                TreeKind::DefDef(definition) => (
                    definition.type_params.clone(),
                    definition.value_param_clauses[0][0],
                    definition.value_param_clauses[1][0],
                ),
                _ => panic!("local declaration should be a DefDef"),
            };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("local method should have a Poly signature");
        };
        assert_eq!(poly.params.len(), 2);
        let Type::Method(first_clause) = typer.store().types.get(poly.result) else {
            panic!("first term clause should be a Method");
        };
        let Type::Method(second_clause) = typer.store().types.get(first_clause.result) else {
            panic!("second term clause should remain nested");
        };
        assert!(matches!(
            typer.store().types.get(first_clause.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(second_clause.params[0].ty),
            Type::ParamRef { binder, index: 1 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(second_clause.result),
            Type::ParamRef { binder, index: 1 } if *binder == signature
        ));
        for parameter_tree in type_parameter_trees {
            let parameter = typer
                .local_method_type_parameter_symbol_at(source, parameter_tree)
                .unwrap();
            assert_eq!(typer.store().symbols.get(parameter).owner, Some(method));
        }
        assert!(
            typer
                .local_method_parameter_symbol_at(source, first_parameter_tree)
                .is_some()
        );
        assert!(
            typer
                .local_method_parameter_symbol_at(source, last_parameter_tree)
                .is_some()
        );
    }

    #[test]
    fn local_method_signature_preserves_curried_and_contextual_clauses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def local[A](first: A)(using context: Int)(last: A): A = last; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let method_tree = match &parsed.ast.get(block_tree).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("outer body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("local method should preserve its type parameter clause");
        };
        let Type::Method(first_clause) = typer.store().types.get(poly.result) else {
            panic!("first term clause should be a Method");
        };
        assert_eq!(first_clause.kind, MethodKind::Plain);
        let Type::Method(contextual_clause) = typer.store().types.get(first_clause.result) else {
            panic!("contextual term clause should remain nested");
        };
        assert_eq!(contextual_clause.kind, MethodKind::Contextual);
        let Type::Method(last_clause) = typer.store().types.get(contextual_clause.result) else {
            panic!("final term clause should remain nested");
        };
        assert_eq!(last_clause.kind, MethodKind::Plain);
        assert!(matches!(
            typer.store().types.get(last_clause.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(last_clause.result),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
    }

    #[test]
    fn forward_local_method_call_completes_its_preentered_signature() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { local(1); def local[A](value: A): A = value; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let call_tree = block.stats[0];
        let local_tree = block.stats[1];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let outer_context = typer.expression_context_for(outer).unwrap();
        let (_, block_context) = preindex_block_for_test(&mut typer, block_tree, outer_context);
        let local = typer.local_method_symbol_at(source, local_tree).unwrap();

        let typed_call = typer.type_expression(call_tree, block_context).unwrap();

        assert_eq!(typer.typed_ast().get(typed_call).ty, definitions.int);
        let SymbolInfo::Complete(local_info) = *typer.store().symbols.info(local) else {
            panic!("forward call should complete the local method symbol")
        };
        let Type::Poly(poly) = typer.store().types.get(local_info) else {
            panic!("forward call should complete the generic local method signature")
        };
        let Type::Method(signature) = typer.store().types.get(poly.result) else {
            panic!("generic local method should contain a term clause")
        };
        assert_eq!(signature.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(signature.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == local_info
        ));
        assert!(matches!(
            typer.store().types.get(signature.result),
            Type::ParamRef { binder, index: 0 } if *binder == local_info
        ));
    }

    #[test]
    fn local_method_signature_uses_enclosing_lexical_type_context() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { class Token; def outer: Int = { def id(value: Token): Token = value; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let local_tree = block.stats[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let local = typer.local_method_symbol_at(source, local_tree).unwrap();

        let signature = typer.complete_symbol(local).unwrap();

        let Type::Method(signature) = typer.store().types.get(signature) else {
            panic!("local method should have a Method signature")
        };
        let token = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let symbol = index.symbol_at(source, tree)?;
                (matches!(node.kind, TreeKind::TypeDef(_))
                    && typer.store().symbols.get(symbol).kind == SymbolKind::Class
                    && typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(symbol).name.text())
                        == "Token")
                    .then_some(symbol)
            })
            .expect("Token class should be named");
        assert!(
            matches!(typer.store().types.get(signature.params[0].ty), Type::TypeRef { target: TypeRefTarget::Symbol(found), .. } if *found == token)
        );
        assert!(matches!(
            (typer.store().types.get(signature.params[0].ty), typer.store().types.get(signature.result)),
            (Type::TypeRef { target: TypeRefTarget::Symbol(parameter), .. }, Type::TypeRef { target: TypeRefTarget::Symbol(result), .. }) if parameter == result && *result == token
        ));
    }

    #[test]
    fn local_dependent_result_is_deferred_without_resolving_a_shadowed_outer_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val item: String = \"outer\"; def outer: Int = { def id(item: Int): item.type = item; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let method_tree = block.stats[0];
        let (parameter_tree, result_tree) = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => (definition.value_param_clauses[0][0], definition.tpt),
            _ => panic!("local declaration should be a DefDef"),
        };
        assert!(matches!(
            parsed.ast.get(result_tree).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::LocalMethodSignatureDeferred {
                feature: "dependent result types",
                ..
            })
        ));
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert!(
            typer
                .local_method_parameter_symbol_at(source, parameter_tree)
                .is_none()
        );
    }

    #[test]
    fn local_methods_defer_inferred_and_unsupported_signature_shapes() {
        let cases = [(
            "def local(value: => Int): Int = value",
            "by-name parameters",
        )];
        for (declaration, _) in cases {
            let source_code = format!("class C {{ def outer: Int = {{ {declaration}; 0 }} }}");
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(&source_code);
            let (outer, block_tree) =
                method_definition_and_rhs(&parsed, &store, &index, source, "outer");
            let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
                panic!("outer body should be a block")
            };
            let local_tree = block.stats[0];
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let context = typer.expression_context_for(outer).unwrap();
            preindex_block_for_test(&mut typer, block_tree, context);
            let local = typer.local_method_symbol_at(source, local_tree).unwrap();

            let error = typer.complete_symbol(local).unwrap_err();
            match (
                declaration.contains("= value") && !declaration.contains(": Int ="),
                error,
            ) {
                (true, TyperError::LocalMethodInferredResultDeferred { symbol, .. }) => {
                    assert_eq!(symbol, local);
                }
                (
                    _,
                    TyperError::LocalMethodSignatureDeferred {
                        symbol, feature, ..
                    },
                ) => {
                    assert_eq!(symbol, local);
                    assert_eq!(
                        feature,
                        cases
                            .iter()
                            .find(|(candidate, _)| candidate == &declaration)
                            .unwrap()
                            .1
                    );
                }
                (_, other) => panic!("unexpected local signature error: {other:?}"),
            }
        }
    }

    #[test]
    fn local_method_repeated_parameter_uses_repeated_and_varargs_signature() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def outer: Int = { def inner(xs: Int*): Int = 1; 0 } }");
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[0];
        let parameter_tree = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("local declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();

        let method_type = typer.complete_symbol(method).unwrap();
        let Type::Method(method_type) = typer.store().types.get(method_type) else {
            panic!("local method should complete to MethodType");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.params[0].ty, definitions.int);
        assert!(method_type.params[0].varargs);
        let parameter = typer
            .local_method_parameter_symbol_at(source, parameter_tree)
            .unwrap();
        let SymbolInfo::Complete(parameter_type) = *typer.store().symbols.info(parameter) else {
            panic!("local parameter should retain its repeated type");
        };
        assert!(matches!(
            typer.store().types.get(parameter_type),
            Type::Repeated { element } if *element == definitions.int
        ));
    }

    #[test]
    fn inferred_local_method_uses_typed_body_and_reifies_result_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def local(value: Int) = value; local(1) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[0];
        let result_tree = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition.tpt,
            _ => panic!("local declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        let typed_block = typer.type_expression(block_tree, context).unwrap();
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let SymbolInfo::Complete(signature) = *typer.store().symbols.info(method) else {
            panic!("inferred local method should be completed");
        };
        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("inferred local method should have a Method signature");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.result, definitions.int);

        let typed_method = typer.source_typed_index().get(source, method_tree).unwrap();
        let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
            panic!("local declaration should produce a typed DefDef");
        };
        assert_eq!(
            typed_definition.tpt,
            typer.source_typed_index().get(source, result_tree).unwrap()
        );
        assert_eq!(
            typer.typed_ast().get(typed_definition.tpt).ty,
            definitions.int
        );
        let rhs = typed_definition.rhs.unwrap();
        let parameter_tree = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => unreachable!(),
        };
        let parameter = typer
            .local_method_parameter_symbol_at(source, parameter_tree)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(rhs).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
        assert_eq!(typer.complete_symbol(method).unwrap(), signature);
        assert!(typer.inferred_method_results_in_progress.is_empty());
        assert_eq!(
            typer.type_expression(block_tree, context).unwrap(),
            typed_block
        );
    }

    #[test]
    fn recursive_inferred_local_method_fails_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def outer: Int = { def loop = loop; loop } }");
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::RecursiveInferredMethodResult { symbol }) if symbol == method
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert!(typer.inferred_method_results_in_progress.is_empty());
    }

    #[test]
    fn mutually_recursive_inferred_local_methods_fail_without_leaking_state() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def first = second; def second = first; first } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let first_tree = block.stats[0];
        let second_tree = block.stats[1];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let first = typer.local_method_symbol_at(source, first_tree).unwrap();
        let second = typer.local_method_symbol_at(source, second_tree).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.complete_symbol(first),
            Err(TyperError::RecursiveInferredMethodResult { symbol }) if symbol == first
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(*typer.store().symbols.info(first), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(second), SymbolInfo::Missing);
        assert!(typer.inferred_method_results_in_progress.is_empty());
    }

    #[test]
    fn explicit_result_breaks_a_local_inference_cycle() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(include_str!(
                "../../tests/fixtures/local-method-results/explicit-breaks-inference-cycle.scala"
            ));
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        typer.type_expression(block_tree, context).unwrap();

        for method_tree in [
            match &parsed.ast.get(block_tree).kind {
                TreeKind::Block(block) => block.stats[0],
                _ => unreachable!(),
            },
            match &parsed.ast.get(block_tree).kind {
                TreeKind::Block(block) => block.stats[1],
                _ => unreachable!(),
            },
        ] {
            let method = typer.local_method_symbol_at(source, method_tree).unwrap();
            if !matches!(*typer.store().symbols.info(method), SymbolInfo::Complete(_)) {
                panic!("explicitly typed recursive method should be complete");
            }
        }
    }

    #[test]
    fn inferred_generic_local_method_rebinds_result_and_supports_forward_call() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { local(1); def local[A](value: A) = value; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[1];
        let (type_parameter_tree, result_tree) = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => (definition.type_params[0], definition.tpt),
            _ => panic!("local declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        typer.type_expression(block_tree, context).unwrap();

        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let SymbolInfo::Complete(signature) = *typer.store().symbols.info(method) else {
            panic!("inferred local method should be complete after its forward call");
        };
        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("generic local method should have a Poly signature");
        };
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("generic local method should retain its term clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.result),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        let typed_method = typer.source_typed_index().get(source, method_tree).unwrap();
        let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
            panic!("generic local method should produce a typed DefDef");
        };
        assert_eq!(
            typed_definition.tpt,
            typer.source_typed_index().get(source, result_tree).unwrap()
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_definition.tpt).ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert_eq!(poly.params.len(), 1);
        let type_parameter = typer
            .local_method_type_parameter_symbol_at(source, type_parameter_tree)
            .unwrap();
        assert_eq!(
            typer.store().symbols.get(type_parameter).owner,
            Some(method)
        );
    }

    #[test]
    fn inferred_curried_local_method_preserves_clause_results() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def local(first: Int)(last: Int) = last; local(1)(2) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        typer.type_expression(block_tree, context).unwrap();

        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let SymbolInfo::Complete(signature) = *typer.store().symbols.info(method) else {
            panic!("inferred local method should be complete");
        };
        let Type::Method(first_clause) = typer.store().types.get(signature) else {
            panic!("local method should retain its first clause");
        };
        let Type::Method(second_clause) = typer.store().types.get(first_clause.result) else {
            panic!("local method should retain its second clause");
        };
        assert_eq!(first_clause.params.len(), 1);
        assert_eq!(second_clause.params.len(), 1);
        assert_eq!(second_clause.result, definitions.int);
    }

    #[test]
    fn local_unsupported_parameter_modifiers_remain_deferred() {
        for (unsupported_modifier, expected_feature) in [
            (Modifier::Erased, "erased parameters"),
            (Modifier::Inline, "parameter modifiers"),
        ] {
            let (mut parsed, mut store, packages, definitions, index, source) = parse_and_name(
                "class C { def outer: Int = { def local(value: Int): Int = value; 0 } }",
            );
            let (outer, block_tree) =
                method_definition_and_rhs(&parsed, &store, &index, source, "outer");
            let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
                panic!("outer body should be a block")
            };
            let method_tree = block.stats[0];
            let parameter_tree = match &parsed.ast.get(method_tree).kind {
                TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
                _ => panic!("local declaration should be a DefDef"),
            };
            let TreeKind::ValDef(parameter) = &mut parsed.ast.get_mut(parameter_tree).kind else {
                panic!("parameter tree should be a ValDef")
            };
            parameter.metadata.modifiers.push(unsupported_modifier);

            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let context = typer.expression_context_for(outer).unwrap();
            preindex_block_for_test(&mut typer, block_tree, context);
            let method = typer.local_method_symbol_at(source, method_tree).unwrap();

            assert!(matches!(
                typer.complete_symbol(method),
                Err(TyperError::LocalMethodSignatureDeferred {
                    feature,
                    ..
                }) if feature == expected_feature
            ));
            assert!(
                typer
                    .local_method_parameter_symbol_at(source, parameter_tree)
                    .is_none()
            );
        }
    }

    #[test]
    fn local_parameter_scope_shadows_the_enclosing_member_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val item: Boolean = true; def outer: Int = { def local(item: Int): Int = item; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let local_tree = block.stats[0];
        let parameter_tree = match &parsed.ast.get(local_tree).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("local declaration should be a DefDef"),
        };
        let body_tree = match &parsed.ast.get(local_tree).kind {
            TreeKind::DefDef(definition) => definition.rhs.unwrap(),
            _ => unreachable!(),
        };
        let body_name = match &parsed.ast.get(body_tree).kind {
            TreeKind::Ident(ident) => ident.name,
            _ => panic!("local method body should reference its parameter"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let local = typer.local_method_symbol_at(source, local_tree).unwrap();
        typer.complete_symbol(local).unwrap();
        let method_context = typer.expression_context_for(local).unwrap();
        let parameter = typer
            .local_method_parameter_symbol_at(source, parameter_tree)
            .unwrap();

        assert_eq!(body_name, typer.store().symbols.get(parameter).name);
        assert_eq!(
            typer
                .store()
                .scopes
                .get(typer.local_method_scope(local).unwrap())
                .lookup(&body_name),
            Some(parameter)
        );
        assert_eq!(
            typer
                .expression_term_candidates(body_name, method_context, body_tree.index(), None)
                .unwrap(),
            vec![parameter]
        );
    }

    #[test]
    fn failed_local_signature_completion_rolls_back_parameters_and_types() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def broken[A](value: MissingType): A = value; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let method_tree = block.stats[0];
        let (type_parameter_tree, parameter_tree) = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => (
                definition.type_params[0],
                definition.value_param_clauses[0][0],
            ),
            _ => panic!("local declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let method_scope = typer.local_method_scope(method).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::TypeNameNotFound { .. })
        ));

        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert!(
            typer
                .store()
                .scopes
                .get(method_scope)
                .lookup_all(&match &parsed.ast.get(parameter_tree).kind {
                    TreeKind::ValDef(parameter) => *parameter.name.as_name(),
                    _ => unreachable!(),
                })
                .is_empty()
        );
        assert!(
            typer
                .store
                .scopes
                .get(method_scope)
                .lookup_all(&match &parsed.ast.get(type_parameter_tree).kind {
                    TreeKind::TypeDef(parameter) => *parameter.name.as_name(),
                    _ => unreachable!(),
                })
                .is_empty()
        );
        assert!(
            typer
                .local_method_parameter_symbol_at(source, parameter_tree)
                .is_none()
        );
    }

    #[test]
    fn explicit_local_method_body_emits_typed_definition_and_reuses_mapping() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer(x: Int): Int = { def inc(y: Int): Int = y; inc(x) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[0];
        let (parameter_tree, result_tree) = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => (definition.value_param_clauses[0][0], definition.tpt),
            _ => panic!("local declaration should be a DefDef"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        let typed_block = typer.type_expression(block_tree, context).unwrap();
        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let typed_method = typer.source_typed_index().get(source, method_tree).unwrap();
        let typed_parameter = typer
            .source_typed_index()
            .get(source, parameter_tree)
            .unwrap();
        let typed_result = typer.source_typed_index().get(source, result_tree).unwrap();
        let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
            panic!("local method should produce a typed DefDef");
        };
        let parameter_symbol = typer
            .local_method_parameter_symbol_at(source, parameter_tree)
            .unwrap();
        assert_eq!(
            typed_definition.value_param_clauses[0],
            vec![typed_parameter]
        );
        assert_eq!(typed_definition.tpt, typed_result);
        assert_eq!(
            typed_definition.rhs,
            Some(
                typer
                    .source_typed_index()
                    .get(
                        source,
                        match &parsed.ast.get(method_tree).kind {
                            TreeKind::DefDef(definition) => definition.rhs.unwrap(),
                            _ => unreachable!(),
                        }
                    )
                    .unwrap()
            )
        );
        assert_eq!(
            typer.typed_ast().get(typed_parameter).position,
            parsed.ast.get(parameter_tree).position
        );
        assert_eq!(typer.typed_ast().get(typed_parameter).ty, definitions.int);
        assert_eq!(typer.typed_ast().get(typed_result).ty, definitions.int);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_method).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == method
        ));
        assert_eq!(
            typer.store().symbols.get(parameter_symbol).owner,
            Some(method)
        );
        assert_eq!(
            typer.type_expression(block_tree, context).unwrap(),
            typed_block
        );
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed_method)
        );
    }

    #[test]
    fn generic_curried_local_method_reifies_all_definition_clauses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def keep[A](first: A)(using context: Int)(last: A): A = last; keep(1)(using 2)(3); keep[Int](4)(using 5)(6) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let method_tree = block.stats[0];
        let (type_parameter_tree, parameter_trees, result_tree) =
            match &parsed.ast.get(method_tree).kind {
                TreeKind::DefDef(definition) => (
                    definition.type_params[0],
                    definition.value_param_clauses.clone(),
                    definition.tpt,
                ),
                _ => panic!("local declaration should be a DefDef"),
            };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        typer.type_expression(block_tree, context).unwrap();

        let typed_method = typer.source_typed_index().get(source, method_tree).unwrap();
        let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
            panic!("generic local method should produce a typed DefDef");
        };
        assert_eq!(typed_definition.type_params.len(), 1);
        assert_eq!(typed_definition.value_param_clauses.len(), 3);
        assert!(
            typed_definition
                .value_param_clauses
                .iter()
                .all(|clause| clause.len() == 1)
        );
        assert_eq!(
            typed_definition.type_params[0],
            typer
                .source_typed_index()
                .get(source, type_parameter_tree)
                .unwrap()
        );
        assert_eq!(
            typed_definition.tpt,
            typer.source_typed_index().get(source, result_tree).unwrap()
        );
        for (typed_clause, source_clause) in typed_definition
            .value_param_clauses
            .iter()
            .zip(parameter_trees)
        {
            assert_eq!(
                typed_clause[0],
                typer
                    .source_typed_index()
                    .get(source, source_clause[0])
                    .unwrap()
            );
        }
        let last_parameter = typed_definition.value_param_clauses[2][0];
        let type_parameter = typer
            .local_method_type_parameter_symbol_at(source, type_parameter_tree)
            .unwrap();
        assert!(matches!(
            typer.typed_ast().get(last_parameter).ty,
            ty if matches!(typer.store().types.get(ty), Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == type_parameter)
        ));
    }

    #[test]
    fn local_generic_and_monomorphic_overloads_compete_through_shared_resolver() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def pick[A](value: A): Int = 1; def pick(value: Int): Int = 2; pick(1) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let (generic_tree, monomorphic_tree) = (block.stats[0], block.stats[1]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        preindex_block_for_test(&mut typer, block_tree, context);
        let generic = typer.local_method_symbol_at(source, generic_tree).unwrap();
        let monomorphic = typer
            .local_method_symbol_at(source, monomorphic_tree)
            .unwrap();
        let result = typer.type_expression(block_tree, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::AmbiguousOverloadApplication { candidates, .. })
                    if candidates.len() == 2
            ),
            "unexpected generic overload result: {result:?}"
        );

        assert_ne!(generic, monomorphic);
    }

    #[test]
    fn forward_local_monomorphic_overload_uses_argument_filtering() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { pick(1); def pick(value: Boolean): Int = 2; def pick(value: Int): Int = 1; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let call_tree = block.stats[0];
        let boolean_method_tree = block.stats[1];
        let integer_method_tree = block.stats[2];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        typer.type_expression(block_tree, context).unwrap();

        let integer_method = typer
            .local_method_symbol_at(source, integer_method_tree)
            .unwrap();
        let boolean_method = typer
            .local_method_symbol_at(source, boolean_method_tree)
            .unwrap();
        let typed_call = typer.source_typed_index().get(source, call_tree).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_call).kind else {
            panic!("forward local overload call should produce an Apply");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == integer_method
        ));
        assert_ne!(integer_method, boolean_method);
    }

    #[test]
    fn indistinguishable_local_overloads_remain_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def pick(value: Int): Int = 1; def pick(other: Int): Int = 2; pick(1) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        assert!(matches!(
            typer.type_expression(block_tree, context),
            Err(TyperError::AmbiguousOverloadApplication { .. })
        ));
    }

    #[test]
    fn losing_generic_local_overload_probe_rolls_back_speculative_state() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def pick[A](value: A, extra: Any): Int = 1; def pick(value: Int, extra: Int): Int = 2; pick(1, 1) } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let generic_tree = block.stats[0];
        let monomorphic_tree = block.stats[1];
        let call_tree = block.expr;
        let TreeKind::Apply(call) = &parsed.ast.get(call_tree).kind else {
            panic!("call should be an Apply");
        };
        let argument_trees = call.args.clone();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        let (_, block_context) = preindex_block_for_test(&mut typer, block_tree, context);
        let generic = typer.local_method_symbol_at(source, generic_tree).unwrap();
        let monomorphic = typer
            .local_method_symbol_at(source, monomorphic_tree)
            .unwrap();
        let generic_callable = typer.complete_symbol(generic).unwrap();
        let monomorphic_callable = typer.complete_symbol(monomorphic).unwrap();
        let arguments = argument_trees
            .into_iter()
            .map(|argument_tree| {
                let typed = typer.type_expression(argument_tree, block_context).unwrap();
                let own_type = typer.typed_ast().get(typed).ty;
                let widened_type = typer.widen_expression_type(own_type).unwrap();
                TypedArgument {
                    typed,
                    own_type,
                    widened_type,
                }
            })
            .collect::<Vec<_>>();
        let mut candidates = [
            ApplicationCandidate {
                symbol: generic,
                callable: generic_callable,
                member: None,
                rejection: None,
            },
            ApplicationCandidate {
                symbol: monomorphic,
                callable: monomorphic_callable,
                member: None,
                rejection: None,
            },
        ];
        let store_checkpoint = typer.store.checkpoint();
        let type_index_checkpoint = typer.type_index.checkpoint();
        let mut info_journal = Vec::new();

        let winner = typer
            .choose_method_overload_candidate(
                &mut candidates,
                &arguments,
                call_tree.index(),
                ApplyKind::Regular,
                &mut info_journal,
            )
            .unwrap();

        assert_eq!(winner.symbol, monomorphic);
        assert!(info_journal.is_empty());
        assert_eq!(typer.store.checkpoint(), store_checkpoint);
        for (tree, _) in parsed.ast.iter() {
            assert_eq!(
                typer.type_index.type_at(source, tree),
                type_index_checkpoint.type_at(source, tree)
            );
        }
    }

    #[test]
    fn nested_local_method_shadows_same_named_outer_method() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def pick[A](value: A): A = value; { def pick(value: Int): Int = value; pick(1) } } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(outer_block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block");
        };
        let outer_method_tree = outer_block.stats[0];
        let TreeKind::Block(inner_block) = &parsed.ast.get(outer_block.expr).kind else {
            panic!("nested block should be the outer expression");
        };
        let inner_method_tree = inner_block.stats[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        typer.type_expression(block_tree, context).unwrap();

        let outer_method = typer
            .local_method_symbol_at(source, outer_method_tree)
            .unwrap();
        let inner_method = typer
            .local_method_symbol_at(source, inner_method_tree)
            .unwrap();
        let typed_call = typer
            .source_typed_index()
            .get(source, inner_block.expr)
            .unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_call).kind else {
            panic!("nested call should produce an Apply");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == inner_method
        ));
        assert_ne!(outer_method, inner_method);
    }

    #[test]
    fn nested_non_generic_local_method_preserves_outer_type_parameter_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def top: Int = { def outer[A](value: A): A = { def inner(nested: A): A = nested; inner(value) }; outer(1) } }",
        );
        let (top, block_tree) = method_definition_and_rhs(&parsed, &store, &index, source, "top");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(top).unwrap();

        typer.type_expression(block_tree, context).unwrap();
    }

    #[test]
    fn typing_an_unused_local_method_publishes_its_signature() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { def unused(value: Int): Int = value; 0 } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let method_tree = match &parsed.ast.get(block_tree).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("outer body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        typer.type_expression(block_tree, context).unwrap();

        let method = typer.local_method_symbol_at(source, method_tree).unwrap();
        let SymbolInfo::Complete(signature) = *typer.store().symbols.info(method) else {
            panic!("typing the definition should publish its method signature");
        };
        assert!(matches!(
            typer.store().types.get(signature),
            Type::Method(_)
        ));
    }

    #[test]
    fn local_method_bodies_support_forward_calls_and_explicit_recursion() {
        for source_code in [
            "class C { def outer: Int = { val answer: Int = inc(1); def inc(n: Int): Int = n; answer } }",
            "class C { def outer: Int = { def loop(n: Int): Int = if true then loop(n) else n; loop(1) } }",
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_code);
            let (outer, block_tree) =
                method_definition_and_rhs(&parsed, &store, &index, source, "outer");
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let context = typer.expression_context_for(outer).unwrap();

            typer.type_expression(block_tree, context).unwrap();

            let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
                unreachable!()
            };
            for method_tree in block
                .stats
                .iter()
                .filter(|tree| matches!(parsed.ast.get(**tree).kind, TreeKind::DefDef(_)))
            {
                let typed = typer
                    .source_typed_index()
                    .get(source, *method_tree)
                    .expect("local method should have a typed replacement");
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
            }
        }
    }

    #[test]
    fn local_method_body_captures_outer_local_and_respects_nested_shadowing() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def outer: Int = { val base: Int = 1; def captured(): Int = base; def nested(): Int = { val base: Int = 2; base }; captured() } }",
        );
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();

        typer.type_expression(block_tree, context).unwrap();

        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            unreachable!()
        };
        let outer_base = typer.local_symbol_at(source, block.stats[0]).unwrap();
        assert_eq!(
            block
                .stats
                .iter()
                .filter(|tree| matches!(parsed.ast.get(**tree).kind, TreeKind::DefDef(_)))
                .count(),
            2
        );
        for method_tree in block
            .stats
            .iter()
            .filter(|tree| matches!(parsed.ast.get(**tree).kind, TreeKind::DefDef(_)))
        {
            assert!(
                typer
                    .source_typed_index()
                    .get(source, *method_tree)
                    .is_some()
            );
        }
        let method_bodies = block
            .stats
            .iter()
            .copied()
            .filter_map(|tree| match &parsed.ast.get(tree).kind {
                TreeKind::DefDef(definition) => Some(definition.rhs.unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let captured_rhs = typer
            .source_typed_index()
            .get(source, method_bodies[0])
            .unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(captured_rhs).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == outer_base
        ));
        let nested_source_rhs = method_bodies[1];
        let TreeKind::Block(nested_source) = &parsed.ast.get(nested_source_rhs).kind else {
            unreachable!()
        };
        let nested_base = typer
            .local_symbol_at(source, nested_source.stats[0])
            .unwrap();
        let typed_nested_rhs = typer
            .source_typed_index()
            .get(source, nested_source_rhs)
            .unwrap();
        let TreeKind::Block(nested_typed) = &typer.typed_ast().get(typed_nested_rhs).kind else {
            panic!("nested local method should retain its typed block body");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(nested_typed.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == nested_base
        ));
        assert_ne!(outer_base, nested_base);
    }

    #[test]
    fn local_method_body_mismatch_rolls_back_signature_and_typed_mappings() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def outer: Int = { def invalid(): Boolean = 1; 0 } }");
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let method_tree = match &parsed.ast.get(block_tree).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("outer body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        let store_checkpoint = typer.store().checkpoint();
        let typed_checkpoint = typer.typed_ast().checkpoint();
        let scope_checkpoint = typer.expression_scopes.len();

        assert!(matches!(
            typer.type_expression(block_tree, context),
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));

        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
        assert_eq!(typer.expression_scopes.len(), scope_checkpoint);
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.local_method_symbol_at(source, method_tree).is_none());
        assert!(typer.local_methods.definitions.is_empty());
    }

    #[test]
    fn failed_block_typing_rolls_back_preentered_local_methods_and_scopes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def outer: Int = { def foo(): Int = 2; foo(); missing } }");
        let (outer, block_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "outer");
        let TreeKind::Block(block) = &parsed.ast.get(block_tree).kind else {
            panic!("outer body should be a block")
        };
        let local_method_tree = block.stats[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(outer).unwrap();
        let store_checkpoint = typer.store().checkpoint();
        let typed_checkpoint = typer.typed_ast().checkpoint();
        let scope_checkpoint = typer.expression_scopes.len();

        let error = typer.type_expression(block_tree, context).unwrap_err();

        assert!(matches!(error, TyperError::TermNameNotFound { .. }));
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
        assert_eq!(typer.expression_scopes.len(), scope_checkpoint);
        assert!(
            typer
                .local_method_symbol_at(source, local_method_tree)
                .is_none()
        );
        assert!(typer.source_typed_index().get(source, block_tree).is_none());
        assert!(typer.local_methods.definitions.is_empty());
    }

    #[test]
    fn inferred_local_value_uses_widened_initializer_type_and_reifies_its_type_tree() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use(x: Int): Int = { val local = x; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block_id = typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        assert_eq!(
            typer.store().symbols.get(local).info,
            SymbolInfo::Complete(definitions.int)
        );

        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_block_id).kind else {
            panic!("source block should produce a typed block");
        };
        let TreeKind::ValDef(typed_local) = &typer.typed_ast().get(typed_block.stats[0]).kind
        else {
            panic!("inferred local should remain a typed ValDef");
        };
        assert_eq!(typer.typed_ast().get(typed_local.tpt).ty, definitions.int);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_local.rhs.unwrap()).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if typer.store().symbols.get(*symbol).kind == SymbolKind::Parameter
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local
        ));
        let source_tpt = match &parsed.ast.get(local_tree).kind {
            TreeKind::ValDef(definition) => definition.tpt,
            _ => unreachable!(),
        };
        assert_eq!(
            typer.source_typed_index().get(source, source_tpt),
            Some(typed_local.tpt)
        );
        assert_eq!(
            typer.typed_ast().get(typed_local.tpt).position,
            parsed.ast.get(source_tpt).position
        );
    }

    #[test]
    fn inferred_local_var_widens_literal_type_and_keeps_constant_rhs() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { var local = 1; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block_id = typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        let info = typer.store().symbols.get(local);
        assert!(info.flags.contains(SymbolFlags::MUTABLE));
        assert_eq!(info.info, SymbolInfo::Complete(definitions.int));

        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_block_id).kind else {
            panic!("source block should produce a typed block");
        };
        let TreeKind::ValDef(typed_local) = &typer.typed_ast().get(typed_block.stats[0]).kind
        else {
            panic!("inferred local should remain a typed ValDef");
        };
        assert_eq!(typer.typed_ast().get(typed_local.tpt).ty, definitions.int);
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_local.rhs.unwrap()).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
    }

    #[test]
    fn inferred_val_literal_matches_scala_39_typed_tree_oracle() {
        let source_text =
            include_str!("../../tests/fixtures/local-value-inference/LocalValueInference.scala");
        let oracle = include_str!(
            "../../tests/fixtures/local-value-inference/LocalValueInference.typed-tree.txt"
        );
        assert!(oracle.contains("val n: Int = 1"));
        dotty_tasty::tasty::TastyFile::parse_scala_3_9(include_bytes!(
            "../../tests/fixtures/local-value-inference/LocalValueInference.tasty"
        ))
        .expect("Scala 3.9.0 oracle TASTy should be structurally readable");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block_id = typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        assert_eq!(
            typer.store().symbols.get(local).info,
            SymbolInfo::Complete(definitions.int)
        );
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_block_id).kind else {
            panic!("source block should produce a typed block");
        };
        let TreeKind::ValDef(typed_local) = &typer.typed_ast().get(typed_block.stats[0]).kind
        else {
            panic!("inferred local should remain a typed ValDef");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_local.rhs.unwrap()).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
    }

    #[test]
    fn inferred_local_uses_typed_application_result() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def id(x: Int): Int = x; def use: Int = { val local = id(1); local } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block_id = typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        assert_eq!(
            typer.store().symbols.get(local).info,
            SymbolInfo::Complete(definitions.int)
        );
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_block_id).kind else {
            panic!("source block should produce a typed block");
        };
        let TreeKind::ValDef(typed_local) = &typer.typed_ast().get(typed_block.stats[0]).kind
        else {
            panic!("inferred local should remain a typed ValDef");
        };
        assert!(matches!(
            typer.typed_ast().get(typed_local.rhs.unwrap()).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(
            typer.typed_ast().get(typed_local.rhs.unwrap()).ty,
            definitions.int
        );
    }

    #[test]
    fn inferred_local_rejects_methodic_initializer_types() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def id(x: Int): Int = x; def use: Int = { val local = id; 1 } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let store_checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::InvalidInferredLocalValueType { .. })
        ));
        assert!(typer.local_symbol_at(source, local_tree).is_none());
        assert!(typer.source_typed_index().is_empty());
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
    }

    #[test]
    fn inferred_local_rejects_standalone_recursive_this_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1 }");
        let typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let recursive_binder = typer.store.types.reserve();
        let binder = recursive_binder.id();
        let recursive_this = typer.store.types.alloc(Type::RecThis { binder });
        typer.store.types.fill(
            recursive_binder,
            Type::Recursive {
                parent: recursive_this,
            },
        );
        let standalone_recursive_this = typer.store.types.alloc(Type::RecThis { binder });

        assert!(matches!(
            typer.validate_inferred_local_value_type(standalone_recursive_this, 0),
            Err(TyperError::InvalidInferredLocalValueType { inferred, .. })
                if inferred == standalone_recursive_this
        ));
        assert!(matches!(
            typer.store.types.get(binder),
            Type::Recursive { parent } if *parent == recursive_this
        ));
    }

    #[test]
    fn inferred_local_rejects_polymorphic_initializer_types() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def id[A](x: A): A = x; def use: Int = { val local = id; 1 } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::InvalidInferredLocalValueType { .. })
        ));
        assert!(typer.local_symbol_at(source, local_tree).is_none());
        assert!(typer.source_typed_index().is_empty());
    }

    #[test]
    fn inferred_local_can_reference_a_method_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use[A](x: A): A = { val local = x; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        let SymbolInfo::Complete(inferred) = typer.store().symbols.get(local).info else {
            panic!("inferred local should be complete");
        };
        assert!(
            matches!(
                typer.store().types.get(inferred),
                Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                    if typer.store().symbols.get(*symbol).kind == SymbolKind::TypeParameter
            ),
            "inferred type was {:?}",
            typer.store().types.get(inferred)
        );
    }

    #[test]
    fn inferred_local_shadows_outer_local_in_nested_block() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def use: Int = { val local = 1; { val local = 2; local } } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (outer_local_tree, inner_local_tree, inner_ref_tree) = {
            let TreeKind::Block(outer) = &parsed.ast.get(rhs).kind else {
                unreachable!()
            };
            let outer_local = outer.stats[0];
            let TreeKind::Block(inner) = &parsed.ast.get(outer.expr).kind else {
                unreachable!()
            };
            (outer_local, inner.stats[0], inner.expr)
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        typer.type_expression(rhs, context).unwrap();
        let outer_local = typer.local_symbol_at(source, outer_local_tree).unwrap();
        let inner_local = typer.local_symbol_at(source, inner_local_tree).unwrap();
        assert_ne!(outer_local, inner_local);
        let typed_inner_ref = typer
            .source_typed_index()
            .get(source, inner_ref_tree)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_inner_ref).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == inner_local
        ));
    }

    #[test]
    fn inferred_local_without_outer_self_name_is_unresolved() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local = local; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert!(typer.local_symbol_at(source, local_tree).is_none());
        assert!(typer.source_typed_index().is_empty());
    }

    #[test]
    fn inferred_local_uses_existing_generic_member_adaptation() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class String; class Box[A] { val value: A = ??? }; class C { def use(box: Box[String]): String = { val local = box.value; local } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        let actual = match typer.store().symbols.get(local).info {
            SymbolInfo::Complete(actual) => actual,
            _ => panic!("inferred local should be complete"),
        };
        let method_type = typer.complete_symbol(method).unwrap();
        let Type::Method(signature) = typer.store().types.get(method_type) else {
            panic!("use should have a method signature");
        };
        assert_eq!(
            typer.store().types.get(actual).reference_symbol(),
            typer.store().types.get(signature.result).reference_symbol(),
            "inferred {:?}, method result {:?}",
            typer.store().types.get(actual),
            typer.store().types.get(signature.result)
        );
    }

    #[test]
    fn inferred_local_initializer_resolves_outer_binding_before_new_binding() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val local: Int = 1; def use: Int = { val local = local; local } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        typer.type_expression(rhs, context).unwrap();
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        assert_eq!(
            typer.store().symbols.get(local).info,
            SymbolInfo::Complete(definitions.int)
        );
    }

    #[test]
    fn failed_inferred_local_rhs_rolls_back_binding_and_typed_nodes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local = 1; missing } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let store_checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert!(typer.local_symbol_at(source, local_tree).is_none());
        assert!(typer.source_typed_index().is_empty());
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
    }

    #[test]
    fn local_value_initializer_must_conform_to_its_explicit_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local: Int = true; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::LocalValueTypeMismatch { expected, .. })
                if expected == definitions.int
        ));
        assert!(typer.local_symbol_at(source, local_tree).is_none());
        assert!(typer.source_typed_index().is_empty());
    }

    #[test]
    fn local_value_without_an_initializer_is_rejected() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local: Int; 1 } }");
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::LocalValueRightHandSideMissing { source: actual, .. })
                if actual == source
        ));
        assert!(typer.local_symbols.is_empty());
    }

    #[test]
    fn local_values_shadow_outer_bindings_and_inner_blocks_restore_them() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val x: String = \"outer\"; def use(x: Boolean): Int = { val x: Int = 1; { val x: Int = 2; x }; x } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let block = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block,
            _ => unreachable!(),
        };
        let outer_local_tree = block.stats[0];
        let inner_tree = block.stats[1];
        let inner_local_tree = match &parsed.ast.get(inner_tree).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed_block = typer.type_expression(rhs, context).unwrap();

        let outer_local = typer.local_symbol_at(source, outer_local_tree).unwrap();
        let inner_local = typer.local_symbol_at(source, inner_local_tree).unwrap();
        assert_ne!(outer_local, inner_local);
        let TreeKind::Block(block) = &typer.typed_ast().get(typed_block).kind else {
            unreachable!();
        };
        let TreeKind::Block(inner_block) = &typer.typed_ast().get(block.stats[1]).kind else {
            unreachable!();
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(inner_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == inner_local
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == outer_local
        ));
        assert_ne!(parameter, outer_local);
    }

    #[test]
    fn generic_local_annotation_resolves_in_the_enclosing_method_context() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A] { def use(value: A): A = { val local: A = value; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
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

        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            unreachable!();
        };
        let local_symbol = typer.local_symbol_at(source, local_tree).unwrap();
        let reference_type = typer.typed_ast().get(block.expr).ty;
        let expected = typer.widen_expression_type(reference_type).unwrap();
        assert_eq!(
            typer.store().symbols.get(local_symbol).info,
            SymbolInfo::Complete(expected)
        );
    }

    #[test]
    fn failed_later_block_statement_rolls_back_local_symbols_and_scope_entries() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local: Int = 1; missing } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let checkpoint = typer.store().checkpoint();
        let scope_count = typer.expression_scopes.len();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.local_symbol_at(source, local_tree).is_none());
        assert_eq!(typer.expression_scopes.len(), scope_count);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
    }

    #[test]
    fn one_to_one_block_stat_expansions_preserve_order_and_are_idempotent() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def use: Int = { val first: Int = 1; val second: Int = 2; first } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_stats = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats.clone(),
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let first = typer.type_expression(rhs, context).unwrap();
        let typed_count = typer.typed_ast().iter().count();
        let second = typer.type_expression(rhs, context).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.typed_ast().iter().count(), typed_count);
        let TreeKind::Block(block) = &typer.typed_ast().get(first).kind else {
            unreachable!();
        };
        assert_eq!(block.stats.len(), source_stats.len());
        assert_eq!(
            block.stats,
            source_stats
                .iter()
                .map(|tree| typer.source_typed_index().get(source, *tree).unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn patdef_expansion_keeps_one_anchor_and_source_binder_provenance() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val (a, b) = pair; 0 } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(node.kind, TreeKind::PhaseSpecific(UntypedNode::PatDef(_))).then_some(tree)
            })
            .unwrap();
        let binder_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "a" => {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let unrelated_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.expr,
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let user_binder = typer.store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(typer.store.names.intern("a"), Namespace::Term),
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(typer.definitions.int),
            origin: SymbolOrigin::Source(source),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let temporary = typer.store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(typer.store.names.intern("$pat"), Namespace::Term),
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(typer.definitions.int),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let emitted = ["anchor", "binder-1", "binder-2"]
            .into_iter()
            .map(|name| {
                let name = Name::new(typer.store.names.intern(name), Namespace::Term);
                let ty = typer.store.types.alloc(Type::NoType);
                typer.typed_arena.alloc(Tree {
                    kind: TreeKind::Ident(Ident {
                        name,
                        backquoted: false,
                    }),
                    position: None,
                    ty,
                })
            })
            .collect::<Vec<_>>();
        let expansion = expression::blocks::TypedStatExpansion {
            anchor: emitted[0],
            emitted: emitted.clone(),
        };
        let mut block_stats = Vec::new();
        expansion.clone().append_to(&mut block_stats);
        assert_eq!(block_stats, emitted);

        typer
            .run_expression_transaction(|typer, _, new_mappings| {
                typer
                    .local_symbols
                    .insert((source, binder_tree), user_binder);
                typer.record_patdef_expansion(
                    source_patdef,
                    context,
                    expansion.clone(),
                    vec![temporary],
                    new_mappings,
                )?;
                Ok(())
            })
            .unwrap();

        assert_eq!(typer.source_typed_index().len(), 1);
        assert_eq!(
            typer.source_typed_index().get(source, source_patdef),
            Some(emitted[0])
        );
        assert!(
            emitted
                .iter()
                .all(|tree| typer.typed_ast().try_get(*tree).is_some())
        );
        assert_eq!(
            typer.local_symbol_at(source, binder_tree),
            Some(user_binder)
        );
        assert_eq!(typer.local_symbol_at(source, source_patdef), None);
        assert_eq!(typer.local_symbol_at(source, unrelated_tree), None);
        let recorded = typer.patdef_expansion_at(source, source_patdef).unwrap();
        assert_eq!(recorded.anchor, emitted[0]);
        assert_eq!(recorded.emitted, emitted);
        assert_eq!(recorded.synthetic_symbols, vec![temporary]);

        let new_mapping_count = typer
            .run_expression_transaction(|typer, _, new_mappings| {
                typer.record_patdef_expansion(
                    source_patdef,
                    context,
                    expansion,
                    vec![temporary],
                    new_mappings,
                )?;
                Ok(new_mappings.len())
            })
            .unwrap();
        assert_eq!(new_mapping_count, 0);
        assert_eq!(typer.source_typed_index().len(), 1);
    }

    #[test]
    fn failed_patdef_expansion_rolls_back_anchor_symbols_and_mappings() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val (a, b) = pair; 0 } }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(node.kind, TreeKind::PhaseSpecific(UntypedNode::PatDef(_))).then_some(tree)
            })
            .unwrap();
        let binder_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "a" => {
                    Some(tree)
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
        let context = typer.expression_context_for(method).unwrap();
        let store_checkpoint = typer.store.checkpoint();
        let scope_count = typer.expression_scopes.len();

        let result: Result<(), TyperError> =
            typer.run_expression_transaction(|typer, _, new_mappings| {
                let scope = context.local_scopes.unwrap();
                typer
                    .expression_scopes
                    .push(typer.expression_scopes[scope.index()].clone());
                let ty = typer.store.types.alloc(Type::NoType);
                let anchor = typer.typed_arena.alloc(Tree {
                    kind: TreeKind::Ident(Ident {
                        name: Name::new(typer.store.names.intern("testAnchor"), Namespace::Term),
                        backquoted: false,
                    }),
                    position: None,
                    ty,
                });
                let user_binder = typer.store.symbols.alloc(dotty_core::Symbol {
                    name: Name::new(typer.store.names.intern("a"), Namespace::Term),
                    owner: Some(context.owner),
                    kind: SymbolKind::Local,
                    flags: SymbolFlags::EMPTY,
                    visibility: Visibility::Public,
                    info: SymbolInfo::Complete(typer.definitions.int),
                    origin: SymbolOrigin::Source(source),
                    annotations: Vec::new(),
                    position: None,
                    links: SymbolLinks::default(),
                });
                let temporary = typer.store.symbols.alloc(dotty_core::Symbol {
                    name: Name::new(typer.store.names.intern("$pat"), Namespace::Term),
                    owner: Some(context.owner),
                    kind: SymbolKind::Local,
                    flags: SymbolFlags::EMPTY,
                    visibility: Visibility::Public,
                    info: SymbolInfo::Complete(typer.definitions.int),
                    origin: SymbolOrigin::Synthetic,
                    annotations: Vec::new(),
                    position: None,
                    links: SymbolLinks::default(),
                });
                typer
                    .local_symbols
                    .insert((source, binder_tree), user_binder);
                typer.record_patdef_expansion(
                    source_patdef,
                    context,
                    expression::blocks::TypedStatExpansion {
                        anchor,
                        emitted: vec![anchor],
                    },
                    vec![temporary],
                    new_mappings,
                )?;
                Err(TyperError::PatDefExpansionConflict {
                    source,
                    tree_index: source_patdef.index(),
                })
            });

        assert!(matches!(
            result,
            Err(TyperError::PatDefExpansionConflict { .. })
        ));
        assert_eq!(typer.store.checkpoint(), store_checkpoint);
        assert_eq!(typer.expression_scopes.len(), scope_count);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.patdef_expansion_at(source, source_patdef).is_none());
        assert!(typer.local_symbol_at(source, binder_tree).is_none());
    }

    #[test]
    fn single_binding_local_patdef_extracts_once_and_enters_only_the_final_binder() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(value: Any): Int = { val Extractor(x) = value; x } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let block = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block,
            _ => panic!("method body should be a block"),
        };
        let source_patdef = block.stats[0];
        let binder_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "x" => {
                    Some(tree)
                }
                _ => None,
            })
            .expect("pattern should have a source binder");
        let source_rhs = match &parsed.ast.get(source_patdef).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
                definition.rhs.expect("PatDef should retain its RHS")
            }
            _ => panic!("first block statement should be a PatDef"),
        };
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
        let TreeKind::Block(typed_block) = &typer.typed_arena.get(typed).kind else {
            panic!("method body should type to a block");
        };
        assert_eq!(typed_block.stats.len(), 1);
        let final_val = typed_block.stats[0];
        let TreeKind::ValDef(final_val_node) = &typer.typed_arena.get(final_val).kind else {
            panic!("PatDef expansion should emit one final ValDef");
        };
        assert_eq!(typer.typed_arena.get(final_val).ty, definitions.int);
        let typed_match = final_val_node
            .rhs
            .expect("final ValDef should have its extraction RHS");
        let TreeKind::Match(synthetic_match) = &typer.typed_arena.get(typed_match).kind else {
            panic!("PatDef RHS should retain the refutable synthetic Match");
        };
        assert_eq!(
            synthetic_match.selector,
            typer.source_typed_index().get(source, source_rhs).unwrap(),
            "the synthetic Match must reuse the one typed source RHS"
        );
        let TreeKind::CaseDef(case_def) = &typer.typed_arena.get(synthetic_match.cases[0]).kind
        else {
            panic!("synthetic Match should contain a single case");
        };
        let TreeKind::UnApply(_) = &typer.typed_arena.get(case_def.pattern).kind else {
            panic!("source extractor pattern should use the shared pattern typer");
        };
        assert_eq!(
            typer.typed_index.get(source, source_patdef),
            Some(final_val)
        );
        assert!(typer.typed_index.get(source, source_rhs).is_some());
        let temporary = typer
            .pattern_bindings
            .by_tree
            .get(&(source, binder_tree))
            .copied()
            .expect("pattern checker should record its temporary binder");
        let final_symbol = typer.local_symbol_at(source, binder_tree).unwrap();
        assert_ne!(temporary, final_symbol);
        assert_eq!(typer.store.symbols.get(temporary).kind, SymbolKind::Local);
        assert_eq!(
            typer.store.symbols.get(final_symbol).kind,
            SymbolKind::Local
        );
        assert!(matches!(
            typer.store.symbols.get(final_symbol).info,
            SymbolInfo::Complete(ty) if ty == definitions.int
        ));
        assert!(matches!(
            typer.store.types.get(typer.typed_arena.get(typed_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if symbol == &final_symbol
        ));
        let typed_binder = typer.typed_index.get(source, binder_tree).unwrap();
        assert!(matches!(
            typer.typed_arena.get(typed_binder).kind,
            TreeKind::Bind(_)
        ));
        let prior_local_symbol_count = typer.local_symbols.len();
        let prior_pattern_binding_count = typer.pattern_bindings.by_tree.len();
        let first_stats = typed_block.stats.clone();
        let repeated = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Block(repeated_block) = &typer.typed_arena.get(repeated).kind else {
            panic!("repeated method body should type to a block");
        };
        assert_eq!(repeated_block.stats, first_stats);
        assert_eq!(typer.local_symbols.len(), prior_local_symbol_count);
        assert_eq!(
            typer.pattern_bindings.by_tree.len(),
            prior_pattern_binding_count
        );
        assert_eq!(
            typer.local_symbol_at(source, binder_tree),
            Some(final_symbol)
        );
    }

    #[test]
    fn multi_binding_tuple_patdef_extracts_once_and_emits_final_locals_in_order() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class MaybeTuple2[A, B](val value: Tuple2[A, B]) { def isEmpty: Boolean = false; def get: Tuple2[A, B] = value }; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): MaybeTuple2[A, B] = new MaybeTuple2(value); def apply[A, B](first: A, second: B): Tuple2[A, B] = new Tuple2(first, second) } }; package app { class C { def use(value: scala.Tuple2[Int, Boolean]): Boolean = { val (first, second) = value; second } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("method body should be a block");
        };
        let source_patdef = source_block.stats[0];
        let (source_rhs, first_tree, second_tree) = match &parsed.ast.get(source_patdef).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
                let TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) =
                    &parsed.ast.get(definition.patterns[0]).kind
                else {
                    panic!("PatDef pattern should be a tuple");
                };
                (
                    definition.rhs.unwrap(),
                    tuple.elements[0],
                    tuple.elements[1],
                )
            }
            _ => panic!("first block statement should be a PatDef"),
        };
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
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should type to a block");
        };
        assert_eq!(block.stats.len(), 3);
        let aggregate_anchor = block.stats[0];
        let TreeKind::ValDef(aggregate) = &typer.typed_ast().get(aggregate_anchor).kind else {
            panic!("PatDef anchor should be its synthetic aggregate ValDef");
        };
        assert_eq!(
            typer.source_typed_index().get(source, source_patdef),
            Some(aggregate_anchor)
        );
        let aggregate_match = aggregate.rhs.unwrap();
        let TreeKind::Match(matched) = &typer.typed_ast().get(aggregate_match).kind else {
            panic!("aggregate ValDef RHS should be the synthetic Match");
        };
        let aggregate_type = typer.typed_ast().get(aggregate_match).ty;
        assert_eq!(
            matched.selector,
            typer.source_typed_index().get(source, source_rhs).unwrap()
        );
        let Type::Applied { tycon, args } = typer.store.types.get(aggregate_type) else {
            panic!("aggregate should have a canonical applied TupleN type");
        };
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(tuple_class),
            ..
        } = typer.store.types.get(*tycon)
        else {
            panic!("aggregate tuple should refer to its canonical class");
        };
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(*tuple_class).name.text()),
            "Tuple2"
        );
        assert_eq!(args, &[definitions.int, definitions.boolean]);
        assert_eq!(typer.typed_ast().get(aggregate_anchor).ty, aggregate_type);
        let TreeKind::CaseDef(case) = &typer.typed_ast().get(matched.cases[0]).kind else {
            panic!("synthetic Match should contain one case");
        };
        assert_eq!(typer.typed_ast().get(case.body).ty, aggregate_type);
        assert!(matches!(
            typer.typed_ast().get(case.body).kind,
            TreeKind::Apply(_)
        ));
        let TreeKind::Apply(aggregate_apply) = &typer.typed_ast().get(case.body).kind else {
            unreachable!();
        };
        let TreeKind::TypeApply(type_apply) = &typer.typed_ast().get(aggregate_apply.function).kind
        else {
            panic!("canonical TupleN.apply should retain explicit inferred type arguments");
        };
        let TreeKind::Select(apply_selection) = &typer.typed_ast().get(type_apply.function).kind
        else {
            panic!("tuple constructor should be selected from its companion");
        };
        assert_eq!(
            typer.store.names.resolve(apply_selection.name.text()),
            "apply"
        );
        assert_eq!(type_apply.args.len(), 2);
        let final_symbols =
            [first_tree, second_tree].map(|binder| typer.local_symbol_at(source, binder).unwrap());
        let temporary_symbols = [first_tree, second_tree]
            .map(|binder| typer.pattern_bindings.by_tree[&(source, binder)]);
        assert_ne!(final_symbols[0], temporary_symbols[0]);
        assert_ne!(final_symbols[1], temporary_symbols[1]);
        assert_ne!(final_symbols[0], final_symbols[1]);
        assert_eq!(
            typer.store.symbols.get(final_symbols[0]).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert_eq!(
            typer.store.symbols.get(final_symbols[1]).info,
            SymbolInfo::Complete(definitions.boolean)
        );
        assert_eq!(
            typer
                .patdef_expansion_at(source, source_patdef)
                .unwrap()
                .emitted,
            block.stats
        );
        for (component_index, final_stat) in block.stats[1..].iter().copied().enumerate() {
            let TreeKind::ValDef(value) = &typer.typed_ast().get(final_stat).kind else {
                panic!("each extracted binder should be a ValDef");
            };
            let TreeKind::Select(selection) = &typer.typed_ast().get(value.rhs.unwrap()).kind
            else {
                panic!("each final binder should select its aggregate tuple component");
            };
            let Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(selector),
            } = typer
                .store
                .types
                .get(typer.typed_ast().get(value.rhs.unwrap()).ty)
            else {
                panic!("tuple component selection should retain its canonical member");
            };
            assert_eq!(
                *prefix,
                typer.typed_ast().get(selection.qualifier).ty,
                "tuple component type should preserve the aggregate local reference as its prefix"
            );
            assert_eq!(
                typer.store.names.resolve(selection.name.text()),
                if component_index == 0 { "_1" } else { "_2" }
            );
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(*selector).name.text()),
                if component_index == 0 { "_1" } else { "_2" }
            );
        }
        let first_pattern = typer.source_typed_index().get(source, first_tree).unwrap();
        let second_pattern = typer.source_typed_index().get(source, second_tree).unwrap();
        assert!(matches!(
            typer.typed_ast().get(first_pattern).kind,
            TreeKind::Bind(_)
        ));
        assert!(matches!(
            typer.typed_ast().get(second_pattern).kind,
            TreeKind::Bind(_)
        ));
        assert_eq!(
            typer.store.symbols.get(final_symbols[0]).owner,
            Some(context.owner)
        );
        assert_eq!(
            typer.store.symbols.get(final_symbols[1]).owner,
            Some(context.owner)
        );
        assert!(
            typer
                .patdef_expansion_at(source, source_patdef)
                .unwrap()
                .synthetic_symbols
                .iter()
                .any(|symbol| {
                    let synthetic = typer.store.symbols.get(*symbol);
                    synthetic.origin == SymbolOrigin::Synthetic
                        && synthetic.flags.contains(SymbolFlags::SYNTHETIC)
                        && synthetic.kind == SymbolKind::Local
                        && !typer.local_symbols.values().any(|local| local == symbol)
                })
        );
        assert!(matches!(
            typer
                .store
                .types
                .get(typer.typed_ast().get(block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if symbol == &final_symbols[1]
        ));
        let prior_local_symbols = typer.local_symbols.len();
        let prior_pattern_bindings = typer.pattern_bindings.by_tree.len();
        let first_stats = block.stats.clone();
        let repeated = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Block(repeated_block) = &typer.typed_ast().get(repeated).kind else {
            panic!("repeated method body should type to a block");
        };
        assert_eq!(repeated_block.stats, first_stats);
        assert_eq!(typer.local_symbols.len(), prior_local_symbols);
        assert_eq!(typer.pattern_bindings.by_tree.len(), prior_pattern_bindings);
    }

    #[test]
    fn mutable_multi_binding_patdef_enters_assignable_final_locals() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class MaybeTuple2[A, B](val value: Tuple2[A, B]) { def isEmpty: Boolean = false; def get: Tuple2[A, B] = value }; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): MaybeTuple2[A, B] = new MaybeTuple2(value); def apply[A, B](first: A, second: B): Tuple2[A, B] = new Tuple2(first, second) } }; class C { def use(value: scala.Tuple2[Int, Boolean], next: Int): Boolean = { var (first, second) = value; first = next; second } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (source_patdef, binders) = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => {
                let patdef = block.stats[0];
                let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) =
                    &parsed.ast.get(patdef).kind
                else {
                    panic!("first statement should be a PatDef");
                };
                let TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) =
                    &parsed.ast.get(definition.patterns[0]).kind
                else {
                    panic!("PatDef should destructure a tuple");
                };
                (patdef, tuple.elements.clone())
            }
            _ => panic!("method body should be a block"),
        };
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
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should remain a block");
        };
        let final_symbols = binders
            .iter()
            .map(|binder| typer.local_symbol_at(source, *binder).unwrap())
            .collect::<Vec<_>>();
        let temporary_symbols = binders
            .iter()
            .map(|binder| typer.pattern_bindings.by_tree[&(source, *binder)])
            .collect::<Vec<_>>();
        assert_eq!(final_symbols.len(), 2);
        for (symbol, temporary) in final_symbols.iter().zip(&temporary_symbols) {
            assert!(
                typer
                    .store
                    .symbols
                    .get(*symbol)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
            );
            assert!(
                !typer
                    .store
                    .symbols
                    .get(*temporary)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
            );
            assert_ne!(symbol, temporary);
        }
        assert_eq!(
            typer.store.symbols.get(final_symbols[0]).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert_eq!(
            typer.store.symbols.get(final_symbols[1]).info,
            SymbolInfo::Complete(definitions.boolean)
        );
        assert_eq!(
            block.stats.len(),
            4,
            "aggregate and both binders precede assignment"
        );
        let TreeKind::Assign(assignment) = &typer.typed_ast().get(block.stats[3]).kind else {
            panic!("assignment should follow all generated PatDef locals");
        };
        assert!(matches!(
            typer.store.types.get(typer.typed_ast().get(assignment.lhs).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if symbol == &final_symbols[0]
        ));
        assert_eq!(
            typer.source_typed_index().get(source, source_patdef),
            Some(block.stats[0])
        );
    }

    #[test]
    fn mutable_single_binding_extractor_patdef_supports_assignment() {
        let source_text = "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Int = { var Extractor(value) = input; value = 2; value } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
        let patterns = match &parsed.ast.get(source_patdef).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => &definition.patterns,
            _ => panic!("first statement should be a PatDef"),
        };
        let pattern_span = parsed.ast.get(patterns[0]).position.unwrap().span().range();
        let binder = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::Ident(ident) = &node.kind else {
                    return None;
                };
                let position = node.position?.span().range();
                (store.names.resolve(ident.name.text()) == "value"
                    && pattern_span.start() <= position.start()
                    && position.end() <= pattern_span.end())
                .then_some(tree)
            })
            .expect("source pattern should retain its binder tree");
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

        let symbol = typer.local_symbol_at(source, binder).unwrap();
        assert!(
            typer
                .store
                .symbols
                .get(symbol)
                .flags
                .contains(SymbolFlags::MUTABLE)
        );
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should remain a block");
        };
        let TreeKind::Assign(assignment) = &typer.typed_ast().get(block.stats[1]).kind else {
            panic!("assignment should follow the extracted local");
        };
        assert!(matches!(
            typer.store.types.get(typer.typed_ast().get(assignment.lhs).ty),
            Type::TermRef { target: TermRefTarget::Symbol(target), .. } if target == &symbol
        ));
    }

    #[test]
    fn zero_binding_mutable_patdef_keeps_the_effect_check_without_symbols() {
        let source_text = "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Int = { var Extractor(_) = input; 1 } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
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
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should remain a block");
        };
        assert!(matches!(
            typer.typed_ast().get(block.stats[0]).kind,
            TreeKind::Match(_)
        ));
        assert!(typer.patdef_expansion_at(source, source_patdef).is_some());
        assert!(typer.local_symbols.is_empty());
    }

    #[test]
    fn lazy_and_explicit_type_patdefs_have_focused_deferrals() {
        for (source_text, expected_kind) in [
            (
                "class C { def use(value: (Int, Int)): Int = { lazy val (left, right) = value; left } }",
                "lazy",
            ),
            (
                "class C { def use(value: (Int, Int)): Int = { val (left, right): (Int, Int) = value; left } }",
                "explicit type",
            ),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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
            assert!(matches!(
                typer.type_expression(rhs, context),
                Err(TyperError::LocalPatDefDeferred { kind, .. }) if kind == expected_kind
            ));
        }
    }

    #[test]
    fn unsupported_infix_patdef_keeps_the_pattern_specific_error() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def use(value: Int): Int = { val left + right = value; 1 } }",
        );
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::InfixPatternDeferred { .. })
        ));
    }

    #[test]
    fn multi_binding_patdef_rejects_duplicate_names_and_rolls_back() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class PairResult[A, B](val _1: A, val _2: B) extends Product; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): PairResult[A, B] = new PairResult(value._1, value._2) } }; class C { def use(value: scala.Tuple2[Int, Boolean]): Int = { val (same, same) = value; same } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let checkpoint = typer.store.checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::DuplicatePatternBinding { .. })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.patdef_expansion_at(source, source_patdef).is_none());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn nested_extractor_multi_binding_patdef_reuses_one_pattern_and_aggregate_match() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class MaybeTuple2[A, B](val value: Tuple2[A, B]) { def isEmpty: Boolean = false; def get: Tuple2[A, B] = value }; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): MaybeTuple2[A, B] = new MaybeTuple2(value); def apply[A, B](first: A, second: B): Tuple2[A, B] = new Tuple2(first, second) } }; package app { object Extractor { def unapply(value: scala.Tuple2[Int, Boolean]): scala.MaybeTuple2[Int, Boolean] = new scala.MaybeTuple2(value) }; class C { def use(value: scala.Tuple2[Int, Boolean]): Boolean = { val Extractor((first, second)) = value; second } } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
        let source_pattern = match &parsed.ast.get(source_patdef).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.patterns[0],
            _ => panic!("first block statement should be a PatDef"),
        };
        let mut pending = vec![source_pattern];
        let mut binder_trees = std::collections::HashMap::new();
        while let Some(tree) = pending.pop() {
            match &parsed.ast.get(tree).kind {
                TreeKind::Ident(ident) => {
                    let name = store.names.resolve(ident.name.text());
                    if name == "first" || name == "second" {
                        binder_trees.insert(name.to_owned(), tree);
                    }
                }
                TreeKind::Bind(binding) => {
                    let name = store.names.resolve(binding.name.text());
                    if name == "first" || name == "second" {
                        binder_trees.insert(name.to_owned(), tree);
                    }
                    pending.push(binding.body);
                }
                TreeKind::NamedArg(argument) => pending.push(argument.arg),
                TreeKind::Typed(typed) => pending.push(typed.expr),
                TreeKind::Apply(application) => {
                    pending.extend(application.args.iter().rev().copied());
                }
                TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
                TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                    pending.extend(tuple.elements.iter().rev().copied());
                }
                TreeKind::UnApply(unapply) => {
                    pending.extend(unapply.patterns.iter().rev().copied());
                }
                _ => {}
            }
        }
        let first_tree = binder_trees["first"];
        let second_tree = binder_trees["second"];
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
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should type to a block");
        };
        assert_eq!(block.stats.len(), 3);
        let patdef = typer.patdef_expansion_at(source, source_patdef).unwrap();
        assert_eq!(patdef.emitted, block.stats);
        let TreeKind::ValDef(aggregate) = &typer.typed_ast().get(patdef.anchor).kind else {
            panic!("nested PatDef should anchor at its aggregate ValDef");
        };
        let TreeKind::Match(matched) = &typer.typed_ast().get(aggregate.rhs.unwrap()).kind else {
            panic!("aggregate RHS should retain the pattern check");
        };
        let TreeKind::CaseDef(case) = &typer.typed_ast().get(matched.cases[0]).kind else {
            panic!("synthetic Match should contain one case");
        };
        let TreeKind::UnApply(_) = &typer.typed_ast().get(case.pattern).kind else {
            panic!("nested extractor pattern should reuse recursive pattern typing");
        };
        assert!(matches!(
            typer.typed_ast().get(case.body).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(
            typer
                .store
                .symbols
                .get(typer.local_symbol_at(source, first_tree).unwrap())
                .info,
            SymbolInfo::Complete(definitions.int)
        );
        assert_eq!(
            typer
                .store
                .symbols
                .get(typer.local_symbol_at(source, second_tree).unwrap())
                .info,
            SymbolInfo::Complete(definitions.boolean)
        );
    }

    #[test]
    fn multi_binding_patdef_rolls_back_when_a_later_tuple_selector_is_ambiguous() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product { def _2(index: Int): B = _2 }; class PairResult[A, B](val _1: A, val _2: B) extends Product; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): PairResult[A, B] = new PairResult(value._1, value._2); def apply[A, B](first: A, second: B): Tuple2[A, B] = new Tuple2(first, second) } }; class C { def use(value: scala.Tuple2[Int, Boolean]): Boolean = { val (first, second) = value; second } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let checkpoint = typer.store.checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::LocalBlockDeclarationDeferred {
                kind: "pattern definition aggregate tuple selector",
                ..
            })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.patdef_expansion_at(source, source_patdef).is_none());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn multi_binding_patdef_reports_the_supported_aggregate_arity_bound() {
        let names = (0..23)
            .map(|index| format!("binder{index}"))
            .collect::<Vec<_>>();
        let source_text = format!(
            "class C {{ def use(value: Any): Int = {{ val ({}) = value; 1 }} }}",
            names.join(", ")
        );
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(&source_text);
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::PatDefAggregateArityDeferred {
                arity: 23,
                max_supported: 22,
                ..
            })
        ));
    }

    #[test]
    fn zero_binding_extractor_patdef_emits_a_unit_match_and_no_local_symbols() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Int = { val Extractor(_) = input; 1 } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let block = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block,
            _ => panic!("method body should be a block"),
        };
        let source_patdef = block.stats[0];
        let source_rhs = match &parsed.ast.get(source_patdef).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.rhs.unwrap(),
            _ => panic!("first block statement should be a PatDef"),
        };
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
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should type to a block");
        };
        assert_eq!(typed_block.stats.len(), 1);
        let anchor = typed_block.stats[0];
        let TreeKind::Match(matched) = &typer.typed_ast().get(anchor).kind else {
            panic!("zero-binding PatDef should emit its synthetic Match directly");
        };
        assert_eq!(typer.typed_ast().get(anchor).ty, definitions.unit);
        assert_eq!(
            matched.selector,
            typer.source_typed_index().get(source, source_rhs).unwrap()
        );
        let TreeKind::CaseDef(case_def) = &typer.typed_ast().get(matched.cases[0]).kind else {
            panic!("synthetic Match should contain one case");
        };
        assert_eq!(typer.typed_ast().get(matched.cases[0]).ty, definitions.unit);
        assert!(matches!(
            typer.typed_ast().get(case_def.pattern).kind,
            TreeKind::UnApply(_)
        ));
        assert!(matches!(
            typer.typed_ast().get(case_def.body).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit
            })
        ));
        assert_eq!(typer.typed_ast().get(case_def.body).ty, definitions.unit);
        assert_eq!(
            typer.source_typed_index().get(source, source_patdef),
            Some(anchor)
        );
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());

        let first_stats = typed_block.stats.clone();
        let repeated = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Block(repeated_block) = &typer.typed_ast().get(repeated).kind else {
            panic!("repeated method body should type to a block");
        };
        assert_eq!(repeated_block.stats, first_stats);
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn zero_binding_tuple_patdef_preserves_pattern_check_and_later_block_statement() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class MaybeTuple2[A, B](val value: Tuple2[A, B]) { def isEmpty: Boolean = false; def get: Tuple2[A, B] = value }; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): MaybeTuple2[A, B] = new MaybeTuple2(value) } }; package app { class C { def use(value: scala.Tuple2[Int, Boolean]): Boolean = { val (_, _) = value; true } } }";
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
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should type to a block");
        };
        assert_eq!(block.stats.len(), 1);
        let TreeKind::Match(matched) = &typer.typed_ast().get(block.stats[0]).kind else {
            panic!("tuple PatDef should preserve a synthetic Match");
        };
        assert_eq!(typer.typed_ast().get(block.stats[0]).ty, definitions.unit);
        let TreeKind::CaseDef(case_def) = &typer.typed_ast().get(matched.cases[0]).kind else {
            panic!("synthetic Match should contain one case");
        };
        assert_eq!(typer.typed_ast().get(matched.cases[0]).ty, definitions.unit);
        assert!(matches!(
            typer.typed_ast().get(case_def.pattern).kind,
            TreeKind::UnApply(_)
        ));
        assert_eq!(typer.typed_ast().get(case_def.body).ty, definitions.unit);
        assert!(matches!(
            typer.store.types.get(typer.typed_ast().get(block.expr).ty),
            Type::Constant(dotty_core::Constant::Boolean(true))
        ));
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn zero_binding_patdef_failure_rolls_back_selector_pattern_and_expansion() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Int = { val Extractor(field = _) = input; 1 } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let checkpoint = typer.store.checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ExtractorPatternArgumentUnsupported {
                issue: ExtractorPatternArgumentIssue::Named,
                ..
            })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.patdef_expansion_at(source, source_patdef).is_none());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn local_patdef_rhs_resolves_outer_same_named_parameter_before_final_binding() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(value: Any): Int = { val Extractor(value) = value; value } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let block = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block,
            _ => panic!("method body should be a block"),
        };
        let source_patdef = block.stats[0];
        let source_rhs = match &parsed.ast.get(source_patdef).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.rhs.unwrap(),
            _ => panic!("first block statement should be a PatDef"),
        };
        let binder_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident)
                    if tree != source_rhs && store.names.resolve(ident.name.text()) == "value" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
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
        let typed_rhs = typer.source_typed_index().get(source, source_rhs).unwrap();
        assert!(matches!(
            typer.store.types.get(typer.typed_ast().get(typed_rhs).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
        let local = typer.local_symbol_at(source, binder_tree).unwrap();
        assert_ne!(local, parameter);
        assert!(matches!(
            typer.store.types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local
        ));
    }

    #[test]
    fn local_patdef_alias_over_extractor_binds_the_original_selector_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Any = { val x @ Extractor(_) = input; x } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let block = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block,
            _ => panic!("method body should be a block"),
        };
        let source_patdef = block.stats[0];
        let binder_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Bind(binding) if store.names.resolve(binding.name.text()) == "x" => {
                    Some(tree)
                }
                _ => None,
            })
            .expect("alias pattern should have a source Bind tree");
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
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should type to a block");
        };
        let final_val = typed_block.stats[0];
        assert_eq!(typer.typed_ast().get(final_val).ty, definitions.any_type);
        let final_symbol = typer.local_symbol_at(source, binder_tree).unwrap();
        assert!(matches!(
            typer.store.symbols.get(final_symbol).info,
            SymbolInfo::Complete(ty) if ty == definitions.any_type
        ));
        let patdef = typer.patdef_expansion_at(source, source_patdef).unwrap();
        let TreeKind::ValDef(final_val) = &typer.typed_ast().get(patdef.anchor).kind else {
            panic!("PatDef anchor should be its final ValDef");
        };
        let TreeKind::Match(matched) = &typer.typed_ast().get(final_val.rhs.unwrap()).kind else {
            panic!("alias extractor definition must remain refutable");
        };
        let TreeKind::CaseDef(case_def) = &typer.typed_ast().get(matched.cases[0]).kind else {
            panic!("synthetic Match should contain a single case");
        };
        let TreeKind::Bind(binding) = &typer.typed_ast().get(case_def.pattern).kind else {
            panic!("typed alias should stay a Bind pattern");
        };
        assert!(matches!(
            typer.typed_ast().get(binding.body).kind,
            TreeKind::UnApply(_)
        ));
    }

    #[test]
    fn unsupported_nested_patdef_pattern_rolls_back_all_pattern_and_local_state() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Int = { val Extractor(x | _) = input; 1 } }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let source_patdef = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => panic!("method body should be a block"),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let checkpoint = typer.store.checkpoint();

        let error = typer.type_expression(rhs, context).unwrap_err();
        assert!(matches!(
            error,
            TyperError::PatternBindingInAlternative { .. }
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.patdef_expansion_at(source, source_patdef).is_none());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn patdef_binder_inventory_reaches_named_arguments_for_focused_pattern_errors() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; class C { def use(input: Any): Int = { val Extractor(field = x) = input; x } }",
        );
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ExtractorPatternArgumentUnsupported {
                issue: ExtractorPatternArgumentIssue::Named,
                ..
            })
        ));
    }

    #[test]
    fn patdef_binder_inventory_ignores_wildcard_aliases() {
        for pattern in ["_ @ Extractor(_) ", "_ @ Extractor(x)"] {
            let source_text = format!(
                "class MaybeInt {{ def isEmpty: Boolean = false; def get: Int = 1 }}; object Extractor {{ def unapply(value: Any): MaybeInt = new MaybeInt }}; class C {{ def use(input: Any): Int = {{ val {pattern} = input; 1 }} }}"
            );
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(&source_text);
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
            let error = typer.type_expression(rhs, context).unwrap_err();

            assert!(matches!(
                error,
                TyperError::WildcardPatternBindingRejected { .. }
            ));
        }
    }

    #[test]
    fn typing_a_local_valdef_outside_a_block_does_not_mutate_the_method_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val local: Int = 1; local } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let local_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.stats[0],
            _ => unreachable!(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let method_scope = context.local_scopes.unwrap();
        let scope = typer.expression_scopes[method_scope.index()].scope;
        let name = match &parsed.ast.get(local_tree).kind {
            TreeKind::ValDef(definition) => *definition.name.as_name(),
            _ => unreachable!(),
        };
        let store_checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(local_tree, context),
            Err(TyperError::LocalValueOutsideBlock { .. })
        ));
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert!(typer.store().scopes.get(scope).lookup_all(&name).is_empty());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.source_typed_index().is_empty());
    }

    #[test]
    fn method_parameter_shadows_a_same_named_class_field() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val x: Int = 1; def shadow(x: Boolean): Boolean = x }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "shadow");
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
        let field = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "x" =>
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
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
        assert_ne!(parameter, field);
    }

    #[test]
    fn method_body_context_falls_through_to_class_members() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val field: Int = 1; def read(x: Int): Int = field }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "read");
        let field = val_symbol(&parsed, &store, &index, source, "field").0;
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
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == field
        ));
    }

    #[test]
    fn method_type_parameters_remain_outside_term_lookup() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def method[A](x: Int): Int = A }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
    }

    #[test]
    fn constructor_body_context_exposes_constructor_owned_parameters() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int)");
        let (constructor, parameter_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                let symbol = index.symbol_at(source, tree)?;
                (store.symbols.get(symbol).kind == SymbolKind::Constructor)
                    .then(|| (symbol, definition.value_param_clauses[0][0]))
            })
            .expect("primary constructor should have a source definition");
        let parameter = index
            .derived_symbol_at(constructor, source, parameter_tree)
            .unwrap();
        let name = store.symbols.get(parameter).name;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let context = typer.expression_context_for(constructor).unwrap();
        let candidates = typer
            .expression_term_candidates(name, context, parameter_tree.index(), None)
            .unwrap();
        assert_eq!(candidates, vec![parameter]);
    }

    #[test]
    fn method_body_context_rejects_a_scope_owned_by_another_symbol() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def method(x: Int): Int = x }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let scope = index.scope_of(method).unwrap();
        store.scopes.get_mut(scope).owner = Some(class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.expression_context_for(method),
            Err(TyperError::ExpressionMethodScopeOwnerMismatch {
                owner,
                scope: actual_scope,
                actual: Some(actual_owner),
            }) if owner == method && actual_scope == scope && actual_owner == class
        ));
    }

    #[test]
    fn method_body_context_reports_a_missing_indexed_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def method(x: Int): Int = x }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            SourceTyper::indexed_method_scope(method, None),
            Err(TyperError::ExpressionMethodScopeMissing { owner }) if owner == method
        ));
        assert!(typer.expression_context_for(method).is_ok());
    }

    #[test]
    fn method_body_context_reports_an_owner_without_source_context() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def method(x: Int): Int = x }");
        let method = symbol(&mut store, SymbolKind::Method, SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.expression_context_for(method),
            Err(TyperError::ExpressionOwnerDeclarationContextMissing { owner }) if owner == method
        ));
    }

    #[test]
    fn pushed_expression_scope_shadows_the_method_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val x: Int = 1; def method(x: Boolean): Boolean = x }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let field = val_symbol(&parsed, &store, &index, source, "x").0;
        let name = store.symbols.get(field).name;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let method_context = typer.expression_context_for(method).unwrap();
        let mut block_scope = dotty_core::Scope::new(Some(method));
        block_scope.enter(name, field);
        let block_scope = typer.store.scopes.alloc(block_scope);
        let block_context = typer.push_local_scope(method_context, block_scope).unwrap();

        let candidates = typer
            .expression_term_candidates(name, block_context, rhs.index(), None)
            .unwrap();
        assert_eq!(candidates, vec![field]);
    }

    #[test]
    fn expression_scope_contexts_are_bound_to_their_source_typer() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def first(x: Int): Int = x; def second(x: Boolean): Boolean = x }",
        );
        let (first, _) = method_definition_and_rhs(&parsed, &store, &index, source, "first");
        let (second, second_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "second");
        let first_context = {
            let mut first_typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            first_typer.expression_context_for(first).unwrap()
        };
        let mut second_typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let second_context = second_typer.expression_context_for(second).unwrap();
        assert_eq!(first_context.local_scopes.unwrap().index(), 0);
        assert_eq!(second_context.local_scopes.unwrap().index(), 0);

        assert!(matches!(
            second_typer.type_expression(second_rhs, first_context),
            Err(TyperError::ExpressionLocalScopeStackForeign { stack })
                if stack == first_context.local_scopes.unwrap()
        ));
    }

    #[test]
    fn local_scope_overload_bucket_is_preserved_without_first_wins() {
        let (mut parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def overloaded(x: Int): Int = x; def overloaded(x: String): String = x; def use(x: Int): Int = x }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let overloads: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "overloaded" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .collect();
        assert_eq!(overloads.len(), 2);
        let local_name_id = store.names.intern("localOverload");
        let local_name = Name::new(local_name_id, Namespace::Term);
        let method_scope = index.scope_of(method).unwrap();
        store
            .scopes
            .get_mut(method_scope)
            .enter(local_name, overloads[0]);
        store
            .scopes
            .get_mut(method_scope)
            .enter(local_name, overloads[1]);
        let local_ident = match &mut parsed.ast.get_mut(rhs).kind {
            TreeKind::Ident(ident) => ident,
            _ => panic!("RHS should be an identifier"),
        };
        local_ident.name = local_name;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let candidates = typer
            .expression_term_candidates(local_name, context, rhs.index(), None)
            .unwrap();
        assert_eq!(candidates, overloads);
        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::OverloadedReferenceDeferred { .. })
        ));
    }

    #[test]
    fn field_identifier_completes_and_uses_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val field: Int = 1; def method: Int = field }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let field = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "field")
                    .then(|| index.symbol_at(source, tree).unwrap())
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
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Ident(_)
        ));
        let Type::TermRef { prefix, target } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("field identifier should retain a term reference")
        };
        assert_eq!(*target, TermRefTarget::Symbol(field));
        assert!(
            matches!(typer.store().types.get(*prefix), Type::ThisType { class: this } if *this == class)
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn inherited_field_identifier_uses_current_this_prefix() {
        let source_text =
            "class Parent { val value: Int }; class Child extends Parent { def use: Int = value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let lexical = index.declaration_context_of(method).unwrap();
        let scope = index.source_context(lexical).lexical_scope;
        let field_name = store.symbols.get(field).name;
        store.scopes.get_mut(scope).enter(field_name, field);
        let context = ExpressionContext {
            lexical,
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
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if matches!(typer.store().types.get(*prefix), Type::ThisType { class } if *class == child)
                    && *symbol == field
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn nested_class_identifier_uses_enclosing_generic_this_prefix() {
        let source_text = "class Outer[A] { val value: A; class Inner { def use: A = value } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
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
        let term_type = typer.typed_ast().get(typed).ty;
        assert!(
            matches!(
                typer.store().types.get(term_type),
                Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                    if *symbol == field
                        && matches!(typer.store().types.get(*prefix), Type::ThisType { class } if *class == outer)
            ),
            "nested member should retain its enclosing class path"
        );
        let widened = typer.widen_expression_type(term_type).unwrap();
        let SymbolInfo::Complete(raw) = *typer.store().symbols.info(field) else {
            panic!("source field should have complete info");
        };
        assert_ne!(
            widened,
            raw,
            "widening should adapt the outer member through Outer[A]: {:?} vs {:?}",
            typer.store().types.get(widened),
            typer.store().types.get(raw)
        );
    }

    #[test]
    fn top_level_value_identifier_uses_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = global }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let lexical = index.declaration_context_of(method).unwrap();
        let name = Name::new(store.names.intern("global"), Namespace::Term);
        let global = store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(class),
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.int),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let scope = index.source_context(lexical).lexical_scope;
        store.scopes.get_mut(scope).enter(name, global);
        let context = ExpressionContext {
            lexical,
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

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == global
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn variable_selection_uses_its_declared_type() {
        let source_text = "class C { var value: Int = 1; def use: Int = this.value }";
        let (kind, _, widened, _, definitions) = type_method_rhs(source_text, "use").unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn explicit_imported_field_identifier_retains_its_symbol_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Lib { val item: Int = 1 }; class C { import Lib.item; def use: Int = item }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let imported = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "item")
                    .then(|| index.symbol_at(source, tree).unwrap())
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
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == definitions.no_prefix && *symbol == imported
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn wildcard_imported_field_identifier_retains_its_symbol_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Lib { val item: Int = 1 }; class C { import Lib.*; def use: Int = item }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let imported = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "item")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
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

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == definitions.no_prefix && *symbol == imported
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn direct_field_selection_produces_a_typed_select() {
        let (kind, _, widened, _, definitions) = type_method_rhs(
            "class C { val value: Int = 1; def use: Int = this.value }",
            "use",
        )
        .unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn this_selection_keeps_the_exact_field_and_this_type_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1; def use: Int = this.value }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let source_qualifier = selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_select = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, source_qualifier)
            .unwrap();
        let qualifier_type = typer.typed_ast().get(typed_qualifier).ty;
        let Type::TermRef { prefix, target } = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_select).ty)
        else {
            panic!("selection should retain a term reference")
        };

        assert_eq!(*prefix, qualifier_type);
        assert!(matches!(
            typer.store().types.get(*prefix),
            Type::ThisType { class: actual } if *actual == class
        ));
        assert_eq!(*target, TermRefTarget::Symbol(field));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed_select).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn generic_this_selection_widens_using_its_class_parameters() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A; def use: A = this.value }");
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
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
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if matches!(typer.store().types.get(*prefix), Type::ThisType { class: receiver } if *receiver == class)
                    && *symbol == field
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
    }

    #[test]
    fn stable_local_selection_keeps_the_qualifier_reference_path() {
        let source_text = "class Text; class Box[A] { val value: A }; class Use { def use(box: Box[Text]): Text = box.value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let box_parameter = val_symbol(&parsed, &store, &index, source, "box").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(box_parameter).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let source_qualifier = selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_select = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, source_qualifier)
            .unwrap();
        let qualifier_type = typer.typed_ast().get(typed_qualifier).ty;
        let Type::TermRef { prefix, target } = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_select).ty)
        else {
            panic!("selection should retain a term reference")
        };
        assert_eq!(*prefix, qualifier_type);
        assert!(matches!(
            typer.store().types.get(*prefix),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == box_parameter
        ));
        assert_eq!(*target, TermRefTarget::Symbol(field));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed_select).ty)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == text
        ));
        let SymbolInfo::Complete(declaration_type) = *typer.store().symbols.info(field) else {
            panic!("selected source field should be completed")
        };
        assert!(matches!(
            typer.store().types.get(declaration_type),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == parameter
        ));
    }

    #[test]
    fn unstable_selection_prefix_fails_atomically() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box { val value: Int = 1 }; class Use { var box: Box = null; def use: Int = box.value }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let store_checkpoint = store.checkpoint();
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
            Err(TyperError::UnstableSelectionPrefix { .. })
        ));
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
    }

    #[test]
    fn by_name_parameter_is_not_a_stable_selection_prefix() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box { val value: Int = 1 }; class Use { def use(box: => Box): Int = box.value }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "box");
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnstableSelectionPrefix { .. })
        ));
    }

    #[test]
    fn direct_unique_method_selection_keeps_the_methodic_type() {
        let (kind, _, _, widened_type, _) = type_method_rhs(
            "class C { def get(): Int = 1; def use: Int = this.get }",
            "use",
        )
        .unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert!(
            matches!(widened_type, Type::Method(_) | Type::Poly(_)),
            "{widened_type:?}"
        );
    }

    #[test]
    fn generic_method_selection_widens_to_receiver_adapted_signature() {
        let source_text = "class Box[A] { def get(): A = null }; class Text; class Use { def use(box: Box[Text]): Text = box.get }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "box");
        let referenced_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "get")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
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
        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Select(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == referenced_method
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        let widened_type = typer.store().types.get(widened);
        let Type::Method(method) = widened_type else {
            panic!("selected method should widen to its method signature")
        };
        assert!(matches!(
            typer.store().types.get(method.result),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == text
        ));
    }

    #[test]
    fn inherited_unique_member_selection_finds_the_parent_field() {
        let (kind, _, widened, _, definitions) = type_method_rhs(
            "class Parent { val value: Int }; class Child extends Parent; class Use { def use(child: Child): Int = child.value }",
            "use",
        )
        .unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn generic_receiver_selection_substitutes_its_type_argument() {
        let source_text = "class Text; class Box[A] { val value: A }; class Use { def use(box: Box[Text]): Text = box.value }";
        let (kind, _, _, ty, _) = type_method_rhs(source_text, "use").unwrap();
        let (parsed, store, _, _, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");

        assert!(matches!(kind, TreeKind::Select(_)));
        assert!(matches!(
            ty,
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if symbol == text
        ));
    }

    #[test]
    fn generic_inherited_selection_substitutes_through_the_parent_view() {
        let source_text = "class Text; class Parent[A] { val value: A }; class Child[B] extends Parent[B]; class Use { def use(child: Child[Text]): Text = child.value }";
        let (kind, _, _, ty, _) = type_method_rhs(source_text, "use").unwrap();
        let (parsed, store, _, _, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");

        assert!(matches!(kind, TreeKind::Select(_)));
        assert!(matches!(
            ty,
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if symbol == text
        ));
    }

    #[test]
    fn missing_member_is_a_typed_expression_error() {
        let source_text = "class C { def use: Int = this.missing }";
        let (parsed, store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut store = store;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let class_info = typer.complete_symbol(class).unwrap();
        let Type::ClassInfo(info) = typer.store.types.get_mut(class_info) else {
            panic!("expected a completed ClassInfo")
        };
        // Give this regression a fully known, parentless lookup graph so the
        // empty result is conclusive rather than blocked by builtin Object.
        info.parents.clear();

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(result, Err(TyperError::MemberNotFound { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn overloaded_selection_is_deferred_without_allocating_a_typed_result() {
        let source_text = "class C { def item(x: Int): Int = x; def item(x: String): Int = 1; def use: Int = this.item }";
        let (parsed, store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let mut store = store;
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
            Err(TyperError::OverloadedSelectionDeferred { .. })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    #[test]
    fn failed_selection_rolls_back_child_ast_index_and_source_completion() {
        let source_text = "class C { def item(x: Int): Int = x; def item(x: String): Int = 1; def use: Int = this.item }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let store_checkpoint = store.checkpoint();
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
            Err(TyperError::OverloadedSelectionDeferred { .. })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn unique_method_identifier_keeps_exact_reference_and_widens_to_signature() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(param: Int): Int = param; def use: Int = method }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let referenced = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == referenced
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert!(matches!(typer.store().types.get(widened), Type::Method(_)));
    }

    #[test]
    fn repeated_identifier_typing_reuses_its_term_reference() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use(param: Int): Int = param }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "param");
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

        let first = typer.type_expression(rhs, context).unwrap();
        let store_checkpoint = typer.store().checkpoint();
        let second = typer.type_expression(rhs, context).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.source_typed_index().len(), 1);
    }

    #[test]
    fn overloaded_method_identifier_is_explicitly_deferred() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int): Int = x; def method(x: String): Int = 1; def use: Int = method }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::OverloadedReferenceDeferred { .. })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn application_lookup_retains_the_full_lexical_overload_bucket() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int): Int = x; def method(x: String): Int = 1; def use: Int = method }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let name = Name::new(store.names.get("method").unwrap(), Namespace::Term);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidates = typer
            .expression_term_candidates(name, context, rhs.index(), None)
            .unwrap();

        assert_eq!(candidates.len(), 2);
        assert_ne!(candidates[0], candidates[1]);
        assert!(
            candidates
                .iter()
                .all(|candidate| typer.store().symbols.get(*candidate).kind == SymbolKind::Method)
        );
    }

    #[test]
    fn application_selects_the_overload_that_accepts_its_argument() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class C { def method(x: A): Int = 1; def method(x: B): Int = 2; def use(b: B): Int = method(b) }",
        );
        let expected_class = class_symbol(&parsed, &store, &index, source, "B");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if index.symbol_at(source, tree) != Some(use_method) {
                    return None;
                }
                Some(
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap(),
                )
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let overloads: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .collect();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let TreeKind::Apply(source_application) = &parsed.ast.get(rhs).kind else {
            panic!("method RHS should be the source application")
        };
        let source_argument = source_application.args[0];
        let typed = typer.type_expression(rhs, context).unwrap();
        let selected = overloads
            .into_iter()
            .find(|symbol| {
                let Ok(info) = typer.complete_symbol(*symbol) else {
                    return false;
                };
                matches!(
                    typer.store().types.get(info),
                    Type::Method(method)
                        if method.params.len() == 1
                            && matches!(typer.store().types.get(method.params[0].ty),
                                Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }
                                    if *class == expected_class)
                )
            })
            .unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("overloaded invocation should produce a typed Apply")
        };
        assert_eq!(
            typer
                .typed_index
                .get(source, source_argument)
                .expect("source argument should have one typed mapping"),
            application.args[0]
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == selected
        ));
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn overload_application_reports_each_nonconforming_candidate() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int): Int = 1; def method(x: Boolean): Int = 2; def use(x: Byte): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if index.symbol_at(source, tree) != Some(use_method) {
                    return None;
                }
                Some(
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap(),
                )
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
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

        let typed_count_before = typer.typed_arena.iter().count();
        let mapping_count_before = typer.typed_index.len();
        let result = typer.type_expression(rhs, context);
        let Err(TyperError::OverloadApplicationNoApplicable { candidates, .. }) = result else {
            panic!("unexpected overload result: {result:?}");
        };
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|(_, rejection)| matches!(
            rejection,
            OverloadRejection::ArgumentNonConformance {
                argument_index: 0,
                ..
            }
        )));
        assert_eq!(typer.typed_arena.iter().count(), typed_count_before);
        assert_eq!(typer.typed_index.len(), mapping_count_before);
    }

    #[test]
    fn overload_application_filters_candidates_with_the_wrong_arity() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class C { def method(x: A): Int = 1; def method(x: A, y: A): Int = 2; def use(a: A): Int = method(a, a) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
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
            panic!("an applicable overload should produce an Apply")
        };
        let TreeKind::Ident(_) = &typer.typed_ast().get(application.function).kind else {
            panic!("the selected method should remain an identifier")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a selected method reference, found {other:?}"),
        };
        let info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method) if method.params.len() == 2
        ));
    }

    #[test]
    fn overload_application_resolves_imported_method_candidates() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; object Lib { def method(x: A): Int = 1; def method(x: B): Int = 2 }; class Client { import Lib.*; def use(x: B): Int = method(x) }",
        );
        let expected_class = class_symbol(&parsed, &store, &index, source, "B");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
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
            panic!("imported overload should produce a typed Apply")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a selected imported overload, found {other:?}"),
        };
        let info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method)
                if matches!(typer.store().types.get(method.params[0].ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == expected_class)
        ));
    }

    #[test]
    fn overload_application_rejects_mixed_method_and_value_candidates() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val method: Int = 0; def method(x: Int): Int = x; def use: Int = method(1) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::MixedApplicationCandidateKinds { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn selected_overload_uses_the_receiver_adapted_candidate() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class C { def method(x: A): Int = 1; def method(x: B): Int = 2 }; class Client { def use(c: C, b: B): Int = c.method(b) }",
        );
        let expected_class = class_symbol(&parsed, &store, &index, source, "B");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if index.symbol_at(source, tree) != Some(use_method) {
                    return None;
                }
                Some(
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap(),
                )
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let overloads: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .collect();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let selected = overloads
            .into_iter()
            .find(|symbol| {
                let Ok(info) = typer.complete_symbol(*symbol) else {
                    return false;
                };
                matches!(
                    typer.store().types.get(info),
                    Type::Method(method)
                        if method.params.len() == 1
                            && matches!(typer.store().types.get(method.params[0].ty),
                                Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }
                                    if *class == expected_class)
                )
            })
            .unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("selected overload invocation should produce a typed Apply")
        };
        assert!(matches!(
            typer.typed_ast().get(application.function).kind,
            TreeKind::Select(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == selected
        ));
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn overload_application_selects_the_unique_most_specific_of_three_candidates() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent {}; class Child extends Parent {}; class Grandchild extends Child {}; class C { def method(x: Parent): Int = 1; def method(x: Child): Int = 2; def method(x: Grandchild): Int = 3; def use(x: Grandchild): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let expected = class_symbol(&parsed, &store, &index, source, "Grandchild");
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
            panic!("the most specific overload should be applied")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected method reference, found {other:?}"),
        };
        let info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method)
                if matches!(typer.store().types.get(method.params[0].ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == expected)
        ));
    }

    #[test]
    fn unrelated_applicable_overloads_remain_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class Both extends A, B {}; class C { def method(x: A): Int = 1; def method(x: B): Int = 2; def use(x: Both): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
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
            Err(TyperError::AmbiguousOverloadApplication { candidates, .. })
                if candidates.len() == 2
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn equally_specific_generic_and_monomorphic_overloads_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class C { def method(x: A): Int = 1; def method[T](x: T): Int = 2; def use(x: A): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
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
            Err(TyperError::AmbiguousOverloadApplication { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn generic_overload_is_selected_when_monomorphic_candidate_does_not_apply() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent {}; class Child extends Parent {}; class C { def method[T <: Parent](x: T): T = x; def method(x: Parent): Parent = x; def use(x: Child): Parent = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let generic_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method"
                        && !definition.type_params.is_empty() =>
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
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("generic overload should produce a typed application")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == generic_method
        ));
        let child = class_symbol(&parsed, typer.store, &index, source, "Child");
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == child
        ));
        let TreeKind::Apply(source_application) = &parsed.ast.get(rhs).kind else {
            panic!("source application should have an Apply node")
        };
        assert!(
            source_application
                .args
                .iter()
                .all(|argument| typer.source_typed_index().get(source, *argument).is_some())
        );
    }

    #[test]
    fn generic_overload_rejected_by_bounds_does_not_block_monomorphic_candidate() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent {}; class Child extends Parent {}; class C { def method[T <: Child](x: T): Int = 1; def method(x: Parent): Int = 2; def use(x: Parent): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let monomorphic = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method"
                        && definition.type_params.is_empty() =>
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
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("the monomorphic candidate should remain applicable")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == monomorphic
        ));
    }

    #[test]
    fn conflicting_generic_candidate_is_reported_as_inapplicable() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method[T](x: T, y: T): Int = 1; def method(x: Int, y: Int): Int = 2; def use: Int = method(1, true) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                result,
                Err(TyperError::OverloadApplicationNoApplicable { ref candidates, .. })
                    if candidates.len() == 2
                        && candidates.iter().any(|(_, rejection)| matches!(
                            rejection,
                            OverloadRejection::TypeArgumentInferenceFailure { parameter_index: Some(0) }
                        ))
            ),
            "unexpected overload result: {result:?}"
        );
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn unconstrained_generic_overload_blocks_a_monomorphic_winner() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method[A, B](x: A): Int = 1; def method(x: Int): Int = 2; def use: Int = method(1) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate { .. })
        ));
    }

    #[test]
    fn generic_candidates_compete_by_instantiated_specificity() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent {}; class Child extends Parent {}; class C { def method[T <: Parent](x: T, y: Parent): Parent = x; def method[T <: Parent](x: T, y: Child): Child = x; def use(x: Child): Child = method(x, x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(_) = &typer.typed_ast().get(typed).kind else {
            panic!("a generic overload should be selected")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == child
        ));
    }

    #[test]
    fn equally_specific_generic_overloads_remain_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class Both extends A, B {}; class C { def method[T](x: T, y: A): Int = 1; def method[T](x: T, y: B): Int = 2; def use(x: Both): Int = method(x, x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
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
            Err(TyperError::AmbiguousOverloadApplication { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn inherited_generic_overload_is_inferred_after_receiver_adaptation() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent {}; class Child extends Parent {}; class Base { def method[T <: Parent](x: T): T = x }; class Derived extends Base { def method(x: Parent): Parent = x }; class Client { def use(value: Derived, child: Child): Child = value.method(child) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let inherited_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == Some(base) =>
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

        let typed = typer
            .type_expression(rhs, context)
            .unwrap_or_else(|error| panic!("typing inherited generic overload failed: {error:?}"));
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("inherited generic overload should be applied")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == inherited_method
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == child
        ));
    }

    #[test]
    fn explicit_type_application_instantiates_a_method_before_apply() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A](value: A): A = value; def use: Int = identity[Int](1) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let identity = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "identity" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let source_type_application = match &parsed.ast.get(rhs).kind {
            TreeKind::Apply(application) => application.function,
            _ => panic!("source body should be an Apply"),
        };
        let source_type_application_position = parsed.ast.get(source_type_application).position;
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
            panic!("the source call should remain an Apply")
        };
        let TreeKind::TypeApply(type_application) =
            &typer.typed_ast().get(application.function).kind
        else {
            panic!("the typed function should retain its explicit TypeApply")
        };
        let instantiated = typer.typed_ast().get(application.function).ty;
        let Type::Method(method) = typer.store().types.get(instantiated) else {
            panic!("TypeApply should instantiate the method's Poly binder")
        };
        assert_eq!(method.params.len(), 1);
        assert_eq!(method.params[0].ty, definitions.int);
        assert_eq!(method.result, definitions.int);
        assert_eq!(
            typer.typed_ast().get(application.function).position,
            source_type_application_position
        );
        let function = typer.typed_ast().get(type_application.function);
        assert!(matches!(
            typer.store().types.get(function.ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == identity
        ));
        assert_eq!(type_application.args.len(), 1);
        let type_argument = type_application.args[0];
        let source_type_argument = match &parsed.ast.get(rhs).kind {
            TreeKind::Apply(application) => match &parsed.ast.get(application.function).kind {
                TreeKind::TypeApply(type_application) => type_application.args[0],
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert!(matches!(
            typer.typed_ast().get(type_argument).kind,
            TreeKind::TypeTree(_)
        ));
        assert_eq!(typer.typed_ast().get(type_argument).ty, definitions.int);
        assert_eq!(
            typer.typed_ast().get(type_argument).position,
            parsed.ast.get(source_type_argument).position
        );
        assert_eq!(
            typer.source_typed_index().get(source, source_type_argument),
            Some(type_argument)
        );
    }

    #[test]
    fn explicit_type_arguments_are_instantiated_by_binder_position() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def choose[A, B](left: A, right: B): B = right; def use: Boolean = choose[Int, Boolean](1, true) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let source_type_arguments = match &parsed.ast.get(rhs).kind {
            TreeKind::Apply(application) => match &parsed.ast.get(application.function).kind {
                TreeKind::TypeApply(type_application) => type_application.args.clone(),
                _ => panic!("source function should have explicit type arguments"),
            },
            _ => panic!("source body should be an Apply"),
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
            panic!("expected a typed Apply")
        };
        let TreeKind::TypeApply(type_application) =
            &typer.typed_ast().get(application.function).kind
        else {
            panic!("expected typed explicit type arguments")
        };
        let Type::Method(method) = typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        else {
            panic!("expected an instantiated Method")
        };
        assert_eq!(method.params[0].ty, definitions.int);
        assert_eq!(method.params[1].ty, definitions.boolean);
        assert_eq!(method.result, definitions.boolean);
        assert_eq!(type_application.args.len(), 2);
        assert_eq!(
            typer.typed_ast().get(type_application.args[0]).ty,
            definitions.int
        );
        assert_eq!(
            typer.typed_ast().get(type_application.args[1]).ty,
            definitions.boolean
        );
        assert_eq!(
            typer
                .source_typed_index()
                .get(source, source_type_arguments[0]),
            Some(type_application.args[0])
        );
        assert_eq!(
            typer
                .source_typed_index()
                .get(source, source_type_arguments[1]),
            Some(type_application.args[1])
        );
    }

    #[test]
    fn explicit_type_application_checks_upper_bounds() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A <: Int](value: A): A = value; def use: Int = identity[Int] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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

        let Type::Method(method) = typer.store().types.get(typer.typed_ast().get(typed).ty) else {
            panic!("the explicit type application should produce a Method")
        };
        assert_eq!(method.params[0].ty, definitions.int);
    }

    #[test]
    fn explicit_type_application_checks_lower_bounds() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A >: Int](value: A): A = value; def use: Any = identity[Any] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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

        let Type::Method(method) = typer.store().types.get(typer.typed_ast().get(typed).ty) else {
            panic!("the explicit type application should produce a Method")
        };
        assert_eq!(method.params[0].ty, definitions.any_type);
    }

    #[test]
    fn explicit_type_application_rejects_an_upper_bound_violation_atomically() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A <: Int](value: A): A = value; def use: Any = identity[Boolean] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let source_argument = match &parsed.ast.get(rhs).kind {
            TreeKind::TypeApply(type_application) => type_application.args[0],
            _ => panic!("source body should be a TypeApply"),
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
            Err(TyperError::ExplicitTypeArgumentBoundViolation {
                parameter_index: 0,
                side: TypeArgumentBoundSide::Upper,
                ..
            })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(
            typer.source_type_index().type_at(source, source_argument),
            None
        );
        assert_eq!(typer.store().checkpoint(), store_before);
    }

    #[test]
    fn explicit_type_application_rejects_a_lower_bound_violation() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A >: Int](value: A): A = value; def use: Any = identity[Boolean] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::ExplicitTypeArgumentBoundViolation {
                parameter_index: 0,
                side: TypeArgumentBoundSide::Lower,
                ..
            })
        ));
    }

    #[test]
    fn explicit_type_application_rejects_aliasing_bounds() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A](value: A): A = value; def use: Any = identity[Int] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let identity = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "identity" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
        let original = typer.complete_symbol(identity).unwrap();
        let Type::Poly(mut poly) = typer.store().types.get(original).clone() else {
            panic!("identity should have a Poly signature")
        };
        poly.params[0].bounds = typer.store.types.alloc(Type::AliasingBounds {
            alias: definitions.int,
        });
        let new_poly = typer.store.types.reserve();
        let new_poly_id = new_poly.id();
        let result = typer.store.types.alloc(Type::ParamRef {
            binder: new_poly_id,
            index: 0,
        });
        let method = typer.store.types.reserve();
        let method_id = method.id();
        let method_type = typer.store.types.fill(
            method,
            Type::Method(MethodType {
                params: vec![dotty_core::MethodParam {
                    name: TermName::new(typer.store.names.intern("value")),
                    ty: result,
                    erased: false,
                    varargs: false,
                }],
                result,
                kind: MethodKind::Plain,
            }),
        );
        assert_eq!(method_type, method_id);
        let complete_poly = typer.store.types.fill(
            new_poly,
            Type::Poly(dotty_core::PolyType {
                params: poly.params,
                result: method_type,
            }),
        );
        typer
            .store
            .symbols
            .set_info(identity, SymbolInfo::Complete(complete_poly));

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnsupportedExplicitTypeArgumentBounds {
                parameter_index: 0,
                ..
            })
        ));
    }

    #[test]
    fn explicit_type_application_rejects_too_few_type_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A, B](value: A): A = value; def use: Any = identity[Int] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::ExplicitTypeApplicationArityMismatch {
                expected: 2,
                actual: 1,
                ..
            })
        ));
    }

    #[test]
    fn explicit_type_application_rejects_too_many_type_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A](value: A): A = value; def use: Any = identity[Int, Boolean] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::ExplicitTypeApplicationArityMismatch {
                expected: 1,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn explicit_type_application_of_an_overloaded_name_is_deferred() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def identity[A](value: A): A = value; def identity[A](value: A, other: A): A = value; def use: Any = identity[Int] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::OverloadedTypeApplicationDeferred { .. })
        ));
    }

    #[test]
    fn applied_type_arguments_are_reified_as_typed_type_trees() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "trait Box[A]; trait Client { def accept[A](value: Int): A; def use: Any = accept[Box[Int]] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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

        let TreeKind::TypeApply(type_application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected explicit type application")
        };
        let type_argument = type_application.args[0];
        assert!(matches!(
            typer.typed_ast().get(type_argument).kind,
            TreeKind::TypeTree(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(type_argument).ty),
            Type::Applied { args, .. } if args == &[definitions.int]
        ));
    }

    #[test]
    fn explicit_type_application_rejects_a_non_polymorphic_callee() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def value: Int = 1; def use: Any = value[Int] }");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
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
            Err(TyperError::ExplicitTypeApplicationCalleeNotPoly { .. })
        ));
    }

    #[test]
    fn selected_type_arguments_are_projected_in_the_expression_context() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "trait Outer { class Inner }; trait Api { def accept[A](value: Int): A; def use: Any = accept[Outer.Inner] }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let expected = class_symbol(&parsed, &store, &index, source, "Inner");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        let TreeKind::TypeApply(type_application) = &typer.typed_ast().get(typed).kind else {
            panic!("expected explicit type application")
        };
        let type_argument = type_application.args[0];
        assert!(matches!(
            typer.typed_ast().get(type_argument).kind,
            TreeKind::TypeTree(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(type_argument).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == expected
        ));
    }

    #[test]
    fn potentially_applicable_repeated_parameter_overload_blocks_selection() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int, y: Int): Int = 1; def method(xs: Int*): Int = 2; def use(x: Int, y: Int): Int = method(x, y) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
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

        let Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate { candidate, .. }) =
            typer.type_expression(rhs, context)
        else {
            panic!("a potentially applicable repeated-parameter overload must block selection")
        };
        let info = typer.complete_symbol(candidate).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method) if method.params.len() == 1 && method.params[0].varargs
        ));
    }

    #[test]
    fn repeated_parameter_overload_with_too_few_arguments_is_filtered() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(): Int = 1; def method(prefix: Int, xs: Int*): Int = 2; def use(): Int = method() }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let expected_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method"
                    && definition.value_param_clauses[0].is_empty())
                .then(|| index.symbol_at(source, tree).unwrap())
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
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("zero-argument overload should produce an Apply")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == expected_method
        ));
    }

    #[test]
    fn wrong_arity_polymorphic_overload_does_not_block_monomorphic_selection() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class C { def method(x: A): Int = 1; def method[T](x: T, y: T): Int = 2; def use(x: A): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
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

        assert!(typer.type_expression(rhs, context).is_ok());
    }

    #[test]
    fn inherited_generic_overload_uses_the_receiver_adapted_signature() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Wider {}; class Text extends Wider {}; class Base[A] { def method(x: A): Int = 1 }; class Child extends Base[Text] { def method(x: Wider): Int = 2 }; class Client { def use(child: Child, x: Text): Int = child.method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let base_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method"
                    && store
                        .symbols
                        .get(index.symbol_at(source, tree).unwrap())
                        .owner
                        == Some(class_symbol(&parsed, &store, &index, source, "Base")))
                .then(|| index.symbol_at(source, tree).unwrap())
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

        let typed = typer
            .type_expression(rhs, context)
            .unwrap_or_else(|error| panic!("typing inherited overload failed: {error:?}"));
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("inherited generic overload should produce an Apply")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == base_method
        ));
        let function_type = typer.typed_ast().get(application.function).ty;
        let widened = typer.widen_expression_type(function_type).unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::Method(method)
                if matches!(typer.store().types.get(method.params[0].ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == text_class)
        ));
    }

    #[test]
    fn inherited_overload_competes_with_a_multi_method_local_bucket() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class ParentType {}; class ChildType extends ParentType {}; class Other {}; class Base { def method(x: ChildType): Int = 1 }; class C extends Base { def method(x: ParentType): Int = 2; def method(x: Other): Int = 3 }; class Client { def use(c: C, x: ChildType): Int = c.method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let base_class = class_symbol(&parsed, &store, &index, source, "Base");
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
            panic!("local overload bucket should produce an Apply")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected method reference, found {other:?}"),
        };
        assert_eq!(typer.store().symbols.get(selected).owner, Some(base_class));
    }

    #[test]
    fn exact_inherited_signature_is_suppressed_by_its_override() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class Base { def method(x: A): Int = 1 }; class C extends Base { def method(x: A): Int = 2 }; class Client { def use(c: C, x: A): Int = c.method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
            local_scopes: None,
        };
        let receiver_class = class_symbol(&parsed, &store, &index, source, "C");
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
            panic!("overridden method should produce an Apply")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected method reference, found {other:?}"),
        };
        assert_eq!(
            typer.store().symbols.get(selected).owner,
            Some(receiver_class)
        );
    }

    #[test]
    fn a_type_namespace_name_does_not_resolve_as_a_term_identifier() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { type item = Int; def method: Int = item }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
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
            Err(TyperError::TermNameNotFound { .. })
        ));
    }

    #[test]
    fn source_object_identifier_retains_object_symbol_and_widens_to_module_class() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O; class C { def use: Any = O }");
        let (object_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(owner, source, object_tree)
            .expect("source object should have a derived ModuleClass");
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
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == object
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == module_class
        ));
    }

    #[test]
    fn source_object_selection_looks_up_members_through_its_module_class() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O { val value: Int = 1 }; class C { def use: Int = O.value }");
        let (object_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(owner, source, object_tree)
            .expect("source object should have a derived ModuleClass");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let qualifier_tree = selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, qualifier_tree)
            .unwrap();
        let qualifier_ty = typer.typed_ast().get(typed_qualifier).ty;
        assert!(matches!(
            typer.store().types.get(qualifier_ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == object
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == qualifier_ty && *symbol == field
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert_eq!(widened, definitions.int);
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
    }

    #[test]
    fn nested_object_selection_retains_the_concrete_receiver_prefix() {
        let source_text = "class Outer { object Nested { val value: Int = 1 } }; class Use { def use(outer: Outer): Int = outer.Nested.value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = val_symbol(&parsed, &store, &index, source, "outer").0;
        let lexical = index.declaration_context_of(parameter).unwrap();
        let context = ExpressionContext {
            lexical,
            owner: method,
            local_scopes: None,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let object_selection_tree = selection.qualifier;
        let TreeKind::Select(object_selection) = &parsed.ast.get(object_selection_tree).kind else {
            panic!("outer selection qualifier should be a selection")
        };
        let receiver_tree = object_selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let receiver = typer
            .source_typed_index()
            .get(source, receiver_tree)
            .unwrap();
        let receiver_type = typer.typed_ast().get(receiver).ty;
        assert!(matches!(
            typer.store().types.get(receiver_type),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
        let object_selection = typer
            .source_typed_index()
            .get(source, object_selection_tree)
            .unwrap();
        let object_type = typer.typed_ast().get(object_selection).ty;
        let widened_object = typer.widen_expression_type(object_type).unwrap();
        let Type::TypeRef { prefix, target } = typer.store().types.get(widened_object) else {
            panic!("nested object should widen to a module-class reference")
        };
        assert_eq!(*prefix, receiver_type);
        let module_class = match target {
            TypeRefTarget::Symbol(symbol) => *symbol,
            _ => panic!("nested object module class should be a symbol reference"),
        };
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn external_object_identifier_stays_explicitly_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Any = External }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let lexical = index.declaration_context_of(method).unwrap();
        let name = Name::new(store.names.intern("External"), Namespace::Term);
        let external = store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: None,
            kind: SymbolKind::Object,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let scope = index.source_context(lexical).lexical_scope;
        store.scopes.get_mut(scope).enter(name, external);
        let context = ExpressionContext {
            lexical,
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
            Err(TyperError::ObjectTermReferenceDeferred { symbol, .. }) if symbol == external
        ));
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn nested_member_selections_preserve_the_full_reference_path() {
        let source_text = "class Leaf { val value: Int = 1 }; class Box { val child: Leaf }; class Use { def use(box: Box): Int = box.child.value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let child = val_symbol(&parsed, &store, &index, source, "child").0;
        let value = val_symbol(&parsed, &store, &index, source, "value").0;
        let box_symbol = val_symbol(&parsed, &store, &index, source, "box").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(box_symbol).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let TreeKind::Select(outer_selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let middle_tree = outer_selection.qualifier;
        let TreeKind::Select(middle_selection) = &parsed.ast.get(middle_tree).kind else {
            panic!("selection qualifier should also be a selection")
        };
        let box_tree = middle_selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let outer = typer.type_expression(rhs, context).unwrap();
        let middle = typer.source_typed_index().get(source, middle_tree).unwrap();
        let typed_box = typer.source_typed_index().get(source, box_tree).unwrap();
        let middle_type = typer.typed_ast().get(middle).ty;
        let box_type = typer.typed_ast().get(typed_box).ty;
        assert!(matches!(
            typer.store().types.get(middle_type),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == box_type && *symbol == child
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(outer).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == middle_type && *symbol == value
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(outer).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn typed_selection_indexes_its_qualifier_and_preserves_source_positions() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1; def use: Int = this.value }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let source_position = parsed.ast.get(rhs).position;
        let TreeKind::Select(source_select) = &parsed.ast.get(rhs).kind else {
            panic!("expected source selection")
        };
        let source_qualifier = source_select.qualifier;
        let qualifier_position = parsed.ast.get(source_qualifier).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_select = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, source_qualifier)
            .expect("typed qualifier should be indexed");

        assert_eq!(
            typer.source_typed_index().get(source, rhs),
            Some(typed_select)
        );
        assert_eq!(
            typer.typed_ast().get(typed_select).position,
            source_position
        );
        assert_eq!(
            typer.typed_ast().get(typed_qualifier).position,
            qualifier_position
        );
        for (_, tree) in typer.typed_ast().iter() {
            assert!(typer.store().types.contains(tree.ty));
        }
    }

    #[test]
    fn application_with_unresolved_callee_reports_name_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = method(1) }");
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
            Err(TyperError::TermNameNotFound { .. })
        ));
    }

    #[test]
    fn numeric_literal_42_becomes_an_int_constant() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 42 }");
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

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(42)
            })
        );
        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::Constant(dotty_core::Constant::Int(42))
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn largest_positive_int_literal_and_separators_are_preserved() {
        let (value, own_type, ty, definitions) =
            type_value_rhs("class C { val value = 2147483647 }");
        assert_eq!(value, dotty_core::Constant::Int(i32::MAX));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.int);

        let (value, own_type, ty, definitions) = type_value_rhs("class C { val value = 4_2 }");
        assert_eq!(value, dotty_core::Constant::Int(42));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.int);
    }

    #[test]
    fn hexadecimal_integer_literal_preserves_radix_bits() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 0xffffffff }");
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

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(-1)
            })
        );
    }

    #[test]
    fn binary_integer_literal_preserves_radix_value() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 0b1010 }");
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

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(10)
            })
        );
        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::Constant(dotty_core::Constant::Int(10))
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn typing_a_literal_twice_reuses_its_typed_tree_and_position() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 42 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
            local_scopes: None,
        };
        let source_position = parsed.ast.get(rhs).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.type_expression(rhs, context).unwrap();
        let second = typer.type_expression(rhs, context).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.typed_ast().get(first).position, source_position);
        assert_eq!(typer.typed_ast().iter().count(), 1);
        assert_eq!(typer.source_typed_index().len(), 1);
    }

    #[test]
    fn parenthesized_literal_maps_to_the_inner_typed_tree() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = (1) }");
        let (method, expression) =
            method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = parsed.ast.get(expression).kind
        else {
            panic!("the source parser should preserve the parentheses wrapper");
        };
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

        let inner_first = typer.type_expression(parens.inner, context).unwrap();
        let first = typer.type_expression(expression, context).unwrap();
        let second = typer.type_expression(expression, context).unwrap();

        assert_eq!(inner_first, first);
        assert_eq!(first, second);
        assert_eq!(
            typer.source_typed_index().get(source, expression),
            Some(first)
        );
        assert_eq!(
            typer.source_typed_index().get(source, parens.inner),
            Some(first)
        );
        assert!(matches!(
            typer.typed_ast().get(first).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(1)
            })
        ));
        assert_eq!(
            typer.typed_ast().get(first).position,
            parsed.ast.get(parens.inner).position,
            "the disappearing wrapper must not overwrite the inner source position"
        );
        assert_eq!(typer.typed_ast().iter().count(), 1);
    }

    #[test]
    fn nested_parenthesized_identifier_preserves_term_reference_identity() {
        let oracle_source = include_str!(
            "../../tests/fixtures/parenthesized-expressions/ParenthesizedExpressions.scala"
        );
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name(oracle_source);
        let (method, expression) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
        let mut wrappers = Vec::new();
        let mut inner = expression;
        while let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &parsed.ast.get(inner).kind
        {
            wrappers.push(inner);
            inner = parens.inner;
        }
        assert_eq!(wrappers.len(), 3, "parser should preserve nested wrappers");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(expression, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
        assert_eq!(typer.source_typed_index().get(source, inner), Some(typed));
        for wrapper in wrappers {
            assert_eq!(typer.source_typed_index().get(source, wrapper), Some(typed));
        }
        let oracle = include_str!(
            "../../tests/fixtures/parenthesized-expressions/ParenthesizedExpressions.typed-tree.txt"
        );
        assert!(oracle.contains("def f(x: Int): Int = x"));
        assert!(oracle.contains("def g: Int = 1"));
        assert!(!oracle.contains("(((x)))"));
    }

    #[test]
    fn parenthesized_selection_application_and_argument_keep_the_selected_method() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def id(x: Int): Int = x; def use(x: Int): Int = (this.id)((x)) }",
        );
        let (method, expression) =
            method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let selected_method = method_symbol(&parsed, &store, &index, source, "id");
        let TreeKind::Apply(application) = &parsed.ast.get(expression).kind else {
            panic!("source should retain an ordinary application");
        };
        assert!(matches!(
            parsed.ast.get(application.function).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(matches!(
            parsed.ast.get(application.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(expression, context).unwrap();

        let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed).kind else {
            panic!("application should remain a typed Apply");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == selected_method
        ));
        assert_eq!(
            typer.source_typed_index().get(source, application.args[0]),
            Some(typed_application.args[0])
        );
    }

    #[test]
    fn expected_type_passes_through_parentheses_and_failed_inner_typing_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def good: Int = (1); def bad: Int = (missing) }");
        let (good_method, good_expression) =
            method_definition_and_rhs(&parsed, &store, &index, source, "good");
        let (bad_method, bad_expression) =
            method_definition_and_rhs(&parsed, &store, &index, source, "bad");
        let TreeKind::PhaseSpecific(UntypedNode::Parens(good_parens)) =
            &parsed.ast.get(good_expression).kind
        else {
            panic!("expected parenthesized literal");
        };
        let good_inner = good_parens.inner;
        let TreeKind::PhaseSpecific(UntypedNode::Parens(bad_parens)) =
            &parsed.ast.get(bad_expression).kind
        else {
            panic!("expected parenthesized unresolved identifier");
        };
        let bad_inner = bad_parens.inner;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let good_context = typer.expression_context_for(good_method).unwrap();

        let good = typer
            .type_expression_expected(good_expression, good_context, definitions.int)
            .unwrap();

        assert_eq!(
            typer.source_typed_index().get(source, good_expression),
            Some(good)
        );
        assert_eq!(
            typer.source_typed_index().get(source, good_inner),
            Some(good)
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(good).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));

        let store_checkpoint = typer.store().checkpoint();
        let ast_len = typer.typed_ast().iter().count();
        let bad_context = typer.expression_context_for(bad_method).unwrap();
        assert!(matches!(
            typer.type_expression(bad_expression, bad_context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), ast_len);
        assert_eq!(typer.source_typed_index().get(source, bad_expression), None);
        assert_eq!(typer.source_typed_index().get(source, bad_inner), None);
    }

    #[test]
    fn parenthesized_if_block_and_constructor_keep_their_semantics() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box(val value: Int); class C { def choose(flag: Boolean): Int = (if (flag) { 1 } else { 2 }); def block: Int = ({ val local = 1; local }); def make: Box = (new Box(1)); def tuple = (1, 2) }",
        );
        let box_symbol = class_symbol(&parsed, &store, &index, source, "Box");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for name in ["choose", "block", "make"] {
            let (method, expression) =
                method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
            let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) =
                &parsed.ast.get(expression).kind
            else {
                panic!("method `{name}` should have a parenthesized body");
            };
            let inner = parens.inner;
            let context = typer.expression_context_for(method).unwrap();

            let typed = typer.type_expression(expression, context).unwrap();

            assert_eq!(
                typer.source_typed_index().get(source, expression),
                Some(typed)
            );
            assert_eq!(typer.source_typed_index().get(source, inner), Some(typed));
            if name == "choose" {
                assert!(matches!(typer.typed_ast().get(typed).kind, TreeKind::If(_)));
            } else if name == "block" {
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::Block(_)
                ));
            } else {
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::Apply(_)
                ));
                assert_eq!(
                    type_symbol(typer.store(), typer.typed_ast().get(typed).ty),
                    box_symbol
                );
            }
        }

        let (_, tuple) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "tuple");
        assert!(matches!(
            parsed.ast.get(tuple).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
    }

    #[test]
    fn expected_expression_type_accepts_a_matching_widened_literal() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1 }");
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

        let typed = typer
            .type_expression_expected(rhs, context, definitions.int)
            .unwrap();

        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::Constant(dotty_core::Constant::Int(1))
        );
    }

    #[test]
    fn ordinary_infix_call_matches_selected_member_application() {
        let source_text =
            include_str!("../../tests/fixtures/infix-expressions/InfixExpressions.scala");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (infix_method, infix_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "infix");
        let (direct_method, direct_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "direct");
        let combine = method_symbol(&parsed, &store, &index, source, "combine");
        assert!(matches!(
            parsed.ast.get(infix_tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let infix_context = typer.expression_context_for(infix_method).unwrap();

        let infix = typer.type_expression(infix_tree, infix_context).unwrap();

        assert!(matches!(
            typer.typed_ast().get(infix).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(
            type_symbol(typer.store(), typer.typed_ast().get(infix).ty),
            class_symbol(&parsed, typer.store(), &index, source, "Box")
        );
        assert_eq!(
            typer.source_typed_index().get(source, infix_tree),
            Some(infix)
        );

        let direct_context = typer.expression_context_for(direct_method).unwrap();
        let direct = typer.type_expression(direct_tree, direct_context).unwrap();
        let TreeKind::Apply(direct_application) = &typer.typed_ast().get(direct).kind else {
            panic!("dot-call should produce a typed application");
        };
        let TreeKind::Apply(infix_application) = &typer.typed_ast().get(infix).kind else {
            unreachable!();
        };
        for function in [direct_application.function, infix_application.function] {
            assert!(matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(function).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == combine
            ));
        }
        assert_eq!(
            typer.typed_ast().get(infix).ty,
            typer.typed_ast().get(direct).ty
        );
        let oracle =
            include_str!("../../tests/fixtures/infix-expressions/InfixExpressions.typed-tree.txt");
        assert!(oracle.contains("def infix(left: Box, right: Box): Box = left.combine(right)"));
    }

    #[test]
    fn prefix_operators_select_the_exact_unary_members() {
        let source_text =
            include_str!("../../tests/fixtures/prefix-expressions/PrefixExpressions.scala");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let cases = [
            ("not", "!", "unary_!"),
            ("complement", "~", "unary_~"),
            ("positive", "+", "unary_+"),
            ("negative", "-", "unary_-"),
        ];
        let expected = cases
            .iter()
            .map(|(method, operator, member)| {
                let (owner, tree) =
                    method_definition_and_rhs(&parsed, &store, &index, source, method);
                let symbol = method_symbol(&parsed, &store, &index, source, member);
                (owner, tree, *operator, symbol)
            })
            .collect::<Vec<_>>();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (owner, tree, operator, symbol) in expected {
            let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) = &parsed.ast.get(tree).kind
            else {
                panic!("the source must retain a PrefixOp");
            };
            assert_eq!(typer.store().names.resolve(prefix.op.text()), operator);
            let context = typer.expression_context_for(owner).unwrap();
            let typed = typer.type_expression(tree, context).unwrap();
            let TreeKind::Select(selection) = &typer.typed_ast().get(typed).kind else {
                panic!("Scala 3.9 types a parameterless unary member as Select");
            };
            assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
            assert_eq!(
                typer.source_typed_index().get(source, prefix.operand),
                Some(selection.qualifier)
            );
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(typed).ty),
                Type::TermRef { target: TermRefTarget::Symbol(selected), .. }
                    if *selected == symbol
            ));
        }
        let oracle = include_str!(
            "../../tests/fixtures/prefix-expressions/PrefixExpressions.typed-tree.txt"
        );
        for expression in [
            "value.unary_!",
            "value.unary_~",
            "value.unary_+",
            "value.unary_-",
        ] {
            assert!(oracle.contains(expression));
        }
    }

    #[test]
    fn prefix_selection_finds_inherited_unary_member() {
        let source_text = "class Base { def unary_~ : Base = this }; class Child extends Base; class Use { def use(child: Child): Base = ~child }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let inherited = method_symbol(&parsed, &store, &index, source, "unary_~");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        let typed = typer.type_expression(tree, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == inherited
        ));
    }

    #[test]
    fn prefix_selection_adapts_an_applied_receiver_member() {
        let source_text = "class Box[A] { def unary_! : A = ??? }; class Use { def use(value: Box[Int]): Int = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let member = method_symbol(&parsed, &store, &index, source, "unary_!");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        let typed = typer.type_expression(tree, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == member
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn prefix_operand_is_typed_once_and_stable_value_member_is_selected() {
        let source_text = "class Box { val unary_! : Box = this }; class Use { def use(value: Box): Box = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (member, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "unary_!");
        let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) = &parsed.ast.get(tree).kind
        else {
            panic!("expected source PrefixOp");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        let typed = typer.type_expression(tree, context).unwrap();

        let TreeKind::Select(selection) = &typer.typed_ast().get(typed).kind else {
            panic!("a stable unary value should remain a selection");
        };
        assert_eq!(
            typer.source_typed_index().get(source, prefix.operand),
            Some(selection.qualifier)
        );
        assert_eq!(
            typer
                .typed_ast()
                .iter()
                .filter(|(_, node)| matches!(node.kind, TreeKind::Ident(_)))
                .count(),
            1,
            "the source operand must produce only one typed identifier"
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == member
        ));
    }

    #[test]
    fn prefix_selection_accepts_mutable_local_operand() {
        let source_text = "class Box { def unary_! : Box = this }; class Use { def use(x: Box): Box = { var y: Box = x; !y } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let member = method_symbol(&parsed, &store, &index, source, "unary_!");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("method body should be a source block");
        };
        let prefix = source_block.expr;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let local = typer
            .local_symbol_at(source, source_block.stats[0])
            .unwrap();
        assert!(
            typer
                .store()
                .symbols
                .get(local)
                .flags
                .contains(SymbolFlags::MUTABLE)
        );
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should produce a typed block");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(typed_block.expr).kind else {
            panic!("prefix expression should select unary_!");
        };
        assert_eq!(
            typer.source_typed_index().get(source, prefix),
            Some(typed_block.expr)
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(selection.qualifier).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_block.expr).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == member
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed_block.expr).ty)
            .unwrap();
        assert_eq!(
            type_symbol(typer.store(), widened),
            class_symbol(&parsed, typer.store(), &index, source, "Box")
        );
    }

    #[test]
    fn prefix_missing_member_rolls_back_operand_and_selection_state() {
        let source_text = "class Box; class Use { def use(value: Box): Box = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) = &parsed.ast.get(tree).kind
        else {
            panic!("expected source PrefixOp");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::MemberNotFound { source: error_source, tree_index, name, .. })
                if error_source == source && tree_index == tree.index()
                    && typer.store().names.resolve(name.text()) == "unary_!"
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().get(source, tree).is_none());
        assert!(
            typer
                .source_typed_index()
                .get(source, prefix.operand)
                .is_none()
        );
    }

    #[test]
    fn prefix_overloaded_member_is_reported_as_ambiguous() {
        let source_text = "class Box { def unary_! : Int = 1; def unary_! : Boolean = true }; class Use { def use(value: Box): Int = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::OverloadedSelectionDeferred { source: error_source, tree_index, name })
                if error_source == source && tree_index == tree.index()
                    && typer.store().names.resolve(name.text()) == "unary_!"
        ));
        assert!(typer.source_typed_index().get(source, tree).is_none());
    }

    #[test]
    fn prefix_empty_parameter_clause_matches_scala_oracle_error() {
        let source_text = "class Box { def unary_!(): Box = this }; class Use { def use(value: Box): Box = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::PrefixMethodNeedsArgumentList { source: error_source, tree_index, name })
                if error_source == source && tree_index == tree.index()
                    && typer.store().names.resolve(name.text()) == "unary_!"
        ));
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn prefix_generic_member_without_result_inference_is_deferred() {
        let source_text = "class Box { def unary_![A]: A = ??? }; class Use { def use(value: Box): Int = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::PrefixPolymorphicDeferred { source: error_source, tree_index, name })
                if error_source == source && tree_index == tree.index()
                    && typer.store().names.resolve(name.text()) == "unary_!"
        ));
        assert!(typer.source_typed_index().get(source, tree).is_none());
    }

    #[test]
    fn prefix_expected_typing_and_repeated_typing_keep_exact_selection() {
        let source_text =
            "class Box { def unary_! : Int = 1 }; class Use { def use(value: Box): Int = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) = &parsed.ast.get(tree).kind
        else {
            panic!("expected source PrefixOp");
        };
        let expected_position = parsed.ast.get(tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        let typed = typer
            .type_expression_expected(tree, context, definitions.int)
            .unwrap();
        let typed_nodes = typer.typed_ast().iter().count();
        let source_mappings = typer.source_typed_index().len();
        assert_eq!(typer.typed_ast().get(typed).position, expected_position);
        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Select(_)
        ));
        assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
        assert!(
            typer
                .source_typed_index()
                .get(source, prefix.operand)
                .is_some()
        );
        assert_eq!(typer.type_expression(tree, context).unwrap(), typed);
        assert_eq!(typer.typed_ast().iter().count(), typed_nodes);
        assert_eq!(typer.source_typed_index().len(), source_mappings);
        assert!(matches!(
            typer.type_expression_expected(tree, context, definitions.boolean),
            Err(TyperError::ExpectedExpressionTypeMismatch { source: error_source, tree_index, actual, expected })
                if error_source == source && tree_index == tree.index()
                    && actual == definitions.int && expected == definitions.boolean
        ));
        assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
    }

    #[test]
    fn prefix_lookup_sees_source_import_and_alias_receivers() {
        let source_text = "package lib { class Box { def unary_! : Int = 1 } }; package app { import lib.Box; class Use { type Alias = Box; def imported(value: Box): Int = !value; def aliased(value: Alias): Int = !value } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let member = method_symbol(&parsed, &store, &index, source, "unary_!");
        let methods = ["imported", "aliased"]
            .map(|name| method_definition_and_rhs(&parsed, &store, &index, source, name));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (owner, tree) in methods {
            let context = typer.expression_context_for(owner).unwrap();
            let typed = typer.type_expression(tree, context).unwrap();
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(typed).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == member
            ));
            assert_eq!(
                typer
                    .widen_expression_type(typer.typed_ast().get(typed).ty)
                    .unwrap(),
                definitions.int
            );
        }
    }

    #[test]
    fn failed_prefix_lookup_rolls_back_source_alias_completion() {
        let source_text =
            "class Box; class Use { type Alias = Box; def use(value: Alias): Box = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::MemberNotFound { tree_index, .. }) if tree_index == tree.index()
        ));
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    #[test]
    fn prefix_lookup_does_not_recomplete_source_alias_in_error_state() {
        let source_text = "class Box { def unary_! : Box = this }; class Use { type Alias = Box; def use(value: Alias): Box = !value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        store.symbols.set_info(alias, SymbolInfo::Error);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::MemberLookup(error))
                if matches!(*error,
                    MemberLookupError::TypeNormalization(
                        crate::types::TypeNormalizeError::AliasInfoIncomplete {
                            symbol,
                            state: crate::types::SymbolInfoState::Error,
                        }
                    ) if symbol == alias)
        ));
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Error);
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    #[test]
    fn nested_prefix_expressions_keep_both_source_mappings() {
        let source_text = "class Box { def unary_! : Box = this }; class Use { def use(value: Box): Box = !(!value) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(outer)) = &parsed.ast.get(tree).kind
        else {
            panic!("expected outer PrefixOp");
        };
        let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) =
            &parsed.ast.get(outer.operand).kind
        else {
            panic!("expected parenthesized inner prefix");
        };
        let inner = parens.inner;
        assert!(matches!(
            parsed.ast.get(inner).kind,
            TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_))
        ));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        let typed = typer.type_expression(tree, context).unwrap();
        let TreeKind::Select(outer_selection) = &typer.typed_ast().get(typed).kind else {
            panic!("outer prefix should select unary_!");
        };
        assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
        assert_eq!(
            typer.source_typed_index().get(source, inner),
            Some(outer_selection.qualifier)
        );
        assert_eq!(
            typer.typed_ast().get(typed).position,
            parsed.ast.get(tree).position
        );
        assert_eq!(
            typer.typed_ast().get(outer_selection.qualifier).position,
            parsed.ast.get(inner).position
        );
    }

    #[test]
    fn source_annotation_constructor_shapes_are_explicit() {
        let source_text = "package scala.annotation { abstract class Annotation }; class unchecked extends scala.annotation.Annotation; class EmptyAnnot extends scala.annotation.Annotation; class Annot(val n: Int) extends scala.annotation.Annotation; class Multi(val first: Int, val second: Int) extends scala.annotation.Annotation; class Triple(val first: Int, val second: Int, val third: Int) extends scala.annotation.Annotation; class Use { def bare(x: Int): Int = x: @unchecked; def empty(x: Int): Int = x: @EmptyAnnot(); def positional(x: Int): Int = x: @Annot(1); def named(x: Int): Int = x: @Annot(n = 1); def mixed(x: Int): Int = x: @Multi(1, second = 2); def mixedAfterNamed(x: Int): Int = x: @Multi(first = 1, 2); def reorderedNamedThenPositional(x: Int): Int = x: @Triple(second = 2, first = 1, 3) }";
        let (parsed, store, _, _, index, source) = parse_and_name(source_text);

        for (method, expected_args) in [
            ("bare", 0),
            ("empty", 0),
            ("positional", 1),
            ("named", 1),
            ("mixed", 2),
            ("mixedAfterNamed", 2),
            ("reorderedNamedThenPositional", 3),
        ] {
            let (_, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method);
            let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
                panic!("{method} should retain an Annotated source node");
            };
            let TreeKind::Apply(application) = &parsed.ast.get(annotated.annotation).kind else {
                panic!("{method} annotation should be an Apply");
            };
            assert_eq!(application.args.len(), expected_args);
            let TreeKind::Select(constructor) = &parsed.ast.get(application.function).kind else {
                panic!("{method} annotation should select a constructor");
            };
            assert_eq!(store.names.resolve(constructor.name.text()), "<init>");
            let TreeKind::New(_) = &parsed.ast.get(constructor.qualifier).kind else {
                panic!("{method} annotation should wrap its type in New");
            };
            if method == "named" || method == "mixed" || method == "mixedAfterNamed" {
                let named_index = if method == "mixedAfterNamed" {
                    0
                } else {
                    application.args.len() - 1
                };
                let TreeKind::NamedArg(named) = &parsed.ast.get(application.args[named_index]).kind
                else {
                    panic!("{method} should preserve its named argument");
                };
                assert_eq!(
                    store.names.resolve(named.name.text()),
                    match method {
                        "named" => "n",
                        "mixed" => "second",
                        _ => "first",
                    }
                );
            }
        }
    }

    #[test]
    fn source_annotation_projection_preserves_class_and_constant_arguments() {
        let source_text = "package scala.annotation { abstract class Annotation }; class unchecked extends scala.annotation.Annotation; class EmptyAnnot extends scala.annotation.Annotation; class Annot(val n: Int) extends scala.annotation.Annotation; class Multi(val first: Int, val second: Int) extends scala.annotation.Annotation; class Triple(val first: Int, val second: Int, val third: Int) extends scala.annotation.Annotation; class Use { def bare(x: Int): Int = x: @unchecked; def empty(x: Int): Int = x: @EmptyAnnot(); def positional(x: Int): Int = x: @Annot(1); def named(x: Int): Int = x: @Annot(n = 1); def mixed(x: Int): Int = x: @Multi(1, second = 2); def mixedAfterNamed(x: Int): Int = x: @Multi(first = 1, 2); def reorderedNamedThenPositional(x: Int): Int = x: @Triple(second = 2, first = 1, 3) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let cases = [
            ("bare", "unchecked", 0),
            ("empty", "EmptyAnnot", 0),
            ("positional", "Annot", 1),
            ("named", "Annot", 1),
            ("mixed", "Multi", 2),
            ("mixedAfterNamed", "Multi", 2),
            ("reorderedNamedThenPositional", "Triple", 3),
        ];
        let methods = cases.map(|(method, class, count)| {
            let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method);
            let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
                panic!("expected annotated source expression");
            };
            (
                method,
                owner,
                annotated.annotation,
                class_symbol(&parsed, &store, &index, source, class),
                count,
            )
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (method, owner, annotation_tree, class, expected_count) in methods {
            let context = typer.expression_context_for(owner).unwrap();
            let id = typer
                .type_source_annotation(annotation_tree, context)
                .unwrap();
            let annotation = typer.store().annotations.get(id);
            assert_eq!(typer.store().annotation_class(annotation), Some(class));
            assert_eq!(annotation.tree, None);
            let dotty_core::types::AnnotationArguments::Known(arguments) = &annotation.arguments
            else {
                panic!("source arguments should be known");
            };
            assert_eq!(arguments.len(), expected_count);
            for (index, argument) in arguments.iter().enumerate() {
                let dotty_core::types::AnnotationValue::Constant(dotty_core::Constant::Int(value)) =
                    &argument.value
                else {
                    panic!("source literal should remain an integer constant");
                };
                let expected_value = match (method, index) {
                    ("mixed", 1) | ("mixedAfterNamed", 1) => 2,
                    ("reorderedNamedThenPositional", 0) => 2,
                    ("reorderedNamedThenPositional", 2) => 3,
                    _ => 1,
                };
                assert_eq!(*value, expected_value);
                let expected_name = match (method, index) {
                    ("named", 0) => Some("n"),
                    ("mixed", 1) => Some("second"),
                    ("mixedAfterNamed", 0) => Some("first"),
                    ("reorderedNamedThenPositional", 0) => Some("second"),
                    ("reorderedNamedThenPositional", 1) => Some("first"),
                    _ => None,
                };
                assert_eq!(
                    argument
                        .name
                        .map(|name| typer.store().names.resolve(name.as_name().text())),
                    expected_name
                );
            }
            assert_eq!(
                typer
                    .type_source_annotation(annotation_tree, context)
                    .unwrap(),
                id
            );
        }
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    fn source_annotation_case(
        spelling: &str,
    ) -> (
        dotty_parser::ParseResult,
        SemanticStore,
        Packages,
        Definitions,
        SourceSemanticIndex,
        SourceId,
    ) {
        let source_text = format!(
            "package scala.annotation {{ abstract class Annotation }}; class Annot(val n: Int) extends scala.annotation.Annotation; class Plain; class Use {{ def use(x: Int): Int = x: @{spelling} }}"
        );
        parse_and_name(&source_text)
    }

    #[test]
    fn source_annotation_projection_rejects_nonconstant_and_duplicate_arguments() {
        for (spelling, duplicate) in [("Annot(x)", false), ("Annot(n = 1, n = 2)", true)] {
            let (parsed, mut store, packages, definitions, index, source) =
                source_annotation_case(spelling);
            let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
            let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
                panic!("expected annotated expression");
            };
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let context = typer.expression_context_for(owner).unwrap();
            let checkpoint = typer.store().checkpoint();

            let error = typer
                .type_source_annotation(annotated.annotation, context)
                .unwrap_err();
            assert!(
                if duplicate {
                    matches!(error, TyperError::SourceAnnotationDuplicateNamedArgument { source: error_source, name, .. }
                        if error_source == source && typer.store().names.resolve(name.as_name().text()) == "n")
                } else {
                    matches!(error, TyperError::SourceAnnotationArgumentNotConstant { source: error_source, .. }
                        if error_source == source)
                },
                "{spelling}: {error:?}"
            );
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert!(typer.source_annotations.is_empty());
            assert_eq!(typer.source_typed_index().len(), 0);
        }
    }

    #[test]
    fn source_annotation_projection_checks_class_and_constructor_boundaries() {
        for spelling in [
            "Plain()",
            "Missing()",
            "Annot()",
            "Annot(true)",
            "Annot(m = 1)",
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                source_annotation_case(spelling);
            let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
            let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
                panic!("expected annotated expression");
            };
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let context = typer.expression_context_for(owner).unwrap();
            let checkpoint = typer.store().checkpoint();

            let error = typer
                .type_source_annotation(annotated.annotation, context)
                .unwrap_err();
            assert!(
                match spelling {
                    "Plain()" =>
                        matches!(error, TyperError::SourceAnnotationNotAnnotationClass { .. }),
                    "Missing()" => matches!(error, TyperError::TypeNameNotFound { .. }),
                    "Annot()" => matches!(
                        error,
                        TyperError::SourceAnnotationConstructorDeferred { .. }
                    ),
                    _ => matches!(
                        error,
                        TyperError::SourceAnnotationConstructorArgumentMismatch { .. }
                    ),
                },
                "{spelling}: {error:?}"
            );
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert!(typer.source_annotations.is_empty());
            assert_eq!(typer.typed_ast().iter().count(), 0);
        }
    }

    #[test]
    fn source_annotation_projection_defers_curried_constructors() {
        let source_text = "package scala.annotation { abstract class Annotation }; class Curried(val first: Int)(val second: Int) extends scala.annotation.Annotation; class Use { def use(x: Int): Int = x: @Curried(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
            panic!("expected annotated expression");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_source_annotation(annotated.annotation, context),
            Err(TyperError::SourceAnnotationConstructorDeferred { source: error_source, .. })
                if error_source == source
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.source_annotations.is_empty());
    }

    #[test]
    fn annotated_term_expressions_preserve_typed_shape_and_result_types() {
        let source_text = "package scala.annotation { abstract class Annotation }; class TermAnnotation extends scala.annotation.Annotation; object StableTerm; object Use { val stableAlias: StableTerm.type = StableTerm; def literal: Int = 1: @TermAnnotation; def stableReference = stableAlias: @TermAnnotation; def unstableParameter(value: Int): Int = value: @TermAnnotation; def nested(value: Int) = (value: @TermAnnotation): @TermAnnotation }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let annotation_class = class_symbol(&parsed, &store, &index, source, "TermAnnotation");
        let (stable_alias, _, _) =
            val_definition_and_rhs(&parsed, &store, &index, source, "stableAlias");
        let cases = ["literal", "stableReference", "unstableParameter", "nested"];
        let methods = cases.map(|method| {
            let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method);
            let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
                panic!("{method} should be a source Annotated term");
            };
            (method, owner, rhs, annotated.expr)
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (method, owner, rhs, source_expr) in methods {
            let context = typer.expression_context_for(owner).unwrap();
            let typed = typer.type_expression(rhs, context).unwrap();
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
            assert_eq!(
                typer.source_typed_index().get(source, source_expr),
                match &typer.typed_ast().get(typed).kind {
                    TreeKind::Typed(wrapper) => Some(wrapper.expr),
                    other => panic!("{method} should lower to Typed, got {other:?}"),
                }
            );
            let TreeKind::Typed(wrapper) = &typer.typed_ast().get(typed).kind else {
                panic!("{method} should lower to an ordinary Typed wrapper");
            };
            assert_eq!(
                typer.typed_ast().get(wrapper.tpt).ty,
                typer.typed_ast().get(typed).ty
            );
            let Type::Annotated {
                underlying,
                annotation,
            } = typer.store().types.get(typer.typed_ast().get(typed).ty)
            else {
                panic!("{method} result type should retain its annotation");
            };
            assert_eq!(
                typer
                    .store()
                    .annotation_class(typer.store().annotations.get(*annotation)),
                Some(annotation_class)
            );
            match method {
                "literal" | "unstableParameter" => assert_eq!(*underlying, definitions.int),
                "stableReference" => {
                    assert_eq!(*underlying, typer.typed_ast().get(wrapper.expr).ty);
                    assert!(matches!(
                        typer.store().types.get(*underlying),
                        Type::TermRef {
                            target: TermRefTarget::Symbol(symbol),
                            ..
                        } if *symbol == stable_alias
                    ));
                }
                "nested" => {
                    let Type::Annotated {
                        annotation: inner_annotation,
                        ..
                    } = typer.store().types.get(*underlying)
                    else {
                        panic!("nested annotations should retain the inner wrapper");
                    };
                    assert_ne!(*annotation, *inner_annotation);
                    let TreeKind::Annotated(outer_source) = &typer.arena.get(rhs).kind else {
                        panic!("outer source tree should remain annotated");
                    };
                    let inner_source_tree = match &typer.arena.get(source_expr).kind {
                        TreeKind::Annotated(_) => source_expr,
                        TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => parens.inner,
                        other => panic!("expected nested source annotation, got {other:?}"),
                    };
                    let TreeKind::Annotated(inner_source) =
                        &typer.arena.get(inner_source_tree).kind
                    else {
                        panic!("inner source tree should remain annotated");
                    };
                    assert_eq!(
                        typer.source_annotations.get(&outer_source.annotation),
                        Some(annotation)
                    );
                    assert_eq!(
                        typer.source_annotations.get(&inner_source.annotation),
                        Some(inner_annotation)
                    );
                }
                _ => unreachable!(),
            }
            let typed_count = typer.typed_ast().iter().count();
            assert_eq!(typer.type_expression(rhs, context).unwrap(), typed);
            assert_eq!(typer.typed_ast().iter().count(), typed_count);
            if method == "literal" {
                assert_eq!(
                    typer
                        .type_expression_expected(rhs, context, definitions.int)
                        .unwrap(),
                    typed
                );
            }
        }
    }

    #[test]
    fn annotated_match_selector_and_method_argument_use_shared_annotation_identity() {
        let source_text = "package scala.annotation { abstract class Annotation }; package scala { class unchecked extends scala.annotation.Annotation }; class Use { import scala.unchecked; def take(value: Int): Int = value; def choose: Int = (1: @unchecked) match { case _ => take(1: @unchecked) } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let unchecked = class_symbol(&parsed, &store, &index, source, "unchecked");
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let TreeKind::Match(match_expr) = &parsed.ast.get(rhs).kind else {
            panic!(
                "annotated selector should reach Match typing, got {:?}",
                parsed.ast.get(rhs).kind
            );
        };
        let TreeKind::PhaseSpecific(UntypedNode::Parens(selector_parens)) =
            &parsed.ast.get(match_expr.selector).kind
        else {
            panic!("expected parenthesized annotated selector");
        };
        let selector_tree = selector_parens.inner;
        let TreeKind::Annotated(selector_annotation) = &parsed.ast.get(selector_tree).kind else {
            panic!("expected annotated Match selector");
        };
        let context = ExpressionContext {
            lexical: index.declaration_context_of(owner).unwrap(),
            owner,
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
        let typed_match = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Match(typed_match_expr) = &typer.typed_ast().get(typed_match).kind else {
            panic!("annotated selector should lower to a typed Match");
        };
        let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(typed_match_expr.cases[0]).kind
        else {
            panic!("Match should retain its typed CaseDef");
        };

        let selector_annotation_id = typer
            .source_annotations
            .get(&selector_annotation.annotation)
            .copied()
            .expect("selector annotation should be projected");
        let selector_typed = typer
            .source_typed_index()
            .get(source, selector_tree)
            .expect("annotated selector should have a typed mapping");
        let selector_type = typer.typed_ast().get(selector_typed).ty;
        assert_eq!(
            typer.typed_ast().get(typed_case.pattern).ty,
            definitions.int,
            "pattern adaptation should look through the annotation wrapper"
        );
        assert!(matches!(
            typer.store().types.get(selector_type),
            Type::Annotated { annotation, .. } if *annotation == selector_annotation_id
        ));
        assert!(
            typer
                .store()
                .has_annotation(selector_type, &["scala", "unchecked"])
        );
        assert_eq!(
            typer
                .store()
                .annotation_class(typer.store().annotations.get(selector_annotation_id)),
            Some(unchecked)
        );

        let call_annotation_tree = parsed
            .ast
            .iter()
            .find_map(|(_, node)| {
                let TreeKind::Annotated(annotated) = &node.kind else {
                    return None;
                };
                (annotated.annotation != selector_annotation.annotation)
                    .then_some(annotated.annotation)
            })
            .expect("method argument annotation should be present");
        let call_annotation_id = typer
            .source_annotations
            .get(&call_annotation_tree)
            .copied()
            .expect("method argument annotation should be projected");
        assert_ne!(selector_annotation_id, call_annotation_id);
    }

    #[test]
    fn annotated_local_assignment_and_branch_flows_keep_expression_support() {
        let source_text = "package scala.annotation { abstract class Annotation }; class TermAnnotation extends scala.annotation.Annotation; class Use { var slot: Int = 0; def inferred = { val local = 1: @TermAnnotation; val matched = (2: @TermAnnotation) match { case _ => 3 }; local }; def assigned = { slot = 2: @TermAnnotation; slot }; def joined = if (true) 3: @TermAnnotation else 4 }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let methods = ["inferred", "assigned", "joined"]
            .map(|method| method_definition_and_rhs(&parsed, &store, &index, source, method));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        for (method, (owner, rhs)) in ["inferred", "assigned", "joined"].into_iter().zip(methods) {
            let context = typer.expression_context_for(owner).unwrap();
            let typed = typer.type_expression(rhs, context);
            assert!(
                typed.is_ok(),
                "{method} should accept annotated expression: {typed:?}"
            );
        }
        assert_eq!(typer.source_annotations.len(), 4);
    }

    #[test]
    fn annotated_expression_projects_constant_arguments_and_focused_errors() {
        let source_text = "package scala.annotation { abstract class Annotation }; class TermAnnotation(val n: Int) extends scala.annotation.Annotation; class Use { def constant: Int = 1: @TermAnnotation(7); def unsupported(value: Int): Int = value: @TermAnnotation(value) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (constant_owner, constant_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "constant");
        let (unsupported_owner, unsupported_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "unsupported");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let constant_context = typer.expression_context_for(constant_owner).unwrap();
        let constant_typed = typer
            .type_expression(constant_rhs, constant_context)
            .unwrap();
        let TreeKind::Annotated(constant_source) = &typer.arena.get(constant_rhs).kind else {
            panic!("constant method should retain its source annotation");
        };
        let annotation_id = typer
            .source_annotations
            .get(&constant_source.annotation)
            .copied()
            .expect("constant annotation should be projected");
        let annotation = typer.store().annotations.get(annotation_id);
        assert!(matches!(
            &annotation.arguments,
            dotty_core::types::AnnotationArguments::Known(arguments)
                if arguments.len() == 1
                    && matches!(
                        arguments[0].value,
                        dotty_core::types::AnnotationValue::Constant(dotty_core::Constant::Int(7))
                    )
        ));
        assert_eq!(annotation.tree, None);
        let TreeKind::Typed(wrapper) = &typer.typed_ast().get(constant_typed).kind else {
            panic!("constant annotation should reify to a Typed wrapper");
        };
        assert_eq!(
            typer.typed_ast().get(constant_typed).position,
            typer.arena.get(constant_rhs).position
        );
        assert_eq!(
            typer.typed_ast().get(wrapper.tpt).position,
            typer.arena.get(constant_source.annotation).position
        );
        assert_eq!(
            typer.typed_ast().get(wrapper.expr).position,
            typer.arena.get(constant_source.expr).position
        );

        let unsupported_context = typer.expression_context_for(unsupported_owner).unwrap();
        let error = typer
            .type_expression(unsupported_rhs, unsupported_context)
            .unwrap_err();
        assert!(
            matches!(error, TyperError::SourceAnnotationArgumentNotConstant { source: error_source, .. }
                if error_source == source),
            "unsupported annotation arguments should stay focused: {error:?}"
        );
    }

    #[test]
    fn annotated_term_annotation_failure_precedes_child_typing() {
        let source_text = "package scala.annotation { abstract class Annotation }; class Plain; class Use { def invalid: Int = missing: @Plain }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "invalid");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(owner).unwrap(),
            owner,
            local_scopes: None,
        };
        let checkpoint = store.checkpoint();
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
                result,
                Err(TyperError::SourceAnnotationNotAnnotationClass { source: error_source, .. })
                    if error_source == source
            ),
            "{result:?}"
        );
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.source_annotations.is_empty());
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    #[test]
    fn annotated_term_expected_type_failure_rolls_back_child_and_annotation() {
        let source_text = "package scala.annotation { abstract class Annotation }; class TermAnnotation extends scala.annotation.Annotation; class Use { def invalid: Boolean = 1: @TermAnnotation }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "invalid");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(owner).unwrap(),
            owner,
            local_scopes: None,
        };
        let checkpoint = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let result = typer.type_expression_expected(rhs, context, definitions.boolean);
        assert!(
            matches!(
                result,
            Err(TyperError::ExpectedExpressionTypeMismatch {
                source: error_source,
                expected,
                ..
            }) if error_source == source && expected == definitions.boolean
            ),
            "{result:?}"
        );
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.source_annotations.is_empty());
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    #[test]
    fn source_annotation_class_without_canonical_base_is_deferred() {
        let source_text = "class Annot; class Use { def use(x: Int): Int = x: @Annot() }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
            panic!("expected annotated expression");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();
        let checkpoint = typer.store().checkpoint();

        assert!(matches!(
            typer.type_source_annotation(annotated.annotation, context),
            Err(TyperError::SourceAnnotationClassDeferred { source: error_source, .. })
                if error_source == source
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.source_annotations.is_empty());
    }

    #[test]
    fn source_annotation_ambiguous_import_is_explicit() {
        let source_text = "package scala.annotation { abstract class Annotation }; package a { class Annot extends scala.annotation.Annotation }; package b { class Annot extends scala.annotation.Annotation }; class Use { import a.*; import b.*; def use(x: Int): Int = x: @Annot() }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
            panic!("expected annotated expression");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        assert!(matches!(
            typer.type_source_annotation(annotated.annotation, context),
            Err(TyperError::AmbiguousTypeName { .. })
        ));
        assert!(typer.source_annotations.is_empty());
    }

    #[test]
    fn source_annotation_allocation_rolls_back_with_enclosing_expression() {
        let (parsed, mut store, packages, definitions, index, source) =
            source_annotation_case("Annot(1)");
        let (owner, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Annotated(annotated) = &parsed.ast.get(rhs).kind else {
            panic!("expected annotated expression");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();
        let checkpoint = typer.store().checkpoint();
        let mut allocated = None;

        let result: Result<(), TyperError> =
            typer.run_expression_transaction(|typer, info_journal, new_mappings| {
                allocated = Some(typer.type_source_annotation_inner(
                    annotated.annotation,
                    context,
                    info_journal,
                    new_mappings,
                )?);
                Err(TyperError::ExpectedExpressionTypeMismatch {
                    source,
                    tree_index: rhs.index(),
                    actual: definitions.int,
                    expected: definitions.boolean,
                })
            });

        assert!(matches!(
            result,
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));
        assert!(
            typer
                .store()
                .annotations
                .try_get(allocated.unwrap())
                .is_none()
        );
        assert!(typer.source_annotations.is_empty());
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(typer.source_typed_index().len(), 0);

        let retried = typer
            .type_source_annotation(annotated.annotation, context)
            .unwrap();
        assert_eq!(retried, allocated.unwrap());
    }

    #[test]
    fn prefix_rejects_an_unexpected_operator_spelling() {
        let source_text = "class Box { def unary_! : Box = this }; class Use { def use(value: Box): Box = !value }";
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name(source_text);
        let (owner, tree) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let unexpected = Name::new(store.names.intern("?"), Namespace::Term);
        let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) =
            &mut parsed.ast.get_mut(tree).kind
        else {
            panic!("expected source PrefixOp");
        };
        prefix.op = unexpected;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(owner).unwrap();

        assert!(matches!(
            typer.type_expression(tree, context),
            Err(TyperError::UnsupportedPrefixOperator { source: error_source, tree_index, operator })
                if error_source == source && tree_index == tree.index() && operator == unexpected
        ));
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn infix_overload_and_generic_method_use_selected_member_resolution() {
        let source_text = "class Box { def combine(value: Int): Int = value; def combine(value: Box): Box = value; def echo[A](value: A): A = value }; class Use { def overloaded(left: Box, right: Box): Box = left `combine` right; def generic(left: Box, right: Box): Box = left `echo` right }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (overloaded_method, overloaded_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "overloaded");
        let (generic_method, generic_tree) =
            method_definition_and_rhs(&parsed, &store, &index, source, "generic");
        let echo = method_symbol(&parsed, &store, &index, source, "echo");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let overloaded_context = typer.expression_context_for(overloaded_method).unwrap();
        let overloaded = typer
            .type_expression(overloaded_tree, overloaded_context)
            .unwrap();
        let TreeKind::Apply(overloaded_application) = &typer.typed_ast().get(overloaded).kind
        else {
            panic!("expected overload infix lowering to produce an Apply")
        };
        let selected_function = overloaded_application.function;
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(selected_function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected overload, found {other:?}"),
        };
        let selected_info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(selected_info),
            Type::Method(method)
                if matches!(typer.store().types.get(method.params[0].ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }
                        if *class == class_symbol(&parsed, typer.store(), &index, source, "Box"))
        ));
        let generic_context = typer.expression_context_for(generic_method).unwrap();
        let generic = typer
            .type_expression(generic_tree, generic_context)
            .unwrap();
        let TreeKind::Apply(generic_application) = &typer.typed_ast().get(generic).kind else {
            panic!("expected generic infix lowering to produce an Apply")
        };
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(generic_infix)) =
            &parsed.ast.get(generic_tree).kind
        else {
            panic!("generic source should be an infix expression")
        };
        assert_eq!(
            typer.source_typed_index().get(source, generic_infix.right),
            Some(generic_application.args[0])
        );
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(generic_application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == echo
        ));
        assert_eq!(
            type_symbol(typer.store(), typer.typed_ast().get(generic).ty),
            class_symbol(&parsed, typer.store(), &index, source, "Box")
        );
    }

    #[test]
    fn nested_infix_calls_preserve_left_associative_application_shape() {
        let source_text = "class Box { def combine(other: Box): Box = this }; class Use { def nested(left: Box, middle: Box, right: Box): Box = left `combine` middle `combine` right }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "nested");
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

        let TreeKind::Apply(outer) = &typer.typed_ast().get(typed).kind else {
            panic!("outer infix operator should produce an Apply")
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(outer.function).kind else {
            panic!("outer infix application should select its method")
        };
        assert!(matches!(
            typer.typed_ast().get(selection.qualifier).kind,
            TreeKind::Apply(_)
        ));
    }

    #[test]
    fn right_associative_infix_is_reported_as_deferred() {
        let source_text = "class Box { def +:(other: Box): Box = this }; class Use { def right(left: Box, right: Box): Box = left +: right }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "right");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let context = typer.expression_context_for(method).unwrap();
        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::RightAssociativeInfixDeferred {
                source: error_source,
                tree_index,
                ..
            }) if error_source == source && tree_index == rhs.index()
        ));
    }

    #[test]
    fn failed_infix_argument_application_rolls_back_typed_state() {
        let source_text = "class Box { def combine(value: Int): Int = value }; class Use { def bad(left: Box, right: Box): Int = left `combine` right }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "bad");
        let checkpoint = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let context = typer.expression_context_for(method).unwrap();
        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::OverloadApplicationNoApplicable { .. })
        ));
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    #[test]
    fn expected_expression_type_accepts_a_supported_subtype() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent; class Child extends Parent; class C { def use(child: Child): Parent = child }",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer.complete_symbol(parent).unwrap();
        typer.complete_symbol(child).unwrap();
        let method_signature = typer.complete_symbol(method).unwrap();
        let Type::Method(signature) = typer.store().types.get(method_signature) else {
            panic!("expected a method signature");
        };
        let expected = signature.result;
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer
            .type_expression_expected(rhs, context, expected)
            .unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { .. }
        ));
    }

    #[test]
    fn incompatible_expected_expression_type_rolls_back_expression_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1 }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
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
            typer.type_expression_expected(rhs, context, definitions.boolean),
            Err(TyperError::ExpectedExpressionTypeMismatch {
                source: actual_source,
                tree_index,
                actual,
                expected,
            }) if actual_source == source
                && tree_index == rhs.index()
                && actual == definitions.int
                && expected == definitions.boolean
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.local_symbols.is_empty());
        assert!(typer.initializing_local_symbols.is_empty());
    }

    #[test]
    fn unsupported_expected_type_relation_has_a_focused_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1 }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let expected = store.types.alloc(Type::And {
            left: definitions.int,
            right: definitions.boolean,
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression_expected(rhs, context, expected),
            Err(TyperError::ExpectedExpressionConformanceUnsupported {
                source: actual_source,
                tree_index,
                expected: actual_expected,
                ..
            }) if actual_source == source
                && tree_index == rhs.index()
                && actual_expected == expected
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
    }

    #[test]
    fn source_type_ascription_builds_typed_expr_and_preserves_literal_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1: Int }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Typed(source_ascription) = &parsed.ast.get(rhs).kind else {
            panic!("method RHS should be a source type ascription");
        };
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let source_position = parsed.ast.get(rhs).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).position, source_position);
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let TreeKind::Typed(ascription) = &typer.typed_ast().get(typed).kind else {
            panic!("source ascription should produce a typed TypedExpr");
        };
        assert_eq!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(ascription.expr).ty),
            &Type::Constant(dotty_core::Constant::Int(1))
        );
        assert_eq!(typer.typed_ast().get(ascription.tpt).ty, definitions.int);
        assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
        assert!(
            typer
                .source_typed_index()
                .get(source, source_ascription.expr)
                .is_some()
        );
        assert!(
            typer
                .source_typed_index()
                .get(source, source_ascription.tpt)
                .is_some()
        );
    }

    #[test]
    fn source_type_ascription_accepts_a_subtype_and_preserves_term_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent; class Child extends Parent; class C { def use(child: Child): Parent = child: Parent }",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = val_symbol(&parsed, &store, &index, source, "child").0;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer.complete_symbol(parent).unwrap();
        typer.complete_symbol(child).unwrap();
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let Type::TypeRef {
            target: TypeRefTarget::Symbol(parent_target),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("ascription should carry the projected parent type");
        };
        assert_eq!(*parent_target, parent);
        let TreeKind::Typed(ascription) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed ascription");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(ascription.expr).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == parameter
        ));
    }

    #[test]
    fn source_type_ascription_resolves_method_type_parameters() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use[A](value: A): A = value: A }");
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

        let TreeKind::Typed(ascription) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed ascription");
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(ascription.tpt).ty),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if typer.store().symbols.get(*symbol).kind == SymbolKind::TypeParameter
        ));
        assert_eq!(
            typer.typed_ast().get(typed).ty,
            typer.typed_ast().get(ascription.tpt).ty
        );
    }

    #[test]
    fn source_type_ascription_types_inside_a_block() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = { val value = 1; value: Int } }");
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

        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should remain a typed block");
        };
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        assert!(matches!(
            typer.typed_ast().get(block.expr).kind,
            TreeKind::Typed(_)
        ));
    }

    #[test]
    fn failed_source_type_ascription_rolls_back_expression_and_type_tree_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1: Boolean }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
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
            typer.type_expression(rhs, context),
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.type_index.type_at(source, rhs).is_none());
    }

    #[test]
    fn source_type_ascription_reports_unprojectable_type_tree_atomically() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = 1: value }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Typed(ascription) = &parsed.ast.get(rhs).kind else {
            panic!("method RHS should be a source type ascription");
        };
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        let before = store.checkpoint();
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
                result,
                Err(TyperError::TypeNameNotFound {
                    source: actual_source,
                    tree_index,
                    name,
                    position: Some(_),
                }) if actual_source == source
                    && tree_index == ascription.tpt.index()
                    && typer.store().names.resolve(name.text()) == "value"
            ),
            "unexpected ascription typing result: {result:?}"
        );
        assert_eq!(typer.store().checkpoint(), before);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.type_index.type_at(source, rhs).is_none());
    }

    #[test]
    fn assignment_to_mutable_local_uses_unit_result_and_keeps_exact_symbol() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Unit = { var value = 1; value = 2 } }");
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

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.unit);
        let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
            panic!("method body should remain a block");
        };
        let assignment = block.expr;
        let TreeKind::Assign(assign) = &typer.typed_ast().get(assignment).kind else {
            panic!("expected a typed assignment");
        };
        let target = match typer
            .store()
            .types
            .get(typer.typed_ast().get(assign.lhs).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a direct local reference, got {other:?}"),
        };
        let target_symbol = typer.store().symbols.get(target);
        assert_eq!(target_symbol.kind, SymbolKind::Local);
        assert!(target_symbol.flags.contains(SymbolFlags::MUTABLE));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(assign.rhs).ty),
            Type::Constant(dotty_core::Constant::Int(2))
        ));
    }

    #[test]
    fn assignment_rejects_immutable_local_and_parameter() {
        for (source_text, assigned_name) in [
            (
                "class C { def use: Unit = { val value = 1; value = 2 } }",
                "value",
            ),
            ("class C { def use(value: Int): Unit = value = 2 }", "value"),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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
                matches!(result, Err(TyperError::AssignmentTargetImmutable { .. })),
                "assignment to `{assigned_name}` should reject immutable target: {result:?}"
            );
        }
    }

    #[test]
    fn assignment_to_generic_mutable_field_uses_receiver_adapted_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A] { var value: A }; class C { def use(box: Box[Int]): Unit = box.value = 1 }",
        );
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
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

        let (lhs, rhs) = match &typer.typed_ast().get(typed).kind {
            TreeKind::Assign(assignment) => (assignment.lhs, assignment.rhs),
            _ => panic!("expected a typed assignment"),
        };
        let qualifier = match &typer.typed_ast().get(lhs).kind {
            TreeKind::Select(selection) => selection.qualifier,
            _ => panic!("field assignment should retain its typed selection"),
        };
        let lhs_type = typer.typed_ast().get(lhs).ty;
        assert!(matches!(
            typer
                .store()
                .types
                .get(lhs_type),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == field
        ));
        let adapted_lhs_type = typer.widen_expression_type(lhs_type).unwrap();
        assert_eq!(adapted_lhs_type, definitions.int);
        assert!(typer.typed_ast().get(qualifier).position.is_some());
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.unit);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(rhs).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
    }

    #[test]
    fn assignment_rejects_immutable_source_field() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1; def use: Unit = value = 2 }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (field, _) = val_symbol(&parsed, &store, &index, source, "value");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::AssignmentTargetImmutable { target, .. }) if target == field
        ));
    }

    #[test]
    fn assignment_rejects_method_reference_targets_by_kind() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def value: Int = 1; def use: Unit = value = 2 }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (target, _) = method_definition_and_rhs(&parsed, &store, &index, source, "value");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::AssignmentTargetKindUnsupported {
                target: actual,
                kind: SymbolKind::Method,
                ..
            }) if actual == target
        ));
    }

    #[test]
    fn assignment_rejects_a_non_reference_lhs_explicitly() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Unit = 1 }");
        let (method, literal) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let position = parsed.ast.get(literal).position;
        let assignment = parsed.ast.alloc(Tree {
            kind: TreeKind::Assign(dotty_core::ast::Assign {
                lhs: literal,
                rhs: literal,
            }),
            position,
            ty: (),
        });
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

        let result = typer.type_expression(assignment, context);
        assert!(
            matches!(
            result,
            Err(TyperError::AssignmentLhsNotAssignable {
                lhs_tree_index,
                ..
            }) if lhs_tree_index == literal.index()
            ),
            "unexpected assignment result: {result:?}"
        );
    }

    #[test]
    fn assignment_rhs_mismatch_rolls_back_all_expression_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Unit = { var value = 1; value = true } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
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
            typer.type_expression(rhs, context),
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.type_index.type_at(source, rhs).is_none());
    }

    #[test]
    fn if_expression_checks_boolean_and_joins_constant_branches_as_int() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def choose(flag: Boolean) = if flag then 1 else 2");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
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

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let (cond, then_branch, else_branch) = match &typer.typed_ast().get(typed).kind {
            TreeKind::If(if_expr) => (if_expr.cond, if_expr.then_branch, if_expr.else_branch),
            _ => panic!("expected a typed if expression"),
        };
        let condition_type = typer.typed_ast().get(cond).ty;
        assert_eq!(
            typer.widen_expression_type(condition_type).unwrap(),
            definitions.boolean
        );
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(then_branch).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(else_branch).ty),
            Type::Constant(dotty_core::Constant::Int(2))
        ));
    }

    #[test]
    fn if_expression_joins_subtype_and_supertype_branches_to_supertype() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent; class Child extends Parent; def choose(flag: Boolean, child: Child, parent: Parent) = if flag then child else parent",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer.complete_symbol(parent).unwrap();
        typer.complete_symbol(child).unwrap();
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("if join should choose the parent type");
        };
        assert_eq!(*symbol, parent);
    }

    #[test]
    fn if_expression_joins_unrelated_classes_into_a_union() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "trait Parent; class Left extends Parent; class Right extends Parent; def choose(flag: Boolean, left: Left, right: Right) = if flag then left else right",
        );
        let parent_class = class_symbol(&parsed, &store, &index, source, "Parent");
        let left_class = class_symbol(&parsed, &store, &index, source, "Left");
        let right_class = class_symbol(&parsed, &store, &index, source, "Right");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer.complete_symbol(parent_class).unwrap();
        typer.complete_symbol(left_class).unwrap();
        typer.complete_symbol(right_class).unwrap();
        let context = typer.expression_context_for(method).unwrap();

        let typed = typer.type_expression(rhs, context).unwrap();

        let joined_type = typer.typed_ast().get(typed).ty;
        let (left, right) = match typer.store().types.get(joined_type) {
            Type::Or { left, right } => (*left, *right),
            _ => panic!("unrelated branch types should be joined as a union"),
        };
        assert!(typer.is_subtype(left, joined_type).unwrap());
        assert!(typer.is_subtype(right, joined_type).unwrap());
        let parent_prefix = typer.type_symbol_prefix(parent_class);
        let parent_type = typer
            .store
            .types
            .alloc(Type::type_ref(parent_prefix, parent_class));
        assert!(typer.is_subtype(joined_type, parent_type).unwrap());
    }

    #[test]
    fn type_parameter_branches_report_unsupported_join_relations() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def choose[A, B](flag: Boolean, left: A, right: B) = if flag then left else right",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IfBranchJoinUnsupported { .. })
        ));
    }

    #[test]
    fn type_parameter_if_condition_reports_unsupported_conformance() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def choose[A](condition: A) = if condition then 1 else 2");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IfConditionConformanceUnsupported { .. })
        ));
    }

    #[test]
    fn if_join_leaves_intersections_explicitly_unsupported() {
        let (arena, mut store, packages, definitions) = setup();
        let intersection = store.types.alloc(Type::And {
            left: definitions.int,
            right: definitions.boolean,
        });
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.join_expression_types(intersection, definitions.int),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
    }

    #[test]
    fn malformed_if_child_tree_has_a_focused_error() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def choose(flag: Boolean) = 1");
        let (method, literal) =
            method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let position = parsed.ast.get(literal).position;
        let mut foreign_arena = AstArena::new();
        let outside_tree = (0..=parsed.ast.iter().count() + 1)
            .map(|_| {
                foreign_arena.alloc(Tree {
                    kind: TreeKind::Literal(dotty_core::ast::Literal {
                        value: dotty_core::Constant::Int(0),
                    }),
                    position: None,
                    ty: (),
                })
            })
            .last()
            .unwrap();
        let if_tree = parsed.ast.alloc(Tree {
            kind: TreeKind::If(dotty_core::ast::If {
                cond: outside_tree,
                then_branch: literal,
                else_branch: literal,
            }),
            position,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(if_tree, context),
            Err(TyperError::IfChildTreeOutsideArena {
                child_tree_index,
                role: "condition",
                ..
            }) if child_tree_index == outside_tree.index()
        ));
    }

    #[test]
    fn non_boolean_if_condition_has_a_focused_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def choose = if 1 then 1 else 2");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IfConditionTypeMismatch { expected, .. })
                if expected == definitions.boolean
        ));
    }

    #[test]
    fn if_expression_types_block_branches_and_synthetic_unit_else() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def choose(flag: Boolean) = if flag then { val result = 1; result } else { val result = 2; result }; def partial(flag: Boolean) = if flag then 1",
        );
        let (choose, choose_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let (partial, partial_rhs) =
            method_definition_and_rhs(&parsed, &store, &index, source, "partial");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let choose_context = typer.expression_context_for(choose).unwrap();
        let chosen = typer.type_expression(choose_rhs, choose_context).unwrap();
        assert_eq!(typer.typed_ast().get(chosen).ty, definitions.int);
        let TreeKind::If(chosen_if) = &typer.typed_ast().get(chosen).kind else {
            panic!("expected if with block branches");
        };
        assert!(matches!(
            typer.typed_ast().get(chosen_if.then_branch).kind,
            TreeKind::Block(_)
        ));
        assert!(matches!(
            typer.typed_ast().get(chosen_if.else_branch).kind,
            TreeKind::Block(_)
        ));

        let partial_context = typer.expression_context_for(partial).unwrap();
        let partial_tree = typer.type_expression(partial_rhs, partial_context).unwrap();
        let TreeKind::If(partial_if) = &typer.typed_ast().get(partial_tree).kind else {
            panic!("expected if with parser-synthesized Unit else branch");
        };
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(partial_if.else_branch).ty),
            Type::Constant(dotty_core::Constant::Unit)
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(partial_tree).ty),
            Type::Or { .. }
        ));
    }

    #[test]
    fn inferred_method_result_uses_joined_if_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def choose(flag: Boolean) = if flag then 1 else 2");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected a method signature");
        };
        assert_eq!(method_type.result, definitions.int);
        assert!(typer.source_typed_index().get(source, rhs).is_some());
    }

    #[test]
    fn while_expression_checks_boolean_and_discards_body_value() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def repeat(flag: Boolean) = while flag do 1");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let position = parsed.ast.get(rhs).position;
        let TreeKind::While(source_while) = parsed.ast.get(rhs).kind else {
            panic!("expected source while expression");
        };
        let condition_position = parsed.ast.get(source_while.cond).position;
        let body_position = parsed.ast.get(source_while.body).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let scope_count = typer.expression_scopes.len();

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.unit);
        assert_eq!(typer.typed_ast().get(typed).position, position);
        let TreeKind::While(typed_while) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed while expression");
        };
        let (typed_cond, typed_body) = (typed_while.cond, typed_while.body);
        assert_eq!(
            typer.typed_ast().get(typed_cond).position,
            condition_position
        );
        assert_eq!(typer.typed_ast().get(typed_body).position, body_position);
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed_cond).ty)
                .unwrap(),
            definitions.boolean
        );
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_body).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert_eq!(typer.expression_scopes.len(), scope_count);
    }

    #[test]
    fn while_expression_types_mutable_block_body_and_nested_if() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def repeat(flag: Boolean) = while flag do { var count = 0; count = if flag then 1 else 2 }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let TreeKind::While(source_while) = parsed.ast.get(rhs).kind else {
            panic!("expected source while expression");
        };
        let TreeKind::Block(source_block) = &parsed.ast.get(source_while.body).kind else {
            panic!("expected block loop body");
        };
        let local_tree = source_block.stats[0];
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

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.unit);
        let local = typer.local_symbol_at(source, local_tree).unwrap();
        assert!(
            typer
                .store()
                .symbols
                .get(local)
                .flags
                .contains(SymbolFlags::MUTABLE)
        );
        let TreeKind::While(typed_while) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed while expression");
        };
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_while.body).kind else {
            panic!("expected typed block loop body");
        };
        let TreeKind::Assign(assignment) = &typer.typed_ast().get(typed_block.expr).kind else {
            unreachable!();
        };
        assert!(matches!(
            typer.typed_ast().get(assignment.rhs).kind,
            TreeKind::If(_)
        ));
    }

    #[test]
    fn non_boolean_while_condition_has_a_focused_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def repeat = while 1 do 2");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::WhileConditionTypeMismatch { expected, .. })
                if expected == definitions.boolean
        ));
    }

    #[test]
    fn malformed_while_child_has_a_focused_error() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def repeat(flag: Boolean) = while flag do 1");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let position = parsed.ast.get(rhs).position;
        let mut foreign_arena = AstArena::new();
        let outside_tree = (0..=parsed.ast.iter().count() + 1)
            .map(|_| {
                foreign_arena.alloc(Tree {
                    kind: TreeKind::Literal(dotty_core::ast::Literal {
                        value: dotty_core::Constant::Boolean(true),
                    }),
                    position: None,
                    ty: (),
                })
            })
            .last()
            .unwrap();
        let while_tree = parsed.ast.alloc(Tree {
            kind: TreeKind::While(dotty_core::ast::While {
                cond: outside_tree,
                body: rhs,
            }),
            position,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(while_tree, context),
            Err(TyperError::WhileChildTreeOutsideArena {
                child_tree_index,
                role: "condition",
                ..
            }) if child_tree_index == outside_tree.index()
        ));
    }

    #[test]
    fn type_parameter_while_condition_reports_unsupported_conformance() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def repeat[A](condition: A) = while condition do 1");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::WhileConditionConformanceUnsupported { .. })
        ));
    }

    #[test]
    fn inferred_method_result_from_while_is_unit() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def repeat(flag: Boolean) = while flag do 1");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected a method signature");
        };
        assert_eq!(method_type.result, definitions.unit);
        assert!(typer.source_typed_index().get(source, rhs).is_some());
    }

    #[test]
    fn explicit_method_return_checks_value_and_has_nothing_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def identity(value: Int): Int = return value");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "identity");
        let position = parsed.ast.get(rhs).position;
        let TreeKind::Return(source_return) = parsed.ast.get(rhs).kind else {
            panic!("expected source return");
        };
        let source_value = source_return.expr.unwrap();
        let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
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

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.nothing_type);
        assert_eq!(typer.typed_ast().get(typed).position, position);
        let TreeKind::Return(typed_return) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed return");
        };
        assert_eq!(typed_return.from, None);
        let typed_value = typed_return.expr.unwrap();
        assert_eq!(
            typer.source_typed_index().get(source, source_value),
            Some(typed_value)
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_value).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
    }

    #[test]
    fn explicit_polymorphic_method_return_uses_its_final_result_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def identity[A](value: Int): Int = return value");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "identity");
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

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.nothing_type);
        let TreeKind::Return(typed_return) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed return");
        };
        assert!(typed_return.expr.is_some());
    }

    #[test]
    fn return_expression_must_conform_to_explicit_method_result() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def invalid: Int = return true");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "invalid");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ReturnExpressionTypeMismatch { expected, .. })
                if expected == definitions.int
        ));
    }

    #[test]
    fn return_in_a_block_types_through_existing_local_scope() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def identity(value: Int): Int = { val copied: Int = value; return copied }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "identity");
        let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
            panic!("expected a source block");
        };
        let source_return = source_block.expr;
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

        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed block");
        };
        assert!(matches!(
            typer.typed_ast().get(typed_block.expr).kind,
            TreeKind::Return(_)
        ));
        assert!(
            typer
                .source_typed_index()
                .get(source, source_return)
                .is_some()
        );
    }

    #[test]
    fn return_in_an_if_branch_joins_with_the_other_branch_through_nothing() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def choose(flag: Boolean, returned: Int, fallback: Int): Int = if flag then return returned else fallback",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
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

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let TreeKind::If(typed_if) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed if");
        };
        assert_eq!(
            typer.typed_ast().get(typed_if.then_branch).ty,
            definitions.nothing_type
        );
        let fallback_type = typer.typed_ast().get(typed_if.else_branch).ty;
        assert_eq!(
            typer.widen_expression_type(fallback_type).unwrap(),
            definitions.int
        );
    }

    #[test]
    fn return_in_a_while_body_uses_the_enclosing_explicit_result() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def find(flag: Boolean, value: Int): Int = { while flag do return value; 0 }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "find");
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

        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed block");
        };
        let TreeKind::While(typed_while) = &typer.typed_ast().get(typed_block.stats[0]).kind else {
            panic!("expected typed while");
        };
        assert!(matches!(
            typer.typed_ast().get(typed_while.body).kind,
            TreeKind::Return(_)
        ));
    }

    #[test]
    fn bare_return_uses_a_synthetic_unit_expression_when_it_conforms() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def stop: Unit = return");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "stop");
        let TreeKind::Return(source_return) = parsed.ast.get(rhs).kind else {
            panic!("expected bare source return");
        };
        assert!(source_return.expr.is_none());
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

        let TreeKind::Return(typed_return) = &typer.typed_ast().get(typed).kind else {
            panic!("expected typed bare return");
        };
        let Some(typed_expr) = typed_return.expr else {
            panic!("Scala's bare return typing supplies a synthetic Unit expression");
        };
        assert!(matches!(
            typer.typed_ast().get(typed_expr).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit
            })
        ));
    }

    #[test]
    fn bare_return_must_conform_to_the_explicit_method_result() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def invalid: Int = return");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "invalid");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ReturnExpressionTypeMismatch { expected, .. })
                if expected == definitions.int
        ));
    }

    #[test]
    fn inferred_result_method_with_nested_return_is_deferred_without_recursion() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def loop = { return loop }");
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "loop");
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
            Err(TyperError::ReturnInInferredResultMethodDeferred {
                method: returned_method,
                ..
            }) if returned_method == method
        ));
        assert!(typer.inferred_method_results_in_progress.is_empty());
    }

    #[test]
    fn return_outside_a_method_is_rejected_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = return 1 }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let mut context = typer.expression_context_for(method).unwrap();
        context.owner = class;

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ReturnOutsideSupportedMethod { owner, .. }) if owner == class
        ));
    }

    #[test]
    fn non_local_return_target_is_deferred() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def target: Int = 0; def use: Int = 1");
        let (method, value) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let target_method = method_definition_and_rhs(&parsed, &store, &index, source, "target").0;
        let SourceDefinition::Canonical {
            source: target_source,
            tree: target_tree,
        } = index.definition_of(target_method).unwrap()
        else {
            panic!("method should have canonical source provenance");
        };
        assert_eq!(target_source, source);
        let position = parsed.ast.get(value).position;
        let returned = parsed.ast.alloc(Tree {
            kind: TreeKind::Return(dotty_core::ast::Return {
                expr: Some(value),
                from: Some(target_tree),
            }),
            position,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(returned, context),
            Err(TyperError::NonLocalReturnDeferred { target_tree_index, .. })
                if target_tree_index == target_tree.index()
        ));
    }

    #[test]
    fn malformed_return_target_has_a_focused_error() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def use: Int = 1");
        let (method, value) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut foreign_arena = AstArena::new();
        let target = (0..=parsed.ast.iter().count() + 1)
            .map(|_| {
                foreign_arena.alloc(Tree {
                    kind: TreeKind::Literal(dotty_core::ast::Literal {
                        value: dotty_core::Constant::Unit,
                    }),
                    position: None,
                    ty: (),
                })
            })
            .last()
            .unwrap();
        let returned = parsed.ast.alloc(Tree {
            kind: TreeKind::Return(dotty_core::ast::Return {
                expr: Some(value),
                from: Some(target),
            }),
            position: None,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(returned, context),
            Err(TyperError::MalformedReturnTarget { target_tree_index, .. })
                if target_tree_index == target.index()
        ));
    }

    #[test]
    fn return_target_with_an_invalid_tree_shape_is_malformed() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def use: Int = 1");
        let (method, value) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let returned = parsed.ast.alloc(Tree {
            kind: TreeKind::Return(dotty_core::ast::Return {
                expr: Some(value),
                from: Some(value),
            }),
            position: None,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();

        assert!(matches!(
            typer.type_expression(returned, context),
            Err(TyperError::MalformedReturnTarget { target_tree_index, .. })
                if target_tree_index == value.index()
        ));
    }

    #[test]
    fn failed_return_value_rolls_back_expression_and_method_info_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def invalid: Int = { var local = 1; return true } }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "invalid");
        let before = store.checkpoint();
        assert!(matches!(
            store.symbols.get(method).info,
            SymbolInfo::Missing
        ));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let scope_count = typer.expression_scopes.len();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ReturnExpressionTypeMismatch { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert!(matches!(
            typer.store().symbols.get(method).info,
            SymbolInfo::Missing
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.local_symbols.is_empty());
        assert_eq!(typer.expression_scopes.len(), scope_count);
    }

    #[test]
    fn failed_while_body_rolls_back_all_expression_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def repeat(flag: Boolean) = while flag do { var count = 0; missing }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "repeat");
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let scope_count = typer.expression_scopes.len();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.local_symbols.is_empty());
        assert_eq!(typer.expression_scopes.len(), scope_count);
    }

    #[test]
    fn failed_else_branch_rolls_back_if_expression_state() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "def choose(flag: Boolean) = if flag then { val local = 1; local } else missing",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let scope_count = typer.expression_scopes.len();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.type_index.type_at(source, rhs).is_none());
        assert!(typer.local_symbols.is_empty());
        assert_eq!(typer.expression_scopes.len(), scope_count);
    }

    #[test]
    fn decimal_and_exponent_literals_become_double_constants() {
        for (source_text, expected) in [
            ("class C { val value = 1.25 }", 1.25_f64),
            ("class C { val value = 1e2 }", 100.0_f64),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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

            assert_eq!(
                typer.typed_ast().get(typed).kind,
                TreeKind::Literal(dotty_core::ast::Literal {
                    value: dotty_core::Constant::double(expected)
                })
            );
            assert_eq!(
                typer.store().types.get(typer.typed_ast().get(typed).ty),
                &Type::Constant(dotty_core::Constant::double(expected))
            );
            assert_eq!(
                typer
                    .widen_expression_type(typer.typed_ast().get(typed).ty)
                    .unwrap(),
                definitions.double
            );
        }
    }

    #[test]
    fn out_of_range_decimal_integer_returns_a_typed_error_without_allocations() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 2147483648 }");
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
        let store_before = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IntegerLiteralOutOfRange { spelling, .. }) if spelling == "2147483648"
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_before);
    }

    #[test]
    fn overflowing_floating_literal_returns_a_typed_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 1e400 }");
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::FloatingLiteralInvalid { spelling, .. }) if spelling == "1e400"
        ));
    }

    #[test]
    fn malformed_integer_radix_returns_an_error_instead_of_panicking() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 1 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let text = store.names.intern("1");
        parsed.ast.get_mut(rhs).kind =
            TreeKind::PhaseSpecific(UntypedNode::Number(NumberLiteral {
                text,
                kind: NumberKind::Whole(1),
            }));
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

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IntegerLiteralOutOfRange { .. })
        ));
    }

    #[test]
    fn unsupported_string_and_null_literals_have_dedicated_errors() {
        for (source_text, expected_error) in [
            ("class C { val value = \"text\" }", "string"),
            ("class C { val value = null }", "null"),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
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
            let result = typer.type_expression(rhs, context);

            match (expected_error, result) {
                ("string", Err(TyperError::StringLiteralTypingDeferred { .. }))
                | ("null", Err(TyperError::NullLiteralTypingDeferred { .. })) => {}
                (variant, other) => panic!("expected {variant} literal deferral, got {other:?}"),
            }
            assert!(typer.typed_ast().iter().next().is_none());
        }
    }

    #[test]
    fn regular_application_types_a_monomorphic_method() {
        let source_text = "class C { def inc(x: Int): Int = x; def use: Int = inc(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let callee = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "inc" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
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
            panic!("regular source call should produce a typed Apply")
        };
        assert_eq!(application.kind, ApplyKind::Regular);
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let function_type = typer.typed_ast().get(application.function).ty;
        assert!(matches!(
            typer.store().types.get(function_type),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == callee
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[0]).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert_eq!(typer.type_expression(rhs, context).unwrap(), typed);
    }

    #[test]
    fn application_reports_exact_arity_mismatch() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f() }";
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
                actual: 0,
                ..
            })
        ));
    }

    #[test]
    fn application_rejects_non_method_callees() {
        let source_text = "class C { val value: Int = 1; def use: Int = value() }";
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
            Err(TyperError::ApplicationCalleeNotMethod { ty, .. }) if ty == definitions.int
        ));
    }

    #[test]
    fn application_reports_argument_type_mismatch_with_index() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(false) }";
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
                Err(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    actual,
                    expected,
                    ..
                }) if *actual == definitions.boolean && *expected == definitions.int
            ),
            "unexpected application result: {result:?}"
        );
    }

    #[test]
    fn failed_application_rolls_back_typed_children_and_semantic_changes() {
        let source_text = "class C { def f(x: Int, y: Int): Int = x; def use: Int = f(1, false) }";
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
        let store_before = typer.store().checkpoint();

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 1,
                    ..
                })
            ),
            "unexpected application result: {result:?}"
        );
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_before);
    }

    #[test]
    fn too_many_application_arguments_reports_exact_arity() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(1, 2) }";
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
    fn by_name_application_parameter_is_deferred() {
        let source_text = "class C { def f(using x: => Int): Int = x; def use: Int = f(using 1) }";
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
            Err(TyperError::ByNameApplicationParameterDeferred {
                parameter_index: 0,
                ..
            })
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
