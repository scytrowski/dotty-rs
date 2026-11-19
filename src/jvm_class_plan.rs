//! Assemble field and method plans for a simple source class with a public primary constructor.

use std::collections::HashMap;
use std::fmt;

use crate::core::{
    AstArena, Modifier, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex,
    SourceTypedIndex, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, Template,
    TermRefTarget, TreeId, TreeKind, Type, TypeRefTarget, Typed, Untyped, UntypedNode, ValDef,
    Visibility, VisibilitySyntax,
};
use crate::jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmConstructorArgumentBinding, JvmFieldPlan,
    JvmFieldSourceVisibility, JvmMethodPlan, JvmPrimaryConstructorPlan,
    JvmPublicBodyFieldSourceProof,
};

/// Result of typing one field RHS and the index from that typing operation.
#[derive(Clone, Copy)]
pub struct TypedFieldInitializer<'a> {
    /// Canonical field in the class being planned.
    pub field: SymbolId,
    /// Typed root returned for this field's source RHS.
    pub root: TreeId<Typed>,
    /// Source-to-typed bindings from the same session arena.
    pub index: &'a SourceTypedIndex,
}

impl TypedFieldInitializer<'_> {
    fn checked_root(
        self,
        source: SourceId,
        rhs: TreeId<Untyped>,
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
    ) -> Result<TreeId<Typed>, ClassPlanError> {
        let field = self.field;
        let invalid = ClassPlanError::InvalidFieldInitializer { field };
        let expected = self.index.get(source, rhs).ok_or(invalid)?;
        if expected != self.root {
            return Err(ClassPlanError::FieldInitializerMismatch {
                field,
                expected,
                actual: self.root,
            });
        }
        let source_rhs = ast.try_get(rhs).ok_or(invalid)?;
        let typed_rhs = typed_ast.try_get(self.root).ok_or(invalid)?;
        if !source_rhs
            .position
            .is_some_and(|pos| pos.source() == source)
            || typed_rhs.position != source_rhs.position
        {
            return Err(invalid);
        }
        Ok(self.root)
    }
}

/// The source shape or supplied methods cannot form a simple class plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassPlanError {
    /// The symbol does not identify an ordinary concrete top-level source class.
    SourceClass(SymbolId),
    /// The parent is unsupported or unresolved, or the template has a self type, derives or uses clause.
    UnsupportedTemplate(SymbolId),
    /// The class header has incomplete or inconsistent bindings for its parent application.
    InvalidParentApplication {
        class: SymbolId,
        tree: TreeId<Untyped>,
    },
    /// The primary constructor has unsupported parameter clauses or formals.
    ConstructorParameters(SymbolId),
    /// A constructor parameter supplies an unsupported default value.
    DefaultConstructorParameter {
        class: SymbolId,
        tree: TreeId<Untyped>,
    },
    /// The primary constructor is missing, restricted, generic or has a body.
    UnsupportedConstructor(SymbolId),
    /// This statement is outside the selected class planning subset.
    UnsupportedStatement {
        class: SymbolId,
        tree: TreeId<Untyped>,
    },
    /// A field input has no matching class-body declaration.
    UnexpectedField { class: SymbolId, field: SymbolId },
    /// An initialized class-body field has no input.
    MissingField { class: SymbolId, field: SymbolId },
    /// Two inputs refer to the same field.
    DuplicateField { class: SymbolId, field: SymbolId },
    /// The source RHS has no usable binding in the supplied typer output.
    InvalidFieldInitializer { field: SymbolId },
    /// The supplied typed root differs from the canonical binding for this RHS.
    FieldInitializerMismatch {
        field: SymbolId,
        expected: TreeId<Typed>,
        actual: TreeId<Typed>,
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

/// Read-only planner for top-level classes with concrete methods and ordinary public and private fields.
/// Provide canonical AST, symbols and method plans from the same session.
/// The backend validates the assembled unit before emission.
pub struct JvmClassPlanner<'a> {
    store: &'a SemanticStore,
    ast: &'a AstArena<Untyped>,
    index: &'a SourceSemanticIndex,
    header: Option<(&'a AstArena<Typed>, &'a SourceTypedIndex)>,
}

