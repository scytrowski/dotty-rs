//! Bounded projection from canonical primitive semantic types to JVM
//! descriptors. This deliberately does not implement general Scala erasure.

use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
use dotty_core::types::{Constant, MethodKind, Type, TypeRefTarget};
use std::collections::HashSet;

use crate::{
    InvalidTypeReason, JvmCodegenError, JvmMethodDescriptor, JvmMethodPlan, JvmReturnType, JvmType,
    JvmValueCategory, PrimitiveBuiltin, UnsupportedJvmTypeKind, UnsupportedMethodParameterKind,
    UnsupportedMethodShapeKind,
};

#[derive(Clone, Copy)]
struct PrimitiveSymbols {
    int: SymbolId,
    boolean: SymbolId,
    unit: SymbolId,
}

pub(super) fn method_descriptor(
    plan: &JvmMethodPlan,
    store: &SemanticStore,
    owner: SymbolId,
    definitions: &Definitions,
) -> Result<JvmMethodDescriptor, JvmCodegenError> {
    let class = store
        .symbols
        .contains(owner)
        .then(|| store.symbols.get(owner))
        .ok_or(JvmCodegenError::UnknownClass { class: owner })?;
    if class.kind != SymbolKind::Class {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: owner,
            expected: crate::SymbolRole::Class,
            found: class.kind,
        });
    }

    let method = store
        .symbols
        .contains(plan.method)
        .then(|| store.symbols.get(plan.method))
        .ok_or(JvmCodegenError::UnknownMethod {
            method: plan.method,
        })?;
    if method.kind != SymbolKind::Method {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: plan.method,
            expected: crate::SymbolRole::Method,
            found: method.kind,
        });
    }
    if method.owner != Some(owner) {
        return Err(JvmCodegenError::WrongOwner {
            symbol: plan.method,
            expected: owner,
            actual: method.owner,
        });
    }

    let method_type_id = complete_info(store, plan.method)?;
    let method_type = match store.types.try_get(method_type_id) {
        Some(Type::Method(method_type)) => method_type,
        Some(Type::Poly(_)) => {
            return Err(JvmCodegenError::UnsupportedMethodShape {
                method: plan.method,
                ty: method_type_id,
                kind: UnsupportedMethodShapeKind::Polymorphic,
            });
        }
        Some(_) => {
            return Err(JvmCodegenError::UnsupportedMethodShape {
                method: plan.method,
                ty: method_type_id,
                kind: UnsupportedMethodShapeKind::NotMethodType,
            });
        }
        None => {
            return Err(JvmCodegenError::InvalidSemanticType {
                symbol: plan.method,
                ty: method_type_id,
                reason: InvalidTypeReason::MissingOrUnfilled,
            });
        }
    };

    if method_type.kind != MethodKind::Plain {
        return Err(JvmCodegenError::UnsupportedMethodShape {
            method: plan.method,
            ty: method_type_id,
            kind: UnsupportedMethodShapeKind::NonPlainClause,
        });
    }
    if method_type.params.len() != plan.parameters.len() {
        return Err(JvmCodegenError::UnsupportedMethodShape {
            method: plan.method,
            ty: method_type_id,
            kind: UnsupportedMethodShapeKind::ParameterCountMismatch,
        });
    }

    // JVMS §4.3.3 counts `this` for an instance method. Every supported
    // parameter occupies one slot. Bound this before allocating or converting
    // the output parameter vector.
    let receiver_slots = usize::from(!method.flags.contains(SymbolFlags::STATIC));
    let slots = method_type.params.len() + receiver_slots;
    if slots > crate::JVM_MAX_PARAMETER_SLOTS {
        return Err(JvmCodegenError::TooManyParameterSlots {
            method: plan.method,
            slots,
        });
    }

    let primitive_symbols = PrimitiveSymbols {
        int: builtin_symbol(store, definitions.int, PrimitiveBuiltin::Int)?,
        boolean: builtin_symbol(store, definitions.boolean, PrimitiveBuiltin::Boolean)?,
        unit: builtin_symbol(store, definitions.unit, PrimitiveBuiltin::Unit)?,
    };

    let mut parameters = Vec::with_capacity(method_type.params.len());
    let mut seen_parameters = HashSet::new();
    for (index, (method_param, parameter_id)) in
        method_type.params.iter().zip(&plan.parameters).enumerate()
    {
        if !seen_parameters.insert(*parameter_id) {
            return Err(JvmCodegenError::DuplicateParameter {
                parameter: *parameter_id,
                method: plan.method,
            });
        }
        if method_param.erased {
            return Err(JvmCodegenError::UnsupportedMethodParameter {
                method: plan.method,
                index,
                kind: UnsupportedMethodParameterKind::Erased,
            });
        }
        if method_param.varargs {
            return Err(JvmCodegenError::UnsupportedMethodParameter {
                method: plan.method,
                index,
                kind: UnsupportedMethodParameterKind::Varargs,
            });
        }

        let parameter = store
            .symbols
            .contains(*parameter_id)
            .then(|| store.symbols.get(*parameter_id))
            .ok_or(JvmCodegenError::UnknownParameter {
                parameter: *parameter_id,
            })?;
        if parameter.kind != SymbolKind::Parameter {
            return Err(JvmCodegenError::UnsupportedSymbolKind {
                symbol: *parameter_id,
                expected: crate::SymbolRole::Parameter,
                found: parameter.kind,
            });
        }
        if parameter.owner != Some(plan.method) {
            return Err(JvmCodegenError::WrongOwner {
                symbol: *parameter_id,
                expected: plan.method,
                actual: parameter.owner,
            });
        }

        let symbol_type = complete_info(store, *parameter_id)?;
        let signature_type = primitive_value_type(
            store,
            method_param.ty,
            *parameter_id,
            plan.method,
            &primitive_symbols,
        )?;
        let symbol_value_type = primitive_value_type(
            store,
            symbol_type,
            *parameter_id,
            plan.method,
            &primitive_symbols,
        )?;
        if signature_type != symbol_value_type {
            return Err(JvmCodegenError::ParameterTypeMismatch {
                method: plan.method,
                parameter: *parameter_id,
                declared: method_param.ty,
                symbol_info: symbol_type,
            });
        }
        parameters.push(symbol_value_type);
    }

    let result =
        match primitive_result_type(store, method_type.result, plan.method, &primitive_symbols)? {
            Some(ty) => JvmReturnType::Value(ty),
            None => JvmReturnType::Void,
        };

    let descriptor_text = format!(
        "({}){}",
        parameters
            .iter()
            .map(|ty| ty.descriptor_char())
            .collect::<String>(),
        result.descriptor_suffix()
    );
    let parsed = MethodDescriptor::parse(&descriptor_text).map_err(|error| {
        JvmCodegenError::InvalidGeneratedDescriptor {
            method: plan.method,
            error,
        }
    })?;

    Ok(JvmMethodDescriptor {
        descriptor: parsed,
        descriptor_text,
        parameters,
        result,
    })
}

