//! Source identity proof and bounded primary-constructor metadata.
use std::collections::HashSet;

use crate::{JvmCodegenError, JvmMethodDescriptor, JvmMethodPlan, JvmParameterLocal};
use dotty_classfile::access_flags::{ACC_PRIVATE, ACC_PUBLIC, MethodAccessFlags};
use dotty_core::ast::{AstArena, Modifier, TreeKind, Typed, TypedAst, Untyped, VisibilitySyntax};
use dotty_core::{
    Definitions, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SourceTypedIndex,
    SymbolId, SymbolInfo, SymbolKind, TreeId, Type, TypeRefTarget, Visibility,
};

/// A source argument and the exact typed root mapped from it by the frontend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmConstructorArgumentBinding {
    /// Original constructor argument in the untyped source arena.
    pub source: TreeId<Untyped>,
    /// Typed replacement for `source` from the same frontend session.
    pub typed: TreeId<Typed>,
}

/// Canonical source field and derived constructor formal for one primary
/// constructor `val`/`var` accessor.
///
/// This is source and semantic identity evidence only. It does not describe a
/// JVM field or generated accessor method; consumers may use it to build a
/// separate JVM-only projection after class metadata validation succeeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmConstructorAccessorBinding {
    class: SymbolId,
    constructor: SymbolId,
    source: SourceId,
    source_parameter: TreeId<Untyped>,
    field: SymbolId,
    parameter: SymbolId,
    source_ordinal: usize,
    mutable: bool,
}

impl JvmConstructorAccessorBinding {
    /// Canonical class that owns the source field.
    pub const fn class(&self) -> SymbolId {
        self.class
    }

    /// Canonical primary constructor that owns the derived formal.
    pub const fn constructor(&self) -> SymbolId {
        self.constructor
    }

    /// Original source unit containing the parameter declaration.
    pub const fn source_id(&self) -> SourceId {
        self.source
    }

    /// Exact constructor parameter declaration tree that introduced the
    /// accessor field.
    pub const fn source_parameter(&self) -> TreeId<Untyped> {
        self.source_parameter
    }

    /// Canonical class-owned source `Field` identity.
    pub const fn field(&self) -> SymbolId {
        self.field
    }

    /// Distinct constructor-owned formal `Parameter` identity.
    pub const fn parameter(&self) -> SymbolId {
        self.parameter
    }

    /// Zero-based position in the flattened primary-constructor parameter
    /// sequence. This is source order, not an inferred JVM slot.
    pub const fn source_ordinal(&self) -> usize {
        self.source_ordinal
    }

    /// Whether the source declaration was `var`.
    pub const fn is_mutable(&self) -> bool {
        self.mutable
    }
}

/// Source- and symbol-checked direct-super constructor invocation evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmDirectSuperConstructorPlan {
    child_class: SymbolId,
    child_constructor: SymbolId,
    parent_class: SymbolId,
    parent_constructor: SymbolId,
    source_application: TreeId<Untyped>,
    typed_application: TreeId<Typed>,
    arguments: Vec<JvmConstructorArgumentBinding>,
    source: SourceId,
}

impl JvmDirectSuperConstructorPlan {
    /// Canonical constructor selected by the typed parent application.
    pub const fn parent_constructor(&self) -> SymbolId {
        self.parent_constructor
    }

    /// Canonical direct parent that owns `parent_constructor`.
    pub const fn parent_class(&self) -> SymbolId {
        self.parent_class
    }

    /// Typed parent application retained from the same source session.
    pub const fn typed_application(&self) -> TreeId<Typed> {
        self.typed_application
    }

    /// Original parent application in the source arena.
    pub const fn source_application(&self) -> TreeId<Untyped> {
        self.source_application
    }

    /// Source unit that owns both parent-application roots.
    pub const fn source_id(&self) -> SourceId {
        self.source
    }

    /// Source argument identities and corresponding typed roots in order.
    pub fn arguments(&self) -> &[JvmConstructorArgumentBinding] {
        &self.arguments
    }

    pub(crate) fn validate_typed_binding(
        &self,
        store: &SemanticStore,
        typed_ast: &TypedAst,
        child_class: SymbolId,
        child_constructor: SymbolId,
    ) -> Result<(), JvmCodegenError> {
        let fail = || JvmCodegenError::InvalidPrimaryConstructor {
            constructor: child_constructor,
            issue: JvmConstructorIssue::ParentConstructorTypedShape,
        };
        if self.child_class != child_class || self.child_constructor != child_constructor {
            return Err(fail());
        }
        let symbol = store
            .symbols
            .contains(self.parent_constructor)
            .then(|| store.symbols.get(self.parent_constructor))
            .ok_or_else(fail)?;
        if symbol.kind != SymbolKind::Constructor || symbol.owner != Some(self.parent_class) {
            return Err(fail());
        }
        if symbol.origin != dotty_core::SymbolOrigin::Source(self.source)
            || !store.symbols.contains(self.child_class)
            || store.symbols.get(self.child_class).origin
                != dotty_core::SymbolOrigin::Source(self.source)
        {
            return Err(fail());
        }
        let application = typed_ast.try_get(self.typed_application).ok_or_else(fail)?;
        let TreeKind::Apply(application) = &application.kind else {
            return Err(fail());
        };
        if application.kind != dotty_core::ast::ApplyKind::Regular
            || application.args.len() != self.arguments.len()
            || application
                .args
                .iter()
                .zip(&self.arguments)
                .any(|(typed, binding)| *typed != binding.typed)
        {
            return Err(fail());
        }
        for binding in &self.arguments {
            let Some(argument) = typed_ast.try_get(binding.typed) else {
                return Err(fail());
            };
            if !matches!(argument.kind, TreeKind::Literal(_) | TreeKind::Ident(_))
                || store.types.try_get(argument.ty).is_none()
            {
                return Err(fail());
            }
        }
        Ok(())
    }
}

