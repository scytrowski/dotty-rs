//! JVM names and class identity derived from canonical semantic symbols.

use std::collections::{HashMap, HashSet};

use dotty_classfile::access_flags::{
    ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC, ACC_SUPER, ClassAccessFlags, FieldAccessFlags,
    MethodAccessFlags,
};
use dotty_core::definitions::Definitions;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, Visibility};
use dotty_core::types::{Type, TypeRefTarget};

use crate::{
    InvalidJvmClassInfoKind, JvmClassIdentity, JvmClassMetadata, JvmCodegenError, JvmFieldMetadata,
    JvmFieldPlan, JvmMethodDispatchMetadata, JvmMethodMetadata, JvmNameKind, JvmOverrideIssue,
    JvmParameterLocal, UnsupportedJvmClassKind, UnsupportedJvmFieldFlag, UnsupportedJvmSymbolFlag,
};

pub(super) fn class_identity(
    store: &SemanticStore,
    class_id: dotty_core::SymbolId,
) -> Result<JvmClassIdentity, JvmCodegenError> {
    let class = store
        .symbols
        .contains(class_id)
        .then(|| store.symbols.get(class_id))
        .ok_or(JvmCodegenError::UnknownClass { class: class_id })?;
    if class.kind != SymbolKind::Class {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: class_id,
            expected: crate::SymbolRole::Class,
            found: class.kind,
        });
    }
    if class.visibility != Visibility::Public {
        return Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: class_id,
            visibility: class.visibility,
        });
    }

    validate_class_flags(class_id, class.flags)?;
    validate_class_info(store, class_id, class.info)?;

    let class_name = store
        .names
        .try_resolve(class.name.text())
        .ok_or_else(|| invalid_name(class_id, JvmNameKind::Class, "<unresolved>"))?;
    if class.name.namespace() != Namespace::Type || !is_simple_identifier(class_name) {
        return Err(invalid_name(
            class_id,
            JvmNameKind::Class,
            class_name.to_owned(),
        ));
    }

    let mut package_segments = Vec::new();
    let mut seen_owners = HashSet::new();
    let mut owner = class
        .owner
        .ok_or(JvmCodegenError::InvalidPackageOwnerChain {
            class: class_id,
            owner: class_id,
        })?;

    loop {
        if !seen_owners.insert(owner) {
            return Err(JvmCodegenError::InvalidPackageOwnerChain {
                class: class_id,
                owner,
            });
        }
        let package = store
            .symbols
            .contains(owner)
            .then(|| store.symbols.get(owner))
            .ok_or(JvmCodegenError::UnknownOwner {
                class: class_id,
                owner,
            })?;
        if package.kind != SymbolKind::Package {
            return Err(JvmCodegenError::UnsupportedOwnerKind {
                class: class_id,
                owner,
                found: package.kind,
            });
        }

        let segment = store
            .names
            .try_resolve(package.name.text())
            .ok_or_else(|| invalid_name(owner, JvmNameKind::PackageSegment, "<unresolved>"))?;
        if package.name.namespace() != Namespace::Term {
            return Err(invalid_name(
                owner,
                JvmNameKind::PackageSegment,
                segment.to_owned(),
            ));
        }
        match package.owner {
            None if segment.is_empty() => break,
            None => {
                return Err(JvmCodegenError::InvalidPackageOwnerChain {
                    class: class_id,
                    owner,
                });
            }
            Some(_) if segment.is_empty() => {
                return Err(JvmCodegenError::InvalidPackageOwnerChain {
                    class: class_id,
                    owner,
                });
            }
            Some(parent) => {
                if !is_simple_identifier(segment) {
                    return Err(invalid_name(
                        owner,
                        JvmNameKind::PackageSegment,
                        segment.to_owned(),
                    ));
                }
                package_segments.push(segment.to_owned());
                owner = parent;
            }
        }
    }

    package_segments.reverse();
    package_segments.push(class_name.to_owned());
    let internal_name = package_segments.join("/");
    Ok(JvmClassIdentity {
        symbol: class_id,
        internal_name,
        access_flags: ClassAccessFlags(
            ACC_PUBLIC
                | ACC_SUPER
                | if class.flags.contains(SymbolFlags::FINAL) {
                    ACC_FINAL
                } else {
                    0
                },
        ),
    })
}

pub(super) fn unit_class_identities(
    unit: &crate::JvmCompilationUnit<'_>,
) -> Result<Vec<JvmClassIdentity>, JvmCodegenError> {
    let mut identities = Vec::with_capacity(unit.classes.len());
    let mut by_name = HashMap::<String, dotty_core::SymbolId>::new();
    for plan in &unit.classes {
        let identity = class_identity(unit.store, plan.class)?;
        let name = identity.internal_name().to_owned();
        if let Some(previous) = by_name.insert(name.clone(), identity.symbol()) {
            return Err(JvmCodegenError::DuplicateJvmClassIdentity {
                class: identity.symbol(),
                previous,
                internal_name: name,
            });
        }
        identities.push(identity);
    }
    Ok(identities)
}