fn complete_info(store: &SemanticStore, symbol: SymbolId) -> Result<TypeId, JvmCodegenError> {
    match store.symbols.info(symbol) {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            Err(JvmCodegenError::MissingSemanticInfo { symbol })
        }
        SymbolInfo::Error => Err(JvmCodegenError::ErrorSemanticInfo { symbol }),
        SymbolInfo::Complete(ty) => match store.types.try_get(*ty) {
            None => Err(JvmCodegenError::InvalidSemanticType {
                symbol,
                ty: *ty,
                reason: InvalidTypeReason::MissingOrUnfilled,
            }),
            Some(Type::NoType) => Err(JvmCodegenError::InvalidSemanticType {
                symbol,
                ty: *ty,
                reason: InvalidTypeReason::NoType,
            }),
            Some(Type::NoPrefix) => Err(JvmCodegenError::InvalidSemanticType {
                symbol,
                ty: *ty,
                reason: InvalidTypeReason::NoPrefix,
            }),
            Some(Type::Error(_)) => Err(JvmCodegenError::InvalidSemanticType {
                symbol,
                ty: *ty,
                reason: InvalidTypeReason::Error,
            }),
            Some(_) => Ok(*ty),
        },
    }
}

fn builtin_symbol(
    store: &SemanticStore,
    ty: TypeId,
    builtin: PrimitiveBuiltin,
) -> Result<SymbolId, JvmCodegenError> {
    match store.types.try_get(ty) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) if store.symbols.contains(*symbol) => Ok(*symbol),
        _ => Err(JvmCodegenError::InvalidBuiltinDefinition { builtin, ty }),
    }
}

