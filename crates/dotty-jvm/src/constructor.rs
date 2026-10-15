//! Source identity proof and bounded primary-constructor metadata.
use crate::{JvmCodegenError, JvmMethodDescriptor, JvmMethodPlan, JvmParameterLocal};
use dotty_classfile::access_flags::{ACC_PRIVATE, ACC_PUBLIC, MethodAccessFlags};
use dotty_core::ast::{AstArena, Modifier, TreeKind, Untyped, VisibilitySyntax};
use dotty_core::{
    Definitions, SemanticStore, SourceDefinition, SourceSemanticIndex, SymbolId, SymbolInfo,
    SymbolKind, Visibility,
};

/// Why an explicit primary constructor cannot be projected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmConstructorIssue {
    SourceIdentity,
    SourceShape,
    ParameterIdentity,
    ParameterSignature,
    DeclarationScope,
    SecondaryConstructor,
    ResultType,
    Visibility,
    Flags,
}

/// Source-checked primary constructor binding. Its private fields retain the
/// exact constructor/parameter identities proven against the shared source AST
/// and semantic index. Creating this proof never parses or types source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmPrimaryConstructorPlan {
    class: SymbolId,
    constructor: SymbolId,
    parameters: Vec<SymbolId>,
    visibility: Visibility,
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
            || def.value_param_clauses.len() != 1
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
        let source_parameters = &def.value_param_clauses[0];
        if parameters.len() != source_parameters.len() {
            return Err(fail(JvmConstructorIssue::ParameterIdentity));
        }
        for (tree, parameter) in source_parameters.iter().zip(&parameters) {
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
                || !(def.metadata.modifiers.len() == 2
                    && def.metadata.modifiers.contains(&Modifier::ParamAccessor)
                    && def.metadata.modifiers.contains(&Modifier::PrivateLocal))
            {
                return Err(fail(JvmConstructorIssue::SourceShape));
            }
        }
        Ok(Self {
            class,
            constructor,
            parameters,
            visibility,
        })
    }

    pub(crate) fn metadata(
        &self,
        store: &SemanticStore,
        definitions: &Definitions,
        owner: SymbolId,
    ) -> Result<JvmConstructorMetadata, JvmCodegenError> {
        let fail = |issue| JvmCodegenError::InvalidPrimaryConstructor {
            constructor: self.constructor,
            issue,
        };
        if owner != self.class {
            return Err(fail(JvmConstructorIssue::SourceIdentity));
        }
        let identity = crate::identity::class_identity(store, owner)?;
        let info = crate::identity::class_info(store, owner)?;
        if info.self_type.is_some() || info.parents.as_slice() != [definitions.object_type] {
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
        let descriptor = crate::descriptor::descriptor_for(&plan, store, owner, definitions, true)?;
        let SymbolInfo::Complete(ty) = constructor.info else {
            unreachable!("descriptor validated info")
        };
        let dotty_core::Type::Method(signature) = store.types.get(ty) else {
            unreachable!("descriptor validated shape")
        };
        let mut parameters = Vec::new();
        for (i, ((id, ty), signature_param)) in self
            .parameters
            .iter()
            .zip(descriptor.parameters())
            .zip(&signature.params)
            .enumerate()
        {
            let parameter = store.symbols.get(*id);
            if !parameter.flags.is_empty()
                || parameter.name.text() != signature_param.name.as_name().text()
            {
                return Err(fail(JvmConstructorIssue::ParameterSignature));
            }
            parameters.push(JvmParameterLocal {
                symbol: *id,
                ty: *ty,
                slot: (i + 1) as u16,
                width: 1,
            });
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
