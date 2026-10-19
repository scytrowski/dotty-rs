//! Whole-class emission from a validated, frontend-owned plan.

use std::collections::HashSet;

use dotty_classfile::access_flags::{ACC_PUBLIC, MethodAccessFlags};
use dotty_classfile::bytecode::CodeBuilder;
use dotty_classfile::class_file::ClassFileVersion;
use dotty_classfile::class_file_builder::ClassFileBuilder;
use dotty_core::ids::SymbolId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::{MethodType, Type, TypeRefTarget};

use crate::{
    GeneratedClass, JVM_CLASSFILE_MAJOR_VERSION, JvmClassInitializationPlan, JvmCodegenError,
    JvmCompilationUnit,
};

pub(super) fn emit_class(
    unit: &JvmCompilationUnit<'_>,
    class_symbol: SymbolId,
    definitions: &dotty_core::definitions::Definitions,
) -> Result<GeneratedClass, JvmCodegenError> {
    unit.validate()?;
    let plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == class_symbol)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: class_symbol,
        })?;

    let all_metadata = unit.metadata(definitions)?;
    let metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == class_symbol)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: class_symbol,
        })?;
    let class_info = super::identity::class_info(unit.store, class_symbol)?;
    validate_emittable_class(plan, metadata, unit.store, definitions, class_info)?;

    let constructor_initializers = match &plan.initialization {
        JvmClassInitializationPlan::ImplicitNoArg => None,
        JvmClassInitializationPlan::PrimaryConstructor(_) => {
            Some(unit.lower_constructor_field_initializers(class_symbol, definitions)?)
        }
        JvmClassInitializationPlan::Unsupported => {
            return Err(JvmCodegenError::UnsupportedConstructor {
                class: class_symbol,
            });
        }
    };

    let mut builder = ClassFileBuilder::new(
        metadata.identity().internal_name(),
        ClassFileVersion {
            major: JVM_CLASSFILE_MAJOR_VERSION,
            minor: 0,
        },
    )
    .map_err(|error| class_file_error(class_symbol, None, error))?;
    builder
        .set_access_flags(metadata.identity().access_flags())
        .map_err(|error| class_file_error(class_symbol, None, error))?;

    for field in metadata.fields() {
        builder
            .add_field(field.name(), field.descriptor(), field.access_flags())
            .map_err(|error| class_file_error(class_symbol, None, error))?;
    }

    match constructor_initializers {
        None => {
            let mut constructor = object_constructor_prefix(class_symbol)?;
            constructor
                .return_void()
                .map_err(|error| JvmCodegenError::ConstructorCodeBuilder {
                    class: class_symbol,
                    error,
                })?;
            builder
                .add_prebuilt_code_method(
                    "<init>",
                    "()V",
                    MethodAccessFlags(ACC_PUBLIC),
                    constructor,
                )
                .map_err(|error| class_file_error(class_symbol, None, error))?;
        }
        Some(initializers) => {
            let constructor_metadata = initializers.constructor();
            let mut constructor = object_constructor_prefix(class_symbol)?;
            initializers.append_to(&mut constructor)?;
            constructor
                .return_void()
                .map_err(|error| JvmCodegenError::ConstructorCodeBuilder {
                    class: class_symbol,
                    error,
                })?;
            builder
                .add_prebuilt_code_method(
                    "<init>",
                    constructor_metadata.descriptor().as_str(),
                    constructor_metadata.access_flags(),
                    constructor,
                )
                .map_err(|error| {
                    class_file_error(class_symbol, Some(constructor_metadata.symbol()), error)
                })?;
        }
    }

    for (method_plan, method_metadata) in plan.methods.iter().zip(metadata.methods()) {
        let body = method_plan.body.ok_or(JvmCodegenError::MissingTypedBody {
            method: method_plan.method,
        })?;
        let code = crate::expression::lower_planned_method_body(
            unit,
            metadata,
            definitions,
            method_metadata,
            body,
        )?;
        builder
            .add_prebuilt_code_method(
                method_metadata.name(),
                method_metadata.descriptor().as_str(),
                method_metadata.access_flags(),
                code,
            )
            .map_err(|error| class_file_error(class_symbol, Some(method_plan.method), error))?;
    }

    let bytes = builder
        .encode()
        .map_err(|error| class_file_error(class_symbol, None, error))?;
    Ok(GeneratedClass {
        internal_name: metadata.identity().internal_name().to_owned(),
        bytes,
    })
}

fn object_constructor_prefix(class: SymbolId) -> Result<CodeBuilder, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    code.aload(0)
        .map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    code.invokespecial("java/lang/Object", "<init>", "()V")
        .map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    Ok(code)
}