pub(super) fn unit_hierarchy(
    unit: &crate::JvmCompilationUnit<'_>,
    definitions: &Definitions,
) -> Result<crate::JvmClassHierarchy, JvmCodegenError> {
    unit.validate()?;
    let plans = unit
        .classes
        .iter()
        .map(|plan| (plan.class, plan))
        .collect::<HashMap<_, _>>();
    for plan in &unit.classes {
        let info = class_info(unit.store, plan.class)?;
        if let [parent_type] = info.parents.as_slice()
            && let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(parent),
                ..
            }) = unit.store.types.try_get(*parent_type)
            && plans.contains_key(parent)
            && unit.store.symbols.get(*parent).visibility != Visibility::Public
        {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(*parent),
                issue: crate::JvmSuperclassIssue::ParentNotPublic,
            });
        }
    }
    let identities = unit_class_identities(unit)?;
    let identity_by_symbol = identities
        .iter()
        .map(|identity| (identity.symbol(), identity))
        .collect::<HashMap<_, _>>();
    let mut parents = HashMap::<dotty_core::SymbolId, dotty_core::SymbolId>::new();

    for plan in &unit.classes {
        let info = class_info(unit.store, plan.class)?;
        if let Some(self_type) = info.self_type {
            return Err(JvmCodegenError::UnsupportedJvmSelfType {
                class: plan.class,
                self_type,
            });
        }
        let [parent_type] = info.parents.as_slice() else {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: None,
                issue: crate::JvmSuperclassIssue::MissingOrMultipleParents,
            });
        };
        let parent = match unit.store.types.try_get(*parent_type) {
            Some(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(parent),
            }) if canonical_class_parent_prefix(
                unit.store,
                *prefix,
                *parent,
                definitions.no_prefix,
            ) =>
            {
                *parent
            }
            _ => {
                return Err(JvmCodegenError::InvalidJvmSuperclass {
                    class: plan.class,
                    parent: None,
                    issue: crate::JvmSuperclassIssue::NonCanonicalParentType,
                });
            }
        };
        if parent == plan.class {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::SelfParent,
            });
        }
        if !unit.store.symbols.contains(parent) {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::WrongParentKind,
            });
        }
        let parent_symbol = unit.store.symbols.get(parent);
        if parent_symbol.kind != SymbolKind::Class {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::WrongParentKind,
            });
        }
        if parent == definitions.object_class && *parent_type != definitions.object_type {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::MissingObjectRoot,
            });
        }
        if parent != definitions.object_class && !plans.contains_key(&parent) {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::ParentNotPlanned,
            });
        }
        parents.insert(plan.class, parent);
    }

    for plan in &unit.classes {
        let mut cursor = plan.class;
        let mut path = HashSet::new();
        loop {
            if !path.insert(cursor) {
                return Err(JvmCodegenError::InvalidJvmSuperclass {
                    class: plan.class,
                    parent: parents.get(&cursor).copied(),
                    issue: crate::JvmSuperclassIssue::InheritanceCycle,
                });
            }
            let Some(parent) = parents.get(&cursor).copied() else {
                break;
            };
            if parent == definitions.object_class {
                break;
            }
            cursor = parent;
        }
    }

    for plan in &unit.classes {
        let mut cursor = plan.class;
        let mut planned_classes = 1;
        while let Some(parent) = parents.get(&cursor).copied() {
            if parent == definitions.object_class {
                break;
            }
            planned_classes += 1;
            if planned_classes > 3 {
                return Err(JvmCodegenError::InvalidJvmSuperclass {
                    class: plan.class,
                    parent: parents.get(&plan.class).copied(),
                    issue: crate::JvmSuperclassIssue::InheritanceTooDeep,
                });
            }
            cursor = parent;
        }
    }

    for plan in &unit.classes {
        let parent = parents[&plan.class];
        if parent == definitions.object_class {
            continue;
        }
        let parent_symbol = unit.store.symbols.get(parent);
        if parent_symbol.visibility != Visibility::Public {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::ParentNotPublic,
            });
        }
        if parent_symbol.flags.contains(SymbolFlags::FINAL) {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::ParentIsFinal,
            });
        }
        let parent_info = class_info(unit.store, parent)?;
        if parent_info.self_type.is_some() {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::ParentHasSelfType,
            });
        }
        let parent_plan = plans[&parent];
        validate_accessible_parent_constructor(unit, plan.class, plan, parent_plan, parent_info)?;
        if class_identity(unit.store, parent).is_err() {
            return Err(JvmCodegenError::InvalidJvmSuperclass {
                class: plan.class,
                parent: Some(parent),
                issue: crate::JvmSuperclassIssue::ParentUnsupportedShape,
            });
        }
    }

    let mut superclasses = Vec::with_capacity(unit.classes.len());
    for plan in &unit.classes {
        let parent = parents[&plan.class];
        let parent_internal_name = if parent == definitions.object_class {
            "java/lang/Object".to_owned()
        } else {
            identity_by_symbol[&parent].internal_name().to_owned()
        };
        let class_identity = identity_by_symbol[&plan.class];
        superclasses.push(crate::JvmSuperclassPlan {
            class: plan.class,
            parent,
            class_internal_name: class_identity.internal_name().to_owned(),
            parent_internal_name,
        });
    }
    Ok(crate::JvmClassHierarchy { superclasses })
}

pub(super) fn canonical_class_parent_prefix(
    store: &SemanticStore,
    prefix: dotty_core::TypeId,
    parent: dotty_core::SymbolId,
    no_prefix: dotty_core::TypeId,
) -> bool {
    if prefix == no_prefix {
        return true;
    }
    let Some(Type::TypeRef {
        prefix: package_prefix,
        target: TypeRefTarget::Symbol(package),
    }) = store.types.try_get(prefix)
    else {
        return false;
    };
    *package_prefix == no_prefix
        && store.symbols.contains(parent)
        && store.symbols.contains(*package)
        && store.symbols.get(parent).owner == Some(*package)
        && store.symbols.get(*package).kind == SymbolKind::Package
}

fn validate_accessible_parent_constructor(
    unit: &crate::JvmCompilationUnit<'_>,
    child: dotty_core::SymbolId,
    child_plan: &crate::JvmClassPlan,
    parent_plan: &crate::JvmClassPlan,
    info: &dotty_core::types::ClassInfo,
) -> Result<(), JvmCodegenError> {
    let direct_super = match &child_plan.initialization {
        crate::JvmClassInitializationPlan::PrimaryConstructor(constructor) => {
            constructor.direct_super_constructor()
        }
        crate::JvmClassInitializationPlan::ImplicitNoArg
        | crate::JvmClassInitializationPlan::Unsupported => None,
    };
    let constructors = unit
        .store
        .scopes
        .get(info.declarations)
        .entered_symbols()
        .filter(|symbol| {
            unit.store.symbols.contains(*symbol)
                && unit.store.symbols.get(*symbol).kind == SymbolKind::Constructor
        })
        .collect::<Vec<_>>();
    let valid_declared_no_arg_constructor = |constructor: dotty_core::SymbolId| {
        let symbol = unit.store.symbols.get(constructor);
        if symbol.owner != Some(parent_plan.class)
            || symbol.name.namespace() != dotty_core::Namespace::Term
            || unit.store.names.try_resolve(symbol.name.text()) != Some("<init>")
            || symbol.visibility != Visibility::Public
            || !symbol.flags.is_empty()
        {
            false
        } else {
            crate::emission::validate_primary_constructor(
                unit.store,
                parent_plan.class,
                constructor,
            )
            .is_ok()
        }
    };
    let declarations_valid = match (
        direct_super,
        &parent_plan.initialization,
        constructors.as_slice(),
    ) {
        (
            Some(proof),
            crate::JvmClassInitializationPlan::PrimaryConstructor(parent_primary),
            [constructor],
        ) if proof.parent_class() == parent_plan.class
            && proof.parent_constructor() == parent_primary.symbol()
            && *constructor == parent_primary.symbol()
            && parent_primary.visibility() == Visibility::Public =>
        {
            let symbol = unit.store.symbols.get(*constructor);
            symbol.owner == Some(parent_plan.class)
                && symbol.kind == SymbolKind::Constructor
                && symbol.name.namespace() == dotty_core::Namespace::Term
                && unit.store.names.try_resolve(symbol.name.text()) == Some("<init>")
                && symbol.visibility == Visibility::Public
                && symbol.flags.is_empty()
        }
        (None, crate::JvmClassInitializationPlan::ImplicitNoArg, []) => true,
        (None, crate::JvmClassInitializationPlan::ImplicitNoArg, [constructor]) => {
            valid_declared_no_arg_constructor(*constructor)
        }
        (None, crate::JvmClassInitializationPlan::PrimaryConstructor(primary), [constructor])
            if *constructor == primary.symbol() && primary.parameters().is_empty() =>
        {
            valid_declared_no_arg_constructor(*constructor)
        }
        _ => false,
    };
    if declarations_valid {
        Ok(())
    } else {
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class: child,
            parent: Some(parent_plan.class),
            issue: crate::JvmSuperclassIssue::ParentConstructorUnavailable,
        })
    }
}