fn primitive_value_type(
    store: &SemanticStore,
    ty: TypeId,
    symbol: SymbolId,
    method: SymbolId,
    primitives: &PrimitiveSymbols,
) -> Result<JvmType, JvmCodegenError> {
    validate_semantic_type(store, symbol, ty)?;
    match store.types.try_get(ty) {
        Some(Type::Constant(Constant::Int(_))) => Ok(JvmType::Int),
        Some(Type::Constant(Constant::Boolean(_))) => Ok(JvmType::Boolean),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        }) if *target == primitives.int => Ok(JvmType::Int),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        }) if *target == primitives.boolean => Ok(JvmType::Boolean),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        }) if *target == primitives.unit => Err(JvmCodegenError::UnitParameter {
            method,
            parameter: symbol,
            ty,
        }),
        _ => Err(unsupported_type_error(store, symbol, ty)),
    }
}

fn primitive_result_type(
    store: &SemanticStore,
    ty: TypeId,
    symbol: SymbolId,
    primitives: &PrimitiveSymbols,
) -> Result<Option<JvmType>, JvmCodegenError> {
    validate_semantic_type(store, symbol, ty)?;
    if matches!(
        store.types.try_get(ty),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        }) if *target == primitives.unit
    ) {
        return Ok(None);
    }
    if matches!(store.types.try_get(ty), Some(Type::Method(_))) {
        return Err(JvmCodegenError::UnsupportedMethodShape {
            method: symbol,
            ty,
            kind: UnsupportedMethodShapeKind::NestedCurriedClause,
        });
    }
    primitive_value_type(store, ty, symbol, symbol, primitives).map(Some)
}

fn validate_semantic_type(
    store: &SemanticStore,
    symbol: SymbolId,
    ty: TypeId,
) -> Result<(), JvmCodegenError> {
    let reason = match store.types.try_get(ty) {
        None => Some(InvalidTypeReason::MissingOrUnfilled),
        Some(Type::NoType) => Some(InvalidTypeReason::NoType),
        Some(Type::NoPrefix) => Some(InvalidTypeReason::NoPrefix),
        Some(Type::Error(_)) => Some(InvalidTypeReason::Error),
        Some(_) => None,
    };
    match reason {
        Some(reason) => Err(JvmCodegenError::InvalidSemanticType { symbol, ty, reason }),
        None => Ok(()),
    }
}