/// Why an explicit primary constructor cannot be projected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmConstructorIssue {
    SourceIdentity,
    SourceShape,
    ParameterIdentity,
    ParameterSignature,
    AccessorFieldIdentity,
    AccessorFieldMetadata,
    AccessorFieldSignature,
    DeclarationScope,
    SecondaryConstructor,
    ResultType,
    Visibility,
    Flags,
    ParentConstructorSourceShape,
    ParentConstructorTypedShape,
    ParentConstructorIdentity,
    ParentConstructorArgumentIdentity,
}

/// Source-checked primary constructor binding. Its private fields retain the
/// exact constructor/parameter identities proven against the shared source AST
/// and semantic index. Creating this proof never parses or types source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmPrimaryConstructorPlan {
    class: SymbolId,
    constructor: SymbolId,
    parameters: Vec<SymbolId>,
    accessor_bindings: Vec<JvmConstructorAccessorBinding>,
    visibility: Visibility,
    source: dotty_core::SourceId,
    direct_super_constructor: Option<JvmDirectSuperConstructorPlan>,
}

/// Read-only JVM constructor signature and instance-parameter slot metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmConstructorMetadata {
    symbol: SymbolId,
    owner_internal_name: String,
    descriptor: JvmMethodDescriptor,
    parameters: Vec<JvmParameterLocal>,
    access_flags: MethodAccessFlags,
}

impl JvmPrimaryConstructorPlan {
    /// Canonical primary constructor identity checked against source evidence.
    pub fn symbol(&self) -> SymbolId {
        self.constructor
    }

    /// Constructor-owned parameter identities in authoritative source order.
    pub fn parameters(&self) -> &[SymbolId] {
        &self.parameters
    }

    /// Source-index-derived accessor bindings in constructor parameter order.
    ///
    /// These bindings preserve canonical IDs and source provenance. Their
    /// semantic store invariants are revalidated before they are published in
    /// [`crate::JvmClassMetadata::accessor_bindings`].
    pub fn accessor_bindings(&self) -> &[JvmConstructorAccessorBinding] {
        &self.accessor_bindings
    }

    /// Canonical source-proven direct-parent constructor call, if the source
    /// template supplies supported parent arguments.
    pub fn direct_super_constructor(&self) -> Option<&JvmDirectSuperConstructorPlan> {
        self.direct_super_constructor.as_ref()
    }