pub(super) fn class_metadata(
    plan: &crate::JvmClassPlan,
    store: &SemanticStore,
    typed_ast: Option<&dotty_core::ast::TypedAst>,
    definitions: &Definitions,
    descriptor_context: Option<(&SemanticStore, &[JvmClassIdentity])>,
    superclass: Option<&crate::JvmSuperclassPlan>,
    unit: Option<&crate::JvmCompilationUnit<'_>>,
) -> Result<JvmClassMetadata, JvmCodegenError> {
    let identity = class_identity(store, plan.class)?;
    let mut fields = Vec::with_capacity(plan.fields.len());
    let mut field_names = HashMap::<String, dotty_core::SymbolId>::new();
    let mut field_symbols = HashSet::new();
    let typed_ast = if plan.fields.is_empty() {
        None
    } else {
        Some(typed_ast.ok_or(JvmCodegenError::FieldTypedAstRequired { class: plan.class })?)
    };
    for field_plan in &plan.fields {
        if !field_symbols.insert(field_plan.field) {
            return Err(JvmCodegenError::DuplicateFieldPlan {
                field: field_plan.field,
            });
        }
        let field = field_metadata(
            field_plan,
            plan.class,
            store,
            typed_ast,
            definitions,
            FieldMetadataContext {
                class_identities: descriptor_context.map(|(_, identities)| identities),
                superclass,
                unit,
            },
        )?;
        if let Some(previous) = field_names.insert(field.name().to_owned(), field.symbol()) {
            return Err(JvmCodegenError::DuplicateJvmFieldName {
                class: plan.class,
                field: field.symbol(),
                previous,
                name: field.name().to_owned(),
            });
        }
        fields.push(field);
    }

    let mut methods = Vec::with_capacity(plan.methods.len());
    let mut identities = HashMap::<(String, String), dotty_core::SymbolId>::new();

    for method_plan in &plan.methods {
        let method = method_metadata(
            method_plan,
            store,
            plan.class,
            definitions,
            descriptor_context,
            unit,
        )?;
        let key = (
            method.name().to_owned(),
            method.descriptor().as_str().to_owned(),
        );
        if let Some(previous) = identities.insert(key.clone(), method.symbol()) {
            return Err(JvmCodegenError::DuplicateJvmMethodIdentity {
                class: plan.class,
                method: method.symbol(),
                previous,
                name: key.0,
                descriptor: key.1,
            });
        }
        methods.push(method);
    }

    let constructor = match &plan.initialization {
        crate::JvmClassInitializationPlan::PrimaryConstructor(constructor) => {
            let class_identities = descriptor_context.map(|(_, identities)| identities);
            Some(constructor.metadata_with_unit(
                store,
                definitions,
                plan.class,
                class_identities,
                superclass,
                unit,
            )?)
        }
        _ => None,
    };
    Ok(JvmClassMetadata {
        constructor,
        identity,
        superclass: superclass.cloned(),
        fields,
        methods,
        dispatch_methods: Vec::new(),
    })
}

struct FieldMetadataContext<'a, 'unit> {
    class_identities: Option<&'a [JvmClassIdentity]>,
    superclass: Option<&'a crate::JvmSuperclassPlan>,
    unit: Option<&'a crate::JvmCompilationUnit<'unit>>,
}

fn field_metadata(
    plan: &JvmFieldPlan,
    owner: dotty_core::SymbolId,
    store: &SemanticStore,
    typed_ast: Option<&dotty_core::ast::TypedAst>,
    definitions: &Definitions,
    context: FieldMetadataContext<'_, '_>,
) -> Result<JvmFieldMetadata, JvmCodegenError> {
    let FieldMetadataContext {
        class_identities,
        superclass,
        unit,
    } = context;
    let field = store
        .symbols
        .contains(plan.field)
        .then(|| store.symbols.get(plan.field))
        .ok_or(JvmCodegenError::UnknownField { field: plan.field })?;
    if field.kind != SymbolKind::Field {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: plan.field,
            expected: crate::SymbolRole::Field,
            found: field.kind,
        });
    }
    if field.owner != Some(owner) {
        return Err(JvmCodegenError::WrongOwner {
            symbol: plan.field,
            expected: owner,
            actual: field.owner,
        });
    }
    if field.visibility != Visibility::Private {
        return Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: plan.field,
            visibility: field.visibility,
        });
    }
    if plan.source_visibility != crate::JvmFieldSourceVisibility::PlainPrivate {
        return Err(JvmCodegenError::UnsupportedJvmFieldSourceVisibility {
            field: plan.field,
            visibility: plan.source_visibility,
        });
    }
    validate_field_flags(plan.field, field.flags)?;

    let class_info = class_info(store, owner)?;
    let valid_object_definition = matches!(
        store.types.try_get(definitions.object_type),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) if *symbol == definitions.object_class
    );
    if let Some(self_type) = class_info.self_type {
        return Err(JvmCodegenError::UnsupportedJvmSelfType {
            class: owner,
            self_type,
        });
    }
    let parent_symbol =
        class_info
            .parents
            .first()
            .and_then(|parent| match store.types.try_get(*parent) {
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                }) => Some(*symbol),
                _ => None,
            });
    let valid_parent = match superclass {
        Some(proof) => {
            proof.class() == owner
                && proof.class_internal_name() == class_identity(store, owner)?.internal_name()
                && parent_symbol == Some(proof.parent())
                && (proof.parent() != definitions.object_class
                    || (class_info.parents.as_slice() == [definitions.object_type]
                        && valid_object_definition))
        }
        None => {
            class_info.parents.as_slice() == [definitions.object_type] && valid_object_definition
        }
    };
    if !valid_parent {
        return Err(JvmCodegenError::UnsupportedJvmSuperclass {
            class: owner,
            parent: class_info.parents.first().copied(),
        });
    }
    if !store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .any(|declaration| declaration == plan.field)
    {
        return Err(JvmCodegenError::FieldNotInClassScope {
            class: owner,
            field: plan.field,
        });
    }

    let field_type_id = match store.symbols.info(plan.field) {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol: plan.field });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol: plan.field }),
        SymbolInfo::Complete(ty) => *ty,
    };
    let field_type = project_field_value_type(
        store,
        definitions,
        plan.field,
        field_type_id,
        class_identities,
        unit,
    )?;

    let initializer_id = plan
        .initializer
        .ok_or(JvmCodegenError::MissingFieldInitializer { field: plan.field })?;
    let typed_ast = typed_ast.ok_or(JvmCodegenError::FieldTypedAstRequired { class: owner })?;
    let initializer =
        typed_ast
            .try_get(initializer_id)
            .ok_or(JvmCodegenError::UnknownFieldInitializer {
                field: plan.field,
                initializer: initializer_id,
            })?;
    let is_planned_new =
        is_canonical_planned_new_initializer(store, typed_ast, initializer_id, unit, definitions)?;
    let initializer_type = if is_planned_new {
        project_field_value_type(
            store,
            definitions,
            plan.field,
            initializer.ty,
            class_identities,
            unit,
        )?
    } else {
        typed_expression_field_value_type(
            store,
            definitions,
            plan.field,
            initializer_id,
            initializer.ty,
            class_identities,
            unit,
        )?
    };
    if matches!(initializer_type, crate::JvmType::Reference(_))
        && !matches!(&initializer.kind, dotty_core::ast::TreeKind::Ident(_))
        && !is_planned_new
    {
        let symbol = match store.types.try_get(initializer.ty) {
            Some(Type::TermRef {
                target: dotty_core::types::TermRefTarget::Symbol(symbol),
                ..
            }) => Some(*symbol),
            _ => None,
        };
        return Err(JvmCodegenError::UnsupportedJvmReferenceFieldInitializer {
            field: plan.field,
            initializer: initializer_id,
            symbol,
        });
    }
    if !crate::reference::is_assignable(unit, initializer_type, field_type, definitions)? {
        return Err(JvmCodegenError::FieldInitializerTypeMismatch {
            field: plan.field,
            initializer: initializer_id,
            expected: field_type,
            found: Some(initializer_type),
        });
    }

    let name = store
        .names
        .try_resolve(field.name.text())
        .ok_or_else(|| invalid_name(plan.field, JvmNameKind::Field, "<unresolved>"))?;
    if field.name.namespace() != Namespace::Term || !is_simple_identifier(name) {
        return Err(invalid_name(
            plan.field,
            JvmNameKind::Field,
            name.to_owned(),
        ));
    }

    let descriptor = field_descriptor(plan.field, field_type_id, field_type, class_identities)?;

    Ok(JvmFieldMetadata {
        symbol: plan.field,
        name: name.to_owned(),
        descriptor,
        value_type: field_type,
        access_flags: FieldAccessFlags(
            ACC_PRIVATE
                | if field.flags.contains(SymbolFlags::MUTABLE) {
                    0
                } else {
                    ACC_FINAL
                },
        ),
        owner,
        initializer: initializer_id,
    })
}

