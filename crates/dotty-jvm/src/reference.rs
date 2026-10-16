//! Checked projection of semantic class references within one emission unit.

use dotty_core::definitions::Definitions;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolKind, SymbolOrigin};
use dotty_core::types::{Type, TypeRefTarget};

use crate::{
    InvalidTypeReason, JvmCodegenError, JvmCompilationUnit, JvmType,
    UnsupportedJvmReferenceTypeKind,
};

pub(super) fn project_type(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<JvmType, JvmCodegenError> {
    unit.validate()?;
    project_type_with_metadata(unit, ty, definitions, None)
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
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) => project_class(unit, ty, *class, definitions, metadata),
        Some(_) => Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        }),
    }
}

fn project_class(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    class: dotty_core::SymbolId,
    definitions: &Definitions,
    metadata: Option<&[crate::JvmClassMetadata]>,
) -> Result<JvmType, JvmCodegenError> {
    let store = unit.store;
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
    if !unit.classes.iter().any(|plan| plan.class == class) {
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
    let object_derived =
        class_info.parents.len() == 1 && type_symbol(store, class_info.parents[0]) == Some(object);
    if !object_derived {
        return Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty,
            kind: UnsupportedJvmReferenceTypeKind::NotObjectDerived,
        });
    }

    // Re-run complete unit metadata validation: this proves the target has a
    // valid top-level class identity, supported ordinary class metadata, no
    // type parameters, and a complete set of caller-owned plans. No semantic
    // store or type-arena state is changed by this projection.
    let generated_metadata;
    let metadata = match metadata {
        Some(metadata) => metadata,
        None => {
            generated_metadata = unit.metadata(definitions)?;
            &generated_metadata
        }
    };
    if !metadata
        .iter()
        .any(|class_metadata| class_metadata.identity().symbol() == class)
    {
        return Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class });
    }

    Ok(JvmType::Reference(class))
}

pub(super) fn project_local_type(
    unit: &JvmCompilationUnit<'_>,
    symbol: dotty_core::SymbolId,
    ty: TypeId,
    definitions: &Definitions,
) -> Result<JvmType, JvmCodegenError> {
    unit.validate()?;
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
            project_type_with_metadata(unit, ty, definitions, metadata)
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
