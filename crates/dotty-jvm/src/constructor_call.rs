//! Read-only resolution of bounded typed constructor applications.

use dotty_classfile::access_flags::{ACC_PUBLIC, MethodAccessFlags};
use dotty_core::ast::{ApplyKind, Tree, TreeKind, Typed};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, Visibility};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};

use crate::{
    JvmClassInitializationPlan, JvmCodegenError, JvmCompilationUnit,
    JvmConstructorCallShapeError as ShapeError, JvmConstructorCallTarget, JvmMethodPlan, JvmType,
};

struct ConstructorReferenceContext<'a> {
    definitions: &'a Definitions,
    class_identities: &'a [crate::JvmClassIdentity],
}

pub(super) fn resolve(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    apply_tree: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<JvmConstructorCallTarget, JvmCodegenError> {
    unit.validate()?;
    let class_identities = crate::identity::unit_class_identities(unit)?;
    let caller_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == caller_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: caller_class,
        })?;
    if !caller_plan
        .methods
        .iter()
        .any(|plan| plan.method == caller_method)
    {
        return Err(JvmCodegenError::CallerMethodNotInJvmPlan {
            class: caller_class,
            method: caller_method,
        });
    }
    let caller_method_plan = caller_plan
        .methods
        .iter()
        .find(|plan| plan.method == caller_method)
        .ok_or(JvmCodegenError::CallerMethodNotInJvmPlan {
            class: caller_class,
            method: caller_method,
        })?;

    let application = tree(unit.store, unit.typed_ast, apply_tree)?;
    let apply = match &application.kind {
        TreeKind::Apply(apply) => apply,
        TreeKind::TypeApply(_) => {
            return Err(unsupported(apply_tree, ShapeError::TypeApplication));
        }
        _ => return Err(unsupported(apply_tree, ShapeError::NotApplication)),
    };
    if apply.kind != ApplyKind::Regular {
        return Err(unsupported(apply_tree, ShapeError::UsingApplication));
    }

    let function = tree(unit.store, unit.typed_ast, apply.function)?;
    let select = match &function.kind {
        TreeKind::Select(select) => select,
        _ => return Err(unsupported(apply.function, ShapeError::FunctionNotSelect)),
    };
    if select.backquoted || unit.store.names.try_resolve(select.name.text()) != Some("<init>") {
        return Err(unsupported(
            apply.function,
            ShapeError::ConstructorReferenceNotCanonical,
        ));
    }
    let qualifier = tree(unit.store, unit.typed_ast, select.qualifier)?;
    let new = match &qualifier.kind {
        TreeKind::New(new) => new,
        _ => return Err(unsupported(select.qualifier, ShapeError::QualifierNotNew)),
    };
    let tpt = tree(unit.store, unit.typed_ast, new.tpt)?;
    if !matches!(tpt.kind, TreeKind::TypeTree(_)) {
        return Err(unsupported(new.tpt, ShapeError::TypeTreeNotCanonicalClass));
    }

    let owner_class = planned_class(unit, tpt.ty, new.tpt, definitions)?;
    let allocated_class = planned_class(unit, qualifier.ty, select.qualifier, definitions).ok();
    if allocated_class != Some(owner_class) {
        return Err(JvmCodegenError::JvmConstructorCallClassMismatch {
            tree: select.qualifier,
            expected: owner_class,
            found: allocated_class,
        });
    }
    let result_class = planned_result_class(unit, application.ty, owner_class, definitions);
    if result_class != Some(owner_class) {
        return Err(JvmCodegenError::JvmConstructorCallResultTypeMismatch {
            tree: apply_tree,
            expected: owner_class,
            found: result_class,
        });
    }

    let (constructor_prefix, constructor) = match unit.store.types.try_get(function.ty) {
        Some(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(constructor),
        }) => (*prefix, *constructor),
        _ => {
            return Err(unsupported(
                apply.function,
                ShapeError::ConstructorReferenceNotCanonical,
            ));
        }
    };
    let prefix_class = planned_class(unit, constructor_prefix, apply.function, definitions).ok();
    if prefix_class != Some(owner_class) {
        return Err(JvmCodegenError::JvmConstructorCallClassMismatch {
            tree: apply.function,
            expected: owner_class,
            found: prefix_class,
        });
    }

    let class_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == owner_class)
        .ok_or(JvmCodegenError::MissingClassPlan { class: owner_class })?;
    let all_metadata = unit.metadata(definitions)?;
    let class_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == owner_class)
        .ok_or(JvmCodegenError::MissingClassPlan { class: owner_class })?;
    let superclass =
        class_metadata
            .superclass()
            .ok_or(JvmCodegenError::UnsupportedJvmSuperclass {
                class: owner_class,
                parent: None,
            })?;
    let class_info = crate::identity::class_info(unit.store, owner_class)?;
    crate::emission::validate_emittable_class(
        class_plan,
        class_metadata,
        unit.store,
        definitions,
        class_info,
        superclass,
    )?;
    if !unit.store.symbols.contains(constructor) {
        return Err(JvmCodegenError::UnknownMethod {
            method: constructor,
        });
    }
    let constructor_symbol = unit.store.symbols.get(constructor);
    if constructor_symbol.owner != Some(owner_class) {
        return Err(JvmCodegenError::JvmConstructorCallWrongOwner {
            tree: apply.function,
            constructor,
            expected: owner_class,
            actual: constructor_symbol.owner,
        });
    }
    if constructor_symbol.kind != SymbolKind::Constructor {
        return Err(JvmCodegenError::JvmConstructorCallNotApproved {
            tree: apply_tree,
            constructor,
            class: owner_class,
        });
    }
    if constructor_symbol.name.namespace() != dotty_core::Namespace::Term
        || unit.store.names.try_resolve(constructor_symbol.name.text()) != Some("<init>")
    {
        return Err(unsupported(
            apply.function,
            ShapeError::ConstructorReferenceNotCanonical,
        ));
    }
    if constructor_symbol.visibility == Visibility::Private && caller_class != owner_class {
        return Err(JvmCodegenError::InaccessibleJvmConstructor {
            tree: apply_tree,
            constructor,
            caller_class,
            owner_class,
        });
    }

    let (descriptor, access_flags, owner_internal_name) = approved_constructor(
        unit,
        class_plan,
        class_metadata,
        constructor,
        apply_tree,
        definitions,
    )?;

    let expected_arguments = descriptor.parameters();
    if apply.args.len() != expected_arguments.len() {
        return Err(JvmCodegenError::JvmConstructorCallArityMismatch {
            tree: apply_tree,
            constructor,
            expected: expected_arguments.len(),
            found: apply.args.len(),
        });
    }
    let mut argument_types = Vec::with_capacity(apply.args.len());
    for (argument, expected) in apply.args.iter().copied().zip(expected_arguments) {
        let argument_tree = tree(unit.store, unit.typed_ast, argument)?;
        let found = planned_argument_type(
            unit,
            caller_class,
            caller_method_plan,
            argument,
            argument_tree,
            definitions,
            &class_identities,
        );
        let assignable = match found {
            Some(found) => {
                crate::reference::is_assignable(Some(unit), found, *expected, definitions)?
            }
            None => false,
        };
        if !assignable {
            return Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
                tree: apply_tree,
                argument,
                expected: *expected,
                found,
            });
        }
        argument_types.push(*expected);
    }

    Ok(JvmConstructorCallTarget {
        owner_class,
        owner_internal_name,
        constructor,
        descriptor,
        access_flags,
        argument_types,
        result_type: JvmType::Reference(owner_class),
    })
}