fn is_canonical_planned_new_initializer(
    store: &SemanticStore,
    typed_ast: &dotty_core::ast::TypedAst,
    initializer_id: dotty_core::TreeId<dotty_core::ast::Typed>,
    unit: Option<&crate::JvmCompilationUnit<'_>>,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    let Some(unit) = unit else {
        return Ok(false);
    };
    let Some(initializer) = typed_ast.try_get(initializer_id) else {
        return Ok(false);
    };
    let dotty_core::ast::TreeKind::Apply(application) = &initializer.kind else {
        return Ok(false);
    };
    if application.kind != dotty_core::ast::ApplyKind::Regular {
        return Ok(false);
    }
    let Some(function) = typed_ast.try_get(application.function) else {
        return Ok(false);
    };
    let dotty_core::ast::TreeKind::Select(select) = &function.kind else {
        return Ok(false);
    };
    if select.backquoted || store.names.try_resolve(select.name.text()) != Some("<init>") {
        return Ok(false);
    }
    let Some(qualifier) = typed_ast.try_get(select.qualifier) else {
        return Ok(false);
    };
    let dotty_core::ast::TreeKind::New(new) = &qualifier.kind else {
        return Ok(false);
    };
    let Some(type_tree) = typed_ast.try_get(new.tpt) else {
        return Ok(false);
    };
    if !matches!(type_tree.kind, dotty_core::ast::TreeKind::TypeTree(_)) {
        return Ok(false);
    }
    let Some(Type::TermRef {
        prefix,
        target: dotty_core::types::TermRefTarget::Symbol(constructor),
    }) = store.types.try_get(function.ty)
    else {
        return Ok(false);
    };
    if !store.symbols.contains(*constructor) {
        return Ok(false);
    }
    let constructor_info = store.symbols.get(*constructor);
    let Some(owner) = constructor_info.owner else {
        return Ok(false);
    };
    if constructor_info.kind != SymbolKind::Constructor
        || constructor_info.name.namespace() != Namespace::Term
        || store.names.try_resolve(constructor_info.name.text()) != Some("<init>")
        || !unit.classes.iter().any(|plan| plan.class == owner)
    {
        return Ok(false);
    }
    let canonical_class_target = |ty: dotty_core::TypeId| {
        let Some(Type::TypeRef { prefix, target }) = store.types.try_get(ty) else {
            return None;
        };
        let TypeRefTarget::Symbol(class) = target else {
            return None;
        };
        (store.symbols.contains(*class)
            && store.symbols.get(*class).kind == SymbolKind::Class
            && unit.classes.iter().any(|plan| plan.class == *class)
            && canonical_class_parent_prefix(store, *prefix, *class, definitions.no_prefix))
        .then_some(*class)
    };
    let class = canonical_class_target(type_tree.ty);
    Ok(class == Some(owner)
        && canonical_class_target(qualifier.ty) == class
        && canonical_class_target(*prefix) == class
        && canonical_class_target(initializer.ty) == class)
}

pub(super) fn project_field_value_type(
    store: &SemanticStore,
    definitions: &Definitions,
    symbol: dotty_core::SymbolId,
    ty: dotty_core::TypeId,
    class_identities: Option<&[JvmClassIdentity]>,
    unit: Option<&crate::JvmCompilationUnit<'_>>,
) -> Result<crate::JvmType, JvmCodegenError> {
    match crate::descriptor::primitive_semantic_value_type(store, definitions, symbol, ty) {
        Err(error @ JvmCodegenError::UnsupportedJvmType { .. }) => match class_identities {
            Some(class_identities) => crate::reference::project_method_signature_type(
                store,
                ty,
                definitions,
                class_identities,
                unit,
            ),
            None => Err(error),
        },
        result => result,
    }
}

fn field_descriptor(
    symbol: dotty_core::SymbolId,
    ty: dotty_core::TypeId,
    value_type: crate::JvmType,
    class_identities: Option<&[JvmClassIdentity]>,
) -> Result<String, JvmCodegenError> {
    match value_type {
        crate::JvmType::Int => Ok("I".to_owned()),
        crate::JvmType::Boolean => Ok("Z".to_owned()),
        crate::JvmType::Long => Err(JvmCodegenError::UnsupportedJvmType {
            symbol,
            ty,
            kind: crate::UnsupportedJvmTypeKind::Other,
        }),
        crate::JvmType::Reference(class) => {
            let Some(identity) = class_identities.and_then(|identities| {
                identities
                    .iter()
                    .find(|identity| identity.symbol() == class)
            }) else {
                return Err(JvmCodegenError::UnsupportedJvmType {
                    symbol,
                    ty,
                    kind: crate::UnsupportedJvmTypeKind::ReferenceClass,
                });
            };
            Ok(format!("L{};", identity.internal_name()))
        }
    }
}

pub(super) fn typed_expression_field_value_type(
    store: &SemanticStore,
    definitions: &Definitions,
    symbol: dotty_core::SymbolId,
    initializer: dotty_core::TreeId<dotty_core::ast::Typed>,
    ty: dotty_core::TypeId,
    class_identities: Option<&[JvmClassIdentity]>,
    unit: Option<&crate::JvmCompilationUnit<'_>>,
) -> Result<crate::JvmType, JvmCodegenError> {
    let target = match store.types.try_get(ty) {
        Some(Type::TermRef {
            target: dotty_core::types::TermRefTarget::Symbol(target),
            ..
        }) => *target,
        _ => {
            let value_type =
                project_field_value_type(store, definitions, symbol, ty, class_identities, unit)?;
            if matches!(value_type, crate::JvmType::Reference(_)) {
                return Err(JvmCodegenError::UnsupportedJvmReferenceFieldInitializer {
                    field: symbol,
                    initializer,
                    symbol: None,
                });
            }
            return Ok(value_type);
        }
    };
    if !store.symbols.contains(target) {
        return Err(JvmCodegenError::UnknownFieldInitializerSymbol {
            field: symbol,
            initializer,
            symbol: target,
        });
    }
    let referenced = store.symbols.get(target);
    let target_ty = match store.symbols.info(target) {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol: target });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol: target }),
        SymbolInfo::Complete(target_ty) => *target_ty,
    };
    let value_type = project_field_value_type(
        store,
        definitions,
        symbol,
        target_ty,
        class_identities,
        unit,
    )?;
    let parameter_owner = referenced.owner.and_then(|owner| {
        store
            .symbols
            .contains(owner)
            .then(|| store.symbols.get(owner))
    });
    if matches!(value_type, crate::JvmType::Reference(_))
        && (referenced.kind != SymbolKind::Parameter
            || !matches!(parameter_owner, Some(method)
                if method.kind == SymbolKind::Constructor
                    && method.owner == store.symbols.get(symbol).owner))
    {
        return Err(JvmCodegenError::UnsupportedJvmReferenceFieldInitializer {
            field: symbol,
            initializer,
            symbol: Some(target),
        });
    }
    Ok(value_type)
}

