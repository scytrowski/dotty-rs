//! Project a concrete source method typed by the frontend into a JVM method plan.

use crate::core::{
    AstArena, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SourceTypedIndex,
    SymbolFlags, SymbolId, SymbolKind, TermRefTarget, TreeId, TreeKind, Type, Typed, Untyped,
};
use crate::jvm::{JvmLocalBinding, JvmMethodPlan};
use crate::typer::{TypedLocalBinding, TypedSourceOutput};

/// Missing or inconsistent evidence for a source method plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodPlanError {
    /// The symbol has no canonical source method definition in the supplied inputs.
    SourceMethod(SymbolId),
    /// The mapped typed definition does not refer to the requested method.
    TypedMethod(SymbolId),
    /// The method has no concrete body with a source-to-typed mapping.
    MissingBody(SymbolId),
    /// The typed definition points at a different body than the mapped source RHS.
    BodyMismatch {
        /// Method symbol submitted to this planning operation.
        method: SymbolId,
        /// Canonical identity or typed root required by the original source binding.
        expected: TreeId<Typed>,
        /// Identity or typed root supplied by the caller instead.
        actual: TreeId<Typed>,
    },
    /// The mapped body is absent from the typed arena or belongs to another source.
    InvalidBody {
        /// Method symbol submitted to this planning operation.
        method: SymbolId,
        /// Supplied typed body root that failed validation.
        body: TreeId<Typed>,
    },
    /// The source and typed definitions have different value parameter clause shapes.
    ParameterClauses(SymbolId),
    /// A formal has no matching typed declaration or canonical parameter symbol.
    Parameter {
        /// Method symbol submitted to this planning operation.
        method: SymbolId,
        /// Source tree associated with the rejected declaration or binding.
        tree: TreeId<Untyped>,
    },
    /// A local binding refers to an invalid symbol or typed declaration.
    Local(SymbolId),
}

impl std::fmt::Display for MethodPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot build JVM method plan: {self:?}")
    }
}

impl std::error::Error for MethodPlanError {}

/// Borrows the naming and typing results of one semantic session. Use the same
/// session store and typed arena when submitting the plan to the JVM backend.
/// IDs from unrelated arenas can occupy the same slot.
///
/// For deferred planning, the typed arena may have been extended by subsequent
/// typers. Supply the original method's index and local bindings alongside it.
pub struct JvmMethodPlanner<'a> {
    /// Store shared by naming, typing and emission.
    pub store: &'a SemanticStore,
    /// Original untyped arena for this source.
    pub source_ast: &'a AstArena<Untyped>,
    /// Canonical symbol bindings for the source declarations.
    pub source_index: &'a SourceSemanticIndex,
    /// Session arena containing the typed definition and its children.
    pub typed_ast: &'a AstArena<Typed>,
    /// Mappings produced when the method was typed.
    pub typed_index: &'a SourceTypedIndex,
    /// Ordinary locals from the same typer output, in typed declaration order.
    pub local_bindings: &'a [TypedLocalBinding],
}

impl<'a> JvmMethodPlanner<'a> {
    /// Borrows an intact output from [`crate::typer::SourceTyper::into_typed_output`].
    pub fn new(
        store: &'a SemanticStore,
        source_ast: &'a AstArena<Untyped>,
        source_index: &'a SourceSemanticIndex,
        output: &'a TypedSourceOutput,
    ) -> Self {
        Self {
            store,
            source_ast,
            source_index,
            typed_ast: &output.typed_ast,
            typed_index: &output.source_typed_index,
            local_bindings: &output.local_bindings,
        }
    }