impl<'a> JvmClassPlanner<'a> {
    /// Borrows a source unit's naming results and the session store.
    pub fn new(
        store: &'a SemanticStore,
        ast: &'a AstArena<Untyped>,
        index: &'a SourceSemanticIndex,
    ) -> Self {
        Self {
            store,
            ast,
            index,
            header: None,
        }
    }

    /// Uses the mappings produced when completing this class header to plan a
    /// direct parent call forwarding constructor parameters. Keep the typed
    /// arena from that operation (or its session extension) through emission.
    pub fn with_typed_header(
        mut self,
        typed_ast: &'a AstArena<Typed>,
        index: &'a SourceTypedIndex,
    ) -> Self {
        self.header = Some((typed_ast, index));
        self
    }

    /// Plans a public primary constructor with at most one value parameter list.
    /// Plain parameters and public constructor `val` and `var` declarations may
    /// share the list. Supply one plan per source method. Generated accessors
    /// come from the constructor bindings.
    pub fn plan(
        &self,
        class: SymbolId,
        methods: impl IntoIterator<Item = JvmMethodPlan>,
    ) -> Result<JvmClassPlan, ClassPlanError> {
        self.assemble(class, methods, |_, tree, _| {
            Err(ClassPlanError::UnsupportedStatement { class, tree })
        })
    }

