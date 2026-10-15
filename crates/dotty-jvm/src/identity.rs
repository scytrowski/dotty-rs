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
    JvmFieldPlan, JvmMethodMetadata, JvmNameKind, JvmParameterLocal, UnsupportedJvmClassKind,
    UnsupportedJvmFieldFlag, UnsupportedJvmSymbolFlag,
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

pub(super) fn class_metadata(
    plan: &crate::JvmClassPlan,
    store: &SemanticStore,
    typed_ast: Option<&dotty_core::ast::TypedAst>,
    definitions: &Definitions,
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
        let field = field_metadata(field_plan, plan.class, store, typed_ast, definitions)?;
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
        let method = method_metadata(method_plan, store, plan.class, definitions)?;
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

    Ok(JvmClassMetadata {
        identity,
        fields,
        methods,
    })
}

fn field_metadata(
    plan: &JvmFieldPlan,
    owner: dotty_core::SymbolId,
    store: &SemanticStore,
    typed_ast: Option<&dotty_core::ast::TypedAst>,
    definitions: &Definitions,
) -> Result<JvmFieldMetadata, JvmCodegenError> {
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
    if class_info.parents.as_slice() != [definitions.object_type] || !valid_object_definition {
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
    let field_type = crate::descriptor::primitive_semantic_value_type(
        store,
        definitions,
        plan.field,
        field_type_id,
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
    let initializer_type =
        typed_expression_primitive_type(store, definitions, plan.field, initializer.ty)?;
    if initializer_type != field_type {
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

    Ok(JvmFieldMetadata {
        symbol: plan.field,
        name: name.to_owned(),
        descriptor: field_type.descriptor_char().to_string(),
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

fn typed_expression_primitive_type(
    store: &SemanticStore,
    definitions: &Definitions,
    symbol: dotty_core::SymbolId,
    ty: dotty_core::TypeId,
) -> Result<crate::JvmType, JvmCodegenError> {
    match store.types.try_get(ty) {
        Some(Type::TermRef {
            target: dotty_core::types::TermRefTarget::Symbol(target),
            ..
        }) => {
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
    let mut metadata = Vec::with_capacity(unit.classes.len());
    let mut identities = HashMap::<String, dotty_core::SymbolId>::new();
    for class_plan in &unit.classes {
        let class = class_plan.metadata_with_typed_ast(unit.store, unit.typed_ast, definitions)?;
        let internal_name = class.identity().internal_name().to_owned();
        if let Some(previous) = identities.insert(internal_name.clone(), class.identity().symbol())
        {
            return Err(JvmCodegenError::DuplicateJvmClassIdentity {
                class: class.identity().symbol(),
                previous,
                internal_name,
            });
        }
        metadata.push(class);
    }
    Ok(metadata)
}

fn method_metadata(
    plan: &crate::JvmMethodPlan,
    store: &SemanticStore,
    owner: dotty_core::SymbolId,
    definitions: &Definitions,
) -> Result<JvmMethodMetadata, JvmCodegenError> {
    // Descriptor projection verifies canonical method/parameter ownership and
    // type correspondence before any output layout is constructed.
    let descriptor = plan.descriptor(store, owner, definitions)?;
    let method = store.symbols.get(plan.method);
    if method.visibility != Visibility::Public {
        return Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: plan.method,
            visibility: method.visibility,
        });
    }
    let access_flags = method_access_flags(plan.method, method.flags)?;
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
    Ok(MethodAccessFlags(
        ACC_PUBLIC
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
