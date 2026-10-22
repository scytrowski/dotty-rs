//! Checked projection of semantic class references within one emission unit.

use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolKind, SymbolOrigin};
use dotty_core::types::{Type, TypeRefTarget};

use crate::{
    InvalidTypeReason, JvmClassIdentity, JvmCodegenError, JvmCompilationUnit, JvmType,
    UnsupportedJvmReferenceTypeKind,
};

/// Returns whether `found` is assignable to `expected` through the validated
/// canonical ancestry in this emission unit, bounded to two user-class edges.
pub(super) fn is_validated_subtype(
    unit: &JvmCompilationUnit<'_>,
    found: SymbolId,
    expected: SymbolId,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    if found == expected {
        return Ok(true);
    }
    let hierarchy = unit.class_hierarchy(definitions)?;
    for class in [found, expected] {
        if !hierarchy
            .superclasses()
            .iter()
            .any(|superclass| superclass.class() == class)
        {
            return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
        }
    }

    let object = definitions.object_class;
    let mut current = found;
    for _ in 0..2 {
        let Some(superclass) = hierarchy
            .superclasses()
            .iter()
            .find(|superclass| superclass.class() == current)
        else {
            return Ok(false);
        };
        current = superclass.parent();
        if current == expected {
            return Ok(true);
        }
        if current == object {
            return Ok(false);
        }
    }
    Ok(false)
}

/// Returns whether a JVM value can flow to `expected` within one validated
/// compilation unit. References may use exact identity or at most two verified
/// user-class ancestry edges; primitive values remain invariant.
pub(super) fn is_assignable(
    unit: Option<&JvmCompilationUnit<'_>>,
    found: JvmType,
    expected: JvmType,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    if found == expected {
        if let (JvmType::Reference(class), Some(unit)) = (found, unit) {
            let hierarchy = unit.class_hierarchy(definitions)?;
            if !hierarchy
                .superclasses()
                .iter()
                .any(|superclass| superclass.class() == class)
            {
                return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
            }
        }
        return Ok(true);
    }
    let (JvmType::Reference(found), JvmType::Reference(expected), Some(unit)) =
        (found, expected, unit)
    else {
        return Ok(false);
    };
    is_validated_subtype(unit, found, expected, definitions)
}

pub(super) fn project_type(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<JvmType, JvmCodegenError> {
    unit.validate()?;
    project_type_with_metadata(unit, ty, definitions, None)
}

/// Proves the narrow checked JVM representation for an explicitly expected
/// sibling union. The source `Type::Or` remains untouched.
pub(super) fn project_sibling_union_to_expected_type(
    unit: &JvmCompilationUnit<'_>,
    source_type: TypeId,
    expected_type: TypeId,
    definitions: &Definitions,
) -> Result<crate::JvmSiblingUnionProjection, JvmCodegenError> {
    unit.validate()?;
    let unsupported = || JvmCodegenError::UnsupportedSiblingUnionProjection {
        source_type,
        expected_type,
    };
    let Some(Type::Or { left, right }) = unit.store.types.try_get(source_type) else {
        return Err(unsupported());
    };
    let source_arms = [*left, *right];
    let mut arm_classes = Vec::with_capacity(2);
    for arm in source_arms {
        let JvmType::Reference(class) = project_expected_class_type(unit, arm, definitions)? else {
            return Err(unsupported());
        };
        arm_classes.push(class);
    }
    let JvmType::Reference(expected_class) =
        project_expected_class_type(unit, expected_type, definitions)?
    else {
        return Err(unsupported());
    };
    let [left_class, right_class] = arm_classes.as_slice() else {
        return Err(unsupported());
    };
    let arm_classes = [*left_class, *right_class];

    // A Type::Or produced by the audited frontend represents two distinct
    // siblings. Ancestor pairs already have an ordinary exact projection and
    // must continue to use that existing path.
    if arm_classes[0] == arm_classes[1]
        || is_validated_subtype(unit, arm_classes[0], arm_classes[1], definitions)?
        || is_validated_subtype(unit, arm_classes[1], arm_classes[0], definitions)?
    {
        return Err(unsupported());
    }

    let hierarchy = unit.class_hierarchy(definitions)?;
    if hierarchy.nearest_common_non_object_ancestor(arm_classes[0], arm_classes[1])
        != Some(expected_class)
        || !is_validated_subtype(unit, arm_classes[0], expected_class, definitions)?
        || !is_validated_subtype(unit, arm_classes[1], expected_class, definitions)?
    {
        return Err(unsupported());
    }

    Ok(crate::JvmSiblingUnionProjection {
        source_type,
        expected_type,
        source_arms: arm_classes,
        representation: JvmType::Reference(expected_class),
    })
}

fn project_expected_class_type(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<JvmType, JvmCodegenError> {
    match project_type_with_metadata(unit, ty, definitions, None) {
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            kind: UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
            ..
        }) => project_constructor_type(unit, ty, definitions),
        result => result,
    }
}