fn approved_constructor(
    unit: &JvmCompilationUnit<'_>,
    class_plan: &crate::JvmClassPlan,
    class_metadata: &crate::JvmClassMetadata,
    constructor: SymbolId,
    tree: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<(crate::JvmMethodDescriptor, MethodAccessFlags, String), JvmCodegenError> {
    match &class_plan.initialization {
        JvmClassInitializationPlan::PrimaryConstructor(plan) => {
            let Some(metadata) = class_metadata.constructor() else {
                return Err(JvmCodegenError::JvmConstructorCallNotApproved {
                    tree,
                    constructor,
                    class: class_plan.class,
                });
            };
            if plan.symbol() != constructor || metadata.symbol() != constructor {
                return Err(JvmCodegenError::JvmConstructorCallNotApproved {
                    tree,
                    constructor,
                    class: class_plan.class,
                });
            }
            Ok((
                metadata.descriptor().clone(),
                metadata.access_flags(),
                metadata.owner_internal_name().to_owned(),
            ))
        }
        JvmClassInitializationPlan::ImplicitNoArg => {
            let store = unit.store;
            if !store.symbols.contains(constructor) {
                return Err(JvmCodegenError::UnknownMethod {
                    method: constructor,
                });
            }
            let symbol = store.symbols.get(constructor);
            if symbol.kind != SymbolKind::Constructor
                || symbol.owner != Some(class_plan.class)
                || symbol.origin != store.symbols.get(class_plan.class).origin
                || symbol.name.namespace() != dotty_core::Namespace::Term
                || store.names.try_resolve(symbol.name.text()) != Some("<init>")
                || symbol.visibility != Visibility::Public
                || !symbol.flags.is_empty()
            {
                return Err(unsupported(tree, ShapeError::InvalidImplicitNoArgProof));
            }
            let info = crate::identity::class_info(store, class_plan.class)?;
            let constructors = store
                .scopes
                .get(info.declarations)
                .entered_symbols()
                .filter(|symbol| {
                    store.symbols.contains(*symbol)
                        && store.symbols.get(*symbol).kind == SymbolKind::Constructor
                })
                .collect::<Vec<_>>();
            if constructors.as_slice() != [constructor] {
                return Err(unsupported(tree, ShapeError::InvalidImplicitNoArgProof));
            }
            let plan = JvmMethodPlan {
                method: constructor,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: None,
            };
            let descriptor = crate::descriptor::descriptor_for(
                &plan,
                store,
                class_plan.class,
                definitions,
                true,
            )?;
            if descriptor.as_str() != "()V" {
                return Err(unsupported(tree, ShapeError::InvalidImplicitNoArgProof));
            }
            Ok((
                descriptor,
                MethodAccessFlags(ACC_PUBLIC),
                class_metadata.identity().internal_name().to_owned(),
            ))
        }
        JvmClassInitializationPlan::Unsupported => {
            Err(unsupported(tree, ShapeError::UnsupportedInitializationPlan))
        }
    }
}

fn planned_class(
    unit: &JvmCompilationUnit<'_>,
    ty: dotty_core::ids::TypeId,
    tree: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<SymbolId, JvmCodegenError> {
    match crate::reference::project_constructor_type(unit, ty, definitions)? {
        JvmType::Reference(class) => Ok(class),
        _ => Err(unsupported(tree, ShapeError::TypeTreeNotCanonicalClass)),
    }
}

fn planned_result_class(
    unit: &JvmCompilationUnit<'_>,
    ty: dotty_core::ids::TypeId,
    expected_class: SymbolId,
    definitions: &Definitions,
) -> Option<SymbolId> {
    match unit.store.types.try_get(ty)? {
        Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        } if *prefix == definitions.no_prefix
            && matches!(unit.store.types.try_get(*prefix), Some(Type::NoPrefix))
            && *class == expected_class =>
        {
            Some(*class)
        }
        _ => crate::reference::project_constructor_type(unit, ty, definitions)
            .ok()
            .and_then(|ty| match ty {
                JvmType::Reference(class) => Some(class),
                _ => None,
            }),
    }
}

fn planned_argument_type(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: &JvmMethodPlan,
    argument: TreeId<Typed>,
    argument_tree: &Tree<Typed>,
    definitions: &Definitions,
    class_identities: &[crate::JvmClassIdentity],
) -> Option<JvmType> {
    let store = unit.store;
    if let TreeKind::This(this) = &argument_tree.kind {
        return (this.qual.is_none()
            && class_identities
                .iter()
                .any(|identity| identity.symbol() == caller_class)
            && matches!(
                store.types.try_get(argument_tree.ty),
                Some(Type::ThisType { class }) if *class == caller_class
            ))
        .then_some(JvmType::Reference(caller_class));
    }
    if matches!(argument_tree.kind, TreeKind::Select(_))
        && crate::resolve_boolean_intrinsic(store, unit.typed_ast, argument, definitions).ok()
            == Some(Some(crate::JvmBooleanIntrinsic::Not))
    {
        return Some(JvmType::Boolean);
    }
    match store.types.try_get(argument_tree.ty)? {
        Type::Constant(Constant::Int(_)) => Some(JvmType::Int),
        Type::Constant(Constant::Boolean(_)) => Some(JvmType::Boolean),
        Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        } if *prefix == definitions.no_prefix
            && matches!(store.types.try_get(*prefix), Some(Type::NoPrefix))
            && Some(*symbol) == type_symbol(store, definitions.int) =>
        {
            Some(JvmType::Int)
        }
        Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        } if *prefix == definitions.no_prefix
            && matches!(store.types.try_get(*prefix), Some(Type::NoPrefix))
            && Some(*symbol) == type_symbol(store, definitions.boolean) =>
        {
            Some(JvmType::Boolean)
        }
        Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        } => {
            let reference_context = ConstructorReferenceContext {
                definitions,
                class_identities,
            };
            term_reference_type(
                unit,
                caller_class,
                caller_method,
                argument_tree,
                *prefix,
                *symbol,
                &reference_context,
            )
        }
        _ => None,
    }
}