    /// Attaches the exact mapped parent application and its typed arguments.
    /// `arguments` must be copied in order from this session's
    /// `SourceTypedIndex`; this adapter checks both arenas and the canonical
    /// selected constructor but never retypes source expressions.
    #[allow(clippy::too_many_arguments)]
    pub fn with_direct_super_constructor(
        mut self,
        parent_application: TreeId<Untyped>,
        typed_parent_application: TreeId<Typed>,
        arguments: Vec<JvmConstructorArgumentBinding>,
        ast: &AstArena<Untyped>,
        index: &SourceSemanticIndex,
        source_typed_index: &SourceTypedIndex,
        store: &SemanticStore,
        typed_ast: &TypedAst,
    ) -> Result<Self, JvmCodegenError> {
        let fail = |issue| JvmCodegenError::InvalidPrimaryConstructor {
            constructor: self.constructor,
            issue,
        };
        if self.direct_super_constructor.is_some() {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        }
        let Some(SourceDefinition::Canonical { source, tree }) = index.definition_of(self.class)
        else {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        };
        if source != self.source || index.symbol_at(source, tree) != Some(self.class) {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        let Some(TreeKind::TypeDef(definition)) = ast.try_get(tree).map(|node| &node.kind) else {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        };
        let Some(TreeKind::Template(template)) = ast.try_get(definition.rhs).map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        };
        if index.symbol_at(source, template.constructor) != Some(self.constructor)
            || template.parents.as_slice() != [parent_application]
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        }
        let Some(TreeKind::Apply(source_apply)) =
            ast.try_get(parent_application).map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        };
        if source_apply.kind != dotty_core::ast::ApplyKind::Regular
            || source_apply.args.len() != arguments.len()
            || source_apply
                .args
                .iter()
                .zip(&arguments)
                .any(|(source, binding)| *source != binding.source)
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorArgumentIdentity));
        }
        if source_typed_index.get(source, parent_application) != Some(typed_parent_application)
            || arguments.iter().any(|binding| {
                source_typed_index.get(source, binding.source) != Some(binding.typed)
            })
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorArgumentIdentity));
        }
        let Some(TreeKind::Select(source_select)) =
            ast.try_get(source_apply.function).map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        };
        if source_select.backquoted
            || store.names.try_resolve(source_select.name.text()) != Some("<init>")
            || !matches!(
                ast.try_get(source_select.qualifier).map(|node| &node.kind),
                Some(TreeKind::New(_))
            )
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorSourceShape));
        }

        let Some(TreeKind::Apply(typed_apply)) = typed_ast
            .try_get(typed_parent_application)
            .map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::ParentConstructorTypedShape));
        };
        let Some(TreeKind::Select(typed_select)) = typed_ast
            .try_get(typed_apply.function)
            .map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::ParentConstructorTypedShape));
        };
        if typed_select.backquoted
            || store.names.try_resolve(typed_select.name.text()) != Some("<init>")
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorTypedShape));
        }
        let Some(TreeKind::New(typed_new)) = typed_ast
            .try_get(typed_select.qualifier)
            .map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::ParentConstructorTypedShape));
        };
        let function_type = typed_ast
            .try_get(typed_apply.function)
            .and_then(|tree| store.types.try_get(tree.ty));
        let parent_constructor = match function_type {
            Some(Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => return Err(fail(JvmConstructorIssue::ParentConstructorIdentity)),
        };
        let parent = store
            .symbols
            .contains(parent_constructor)
            .then(|| store.symbols.get(parent_constructor))
            .ok_or_else(|| fail(JvmConstructorIssue::ParentConstructorIdentity))?;
        if parent.kind != SymbolKind::Constructor
            || parent.owner.is_none()
            || !matches!(
                typed_ast
                    .try_get(typed_new.tpt)
                    .and_then(|tree| store.types.try_get(tree.ty)),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(class),
                    ..
                }) if Some(*class) == parent.owner
            )
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorIdentity));
        }
        let parent_class = parent.owner.expect("checked above");
        if typed_apply.kind != dotty_core::ast::ApplyKind::Regular
            || typed_apply.args.len() != arguments.len()
            || typed_apply
                .args
                .iter()
                .zip(&arguments)
                .any(|(typed, binding)| *typed != binding.typed)
        {
            return Err(fail(JvmConstructorIssue::ParentConstructorArgumentIdentity));
        }
        let proof = JvmDirectSuperConstructorPlan {
            child_class: self.class,
            child_constructor: self.constructor,
            parent_class,
            parent_constructor,
            source_application: parent_application,
            typed_application: typed_parent_application,
            arguments,
            source,
        };
        proof.validate_typed_binding(store, typed_ast, self.class, self.constructor)?;
        self.direct_super_constructor = Some(proof);
        Ok(self)
    }

    pub(crate) const fn visibility(&self) -> Visibility {
        self.visibility
    }

    /// Checks frontend-collected identities against Template.constructor and
    /// the index's derived parameter mappings. Unsupported syntax is rejected
    /// here, before semantic signatures can erase defaults/accessor markers.
    pub fn from_source(
        class: SymbolId,
        constructor: SymbolId,
        parameters: Vec<SymbolId>,
        ast: &AstArena<Untyped>,
        index: &SourceSemanticIndex,
    ) -> Result<Self, JvmCodegenError> {
        let fail = |issue| JvmCodegenError::InvalidPrimaryConstructor { constructor, issue };
        let Some(SourceDefinition::Canonical { source, tree }) = index.definition_of(class) else {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        };
        if index.symbol_at(source, tree) != Some(class) {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        let Some(TreeKind::TypeDef(def)) = ast.try_get(tree).map(|node| &node.kind) else {
            return Err(fail(JvmConstructorIssue::SourceShape));
        };
        let Some(TreeKind::Template(template)) = ast.try_get(def.rhs).map(|node| &node.kind) else {
            return Err(fail(JvmConstructorIssue::SourceShape));
        };
        if index.symbol_at(source, template.constructor) != Some(constructor)
            || index.definition_of(constructor)
                != Some(SourceDefinition::Canonical {
                    source,
                    tree: template.constructor,
                })
        {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        let Some(TreeKind::DefDef(def)) = ast.try_get(template.constructor).map(|node| &node.kind)
        else {
            return Err(fail(JvmConstructorIssue::SourceShape));
        };
        if !def.type_params.is_empty()
            || def.value_param_clauses.len() > 1
            || !def.metadata.modifiers.is_empty()
            || !def.metadata.annotations.is_empty()
        {
            return Err(fail(JvmConstructorIssue::SourceShape));
        }
        let visibility = match def.metadata.visibility {
            None => Visibility::Public,
            Some(VisibilitySyntax::Private { qualifier: None }) => Visibility::Private,
            _ => return Err(fail(JvmConstructorIssue::Visibility)),
        };
        let source_parameters = def
            .value_param_clauses
            .first()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if parameters.len() != source_parameters.len() {
            return Err(fail(JvmConstructorIssue::ParameterIdentity));
        }
        let mut accessor_bindings = Vec::new();
        for (source_ordinal, (tree, parameter)) in
            source_parameters.iter().zip(&parameters).enumerate()
        {
            if index.derived_symbol_at(constructor, source, *tree) != Some(*parameter)
                || index.definition_of(*parameter)
                    != Some(SourceDefinition::Derived {
                        source,
                        tree: *tree,
                    })
            {
                return Err(fail(JvmConstructorIssue::ParameterIdentity));
            }
            let Some(TreeKind::ValDef(def)) = ast.try_get(*tree).map(|node| &node.kind) else {
                return Err(fail(JvmConstructorIssue::SourceShape));
            };
            if def.rhs.is_some()
                || def.metadata.visibility.is_some()
                || !def.metadata.annotations.is_empty()
            {
                return Err(fail(JvmConstructorIssue::SourceShape));
            }
            let mutable = match def.metadata.modifiers.as_slice() {
                [Modifier::ParamAccessor, Modifier::PrivateLocal] => continue,
                [Modifier::ParamAccessor] => false,
                [Modifier::ParamAccessor, Modifier::Var] => true,
                _ => return Err(fail(JvmConstructorIssue::SourceShape)),
            };
            let Some(field) = index.symbol_at(source, *tree) else {
                return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
            };
            if field == *parameter
                || index.definition_of(field)
                    != Some(SourceDefinition::Canonical {
                        source,
                        tree: *tree,
                    })
            {
                return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
            }
            accessor_bindings.push(JvmConstructorAccessorBinding {
                class,
                constructor,
                source,
                source_parameter: *tree,
                field,
                parameter: *parameter,
                source_ordinal,
                mutable,
            });
        }
        Ok(Self {
            class,
            constructor,
            parameters,
            accessor_bindings,
            visibility,
            source,
            direct_super_constructor: None,
        })
    }

    pub(crate) fn validate_binding(
        &self,
        store: &SemanticStore,
        owner: SymbolId,
    ) -> Result<(), JvmCodegenError> {
        let fail = |issue| JvmCodegenError::InvalidPrimaryConstructor {
            constructor: self.constructor,
            issue,
        };
        if owner != self.class
            || !store.symbols.contains(owner)
            || !store.symbols.contains(self.constructor)
        {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        let constructor = store.symbols.get(self.constructor);
        if constructor.kind != SymbolKind::Constructor
            || constructor.owner != Some(owner)
            || constructor.origin != dotty_core::SymbolOrigin::Source(self.source)
            || store.symbols.get(owner).origin != dotty_core::SymbolOrigin::Source(self.source)
            || constructor.name.namespace() != dotty_core::Namespace::Term
            || store.names.try_resolve(constructor.name.text()) != Some("<init>")
        {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        if constructor.visibility != self.visibility {
            return Err(fail(JvmConstructorIssue::Visibility));
        }
        if !constructor.flags.is_empty() {
            return Err(fail(JvmConstructorIssue::Flags));
        }
        for id in &self.parameters {
            if !store.symbols.contains(*id) {
                return Err(JvmCodegenError::UnknownParameter { parameter: *id });
            }
            let parameter = store.symbols.get(*id);
            if parameter.kind != SymbolKind::Parameter
                || parameter.owner != Some(self.constructor)
                || parameter.origin != dotty_core::SymbolOrigin::Source(self.source)
            {
                return Err(fail(JvmConstructorIssue::ParameterIdentity));
            }
        }
        if self.accessor_bindings.is_empty() {
            return Ok(());
        }
        let SymbolInfo::Complete(class_info_id) = store.symbols.get(owner).info else {
            return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
        };
        let Some(Type::ClassInfo(class_info)) = store.types.try_get(class_info_id) else {
            return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
        };
        if class_info.class != owner
            || !store.scopes.contains(class_info.declarations)
            || store.scopes.get(class_info.declarations).owner != Some(owner)
        {
            return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
        }
        let SymbolInfo::Complete(constructor_info) = constructor.info else {
            return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
        };
        let Some(Type::Method(constructor_signature)) = store.types.try_get(constructor_info)
        else {
            return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
        };
        let mut fields = HashSet::with_capacity(self.accessor_bindings.len());
        let mut formals = HashSet::with_capacity(self.accessor_bindings.len());
        let mut ordinals = HashSet::with_capacity(self.accessor_bindings.len());
        for binding in &self.accessor_bindings {
            if binding.class != owner
                || binding.constructor != self.constructor
                || binding.source != self.source
                || self.parameters.get(binding.source_ordinal) != Some(&binding.parameter)
                || binding.field == binding.parameter
                || !fields.insert(binding.field)
                || !formals.insert(binding.parameter)
                || !ordinals.insert(binding.source_ordinal)
            {
                return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
            }
            if !store.symbols.contains(binding.field) {
                return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
            }
            let field = store.symbols.get(binding.field);
            if field.kind != SymbolKind::Field
                || field.owner != Some(owner)
                || field.origin != dotty_core::SymbolOrigin::Source(self.source)
                || field.visibility != Visibility::Public
                || field.flags
                    != if binding.mutable {
                        dotty_core::SymbolFlags::MUTABLE
                    } else {
                        dotty_core::SymbolFlags::EMPTY
                    }
                || field.name != store.symbols.get(binding.parameter).name
            {
                return Err(fail(JvmConstructorIssue::AccessorFieldMetadata));
            }
            let declarations = store.scopes.get(class_info.declarations);
            let same_name = declarations.lookup_all(&field.name);
            if !same_name.contains(&binding.field)
                || same_name
                    .iter()
                    .filter(|symbol| {
                        store.symbols.contains(**symbol)
                            && store.symbols.get(**symbol).kind == SymbolKind::Field
                            && store.symbols.get(**symbol).owner == Some(owner)
                    })
                    .count()
                    != 1
            {
                return Err(fail(JvmConstructorIssue::AccessorFieldIdentity));
            }
            let parameter = store.symbols.get(binding.parameter);
            if !parameter.flags.is_empty() {
                return Err(fail(JvmConstructorIssue::AccessorFieldMetadata));
            }
            let SymbolInfo::Complete(field_type) = field.info else {
                return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
            };
            let SymbolInfo::Complete(parameter_type) = parameter.info else {
                return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
            };
            let Some(signature_parameter) =
                constructor_signature.params.get(binding.source_ordinal)
            else {
                return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
            };
            let Some(field_type) = store.types.try_get(field_type) else {
                return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
            };
            if store.types.try_get(parameter_type) != Some(field_type)
                || store.types.try_get(signature_parameter.ty) != Some(field_type)
            {
                return Err(fail(JvmConstructorIssue::AccessorFieldSignature));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn metadata(
        &self,
        store: &SemanticStore,
        definitions: &Definitions,
        owner: SymbolId,
        class_identities: Option<&[crate::JvmClassIdentity]>,
        superclass: Option<&crate::JvmSuperclassPlan>,
    ) -> Result<JvmConstructorMetadata, JvmCodegenError> {
        self.metadata_with_unit(
            store,
            definitions,
            owner,
            class_identities,
            superclass,
            None,
        )
    }

    pub(crate) fn metadata_with_unit(
        &self,
        store: &SemanticStore,
        definitions: &Definitions,
        owner: SymbolId,
        class_identities: Option<&[crate::JvmClassIdentity]>,
        superclass: Option<&crate::JvmSuperclassPlan>,
        unit: Option<&crate::JvmCompilationUnit<'_>>,
    ) -> Result<JvmConstructorMetadata, JvmCodegenError> {
        let fail = |issue| JvmCodegenError::InvalidPrimaryConstructor {
            constructor: self.constructor,
            issue,
        };
        if owner != self.class {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        self.validate_binding(store, owner)?;
        let identity = crate::identity::class_identity(store, owner)?;
        let info = crate::identity::class_info(store, owner)?;
        let valid_parent = match superclass {
            Some(superclass) if superclass.class() == owner => {
                matches!(
                    info.parents.as_slice(),
                    [parent]
                        if matches!(
                            store.types.try_get(*parent),
                            Some(dotty_core::Type::TypeRef {
                                prefix,
                                target: dotty_core::TypeRefTarget::Symbol(symbol),
                            }) if *symbol == superclass.parent()
                                && crate::identity::canonical_class_parent_prefix(
                                    store,
                                    *prefix,
                                    *symbol,
                                    definitions.no_prefix,
                                )
                        )
                )
            }
            _ => {
                info.parents.as_slice() == [definitions.object_type]
                    && matches!(store.types.try_get(definitions.object_type), Some(dotty_core::Type::TypeRef { prefix, target: dotty_core::TypeRefTarget::Symbol(symbol) }) if *prefix == definitions.no_prefix && *symbol == definitions.object_class)
            }
        };
        if info.self_type.is_some() || !valid_parent {
            return Err(JvmCodegenError::UnsupportedJvmSuperclass {
                class: owner,
                parent: info.parents.first().copied(),
            });
        }
        let mut constructors = Vec::new();
        for id in store.scopes.get(info.declarations).entered_symbols() {
            if !store.symbols.contains(id) {
                return Err(JvmCodegenError::InvalidClassDeclaration {
                    class: owner,
                    symbol: id,
                });
            }
            if store.symbols.get(id).kind == SymbolKind::Constructor {
                constructors.push(id);
            }
        }
        if constructors.len() > 1 {
            return Err(fail(JvmConstructorIssue::SecondaryConstructor));
        }
        if constructors.as_slice() != [self.constructor] {
            return Err(fail(JvmConstructorIssue::DeclarationScope));
        }
        let constructor = store.symbols.get(self.constructor);
        if constructor.visibility != self.visibility {
            return Err(fail(JvmConstructorIssue::Visibility));
        }
        if !constructor.flags.is_empty() {
            return Err(fail(JvmConstructorIssue::Flags));
        }
        let plan = JvmMethodPlan {
            method: self.constructor,
            parameters: self.parameters.clone(),
            locals: Vec::new(),
            body: None,
        };
        let descriptor = match class_identities {
            Some(class_identities) => {
                crate::descriptor::constructor_descriptor_with_class_identities(
                    &plan,
                    store,
                    owner,
                    definitions,
                    class_identities,
                    unit,
                )?
            }
            None => crate::descriptor::descriptor_for(&plan, store, owner, definitions, true)?,
        };
        let SymbolInfo::Complete(ty) = constructor.info else {
            unreachable!("descriptor validated info")
        };
        let dotty_core::Type::Method(signature) = store.types.get(ty) else {
            unreachable!("descriptor validated shape")
        };
        let mut parameters = Vec::with_capacity(self.parameters.len());
        let mut slot = 1u16; // Slot zero is the instance receiver.
        for ((id, ty), signature_param) in self
            .parameters
            .iter()
            .zip(descriptor.parameters())
            .zip(&signature.params)
        {
            let parameter = store.symbols.get(*id);
            let signature_type_matches = match (ty, store.types.try_get(signature_param.ty)) {
                (crate::JvmType::Int, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.int)
                }
                (crate::JvmType::Boolean, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.boolean)
                }
                (crate::JvmType::Float, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.float)
                }
                (crate::JvmType::Long, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.long)
                }
                (crate::JvmType::Double, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.double)
                }
                (
                    crate::JvmType::Reference(class),
                    Some(dotty_core::Type::TypeRef {
                        target: dotty_core::TypeRefTarget::Symbol(symbol),
                        ..
                    }),
                ) => *class == *symbol,
                _ => false,
            };
            if !parameter.flags.is_empty()
                || parameter.name.text() != signature_param.name.as_name().text()
                || !signature_type_matches
            {
                return Err(fail(JvmConstructorIssue::ParameterSignature));
            }
            parameters.push(JvmParameterLocal {
                symbol: *id,
                ty: *ty,
                slot,
                width: ty.slot_width(),
            });
            slot += u16::from(ty.slot_width());
        }
        let access_flags = MethodAccessFlags(match self.visibility {
            Visibility::Public => ACC_PUBLIC,
            Visibility::Private => ACC_PRIVATE,
            _ => return Err(fail(JvmConstructorIssue::Visibility)),
        });
        Ok(JvmConstructorMetadata {
            symbol: self.constructor,
            owner_internal_name: identity.internal_name().to_owned(),
            descriptor,
            parameters,
            access_flags,
        })
    }
}

fn exact_type_matches(
    store: &SemanticStore,
    actual: dotty_core::TypeId,
    expected: dotty_core::TypeId,
) -> bool {
    matches!(
        (store.types.try_get(actual), store.types.try_get(expected)),
        (Some(actual), Some(expected)) if actual == expected
    )
}

impl JvmConstructorMetadata {
    /// Canonical primary constructor symbol.
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }
    /// Canonical owning class's JVM internal name.
    pub fn owner_internal_name(&self) -> &str {
        &self.owner_internal_name
    }
    /// JVM descriptor, always returning void.
    pub fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }
    /// Constructor-owned identities in source parameter order, after slot zero.
    pub fn parameters(&self) -> &[JvmParameterLocal] {
        &self.parameters
    }
    /// Proven public or private constructor accessibility.
    pub fn access_flags(&self) -> MethodAccessFlags {
        self.access_flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::{AstArena, Tree, TypeTree};
    use dotty_core::{MethodType, Name, Namespace, Scope, Symbol, SymbolLinks, SymbolOrigin, Type};

    #[test]
    fn primitive_constructor_type_match_rejects_noncanonical_prefixes() {
        for is_int in [true, false] {
            let (mut store, definitions, mut plan) = fixture();
            let canonical = if is_int {
                definitions.int
            } else {
                definitions.boolean
            };
            let malformed_prefix = store.types.alloc(Type::NoType);
            let Type::TypeRef { target, .. } = store.types.get(canonical) else {
                panic!("primitive definition is a TypeRef")
            };
            let malformed = store.types.alloc(Type::TypeRef {
                prefix: malformed_prefix,
                target: *target,
            });
            assert!(exact_type_matches(&store, canonical, canonical));
            assert!(!exact_type_matches(&store, malformed, canonical));

            let parameter_name = store.names.intern("value");
            let parameter = store.symbols.alloc(Symbol {
                name: Name::new(parameter_name, Namespace::Term),
                owner: Some(plan.constructor),
                kind: SymbolKind::Parameter,
                flags: dotty_core::SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Complete(malformed),
                origin: SymbolOrigin::Source(plan.source),
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            let SymbolInfo::Complete(constructor_type) = store.symbols.get(plan.constructor).info
            else {
                panic!("fixture constructor is complete")
            };
            let Type::Method(mut signature) = store.types.get(constructor_type).clone() else {
                panic!("fixture constructor has a method signature")
            };
            signature.params.push(dotty_core::MethodParam {
                name: dotty_core::TermName::new(parameter_name),
                ty: malformed,
                erased: false,
                varargs: false,
            });
            let constructor_type = store.types.alloc(Type::Method(signature));
            store
                .symbols
                .set_info(plan.constructor, SymbolInfo::Complete(constructor_type));
            plan.parameters.push(parameter);

            assert!(matches!(
                plan.metadata(&store, &definitions, plan.class, None, None),
                Err(JvmCodegenError::InvalidPrimaryConstructor {
                    issue: JvmConstructorIssue::ParameterSignature,
                    ..
                })
            ));
        }
    }

    fn fixture() -> (SemanticStore, Definitions, JvmPrimaryConstructorPlan) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source = dotty_core::SourceId::from_index(1);
        let mut alloc = |text: &str, namespace, kind, owner| {
            let name = Name::new(store.names.intern(text), namespace);
            store.symbols.alloc(Symbol {
                name,
                kind,
                owner,
                flags: dotty_core::SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Source(source),
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            })
        };
        let package = alloc("", Namespace::Term, SymbolKind::Package, None);
        let class = alloc("C", Namespace::Type, SymbolKind::Class, Some(package));
        let constructor = alloc(
            "<init>",
            Namespace::Term,
            SymbolKind::Constructor,
            Some(class),
        );
        let scope = store.scopes.alloc(Scope::new(Some(class)));
        store
            .scopes
            .get_mut(scope)
            .enter(store.symbols.get(constructor).name, constructor);
        let info = store.types.alloc(Type::ClassInfo(dotty_core::ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: vec![definitions.object_type],
            declarations: scope,
            self_type: None,
        }));
        store.symbols.set_info(class, SymbolInfo::Complete(info));
        let result = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let signature = store.types.alloc(Type::Method(MethodType {
            params: Vec::new(),
            result,
            kind: dotty_core::MethodKind::Plain,
        }));
        store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(signature));
        (
            store,
            definitions,
            JvmPrimaryConstructorPlan {
                class,
                constructor,
                parameters: Vec::new(),
                accessor_bindings: Vec::new(),
                visibility: Visibility::Public,
                source,
                direct_super_constructor: None,
            },
        )
    }

    fn accessor_fixture() -> (SemanticStore, Definitions, JvmPrimaryConstructorPlan) {
        let (mut store, definitions, mut plan) = fixture();
        let class = plan.class;
        let constructor = plan.constructor;
        let source = plan.source;
        let mut ast = AstArena::<Untyped>::new();
        let source_parameter = ast.alloc(Tree {
            kind: TreeKind::TypeTree(TypeTree),
            position: None,
            ty: (),
        });
        let name = Name::new(store.names.intern("x"), Namespace::Term);
        let field = store.symbols.alloc(Symbol {
            name,
            owner: Some(class),
            kind: SymbolKind::Field,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.int),
            origin: SymbolOrigin::Source(source),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let parameter = store.symbols.alloc(Symbol {
            name,
            owner: Some(constructor),
            kind: SymbolKind::Parameter,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.int),
            origin: SymbolOrigin::Source(source),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let SymbolInfo::Complete(constructor_type) = store.symbols.get(constructor).info else {
            panic!("fixture constructor type is complete");
        };
        let Type::Method(mut signature) = store.types.get(constructor_type).clone() else {
            panic!("fixture constructor has a method type");
        };
        signature.params.push(dotty_core::MethodParam {
            name: dotty_core::TermName::new(name.text()),
            ty: definitions.int,
            erased: false,
            varargs: false,
        });
        let constructor_type = store.types.alloc(Type::Method(signature));
        store
            .symbols
            .set_info(constructor, SymbolInfo::Complete(constructor_type));
        let SymbolInfo::Complete(class_type) = store.symbols.get(class).info else {
            panic!("fixture class info is complete");
        };
        let Type::ClassInfo(class_info) = store.types.get(class_type) else {
            panic!("fixture class info has a class scope");
        };
        store
            .scopes
            .get_mut(class_info.declarations)
            .enter(name, field);
        plan.parameters.push(parameter);
        plan.accessor_bindings.push(JvmConstructorAccessorBinding {
            class,
            constructor,
            source,
            source_parameter,
            field,
            parameter,
            source_ordinal: 0,
            mutable: false,
        });
        (store, definitions, plan)
    }

    #[test]
    fn accessor_binding_revalidation_rejects_stale_mismatched_and_duplicate_metadata() {
        use JvmConstructorIssue as Issue;

        enum Malformation {
            WrongOwner,
            WrongKind,
            WrongMutability,
            WrongType,
            MissingScopeEntry,
            DuplicateField,
            WrongOrdinal,
            WrongSource,
        }

        for (malformation, expected) in [
            (Malformation::WrongOwner, Issue::AccessorFieldMetadata),
            (Malformation::WrongKind, Issue::AccessorFieldMetadata),
            (Malformation::WrongMutability, Issue::AccessorFieldMetadata),
            (Malformation::WrongType, Issue::AccessorFieldSignature),
            (
                Malformation::MissingScopeEntry,
                Issue::AccessorFieldIdentity,
            ),
            (Malformation::DuplicateField, Issue::AccessorFieldIdentity),
            (Malformation::WrongOrdinal, Issue::AccessorFieldIdentity),
            (Malformation::WrongSource, Issue::AccessorFieldIdentity),
        ] {
            let (mut store, definitions, mut plan) = accessor_fixture();
            let binding = plan.accessor_bindings[0];
            let class = plan.class;
            let constructor = plan.constructor;
            let field = binding.field;
            let SymbolInfo::Complete(class_type) = store.symbols.get(class).info else {
                panic!("fixture class info is complete");
            };
            let Type::ClassInfo(class_info) = store.types.get(class_type) else {
                panic!("fixture class info has a class scope");
            };
            let scope = class_info.declarations;
            match malformation {
                Malformation::WrongOwner => {
                    store.symbols.get_mut(field).owner = Some(constructor);
                }
                Malformation::WrongKind => {
                    store.symbols.get_mut(field).kind = SymbolKind::Parameter;
                }
                Malformation::WrongMutability => {
                    store.symbols.get_mut(field).flags = dotty_core::SymbolFlags::MUTABLE;
                }
                Malformation::WrongType => {
                    store
                        .symbols
                        .set_info(field, SymbolInfo::Complete(definitions.boolean));
                }
                Malformation::MissingScopeEntry => {
                    store.scopes.get_mut(scope).remove(field);
                }
                Malformation::DuplicateField => {
                    let duplicate = store.symbols.alloc(store.symbols.get(field).clone());
                    store
                        .scopes
                        .get_mut(scope)
                        .enter(store.symbols.get(field).name, duplicate);
                }
                Malformation::WrongOrdinal => {
                    plan.accessor_bindings[0].source_ordinal = 1;
                }
                Malformation::WrongSource => {
                    plan.accessor_bindings[0].source = dotty_core::SourceId::from_index(2);
                }
            }
            assert!(matches!(
                plan.validate_binding(&store, class),
                Err(JvmCodegenError::InvalidPrimaryConstructor {
                    issue,
                    ..
                }) if issue == expected
            ));
        }
    }

    #[test]
    fn malformed_constructor_metadata_fails_without_mutation() {
        let (mut store, definitions, plan) = fixture();
        let checkpoint = store.checkpoint();
        assert_eq!(
            plan.metadata(&store, &definitions, plan.class, None, None)
                .unwrap()
                .descriptor()
                .as_str(),
            "()V"
        );
        assert_eq!(store.checkpoint(), checkpoint);
        let original = store.symbols.get(plan.constructor).clone();
        store.symbols.get_mut(plan.constructor).owner = None;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::SourceIdentity,
                ..
            })
        ));
        *store.symbols.get_mut(plan.constructor) = original.clone();
        store.symbols.get_mut(plan.constructor).kind = SymbolKind::Method;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::SourceIdentity,
                ..
            })
        ));
        *store.symbols.get_mut(plan.constructor) = original.clone();
        store.symbols.get_mut(plan.constructor).visibility = Visibility::Private;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::Visibility,
                ..
            })
        ));
        *store.symbols.get_mut(plan.constructor) = original.clone();
        let unfilled = store.types.reserve().id();
        for info in [
            SymbolInfo::Missing,
            SymbolInfo::Error,
            SymbolInfo::Complete(unfilled),
        ] {
            store.symbols.set_info(plan.constructor, info);
            assert!(
                plan.metadata(&store, &definitions, plan.class, None, None)
                    .is_err()
            );
            let ast = dotty_core::ast::TypedAst::new();
            let unit = crate::JvmCompilationUnit {
                store: &store,
                typed_ast: &ast,
                classes: vec![crate::JvmClassPlan {
                    class: plan.class,
                    fields: Vec::new(),
                    methods: Vec::new(),
                    initialization: crate::JvmClassInitializationPlan::PrimaryConstructor(
                        plan.clone(),
                    ),
                }],
            };
            assert!(unit.validate().is_err());
        }
        *store.symbols.get_mut(plan.constructor) = original;
        let SymbolInfo::Complete(ty) = store.symbols.get(plan.constructor).info else {
            panic!()
        };
        let Type::Method(mut signature) = store.types.get(ty).clone() else {
            panic!()
        };
        signature.result = definitions.int;
        let wrong_result = store.types.alloc(Type::Method(signature));
        store
            .symbols
            .set_info(plan.constructor, SymbolInfo::Complete(wrong_result));
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::ResultType,
                ..
            })
        ));
    }
    #[test]
    fn stale_parameter_and_constructor_identities_are_typed_errors() {
        let (mut store, definitions, mut plan) = fixture();
        plan.parameters.push(plan.class);
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::ParameterIdentity,
                ..
            })
        ));
        plan.parameters.clear();
        let valid = plan.clone();
        let mut other = SemanticStore::new();
        let stale = loop {
            let name =
                dotty_core::Name::new(other.names.intern("stale"), dotty_core::Namespace::Term);
            let id = other.symbols.alloc(Symbol {
                name,
                owner: None,
                kind: SymbolKind::Parameter,
                flags: dotty_core::SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            if !store.symbols.contains(id) {
                break id;
            }
        };
        plan.parameters.push(stale);
        assert_eq!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::UnknownParameter { parameter: stale })
        );
        plan.parameters.clear();
        plan.constructor = stale;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::SourceIdentity,
                ..
            })
        ));
        // A valid source proof also fails closed if the class signature is corrupted.
        store
            .symbols
            .set_info(valid.class, SymbolInfo::Complete(definitions.int));
        assert!(matches!(
            valid.metadata(&store, &definitions, valid.class, None, None),
            Err(JvmCodegenError::InvalidJvmClassInfo { .. })
        ));
    }
}
