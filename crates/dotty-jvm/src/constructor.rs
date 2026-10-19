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
    source: dotty_core::SourceId,
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
            source,
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
        Ok(())
    }

    pub(crate) fn metadata(
        &self,
        store: &SemanticStore,
        definitions: &Definitions,
        owner: SymbolId,
        class_identities: Option<&[crate::JvmClassIdentity]>,
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
        if info.self_type.is_some()
            || info.parents.as_slice() != [definitions.object_type]
            || !matches!(store.types.try_get(definitions.object_type), Some(dotty_core::Type::TypeRef { target: dotty_core::TypeRefTarget::Symbol(symbol), .. }) if *symbol == definitions.object_class)
        {
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
        let mut parameters = Vec::new();
        for (i, ((id, ty), signature_param)) in self
            .parameters
            .iter()
            .zip(descriptor.parameters())
            .zip(&signature.params)
            .enumerate()
        {
            let parameter = store.symbols.get(*id);
            let signature_type_matches = match (ty, store.types.try_get(signature_param.ty)) {
                (crate::JvmType::Int, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.int)
                }
                (crate::JvmType::Boolean, Some(dotty_core::Type::TypeRef { .. })) => {
                    exact_type_matches(store, signature_param.ty, definitions.boolean)
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
                plan.metadata(&store, &definitions, plan.class, None),
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
                visibility: Visibility::Public,
                source,
            },
        )
    }

    #[test]
    fn malformed_constructor_metadata_fails_without_mutation() {
        let (mut store, definitions, plan) = fixture();
        let checkpoint = store.checkpoint();
        assert_eq!(
            plan.metadata(&store, &definitions, plan.class, None)
                .unwrap()
                .descriptor()
                .as_str(),
            "()V"
        );
        assert_eq!(store.checkpoint(), checkpoint);
        let original = store.symbols.get(plan.constructor).clone();
        store.symbols.get_mut(plan.constructor).owner = None;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::SourceIdentity,
                ..
            })
        ));
        *store.symbols.get_mut(plan.constructor) = original.clone();
        store.symbols.get_mut(plan.constructor).kind = SymbolKind::Method;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None),
            Err(JvmCodegenError::InvalidPrimaryConstructor {
                issue: JvmConstructorIssue::SourceIdentity,
                ..
            })
        ));
        *store.symbols.get_mut(plan.constructor) = original.clone();
        store.symbols.get_mut(plan.constructor).visibility = Visibility::Private;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None),
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
                plan.metadata(&store, &definitions, plan.class, None)
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
            plan.metadata(&store, &definitions, plan.class, None),
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
            plan.metadata(&store, &definitions, plan.class, None),
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
            plan.metadata(&store, &definitions, plan.class, None),
            Err(JvmCodegenError::UnknownParameter { parameter: stale })
        );
        plan.parameters.clear();
        plan.constructor = stale;
        assert!(matches!(
            plan.metadata(&store, &definitions, plan.class, None),
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
            valid.metadata(&store, &definitions, valid.class, None),
            Err(JvmCodegenError::InvalidJvmClassInfo { .. })
        ));
    }
}