pub(super) fn typed_expression_primitive_type(
    store: &SemanticStore,
    definitions: &Definitions,
    symbol: dotty_core::SymbolId,
    initializer: dotty_core::TreeId<dotty_core::ast::Typed>,
    ty: dotty_core::TypeId,
) -> Result<crate::JvmType, JvmCodegenError> {
    match store.types.try_get(ty) {
        Some(Type::TermRef {
            target: dotty_core::types::TermRefTarget::Symbol(target),
            ..
        }) => {
            if !store.symbols.contains(*target) {
                return Err(JvmCodegenError::UnknownFieldInitializerSymbol {
                    field: symbol,
                    initializer,
                    symbol: *target,
                });
            }
            let target_ty = match store.symbols.info(*target) {
                SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                    return Err(JvmCodegenError::MissingSemanticInfo { symbol: *target });
                }
                SymbolInfo::Error => {
                    return Err(JvmCodegenError::ErrorSemanticInfo { symbol: *target });
                }
                SymbolInfo::Complete(target_ty) => *target_ty,
            };
            crate::descriptor::primitive_semantic_value_type(store, definitions, symbol, target_ty)
        }
        _ => crate::descriptor::primitive_semantic_value_type(store, definitions, symbol, ty),
    }
}

pub(super) fn validate_field_flags(
    field: dotty_core::SymbolId,
    flags: SymbolFlags,
) -> Result<(), JvmCodegenError> {
    for (flag, unsupported) in [
        (SymbolFlags::ABSTRACT, UnsupportedJvmFieldFlag::Abstract),
        (SymbolFlags::SEALED, UnsupportedJvmFieldFlag::Sealed),
        (SymbolFlags::CASE, UnsupportedJvmFieldFlag::Case),
        (SymbolFlags::IMPLICIT, UnsupportedJvmFieldFlag::Implicit),
        (SymbolFlags::GIVEN, UnsupportedJvmFieldFlag::Given),
        (SymbolFlags::LAZY, UnsupportedJvmFieldFlag::Lazy),
        (SymbolFlags::INLINE, UnsupportedJvmFieldFlag::Inline),
        (
            SymbolFlags::TRANSPARENT,
            UnsupportedJvmFieldFlag::Transparent,
        ),
        (SymbolFlags::OPAQUE, UnsupportedJvmFieldFlag::Opaque),
        (SymbolFlags::EXTENSION, UnsupportedJvmFieldFlag::Extension),
        (SymbolFlags::STATIC, UnsupportedJvmFieldFlag::Static),
        (SymbolFlags::SYNTHETIC, UnsupportedJvmFieldFlag::Synthetic),
        (
            SymbolFlags::JAVA_DEFINED,
            UnsupportedJvmFieldFlag::JavaDefined,
        ),
        (SymbolFlags::ERASED, UnsupportedJvmFieldFlag::Erased),
        (SymbolFlags::OVERRIDE, UnsupportedJvmFieldFlag::Override),
        (SymbolFlags::ENUM, UnsupportedJvmFieldFlag::Enum),
        (SymbolFlags::EXPORTED, UnsupportedJvmFieldFlag::Exported),
    ] {
        if flags.contains(flag) {
            return Err(JvmCodegenError::UnsupportedJvmFieldFlag {
                field,
                flag: unsupported,
            });
        }
    }
    Ok(())
}

pub(super) fn unit_metadata(
    unit: &crate::JvmCompilationUnit<'_>,
    definitions: &Definitions,
) -> Result<Vec<JvmClassMetadata>, JvmCodegenError> {
    let hierarchy = unit_hierarchy(unit, definitions)?;
    let class_identities = unit_class_identities(unit)?;
    let mut metadata = Vec::with_capacity(unit.classes.len());
    for class_plan in &unit.classes {
        let superclass = hierarchy
            .superclasses()
            .iter()
            .find(|superclass| superclass.class() == class_plan.class)
            .expect("every validated class has one direct superclass");
        let class = class_metadata(
            class_plan,
            unit.store,
            Some(unit.typed_ast),
            definitions,
            Some((unit.store, &class_identities)),
            Some(superclass),
            Some(unit),
        )?;
        metadata.push(class);
    }
    validate_final_parent_method_collisions(unit, &metadata)?;
    validate_and_build_method_dispatch(unit, definitions, &mut metadata)?;
    Ok(metadata)
}