/// Projects the canonical class type used by source constructor applications.
/// Source typing retains the owning package as the class reference prefix;
/// accepting it here still requires an exact root-package TypeRef matching the
/// class symbol's canonical owner.
pub(super) fn project_constructor_type(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<JvmType, JvmCodegenError> {
    unit.validate()?;
    let Some(Type::TypeRef { prefix, target }) = unit.store.types.try_get(ty) else {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        });
    };
    let TypeRefTarget::Symbol(class) = target else {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        });
    };
    let expected_package = unit
        .store
        .symbols
        .contains(*class)
        .then(|| unit.store.symbols.get(*class).owner)
        .flatten();
    let canonical_package_prefix = matches!(
        unit.store.types.try_get(*prefix),
        Some(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package),
        }) if *package_prefix == definitions.no_prefix
            && matches!(unit.store.types.try_get(*package_prefix), Some(Type::NoPrefix))
            && Some(*package) == expected_package
            && unit.store.symbols.contains(*package)
            && unit.store.symbols.get(*package).kind == SymbolKind::Package
    );
    if !canonical_package_prefix {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
        });
    }
    project_class(Some(unit), unit.store, ty, *class, definitions, None, None)
}

/// Projects a method signature reference against identities already validated
/// for the enclosing emission unit. The source TypeRef must use either the
/// canonical no-prefix form or the exact package owner of its target class.
pub(super) fn project_method_signature_type(
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
    class_identities: &[JvmClassIdentity],
    unit: Option<&JvmCompilationUnit<'_>>,
) -> Result<JvmType, JvmCodegenError> {
    let class = method_signature_class_target(store, ty, definitions)?;
    project_class(
        unit,
        store,
        ty,
        class,
        definitions,
        None,
        Some(class_identities),
    )
}

/// Returns the target of a supported method-signature class reference after
/// checking the same canonical prefix rules used by JVM type projection.
pub(super) fn method_signature_class_target(
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<dotty_core::SymbolId, JvmCodegenError> {
    let Some(Type::TypeRef { prefix, target }) = store.types.try_get(ty) else {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        });
    };
    let TypeRefTarget::Symbol(class) = target else {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        });
    };
    let expected_package = store
        .symbols
        .contains(*class)
        .then(|| store.symbols.get(*class).owner)
        .flatten();
    let canonical_no_prefix = *prefix == definitions.no_prefix
        && matches!(store.types.try_get(*prefix), Some(Type::NoPrefix));
    let canonical_package_prefix = matches!(
        store.types.try_get(*prefix),
        Some(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package),
        }) if *package_prefix == definitions.no_prefix
            && matches!(store.types.try_get(*package_prefix), Some(Type::NoPrefix))
            && Some(*package) == expected_package
            && store.symbols.contains(*package)
            && store.symbols.get(*package).kind == SymbolKind::Package
    );
    if !canonical_no_prefix && !canonical_package_prefix {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
        });
    }
    Ok(*class)
}

fn project_type_with_metadata(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
    metadata: Option<&[crate::JvmClassMetadata]>,
) -> Result<JvmType, JvmCodegenError> {
    match unit.store.types.try_get(ty) {
        None => Err(JvmCodegenError::InvalidReferenceType {
            ty,
            reason: InvalidTypeReason::MissingOrUnfilled,
        }),
        Some(Type::NoType) => Err(JvmCodegenError::InvalidReferenceType {
            ty,
            reason: InvalidTypeReason::NoType,
        }),
        Some(Type::NoPrefix) => Err(JvmCodegenError::InvalidReferenceType {
            ty,
            reason: InvalidTypeReason::NoPrefix,
        }),
        Some(Type::Error(_)) => Err(JvmCodegenError::InvalidReferenceType {
            ty,
            reason: InvalidTypeReason::Error,
        }),
        Some(Type::TypeRef { prefix, target }) => {
            match unit.store.types.try_get(*prefix) {
                None => {
                    return Err(JvmCodegenError::InvalidReferenceType {
                        ty: *prefix,
                        reason: InvalidTypeReason::MissingOrUnfilled,
                    });
                }
                Some(Type::Error(_)) => {
                    return Err(JvmCodegenError::InvalidReferenceType {
                        ty: *prefix,
                        reason: InvalidTypeReason::Error,
                    });
                }
                Some(Type::NoType) => {
                    return Err(JvmCodegenError::InvalidReferenceType {
                        ty: *prefix,
                        reason: InvalidTypeReason::NoType,
                    });
                }
                Some(Type::NoPrefix)
                    if *prefix == definitions.no_prefix
                        && matches!(
                            unit.store.types.try_get(definitions.no_prefix),
                            Some(Type::NoPrefix)
                        ) => {}
                _ => {
                    return Err(JvmCodegenError::UnsupportedJvmReferenceType {
                        ty,
                        kind: UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
                    });
                }
            }
            match target {
                TypeRefTarget::Symbol(class) => project_class(
                    Some(unit),
                    unit.store,
                    ty,
                    *class,
                    definitions,
                    metadata,
                    None,
                ),
                _ => Err(JvmCodegenError::UnsupportedJvmReferenceType {
                    ty,
                    kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
                }),
            }
        }
        Some(_) => Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        }),
    }
}