fn term_reference_type(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: &JvmMethodPlan,
    argument_tree: &Tree<Typed>,
    prefix: dotty_core::TypeId,
    symbol: SymbolId,
    reference_context: &ConstructorReferenceContext<'_>,
) -> Option<JvmType> {
    let store = unit.store;
    let target = store
        .symbols
        .contains(symbol)
        .then(|| store.symbols.get(symbol))?;
    let reference_is_planned = match target.kind {
        SymbolKind::Parameter => {
            target.owner == Some(caller_method.method)
                && caller_method.parameters.contains(&symbol)
                && is_ident_of(argument_tree, target.name)
                && prefix == reference_context.definitions.no_prefix
                && matches!(store.types.try_get(prefix), Some(Type::NoPrefix))
        }
        SymbolKind::Local => {
            target.owner == Some(caller_method.method)
                && caller_method
                    .locals
                    .iter()
                    .any(|binding| binding.symbol == symbol)
                && is_ident_of(argument_tree, target.name)
                && prefix == reference_context.definitions.no_prefix
                && matches!(store.types.try_get(prefix), Some(Type::NoPrefix))
        }
        SymbolKind::Field => {
            if target.owner != Some(caller_class)
                || !unit
                    .classes
                    .iter()
                    .find(|plan| plan.class == caller_class)
                    .is_some_and(|plan| plan.fields.iter().any(|field| field.field == symbol))
            {
                false
            } else {
                match &argument_tree.kind {
                    TreeKind::Ident(ident) => {
                        ident.name == target.name && is_this_prefix(store, prefix, caller_class)
                    }
                    TreeKind::Select(select) => {
                        if select.name != target.name || select.backquoted {
                            false
                        } else {
                            unit.typed_ast
                                .try_get(select.qualifier)
                                .is_some_and(|receiver| {
                                    matches!(
                                        &receiver.kind,
                                        TreeKind::This(dotty_core::ast::This { qual: None })
                                    ) && is_this_prefix(store, receiver.ty, caller_class)
                                        && prefix == receiver.ty
                                })
                        }
                    }
                    _ => false,
                }
            }
        }
        _ => false,
    };
    if !reference_is_planned {
        return None;
    }
    let semantic_type = match &target.info {
        SymbolInfo::Complete(ty) => ty,
        SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => return None,
    };
    if target.kind == SymbolKind::Field {
        crate::descriptor::primitive_semantic_value_type(
            store,
            reference_context.definitions,
            symbol,
            *semantic_type,
        )
        .ok()
    } else {
        let found = planned_term_value_type(
            unit,
            symbol,
            *semantic_type,
            reference_context.definitions,
            reference_context.class_identities,
        )?;
        if target.kind == SymbolKind::Local
            && matches!(found, JvmType::Reference(_))
            && (caller_method
                .locals
                .iter()
                .find(|binding| binding.symbol == symbol)
                .is_none_or(|binding| binding.mutable)
                || target
                    .flags
                    .contains(dotty_core::symbols::SymbolFlags::MUTABLE))
        {
            return None;
        }
        Some(found)
    }
}