fn validate_and_build_method_dispatch(
    unit: &crate::JvmCompilationUnit<'_>,
    definitions: &Definitions,
    metadata: &mut [JvmClassMetadata],
) -> Result<(), JvmCodegenError> {
    for class in metadata.iter() {
        let declarations = declared_methods(unit, class.identity().symbol())?;
        for method in class.methods() {
            if !declarations.contains(&method.symbol()) {
                return Err(JvmCodegenError::MethodNotInClassDeclarations {
                    class: class.identity().symbol(),
                    method: method.symbol(),
                });
            }
        }
    }

    for child in metadata.iter() {
        let mut ancestors = Vec::new();
        let mut next = child.superclass().map(|superclass| superclass.parent());
        while let Some(parent) = next.and_then(|parent_symbol| {
            metadata
                .iter()
                .find(|candidate| candidate.identity().symbol() == parent_symbol)
        }) {
            ancestors.push(parent);
            next = parent.superclass().map(|superclass| superclass.parent());
        }
        let Some(nearest_ancestor) = ancestors.first().copied() else {
            continue;
        };

        for child_method in child.methods() {
            let child_symbol = unit.store.symbols.get(child_method.symbol());
            let has_override_modifier = child_symbol.flags.contains(SymbolFlags::OVERRIDE);
            let same_name_declarations = ancestors
                .iter()
                .map(|ancestor| {
                    declared_methods(unit, ancestor.identity().symbol()).map(|methods| {
                        methods
                            .into_iter()
                            .filter(|symbol| {
                                unit.store.symbols.get(*symbol).name == child_symbol.name
                            })
                            .map(|symbol| (*ancestor, symbol))
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let exact_planned = ancestors.iter().find_map(|ancestor| {
                ancestor.methods().iter().find_map(|parent_method| {
                    (parent_method.name() == child_method.name()
                        && parent_method.descriptor().as_str()
                            == child_method.descriptor().as_str())
                    .then_some((*ancestor, parent_method))
                })
            });
            let mut same_parameter_parent = None;
            if !has_override_modifier {
                for (ancestor, parent_symbol) in &same_name_declarations {
                    if unit.store.symbols.get(*parent_symbol).visibility != Visibility::Public {
                        continue;
                    }
                    if same_semantic_method_parameters(
                        unit.store,
                        child_method.symbol(),
                        *parent_symbol,
                        definitions,
                    )? {
                        same_parameter_parent = Some((*ancestor, *parent_symbol));
                        break;
                    }
                }
            }

            if has_override_modifier {
                if same_name_declarations.is_empty() {
                    return Err(invalid_override(
                        child.identity().symbol(),
                        child_method.symbol(),
                        nearest_ancestor.identity().symbol(),
                        None,
                        JvmOverrideIssue::NoParentDeclaration,
                    ));
                }
                let Some((parent, parent_method)) = exact_planned else {
                    let matching_unplanned =
                        same_name_declarations
                            .iter()
                            .copied()
                            .find(|(ancestor, symbol)| {
                                !ancestor
                                    .methods()
                                    .iter()
                                    .any(|planned| planned.symbol() == *symbol)
                            });
                    let (error_parent, error_method) = matching_unplanned
                        .or_else(|| same_name_declarations.first().copied())
                        .map_or((nearest_ancestor, None), |(ancestor, symbol)| {
                            (ancestor, Some(symbol))
                        });
                    return Err(invalid_override(
                        child.identity().symbol(),
                        child_method.symbol(),
                        error_parent.identity().symbol(),
                        error_method,
                        if matching_unplanned.is_some() {
                            JvmOverrideIssue::ParentMethodUnplanned
                        } else {
                            JvmOverrideIssue::SignatureMismatch
                        },
                    ));
                };
                let parent_symbol = unit.store.symbols.get(parent_method.symbol());
                if parent_symbol.visibility != Visibility::Public {
                    return Err(invalid_override(
                        child.identity().symbol(),
                        child_method.symbol(),
                        parent.identity().symbol(),
                        Some(parent_method.symbol()),
                        JvmOverrideIssue::InaccessibleParentMethod,
                    ));
                }
                if child_symbol.visibility != Visibility::Public {
                    return Err(invalid_override(
                        child.identity().symbol(),
                        child_method.symbol(),
                        parent.identity().symbol(),
                        Some(parent_method.symbol()),
                        JvmOverrideIssue::NarrowerVisibility,
                    ));
                }
                if !same_semantic_method_type(
                    unit.store,
                    child_method.symbol(),
                    parent_method.symbol(),
                    definitions,
                )? {
                    return Err(invalid_override(
                        child.identity().symbol(),
                        child_method.symbol(),
                        parent.identity().symbol(),
                        Some(parent_method.symbol()),
                        JvmOverrideIssue::SignatureMismatch,
                    ));
                }
            } else if let Some(parent_method) = exact_planned
                && unit.store.symbols.get(parent_method.1.symbol()).visibility == Visibility::Public
            {
                return Err(invalid_override(
                    child.identity().symbol(),
                    child_method.symbol(),
                    parent_method.0.identity().symbol(),
                    Some(parent_method.1.symbol()),
                    if child_symbol.visibility == Visibility::Public {
                        JvmOverrideIssue::OverrideModifierRequired
                    } else {
                        JvmOverrideIssue::NarrowerVisibility
                    },
                ));
            } else if let Some((ancestor, parent_symbol)) = same_parameter_parent {
                let parent_method = ancestor
                    .methods()
                    .iter()
                    .find(|planned| planned.symbol() == parent_symbol);
                let issue = match parent_method {
                    None => JvmOverrideIssue::ParentMethodUnplanned,
                    Some(parent_method)
                        if !same_semantic_method_type(
                            unit.store,
                            child_method.symbol(),
                            parent_method.symbol(),
                            definitions,
                        )? =>
                    {
                        JvmOverrideIssue::SignatureMismatch
                    }
                    Some(_) if child_symbol.visibility != Visibility::Public => {
                        JvmOverrideIssue::NarrowerVisibility
                    }
                    Some(_) => JvmOverrideIssue::OverrideModifierRequired,
                };
                return Err(invalid_override(
                    child.identity().symbol(),
                    child_method.symbol(),
                    ancestor.identity().symbol(),
                    Some(parent_symbol),
                    issue,
                ));
            }
        }
    }

    let mut dispatch_by_class = Vec::with_capacity(metadata.len());
    for class in metadata.iter() {
        let mut active = HashSet::new();
        dispatch_by_class.push(effective_dispatch_methods(
            class.identity().symbol(),
            class.identity().symbol(),
            metadata,
            &mut active,
        )?);
    }
    for (class, dispatch) in metadata.iter_mut().zip(dispatch_by_class) {
        class.dispatch_methods = dispatch;
    }
    Ok(())
}

fn declared_methods(
    unit: &crate::JvmCompilationUnit<'_>,
    class: dotty_core::SymbolId,
) -> Result<Vec<dotty_core::SymbolId>, JvmCodegenError> {
    let info = class_info(unit.store, class)?;
    let mut methods = Vec::new();
    let mut seen = HashSet::new();
    for declaration in unit.store.scopes.get(info.declarations).entered_symbols() {
        let symbol = unit
            .store
            .symbols
            .contains(declaration)
            .then(|| unit.store.symbols.get(declaration))
            .ok_or(JvmCodegenError::InvalidClassDeclaration {
                class,
                symbol: declaration,
            })?;
        if symbol.kind != SymbolKind::Method {
            continue;
        }
        if symbol.owner != Some(class) {
            return Err(JvmCodegenError::WrongOwner {
                symbol: declaration,
                expected: class,
                actual: symbol.owner,
            });
        }
        if !seen.insert(declaration) {
            return Err(JvmCodegenError::InvalidClassDeclaration {
                class,
                symbol: declaration,
            });
        }
        methods.push(declaration);
    }
    Ok(methods)
}

fn invalid_override(
    class: dotty_core::SymbolId,
    method: dotty_core::SymbolId,
    parent: dotty_core::SymbolId,
    parent_method: Option<dotty_core::SymbolId>,
    issue: JvmOverrideIssue,
) -> JvmCodegenError {
    JvmCodegenError::InvalidJvmOverride {
        class,
        method,
        parent,
        parent_method,
        issue,
    }
}

fn same_semantic_method_type(
    store: &SemanticStore,
    left: dotty_core::SymbolId,
    right: dotty_core::SymbolId,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    let left_type = method_info_type(store, left)?;
    let right_type = method_info_type(store, right)?;
    let (Some(Type::Method(left)), Some(Type::Method(right))) = (
        store.types.try_get(left_type),
        store.types.try_get(right_type),
    ) else {
        return Ok(false);
    };
    if left.kind != right.kind || left.params.len() != right.params.len() {
        return Ok(false);
    }
    for (left, right) in left.params.iter().zip(&right.params) {
        if left.erased != right.erased
            || left.varargs != right.varargs
            || !same_semantic_signature_type(store, left.ty, right.ty, definitions)?
        {
            return Ok(false);
        }
    }
    same_semantic_signature_type(store, left.result, right.result, definitions)
}

fn same_semantic_method_parameters(
    store: &SemanticStore,
    left: dotty_core::SymbolId,
    right: dotty_core::SymbolId,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    let left_type = method_info_type(store, left)?;
    let right_type = method_info_type(store, right)?;
    let (Some(Type::Method(left)), Some(Type::Method(right))) = (
        store.types.try_get(left_type),
        store.types.try_get(right_type),
    ) else {
        return Ok(false);
    };
    if left.kind != right.kind || left.params.len() != right.params.len() {
        return Ok(false);
    }
    for (left, right) in left.params.iter().zip(&right.params) {
        if left.erased != right.erased
            || left.varargs != right.varargs
            || !same_semantic_signature_type(store, left.ty, right.ty, definitions)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn same_semantic_signature_type(
    store: &SemanticStore,
    left: dotty_core::TypeId,
    right: dotty_core::TypeId,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    if store.types.try_get(left) == store.types.try_get(right) {
        return Ok(true);
    }
    let (Some(Type::TypeRef { .. }), Some(Type::TypeRef { .. })) =
        (store.types.try_get(left), store.types.try_get(right))
    else {
        return Ok(false);
    };
    Ok(
        crate::reference::method_signature_class_target(store, left, definitions)?
            == crate::reference::method_signature_class_target(store, right, definitions)?,
    )
}

fn method_info_type(
    store: &SemanticStore,
    symbol: dotty_core::SymbolId,
) -> Result<dotty_core::TypeId, JvmCodegenError> {
    let info = store
        .symbols
        .contains(symbol)
        .then(|| store.symbols.info(symbol))
        .ok_or(JvmCodegenError::UnknownMethod { method: symbol })?;
    match info {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            Err(JvmCodegenError::MissingSemanticInfo { symbol })
        }
        SymbolInfo::Error => Err(JvmCodegenError::ErrorSemanticInfo { symbol }),
        SymbolInfo::Complete(ty) if store.types.contains(*ty) => Ok(*ty),
        SymbolInfo::Complete(ty) => Err(JvmCodegenError::InvalidSemanticType {
            symbol,
            ty: *ty,
            reason: crate::InvalidTypeReason::MissingOrUnfilled,
        }),
    }
}

fn effective_dispatch_methods(
    class: dotty_core::SymbolId,
    receiver: dotty_core::SymbolId,
    metadata: &[JvmClassMetadata],
    active: &mut HashSet<dotty_core::SymbolId>,
) -> Result<Vec<JvmMethodDispatchMetadata>, JvmCodegenError> {
    if !active.insert(class) {
        return Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: None,
            issue: crate::JvmSuperclassIssue::InheritanceCycle,
        });
    }
    let class_metadata = metadata
        .iter()
        .find(|candidate| candidate.identity().symbol() == class)
        .ok_or(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: None,
            issue: crate::JvmSuperclassIssue::ParentNotPlanned,
        })?;
    let mut methods = match class_metadata.superclass() {
        Some(superclass)
            if metadata
                .iter()
                .any(|candidate| candidate.identity().symbol() == superclass.parent()) =>
        {
            effective_dispatch_methods(superclass.parent(), receiver, metadata, active)?
        }
        _ => Vec::new(),
    };
    active.remove(&class);

    for method in class_metadata
        .methods()
        .iter()
        .filter(|method| method.access_flags().is_public())
    {
        let inherited = methods.iter_mut().find(|candidate| {
            candidate.name == method.name()
                && candidate.descriptor.as_str() == method.descriptor().as_str()
        });
        let entry = JvmMethodDispatchMetadata {
            receiver_class: receiver,
            declaring_class: class,
            method: method.symbol(),
            name: method.name().to_owned(),
            descriptor: method.descriptor().clone(),
        };
        if let Some(inherited) = inherited {
            *inherited = entry;
        } else {
            methods.push(entry);
        }
    }
    Ok(methods)
}

fn validate_final_parent_method_collisions(
    unit: &crate::JvmCompilationUnit<'_>,
    metadata: &[JvmClassMetadata],
) -> Result<(), JvmCodegenError> {
    for child in metadata {
        let mut ancestor = child.superclass().map(|superclass| superclass.parent());
        while let Some(parent) = ancestor.and_then(|parent_symbol| {
            metadata
                .iter()
                .find(|candidate| candidate.identity().symbol() == parent_symbol)
        }) {
            for child_method in child.methods() {
                for parent_method in parent.methods() {
                    if parent_method.access_flags().is_final()
                        && !parent_method.access_flags().is_private()
                        && child_method.name() == parent_method.name()
                        && child_method.descriptor().as_str() == parent_method.descriptor().as_str()
                    {
                        return Err(JvmCodegenError::FinalJvmMethodOverride {
                            class: child.identity().symbol(),
                            method: child_method.symbol(),
                            parent: parent.identity().symbol(),
                            parent_method: parent_method.symbol(),
                            name: child_method.name().to_owned(),
                            descriptor: Some(child_method.descriptor().as_str().to_owned()),
                        });
                    }
                }
            }

            let parent_info = class_info(unit.store, parent.identity().symbol())?;
            for parent_method in unit
                .store
                .scopes
                .get(parent_info.declarations)
                .entered_symbols()
                .filter(|symbol| {
                    unit.store.symbols.contains(*symbol)
                        && unit.store.symbols.get(*symbol).kind == SymbolKind::Method
                        && unit
                            .store
                            .symbols
                            .get(*symbol)
                            .flags
                            .contains(SymbolFlags::FINAL)
                        && unit.store.symbols.get(*symbol).visibility != Visibility::Private
                })
                .filter(|symbol| {
                    !parent
                        .methods()
                        .iter()
                        .any(|planned| planned.symbol() == *symbol)
                })
            {
                let parent_symbol = unit.store.symbols.get(parent_method);
                let Some(name) = unit.store.names.try_resolve(parent_symbol.name.text()) else {
                    continue;
                };
                if let Some(child_method) =
                    child.methods().iter().find(|method| method.name() == name)
                {
                    return Err(JvmCodegenError::FinalJvmMethodOverride {
                        class: child.identity().symbol(),
                        method: child_method.symbol(),
                        parent: parent.identity().symbol(),
                        parent_method,
                        name: name.to_owned(),
                        descriptor: None,
                    });
                }
            }

            ancestor = parent.superclass().map(|superclass| superclass.parent());
        }
    }
    Ok(())
}

fn method_metadata(
    plan: &crate::JvmMethodPlan,
    store: &SemanticStore,
    owner: dotty_core::SymbolId,
    definitions: &Definitions,
    descriptor_context: Option<(&SemanticStore, &[JvmClassIdentity])>,
    unit: Option<&crate::JvmCompilationUnit<'_>>,
) -> Result<JvmMethodMetadata, JvmCodegenError> {
    // Descriptor projection verifies canonical method/parameter ownership and
    // type correspondence before any output layout is constructed.
    let descriptor = match descriptor_context {
        Some((descriptor_store, class_identities)) => {
            crate::descriptor::method_descriptor_with_class_identities(
                plan,
                descriptor_store,
                owner,
                definitions,
                class_identities,
                unit,
            )?
        }
        None => plan.descriptor(store, owner, definitions)?,
    };
    let method = store.symbols.get(plan.method);
    if !matches!(method.visibility, Visibility::Public | Visibility::Private) {
        return Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: plan.method,
            visibility: method.visibility,
        });
    }
    let access_flags = method_access_flags(plan.method, method.visibility, method.flags)?;
    let name = store
        .names
        .try_resolve(method.name.text())
        .ok_or_else(|| invalid_name(plan.method, JvmNameKind::Method, "<unresolved>"))?;
    if method.name.namespace() != Namespace::Term || !is_simple_identifier(name) {
        return Err(invalid_name(
            plan.method,
            JvmNameKind::Method,
            name.to_owned(),
        ));
    }

    let mut slot = 1u16; // Slot zero is `this`; only instance methods are accepted.
    let mut parameters = Vec::with_capacity(plan.parameters.len());
    for (parameter, ty) in plan.parameters.iter().copied().zip(descriptor.parameters()) {
        let width = ty.slot_width();
        parameters.push(JvmParameterLocal {
            symbol: parameter,
            ty: *ty,
            slot,
            width,
        });
        slot += u16::from(width);
    }

    Ok(JvmMethodMetadata {
        symbol: plan.method,
        name: name.to_owned(),
        descriptor,
        access_flags,
        parameters,
    })
}