fn unsupported_type_error(store: &SemanticStore, symbol: SymbolId, ty: TypeId) -> JvmCodegenError {
    let kind = match store.types.try_get(ty) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        }) if store.symbols.contains(*target) => match store.symbols.get(*target).kind {
            SymbolKind::TypeAlias => UnsupportedJvmTypeKind::Alias,
            SymbolKind::TypeParameter => UnsupportedJvmTypeKind::TypeParameter,
            SymbolKind::Object | SymbolKind::ModuleClass => UnsupportedJvmTypeKind::ScalaObject,
            SymbolKind::Class | SymbolKind::Trait => UnsupportedJvmTypeKind::ReferenceClass,
            _ => UnsupportedJvmTypeKind::Other,
        },
        Some(Type::TypeRef { .. }) => UnsupportedJvmTypeKind::NamedReference,
        Some(Type::TermRef { target, .. }) => match target.symbol() {
            Some(target)
                if store.symbols.contains(target)
                    && matches!(
                        store.symbols.get(target).kind,
                        SymbolKind::Object | SymbolKind::ModuleClass
                    ) =>
            {
                UnsupportedJvmTypeKind::ScalaObject
            }
            _ => UnsupportedJvmTypeKind::TermReference,
        },
        Some(Type::Poly(_)) | Some(Type::TypeLambda(_)) | Some(Type::ParamRef { .. }) => {
            UnsupportedJvmTypeKind::TypeParameter
        }
        Some(Type::Method(_)) => UnsupportedJvmTypeKind::NestedMethod,
        Some(Type::Match(_)) | Some(Type::MatchCase { .. }) => UnsupportedJvmTypeKind::MatchType,
        Some(Type::Refined { .. }) => UnsupportedJvmTypeKind::RefinedType,
        Some(Type::Or { .. }) => UnsupportedJvmTypeKind::Union,
        Some(Type::And { .. }) => UnsupportedJvmTypeKind::Intersection,
        Some(Type::JavaArray { .. }) => UnsupportedJvmTypeKind::Array,
        Some(Type::ByName { .. }) => UnsupportedJvmTypeKind::ByName,
        Some(Type::Repeated { .. }) => UnsupportedJvmTypeKind::Repeated,
        Some(Type::Bounds { .. })
        | Some(Type::AliasingBounds { .. })
        | Some(Type::Wildcard { .. }) => UnsupportedJvmTypeKind::Bounds,
        Some(Type::Constant(_)) => UnsupportedJvmTypeKind::NonPrimitiveConstant,
        Some(Type::ThisType { .. })
        | Some(Type::SuperType { .. })
        | Some(Type::ClassInfo(_))
        | Some(Type::Applied { .. }) => UnsupportedJvmTypeKind::ReferenceClass,
        _ => UnsupportedJvmTypeKind::Other,
    };
    JvmCodegenError::UnsupportedJvmType { symbol, ty, kind }
}

impl JvmType {
    const fn descriptor_char(self) -> char {
        match self {
            Self::Int => 'I',
            Self::Boolean => 'Z',
        }
    }

    /// JVM operand-stack category. Both supported primitives use an
    /// int-like, category-one slot while retaining distinct descriptors.
    pub const fn category(self) -> JvmValueCategory {
        JvmValueCategory::IntLike
    }

    const fn field_type(self) -> FieldType {
        match self {
            Self::Int => FieldType::Int,
            Self::Boolean => FieldType::Boolean,
        }
    }
}

impl JvmReturnType {
    const fn descriptor_suffix(self) -> &'static str {
        match self {
            Self::Value(JvmType::Int) => "I",
            Self::Value(JvmType::Boolean) => "Z",
            Self::Void => "V",
        }
    }

    /// Return operand category, or `None` for the result-only JVM `void`.
    pub const fn category(self) -> Option<JvmValueCategory> {
        match self {
            Self::Value(ty) => Some(ty.category()),
            Self::Void => None,
        }
    }
}

impl JvmMethodDescriptor {
    /// Validated JVM method descriptor text, such as `(ZII)I`.
    pub fn as_str(&self) -> &str {
        &self.descriptor_text
    }

    /// Parsed descriptor from `dotty-classfile` for downstream classfile APIs.
    pub fn parsed(&self) -> &MethodDescriptor {
        &self.descriptor
    }

    /// Supported parameter values in declaration order.
    pub fn parameters(&self) -> &[JvmType] {
        &self.parameters
    }

    /// Supported result category, including result-only `Void`.
    pub const fn result(&self) -> JvmReturnType {
        self.result
    }
}

impl From<JvmType> for FieldType {
    fn from(value: JvmType) -> Self {
        value.field_type()
    }
}