    /// Assembles ordinary public and private instance `val` and `var` fields and concrete methods. Supply
    /// the fields in any order and keep all typed roots in the same session
    /// arena used for emission. The plan initializes fields in template order.
    pub fn plan_with_fields<'t>(
        &self,
        class: SymbolId,
        methods: impl IntoIterator<Item = JvmMethodPlan>,
        typed_ast: &AstArena<Typed>,
        initializers: impl IntoIterator<Item = TypedFieldInitializer<'t>>,
    ) -> Result<JvmClassPlan, ClassPlanError> {
        let mut fields = Vec::new();
        let mut declarations = HashMap::new();
        let mut plan = self.assemble(class, methods, |source, tree, definition| {
            let error = ClassPlanError::UnsupportedStatement { class, tree };
            let field = self
                .member(class, source, tree, SymbolKind::Field)
                .ok_or(error)?;
            let symbol = self.store.symbols.get(field);
            let modifiers = &definition.metadata.modifiers;
            let mutability_flag = if modifiers.contains(&Modifier::Var) {
                SymbolFlags::MUTABLE
            } else {
                SymbolFlags::EMPTY
            };
            let ordinary = match (symbol.visibility, &definition.metadata.visibility) {
                (Visibility::Public, None) => {
                    modifiers.iter().all(|m| *m == Modifier::Var) && symbol.flags == mutability_flag
                }
                (Visibility::Private, Some(VisibilitySyntax::Private { qualifier: None })) => {
                    modifiers
                        .iter()
                        .all(|m| matches!(m, Modifier::Final | Modifier::Var))
                        && symbol.flags.difference(SymbolFlags::FINAL) == mutability_flag
                }
                _ => false,
            };
            if !ordinary || !definition.metadata.annotations.is_empty() {
                return Err(error);
            }
            let rhs = definition.rhs.ok_or(error)?;
            if declarations
                .insert(field, (source, tree, rhs, fields.len()))
                .is_some()
            {
                return Err(error);
            }
            fields.push((field, None));
            Ok(())
        })?;
        for input in initializers {
            let field = input.field;
            let &(source, tree, rhs, position) = declarations
                .get(&field)
                .ok_or(ClassPlanError::UnexpectedField { class, field })?;
            let slot = &mut fields[position].1;
            if slot.is_some() {
                return Err(ClassPlanError::DuplicateField { class, field });
            }
            let root = input.checked_root(source, rhs, self.ast, typed_ast)?;
            let source_visibility =
                if self.store.symbols.get(field).visibility == Visibility::Public {
                    let proof = JvmPublicBodyFieldSourceProof::from_source(
                        class,
                        field,
                        source,
                        tree,
                        root,
                        self.ast,
                        self.index,
                        input.index,
                        typed_ast,
                        self.store,
                    )
                    .map_err(|_| ClassPlanError::InvalidFieldInitializer { field })?;
                    JvmFieldSourceVisibility::PlainPublic(proof)
                } else {
                    JvmFieldSourceVisibility::PlainPrivate
                };
            *slot = Some(JvmFieldPlan {
                field,
                source_visibility,
                initializer: Some(root),
            });
        }
        plan.fields = fields
            .into_iter()
            .map(|(field, slot)| slot.ok_or(ClassPlanError::MissingField { class, field }))
            .collect::<Result<_, _>>()?;
        Ok(plan)
    }

    fn assemble(
        &self,
        class: SymbolId,
        methods: impl IntoIterator<Item = JvmMethodPlan>,
        mut visit_field: impl FnMut(
            SourceId,
            TreeId<Untyped>,
            &ValDef<Untyped>,
        ) -> Result<(), ClassPlanError>,
    ) -> Result<JvmClassPlan, ClassPlanError> {
        let (source, template) = self.template(class)?;
        let initialization = self.constructor_plan(class, source, template)?;
        let initialization = match template.parents.as_slice() {
            [] => initialization,
            [parent] if self.has_source_parent(class) => {
                match &self
                    .ast
                    .try_get(*parent)
                    .ok_or(ClassPlanError::UnsupportedTemplate(class))?
                    .kind
                {
                    TreeKind::Ident(ident) if ident.name.is_type() => initialization,
                    TreeKind::Apply(application) => self.plan_parent_call(
                        class,
                        source,
                        *parent,
                        &application.args,
                        initialization,
                    )?,
                    _ => return Err(ClassPlanError::UnsupportedTemplate(class)),
                }
            }
            _ => return Err(ClassPlanError::UnsupportedTemplate(class)),
        };

        let mut positions = HashMap::with_capacity(template.body.len());
        let mut slots = Vec::with_capacity(template.body.len());
        for &tree in &template.body {
            let error = ClassPlanError::UnsupportedStatement { class, tree };
            let definition = match &self.ast.try_get(tree).ok_or(error)?.kind {
                TreeKind::DefDef(definition) => definition,
                TreeKind::ValDef(field) => {
                    visit_field(source, tree, field)?;
                    continue;
                }
                _ => return Err(error),
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
            initialization,
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
        if template.self_val.is_some()
            || !template.metadata.derives.is_empty()
            || !template.metadata.uses.is_empty()
        {
            return Err(ClassPlanError::UnsupportedTemplate(class));
        }
        Ok((source, template))
    }

    fn has_source_parent(&self, class: SymbolId) -> bool {
        let SymbolInfo::Complete(info) = *self.store.symbols.info(class) else {
            return false;
        };
        let Some(Type::ClassInfo(info)) = self.store.types.try_get(info) else {
            return false;
        };
        let [parent] = info.parents.as_slice() else {
            return false;
        };
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(parent),
            ..
        }) = self.store.types.try_get(*parent)
        else {
            return false;
        };
        if info.class != class || !self.store.symbols.contains(*parent) {
            return false;
        }
        let symbol = self.store.symbols.get(*parent);
        symbol.kind == SymbolKind::Class && matches!(symbol.origin, SymbolOrigin::Source(_))
    }

    fn plan_parent_call(
        &self,
        class: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
        args: &[TreeId<Untyped>],
        initialization: JvmClassInitializationPlan,
    ) -> Result<JvmClassInitializationPlan, ClassPlanError> {
        let unsupported = ClassPlanError::UnsupportedTemplate(class);
        let JvmClassInitializationPlan::PrimaryConstructor(constructor) = initialization else {
            return Err(unsupported);
        };
        let (typed_ast, index) = self.header.ok_or(unsupported)?;
        let invalid = ClassPlanError::InvalidParentApplication { class, tree };
        let typed_call = index.get(source, tree).ok_or(invalid)?;
        let [arg] = args else { return Err(unsupported) };
        let source_arg = self.ast.try_get(*arg).ok_or(invalid)?;
        if !matches!(&source_arg.kind, TreeKind::Ident(ident) if ident.name.is_term()) {
            return Err(unsupported);
        }
        let typed = index.get(source, *arg).ok_or(invalid)?;
        // Only a reference to a formal of this exact child constructor may be forwarded.
        let node = typed_ast.try_get(typed).ok_or(invalid)?;
        if !source_arg
            .position
            .is_some_and(|pos| pos.source() == source)
            || node.position != source_arg.position
            || !matches!(&node.kind, TreeKind::Ident(_))
            || !matches!(self.store.types.try_get(node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                if constructor.parameters().contains(symbol))
        {
            return Err(invalid);
        }
        constructor
            .with_direct_super_constructor(
                tree,
                typed_call,
                vec![JvmConstructorArgumentBinding {
                    source: *arg,
                    typed,
                }],
                self.ast,
                self.index,
                index,
                self.store,
                typed_ast,
            )
            .map(JvmClassInitializationPlan::PrimaryConstructor)
            .map_err(|_| invalid)
    }

    fn constructor_plan(
        &self,
        class: SymbolId,
        source: SourceId,
        template: &Template<Untyped>,
    ) -> Result<JvmClassInitializationPlan, ClassPlanError> {
        let error = ClassPlanError::UnsupportedConstructor(class);
        let tree = template.constructor;
        let constructor = self
            .member(class, source, tree, SymbolKind::Constructor)
            .ok_or(error)?;
        let Some(TreeKind::DefDef(definition)) = self.ast.try_get(tree).map(|node| &node.kind)
        else {
            return Err(error);
        };
        if self.store.symbols.get(constructor).visibility != Visibility::Public
            || definition.metadata.visibility.is_some()
            || !definition.type_params.is_empty()
            || definition.rhs.is_some()
        {
            return Err(error);
        }
        let unsupported = ClassPlanError::ConstructorParameters(class);
        let clause = match definition.value_param_clauses.as_slice() {
            [] => return Ok(JvmClassInitializationPlan::ImplicitNoArg),
            [clause] if clause.is_empty() => return Ok(JvmClassInitializationPlan::ImplicitNoArg),
            [clause] => clause,
            _ => return Err(unsupported),
        };
        let mut parameters = Vec::with_capacity(clause.len());
        for &tree in clause {
            let Some(TreeKind::ValDef(param)) = self.ast.try_get(tree).map(|node| &node.kind)
            else {
                return Err(unsupported);
            };
            if param.rhs.is_some() {
                return Err(ClassPlanError::DefaultConstructorParameter { class, tree });
            }
            if matches!(
                &self.ast.try_get(param.tpt).ok_or(unsupported)?.kind,
                TreeKind::ByNameTypeTree(_) | TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_))
            ) {
                return Err(unsupported);
            }
            let parameter = self
                .index
                .derived_symbol_at(constructor, source, tree)
                .ok_or(unsupported)?;
            parameters.push(parameter);
        }
        let proof = JvmPrimaryConstructorPlan::from_source(
            class,
            constructor,
            parameters,
            self.ast,
            self.index,
        )
        .map_err(|_| unsupported)?;
        Ok(JvmClassInitializationPlan::PrimaryConstructor(proof))
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