fn method_access_flags(
    method: dotty_core::SymbolId,
    visibility: Visibility,
    flags: SymbolFlags,
) -> Result<MethodAccessFlags, JvmCodegenError> {
    if flags.contains(SymbolFlags::ABSTRACT) {
        return Err(JvmCodegenError::UnsupportedJvmSymbolFlag {
            symbol: method,
            flag: UnsupportedJvmSymbolFlag::Abstract,
        });
    }
    for (flag, unsupported) in [
        (SymbolFlags::SEALED, UnsupportedJvmSymbolFlag::Sealed),
        (SymbolFlags::CASE, UnsupportedJvmSymbolFlag::Case),
        (SymbolFlags::IMPLICIT, UnsupportedJvmSymbolFlag::Implicit),
        (SymbolFlags::GIVEN, UnsupportedJvmSymbolFlag::Given),
        (SymbolFlags::LAZY, UnsupportedJvmSymbolFlag::Lazy),
        (SymbolFlags::MUTABLE, UnsupportedJvmSymbolFlag::Mutable),
        (SymbolFlags::INLINE, UnsupportedJvmSymbolFlag::Inline),
        (
            SymbolFlags::TRANSPARENT,
            UnsupportedJvmSymbolFlag::Transparent,
        ),
        (SymbolFlags::OPAQUE, UnsupportedJvmSymbolFlag::Opaque),
        (SymbolFlags::EXTENSION, UnsupportedJvmSymbolFlag::Extension),
        (SymbolFlags::STATIC, UnsupportedJvmSymbolFlag::Static),
        (SymbolFlags::SYNTHETIC, UnsupportedJvmSymbolFlag::Synthetic),
        (
            SymbolFlags::JAVA_DEFINED,
            UnsupportedJvmSymbolFlag::JavaDefined,
        ),
        (SymbolFlags::ERASED, UnsupportedJvmSymbolFlag::Erased),
        (SymbolFlags::ENUM, UnsupportedJvmSymbolFlag::Enum),
        (SymbolFlags::EXPORTED, UnsupportedJvmSymbolFlag::Exported),
    ] {
        if flags.contains(flag) {
            return Err(JvmCodegenError::UnsupportedJvmSymbolFlag {
                symbol: method,
                flag: unsupported,
            });
        }
    }
    // `override` has no JVM access bit. It does not alter method identity.
    let access = match visibility {
        Visibility::Public => ACC_PUBLIC,
        Visibility::Private => ACC_PRIVATE,
        visibility => {
            return Err(JvmCodegenError::UnsupportedJvmVisibility {
                symbol: method,
                visibility,
            });
        }
    };
    Ok(MethodAccessFlags(
        access
            | if flags.contains(SymbolFlags::FINAL) {
                ACC_FINAL
            } else {
                0
            },
    ))
}

