//! Assemble method plans for a simple source class with an implicit no-arg constructor.

use std::collections::HashMap;
use std::fmt;

use crate::core::{
    AstArena, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SymbolFlags,
    SymbolId, SymbolKind, SymbolOrigin, Template, TreeId, TreeKind, Untyped, Visibility,
};
use crate::jvm::{JvmClassInitializationPlan, JvmClassPlan, JvmMethodPlan};

/// The source shape or supplied methods cannot form a simple class plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassPlanError {
    /// The symbol does not identify an ordinary concrete top-level source class.
    SourceClass(SymbolId),
    /// The class has an explicit parent, self type, derives or uses clause.
    UnsupportedTemplate(SymbolId),
    /// The primary constructor has parameters.
    ConstructorParameters(SymbolId),
    /// The primary constructor is missing, restricted, generic or has a body.
    UnsupportedConstructor(SymbolId),
    /// This template statement is outside the concrete-method subset.
    UnsupportedStatement {
        class: SymbolId,
        tree: TreeId<Untyped>,
    },
    /// A supplied plan has no corresponding concrete method in this class.
    UnexpectedMethod { class: SymbolId, method: SymbolId },
    /// A concrete source method has no supplied plan.
    MissingMethod { class: SymbolId, method: SymbolId },
    /// More than one plan was supplied for the method.
    DuplicateMethod { class: SymbolId, method: SymbolId },
    /// The supplied method plan has no body.
    MissingMethodBody { class: SymbolId, method: SymbolId },
}

impl fmt::Display for ClassPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot build JVM class plan: {self:?}")
    }
}
impl std::error::Error for ClassPlanError {}

/// Read-only planner for top-level classes containing only concrete methods.
/// Provide canonical AST, symbols and method plans from the same session.
/// The backend validates the assembled unit before emission.
pub struct JvmClassPlanner<'a> {
    store: &'a SemanticStore,
    ast: &'a AstArena<Untyped>,
    index: &'a SourceSemanticIndex,
}

impl<'a> JvmClassPlanner<'a> {
    /// Borrows a source unit's naming results and the session store.
    pub fn new(
        store: &'a SemanticStore,
        ast: &'a AstArena<Untyped>,
        index: &'a SourceSemanticIndex,
    ) -> Self {
        Self { store, ast, index }
    }

    /// Proves the public no-argument initialization shape and assembles the
    /// supplied method plans in source declaration order. Each source method
    /// requires exactly one plan from the caller.
    pub fn plan(
        &self,
        class: SymbolId,
        methods: impl IntoIterator<Item = JvmMethodPlan>,
    ) -> Result<JvmClassPlan, ClassPlanError> {
        let (source, template) = self.template(class)?;
        self.check_constructor(class, source, template)?;

        let mut positions = HashMap::with_capacity(template.body.len());
        let mut slots = Vec::with_capacity(template.body.len());
        for &tree in &template.body {
            let error = ClassPlanError::UnsupportedStatement { class, tree };
            let Some(TreeKind::DefDef(definition)) = self.ast.try_get(tree).map(|node| &node.kind)
            else {
                return Err(error);
            };
            let method = self
                .member(class, source, tree, SymbolKind::Method)
                .ok_or(error)?;
            if definition.rhs.is_none() || positions.insert(method, slots.len()).is_some() {
                return Err(error);
            }
            slots.push((method, None));
        }

        for plan in methods {
            let method = plan.method;
            let &position = positions
                .get(&method)
                .ok_or(ClassPlanError::UnexpectedMethod { class, method })?;
            if plan.body.is_none() {
                return Err(ClassPlanError::MissingMethodBody { class, method });
            }
            if slots[position].1.replace(plan).is_some() {
                return Err(ClassPlanError::DuplicateMethod { class, method });
            }
        }
        let methods = slots
            .into_iter()
            .map(|(method, plan)| plan.ok_or(ClassPlanError::MissingMethod { class, method }))
            .collect::<Result<_, _>>()?;
        Ok(JvmClassPlan {
            class,
            methods,
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::ImplicitNoArg,
        })
    }

    fn template(&self, class: SymbolId) -> Result<(SourceId, &Template<Untyped>), ClassPlanError> {
        let error = ClassPlanError::SourceClass(class);
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(class)
        else {
            return Err(error);
        };
        if !self.store.symbols.contains(class) || self.index.symbol_at(source, tree) != Some(class)
        {
            return Err(error);
        }
        let symbol = self.store.symbols.get(class);
        if symbol.kind != SymbolKind::Class
            || symbol.origin != SymbolOrigin::Source(source)
            || !symbol
                .flags
                .intersection(
                    SymbolFlags::CASE
                        .union(SymbolFlags::ENUM)
                        .union(SymbolFlags::ABSTRACT),
                )
                .is_empty()
            || !symbol.owner.is_some_and(|owner| {
                self.store.symbols.contains(owner)
                    && self.store.symbols.get(owner).kind == SymbolKind::Package
            })
        {
            return Err(error);
        }
        let node = self.ast.try_get(tree).ok_or(error)?;
        if node.position.is_some_and(|pos| pos.source() != source) {
            return Err(error);
        }
        let TreeKind::TypeDef(definition) = &node.kind else {
            return Err(error);
        };
        let Some(TreeKind::Template(template)) =
            self.ast.try_get(definition.rhs).map(|node| &node.kind)
        else {
            return Err(error);
        };
        if !template.parents.is_empty()
            || template.self_val.is_some()
            || !template.metadata.derives.is_empty()
            || !template.metadata.uses.is_empty()
        {
            return Err(ClassPlanError::UnsupportedTemplate(class));
        }
        Ok((source, template))
    }

    fn check_constructor(
        &self,
        class: SymbolId,
        source: SourceId,
        template: &Template<Untyped>,
    ) -> Result<(), ClassPlanError> {
        let error = ClassPlanError::UnsupportedConstructor(class);
        let tree = template.constructor;
        let constructor = self
            .member(class, source, tree, SymbolKind::Constructor)
            .ok_or(error)?;
        let Some(TreeKind::DefDef(definition)) = self.ast.try_get(tree).map(|node| &node.kind)
        else {
            return Err(error);
        };
        if definition
            .value_param_clauses
            .iter()
            .any(|clause| !clause.is_empty())
        {
            return Err(ClassPlanError::ConstructorParameters(class));
        }
        if self.store.symbols.get(constructor).visibility != Visibility::Public
            || definition.metadata.visibility.is_some()
            || !definition.type_params.is_empty()
            || definition.rhs.is_some()
        {
            return Err(error);
        }
        Ok(())
    }

    fn member(
        &self,
        class: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
        kind: SymbolKind,
    ) -> Option<SymbolId> {
        let id = self.index.symbol_at(source, tree)?;
        if !self.store.symbols.contains(id)
            || self.index.definition_of(id) != Some(SourceDefinition::Canonical { source, tree })
        {
            return None;
        }
        let symbol = self.store.symbols.get(id);
        (symbol.owner == Some(class)
            && symbol.kind == kind
            && symbol.origin == SymbolOrigin::Source(source))
        .then_some(id)
    }
}