pub(super) fn validate_emittable_class(
    plan: &crate::JvmClassPlan,
    metadata: &crate::JvmClassMetadata,
    store: &SemanticStore,
    definitions: &dotty_core::definitions::Definitions,
    class_info: &dotty_core::types::ClassInfo,
) -> Result<(), JvmCodegenError> {
    if let Some(self_type) = class_info.self_type {
        return Err(JvmCodegenError::UnsupportedJvmSelfType {
            class: plan.class,
            self_type,
        });
    }

    let parent = class_info.parents.first().copied();
    let valid_object_definition = matches!(
        store.types.try_get(definitions.object_type),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) if *symbol == definitions.object_class
    );
    if class_info.parents.len() != 1
        || parent != Some(definitions.object_type)
        || !valid_object_definition
    {
        return Err(JvmCodegenError::UnsupportedJvmSuperclass {
            class: plan.class,
            parent,
        });
    }

    let declarations = store.scopes.get(class_info.declarations).entered_symbols();
    let declared: HashSet<_> = declarations.collect();
    if let Some(symbol) = declared
        .iter()
        .copied()
        .find(|symbol| !store.symbols.contains(*symbol))
    {
        return Err(JvmCodegenError::InvalidClassDeclaration {
            class: plan.class,
            symbol,
        });
    }
    let planned_methods: HashSet<_> = plan.methods.iter().map(|method| method.method).collect();
    let planned_fields: HashSet<_> = metadata
        .fields()
        .iter()
        .map(|field| field.symbol())
        .collect();
    for method in &plan.methods {
        if !declared.contains(&method.method) {
            return Err(JvmCodegenError::MethodNotInClassDeclarations {
                class: plan.class,
                method: method.method,
            });
        }
    }
    for field in metadata.fields() {
        if !declared.contains(&field.symbol()) || field.owner() != plan.class {
            return Err(JvmCodegenError::FieldEmissionUnsupported {
                class: plan.class,
                field: field.symbol(),
            });
        }
    }

    let mut constructors = Vec::new();
    let mut owned_declarations = declared
        .into_iter()
        .filter(|symbol| store.symbols.get(*symbol).owner == Some(plan.class))
        .collect::<Vec<_>>();
    owned_declarations.sort_by_key(|symbol| symbol.index());
    for member in owned_declarations {
        let symbol = store.symbols.get(member);
        match symbol.kind {
            SymbolKind::Method if planned_methods.contains(&member) => {}
            SymbolKind::Method => {
                return Err(JvmCodegenError::MethodNotInJvmPlanForBody {
                    class: plan.class,
                    method: member,
                });
            }
            SymbolKind::Field if planned_fields.contains(&member) => {}
            SymbolKind::Field => {
                return Err(JvmCodegenError::FieldEmissionUnsupported {
                    class: plan.class,
                    field: member,
                });
            }
            SymbolKind::Constructor => constructors.push(member),
            kind => {
                return Err(JvmCodegenError::UnsupportedJvmClassMember {
                    class: plan.class,
                    member,
                    kind,
                });
            }
        }
    }

    if constructors.len() > 1 {
        return Err(JvmCodegenError::UnsupportedConstructor { class: plan.class });
    }
    match (
        &plan.initialization,
        metadata.constructor(),
        constructors.as_slice(),
    ) {
        (JvmClassInitializationPlan::ImplicitNoArg, None, []) => {}
        (JvmClassInitializationPlan::ImplicitNoArg, None, [constructor]) => {
            validate_primary_constructor(store, plan.class, *constructor)?;
        }
        (JvmClassInitializationPlan::PrimaryConstructor(_), Some(metadata), [constructor])
            if metadata.symbol() == *constructor => {}
        (JvmClassInitializationPlan::Unsupported, _, _) => {
            return Err(JvmCodegenError::UnsupportedConstructor { class: plan.class });
        }
        _ => return Err(JvmCodegenError::UnsupportedConstructor { class: plan.class }),
    }
    if matches!(
        &plan.initialization,
        JvmClassInitializationPlan::ImplicitNoArg
    ) && let Some(field) = metadata.fields().first()
    {
        return Err(JvmCodegenError::FieldEmissionUnsupported {
            class: plan.class,
            field: field.symbol(),
        });
    }
    Ok(())
}

pub(super) fn validate_primary_constructor(
    store: &SemanticStore,
    class: SymbolId,
    constructor: SymbolId,
) -> Result<(), JvmCodegenError> {
    let symbol = store.symbols.get(constructor);
    if store.names.try_resolve(symbol.name.text()) != Some("<init>") {
        return Err(JvmCodegenError::UnsupportedConstructor { class });
    }
    match symbol.info {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => Ok(()),
        SymbolInfo::Error => Err(JvmCodegenError::UnsupportedConstructor { class }),
        SymbolInfo::Complete(ty) => match store.types.try_get(ty) {
            Some(Type::Method(MethodType { params, result, .. })) if params.is_empty() => {
                match store.types.try_get(*result) {
                    Some(Type::Method(_) | Type::Poly(_)) | None => {
                        Err(JvmCodegenError::UnsupportedConstructor { class })
                    }
                    Some(_) => Ok(()),
                }
            }
            _ => Err(JvmCodegenError::UnsupportedConstructor { class }),
        },
    }
}

fn class_file_error(
    class: SymbolId,
    method: Option<SymbolId>,
    error: dotty_classfile::class_file_builder::ClassFileBuildError,
) -> JvmCodegenError {
    JvmCodegenError::ClassFileBuild {
        class,
        method,
        error,
    }
}