pub(super) fn class_info(
    store: &SemanticStore,
    class: dotty_core::SymbolId,
) -> Result<&dotty_core::types::ClassInfo, JvmCodegenError> {
    let symbol = store
        .symbols
        .contains(class)
        .then(|| store.symbols.get(class))
        .ok_or(JvmCodegenError::UnknownClass { class })?;
    validate_class_info(store, class, symbol.info)
}

fn validate_class_info(
    store: &SemanticStore,
    class: dotty_core::SymbolId,
    info: SymbolInfo,
) -> Result<&dotty_core::types::ClassInfo, JvmCodegenError> {
    let ty = match info {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol: class });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol: class }),
        SymbolInfo::Complete(ty) => ty,
    };
    let Some(semantic_type) = store.types.try_get(ty) else {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::MissingOrUnfilled,
        ));
    };
    let Type::ClassInfo(class_info) = semantic_type else {
        return Err(invalid_class_info(
            class,
            Some(ty),
            match semantic_type {
                Type::Error(_) => InvalidJvmClassInfoKind::Error,
                _ => InvalidJvmClassInfoKind::NotClassInfo,
            },
        ));
    };
    if class_info.class != class {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::WrongClassSymbol,
        ));
    }
    if !store.scopes.contains(class_info.declarations) {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::MissingDeclarationScope,
        ));
    }
    if store.scopes.get(class_info.declarations).owner != Some(class) {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::WrongDeclarationScopeOwner,
        ));
    }

    let mut declarations = store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .collect::<Vec<_>>();
    declarations.sort_by_key(|symbol| symbol.index());
    for declaration in declarations {
        let Some(symbol) = store
            .symbols
            .contains(declaration)
            .then(|| store.symbols.get(declaration))
        else {
            return Err(JvmCodegenError::InvalidClassDeclaration {
                class,
                symbol: declaration,
            });
        };
        if symbol.owner == Some(class) && symbol.kind == SymbolKind::TypeParameter {
            return Err(JvmCodegenError::UnsupportedJvmClass {
                class,
                kind: UnsupportedJvmClassKind::Generic,
            });
        }
    }
    Ok(class_info)
}

fn validate_class_flags(
    class: dotty_core::SymbolId,
    flags: SymbolFlags,
) -> Result<(), JvmCodegenError> {
    if flags.contains(SymbolFlags::ABSTRACT) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Abstract,
        });
    }
    if flags.contains(SymbolFlags::ENUM) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Enum,
        });
    }
    if flags.contains(SymbolFlags::SYNTHETIC) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Synthetic,
        });
    }
    if flags.contains(SymbolFlags::CASE) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Case,
        });
    }
    if flags.contains(SymbolFlags::SEALED) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Sealed,
        });
    }
    for (flag, unsupported) in [
        (SymbolFlags::IMPLICIT, UnsupportedJvmSymbolFlag::Implicit),
        (SymbolFlags::GIVEN, UnsupportedJvmSymbolFlag::Given),
        (SymbolFlags::LAZY, UnsupportedJvmSymbolFlag::Lazy),
        (SymbolFlags::MUTABLE, UnsupportedJvmSymbolFlag::Mutable),
        (SymbolFlags::INLINE, UnsupportedJvmSymbolFlag::Inline),
        (
            SymbolFlags::TRANSPARENT,
            UnsupportedJvmSymbolFlag::Transparent,
        ),
        (SymbolFlags::OPAQUE, UnsupportedJvmSymbolFlag::Opaque),
        (SymbolFlags::EXTENSION, UnsupportedJvmSymbolFlag::Extension),
        (SymbolFlags::STATIC, UnsupportedJvmSymbolFlag::Static),
        (
            SymbolFlags::JAVA_DEFINED,
            UnsupportedJvmSymbolFlag::JavaDefined,
        ),
        (SymbolFlags::ERASED, UnsupportedJvmSymbolFlag::Erased),
        (SymbolFlags::OVERRIDE, UnsupportedJvmSymbolFlag::Override),
        (SymbolFlags::EXPORTED, UnsupportedJvmSymbolFlag::Exported),
    ] {
        if flags.contains(flag) {
            return Err(JvmCodegenError::UnsupportedJvmSymbolFlag {
                symbol: class,
                flag: unsupported,
            });
        }
    }
    Ok(())
}

fn is_simple_identifier(text: &str) -> bool {
    if text == "_" {
        return false;
    }
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_')
        && chars.all(|character| character.is_alphanumeric() || character == '_')
}

fn invalid_name(
    symbol: dotty_core::SymbolId,
    kind: JvmNameKind,
    text: impl Into<String>,
) -> JvmCodegenError {
    JvmCodegenError::InvalidJvmName {
        symbol,
        kind,
        text: text.into(),
    }
}

fn invalid_class_info(
    class: dotty_core::SymbolId,
    ty: Option<dotty_core::TypeId>,
    kind: InvalidJvmClassInfoKind,
) -> JvmCodegenError {
    JvmCodegenError::InvalidJvmClassInfo { class, ty, kind }
}