fn planned_term_value_type(
    unit: &JvmCompilationUnit<'_>,
    symbol: SymbolId,
    ty: dotty_core::ids::TypeId,
    definitions: &Definitions,
    class_identities: &[crate::JvmClassIdentity],
) -> Option<JvmType> {
    match crate::descriptor::primitive_semantic_value_type(unit.store, definitions, symbol, ty) {
        Ok(value_type) => Some(value_type),
        Err(JvmCodegenError::UnsupportedJvmType { .. }) => {
            crate::reference::project_method_signature_type(
                unit.store,
                ty,
                definitions,
                class_identities,
            )
            .ok()
        }
        Err(_) => None,
    }
}

fn is_ident_of(tree: &Tree<Typed>, name: dotty_core::Name) -> bool {
    matches!(&tree.kind, TreeKind::Ident(ident) if ident.name == name && !ident.backquoted)
}

fn is_this_prefix(store: &SemanticStore, prefix: dotty_core::TypeId, class: SymbolId) -> bool {
    matches!(store.types.try_get(prefix), Some(Type::ThisType { class: found }) if *found == class)
}

fn type_symbol(store: &SemanticStore, ty: dotty_core::ids::TypeId) -> Option<SymbolId> {
    match store.types.try_get(ty)? {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn tree<'a>(
    store: &SemanticStore,
    ast: &'a dotty_core::ast::TypedAst,
    id: TreeId<Typed>,
) -> Result<&'a Tree<Typed>, JvmCodegenError> {
    let tree = ast
        .try_get(id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: id,
            node: "missing tree",
        })?;
    super::validate_tree_type(store, id, tree)?;
    Ok(tree)
}

fn unsupported(tree: TreeId<Typed>, kind: ShapeError) -> JvmCodegenError {
    JvmCodegenError::UnsupportedJvmConstructorCall { tree, kind }
}