    /// Builds a plan for a concrete member typed by
    /// [`crate::typer::SourceTyper::type_method_definition`]. The typed
    /// definition and its body must agree with both canonical mappings.
    /// Backend validation and lowering take place on the assembled class plan.
    pub fn plan(&self, method: SymbolId) -> Result<JvmMethodPlan, MethodPlanError> {
        let Some(SourceDefinition::Canonical { source, tree }) =
            self.source_index.definition_of(method)
        else {
            return Err(MethodPlanError::SourceMethod(method));
        };
        if !self.store.symbols.contains(method)
            || self.store.symbols.get(method).kind != SymbolKind::Method
            || self.source_index.symbol_at(source, tree) != Some(method)
        {
            return Err(MethodPlanError::SourceMethod(method));
        }
        let source_node = self
            .source_ast
            .try_get(tree)
            .ok_or(MethodPlanError::SourceMethod(method))?;
        if source_node
            .position
            .is_some_and(|position| position.source() != source)
        {
            return Err(MethodPlanError::SourceMethod(method));
        }
        let TreeKind::DefDef(def) = &source_node.kind else {
            return Err(MethodPlanError::SourceMethod(method));
        };
        let typed_node = self
            .typed_index
            .get(source, tree)
            .and_then(|id| self.typed_ast.try_get(id))
            .ok_or(MethodPlanError::TypedMethod(method))?;
        let TreeKind::DefDef(typed) = &typed_node.kind else {
            return Err(MethodPlanError::TypedMethod(method));
        };
        if !matches!(self.store.types.try_get(typed_node.ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method)
        {
            return Err(MethodPlanError::TypedMethod(method));
        }
        let expected = def
            .rhs
            .and_then(|rhs| self.typed_index.get(source, rhs))
            .ok_or(MethodPlanError::MissingBody(method))?;
        let body = typed.rhs.ok_or(MethodPlanError::MissingBody(method))?;
        if expected != body {
            return Err(MethodPlanError::BodyMismatch {
                method,
                expected,
                actual: body,
            });
        }
        if !self.typed_ast.try_get(body).is_some_and(|node| {
            node.position
                .is_none_or(|position| position.source() == source)
        }) {
            return Err(MethodPlanError::InvalidBody { method, body });
        }

        let parameters = self.parameters(
            method,
            source,
            &def.value_param_clauses,
            &typed.value_param_clauses,
        )?;
        let mut locals = Vec::new();
        for binding in self.local_bindings {
            let id = binding.symbol();
            if !self.store.symbols.contains(id) {
                return Err(MethodPlanError::Local(id));
            }
            let symbol = self.store.symbols.get(id);
            if symbol.owner != Some(method) {
                continue;
            }
            if symbol.kind != SymbolKind::Local
                || !matches!(
                    self.typed_ast
                        .try_get(binding.declaration())
                        .map(|n| &n.kind),
                    Some(TreeKind::ValDef(_))
                )
            {
                return Err(MethodPlanError::Local(id));
            }
            locals.push(JvmLocalBinding::ordinary(
                binding.declaration(),
                id,
                symbol.flags.contains(SymbolFlags::MUTABLE),
            ));
        }
        Ok(JvmMethodPlan {
            method,
            parameters,
            body: Some(body),
            locals,
        })
    }

    fn parameters(
        &self,
        method: SymbolId,
        source: SourceId,
        clauses: &[Vec<TreeId<Untyped>>],
        typed_clauses: &[Vec<TreeId<Typed>>],
    ) -> Result<Vec<SymbolId>, MethodPlanError> {
        if clauses.len() != typed_clauses.len() {
            return Err(MethodPlanError::ParameterClauses(method));
        }
        let mut parameters = Vec::new();
        for (clause, typed) in clauses.iter().zip(typed_clauses) {
            if clause.len() != typed.len() {
                return Err(MethodPlanError::ParameterClauses(method));
            }
            for (&tree, &typed_tree) in clause.iter().zip(typed) {
                let error = MethodPlanError::Parameter { method, tree };
                let id = self.source_index.symbol_at(source, tree).ok_or(error)?;
                if !self.store.symbols.contains(id)
                    || self.typed_index.get(source, tree) != Some(typed_tree)
                    || !matches!(
                        self.typed_ast.try_get(typed_tree).map(|n| &n.kind),
                        Some(TreeKind::ValDef(_))
                    )
                {
                    return Err(error);
                }
                let symbol = self.store.symbols.get(id);
                if symbol.kind != SymbolKind::Parameter || symbol.owner != Some(method) {
                    return Err(error);
                }
                parameters.push(id);
            }
        }
        Ok(parameters)
    }
}