fn project_class(
    unit: Option<&JvmCompilationUnit<'_>>,
    store: &SemanticStore,
    ty: TypeId,
    class: dotty_core::SymbolId,
    definitions: &Definitions,
    metadata: Option<&[crate::JvmClassMetadata]>,
    class_identities: Option<&[JvmClassIdentity]>,
) -> Result<JvmType, JvmCodegenError> {
    if !store.symbols.contains(class) {
        return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
    }
    let symbol = store.symbols.get(class);
    if symbol.kind != SymbolKind::Class {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::UnsupportedSymbolKind,
        });
    }

    let object = match store.types.try_get(definitions.object_type) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(object),
            ..
        }) => *object,
        _ => {
            return Err(JvmCodegenError::UnsupportedJvmReferenceType {
                ty,
                kind: UnsupportedJvmReferenceTypeKind::UnsupportedClassMetadata,
            });
        }
    };
    if class == object {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::ObjectClass,
        });
    }
    if class_identities.is_none()
        && !unit.is_some_and(|unit| unit.classes.iter().any(|plan| plan.class == class))
    {
        return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
    }

    if matches!(
        symbol.origin,
        SymbolOrigin::Classfile(_) | SymbolOrigin::Tasty(_) | SymbolOrigin::Builtin
    ) {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::ExternalClass,
        });
    }
    if symbol
        .flags
        .contains(dotty_core::symbols::SymbolFlags::SYNTHETIC)
    {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::AnonymousClass,
        });
    }

    let class_info = match crate::identity::class_info(store, class) {
        Ok(info) => info,
        Err(JvmCodegenError::UnsupportedJvmClass {
            kind: crate::UnsupportedJvmClassKind::Generic,
            ..
        }) => {
            return Err(JvmCodegenError::UnsupportedJvmReferenceType {
                ty,
                kind: UnsupportedJvmReferenceTypeKind::GenericClass,
            });
        }
        Err(error) => return Err(error),
    };

    // Validate complete unit metadata before asking the hierarchy proof about
    // this exact target. An unplanned canonical class has no ancestry proof in
    // this unit and should remain a planned-target error.
    let is_planned = if let Some(class_identities) = class_identities {
        class_identities
            .iter()
            .any(|identity| identity.symbol() == class)
    } else {
        let generated_metadata;
        let metadata = match metadata {
            Some(metadata) => metadata,
            None => {
                let Some(unit) = unit else {
                    return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
                };
                generated_metadata = unit.metadata(definitions)?;
                &generated_metadata
            }
        };
        metadata
            .iter()
            .any(|class_metadata| class_metadata.identity().symbol() == class)
    };
    if !is_planned {
        return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
    }

    let object_rooted = if let Some(unit) = unit {
        let hierarchy = unit.class_hierarchy(definitions)?;
        let mut cursor = class;
        let mut rooted = false;
        while let Some(edge) = hierarchy
            .superclasses()
            .iter()
            .find(|edge| edge.class() == cursor)
        {
            if edge.parent() == object {
                rooted = true;
                break;
            }
            cursor = edge.parent();
        }
        rooted
    } else if class_info.parents.len() != 1 {
        false
    } else if let Some(parent) = type_symbol(store, class_info.parents[0]) {
        if parent == object {
            true
        } else {
            let parent_is_planned = class_identities.is_some_and(|identities| {
                identities
                    .iter()
                    .any(|identity| identity.symbol() == parent)
            });
            if parent_is_planned {
                let parent_info = crate::identity::class_info(store, parent)?;
                parent_info.parents.len() == 1
                    && type_symbol(store, parent_info.parents[0]) == Some(object)
            } else {
                false
            }
        }
    } else {
        false
    };
    if !object_rooted {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotObjectDerived,
        });
    }

    Ok(JvmType::Reference(class))
}

pub(super) fn project_local_type(
    unit: &JvmCompilationUnit<'_>,
    symbol: dotty_core::SymbolId,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<JvmType, JvmCodegenError> {
    project_local_type_with_metadata(unit, symbol, ty, definitions, None)
}

pub(super) fn project_local_type_with_metadata(
    unit: &JvmCompilationUnit<'_>,
    symbol: dotty_core::SymbolId,
    ty: TypeId,
    definitions: &Definitions,
    metadata: Option<&[crate::JvmClassMetadata]>,
) -> Result<JvmType, JvmCodegenError> {
    match crate::descriptor::primitive_semantic_value_type(unit.store, definitions, symbol, ty) {
        Ok(value) => Ok(value),
        Err(JvmCodegenError::UnsupportedJvmType { .. }) => {
            match project_type_with_metadata(unit, ty, definitions, metadata) {
                Err(JvmCodegenError::UnsupportedJvmReferenceType {
                    kind: crate::UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
                    ..
                }) => project_constructor_type(unit, ty, definitions),
                result => result,
            }
        }
        Err(error) => Err(error),
    }
}

fn type_symbol(store: &SemanticStore, ty: TypeId) -> Option<dotty_core::SymbolId> {
    match store.types.try_get(ty)? {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}
