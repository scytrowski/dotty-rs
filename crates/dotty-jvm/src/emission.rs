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
    let superclass = metadata
        .superclass()
        .ok_or(JvmCodegenError::UnsupportedJvmSuperclass {
            class: class_symbol,
            parent: class_info.parents.first().copied(),
        })?;
    validate_emittable_class(
        plan,
        metadata,
        unit.store,
        definitions,
        class_info,
        superclass,
    )?;

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
    let verification_hierarchy = unit
        .class_hierarchy(definitions)?
        .internal_name_edges()
        .into_iter()
        .map(|edge| {
            (
                edge.child_internal_name().to_owned(),
                edge.parent_internal_name().to_owned(),
            )
        });
    let verification_hierarchy = dotty_classfile::bytecode::ClassHierarchy::from_edges(
        verification_hierarchy,
    )
    .map_err(|error| {
        class_file_error(
            class_symbol,
            None,
            dotty_classfile::class_file_builder::ClassFileBuildError::Analysis(
                dotty_classfile::bytecode::AnalysisError::InvalidClassHierarchy(error),
            ),
        )
    })?;
    builder.set_verification_hierarchy(verification_hierarchy);
    if superclass.parent() != definitions.object_class {
        builder
            .set_super_class(Some(superclass.parent_internal_name()))
            .map_err(|error| class_file_error(class_symbol, None, error))?;
    }

    for accessor in metadata.accessors() {
        builder
            .add_field(
                accessor.field_name(),
                accessor.field_descriptor(),
                accessor.field_access_flags(),
            )
            .map_err(|error| {
                class_file_error(class_symbol, Some(accessor.source_field()), error)
            })?;
    }
    for field in metadata.fields() {
        builder
            .add_field(field.name(), field.descriptor(), field.access_flags())
            .map_err(|error| class_file_error(class_symbol, None, error))?;
    }

    match constructor_initializers {
        None => {
            let mut constructor =
                constructor_prefix(class_symbol, superclass.parent_internal_name())?;
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
            let mut constructor = CodeBuilder::new();
            if !crate::constructor_initializers::append_direct_super_constructor_call(
                unit,
                class_symbol,
                definitions,
                &mut constructor,
            )? {
                constructor = constructor_prefix(class_symbol, superclass.parent_internal_name())?;
            }
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

    for accessor in metadata.accessors() {
        let getter = accessor.getter();
        let mut code = CodeBuilder::new();
        code.aload(0)
            .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
                class: class_symbol,
                field: accessor.source_field(),
                error,
            })?;
        code.getfield(
            metadata.identity().internal_name(),
            accessor.field_name(),
            accessor.field_descriptor(),
        )
        .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
            class: class_symbol,
            field: accessor.source_field(),
            error,
        })?;
        match accessor.value_type() {
            crate::JvmType::Int | crate::JvmType::Boolean => code.ireturn(),
            crate::JvmType::Float => code.freturn(),
            crate::JvmType::Long => code.lreturn(),
            crate::JvmType::Double => code.dreturn(),
            crate::JvmType::Reference(_) => code.areturn(),
        }
        .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
            class: class_symbol,
            field: accessor.source_field(),
            error,
        })?;
        builder
            .add_prebuilt_code_method(
                getter.name(),
                getter.descriptor().as_str(),
                getter.access_flags(),
                code,
            )
            .map_err(|error| {
                class_file_error(class_symbol, Some(accessor.source_field()), error)
            })?;

        if let Some(setter) = accessor.setter() {
            let mut code = CodeBuilder::new();
            code.aload(0)
                .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
                    class: class_symbol,
                    field: accessor.source_field(),
                    error,
                })?;
            match accessor.value_type() {
                crate::JvmType::Int | crate::JvmType::Boolean => code.iload(1),
                crate::JvmType::Float => code.fload(1),
                crate::JvmType::Long => code.lload(1),
                crate::JvmType::Double => code.dload(1),
                crate::JvmType::Reference(_) => code.aload(1),
            }
            .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
                class: class_symbol,
                field: accessor.source_field(),
                error,
            })?;
            code.putfield(
                metadata.identity().internal_name(),
                accessor.field_name(),
                accessor.field_descriptor(),
            )
            .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
                class: class_symbol,
                field: accessor.source_field(),
                error,
            })?;
            code.return_void()
                .map_err(|error| JvmCodegenError::AccessorMethodCodeBuilder {
                    class: class_symbol,
                    field: accessor.source_field(),
                    error,
                })?;
            builder
                .add_prebuilt_code_method(
                    setter.name(),
                    setter.descriptor().as_str(),
                    setter.access_flags(),
                    code,
                )
                .map_err(|error| {
                    class_file_error(class_symbol, Some(accessor.source_field()), error)
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

fn constructor_prefix(class: SymbolId, parent: &str) -> Result<CodeBuilder, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    code.aload(0)
        .map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    code.invokespecial(parent, "<init>", "()V")
        .map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    Ok(code)
}

pub(super) fn validate_emittable_class(
    plan: &crate::JvmClassPlan,
    metadata: &crate::JvmClassMetadata,
    store: &SemanticStore,
    definitions: &dotty_core::definitions::Definitions,
    class_info: &dotty_core::types::ClassInfo,
    superclass: &crate::JvmSuperclassPlan,
) -> Result<(), JvmCodegenError> {
    if let Some(field) = plan.fields.iter().find(|field| {
        matches!(
            field.source_visibility,
            crate::JvmFieldSourceVisibility::PlainPublic(_)
        )
    }) {
        return Err(JvmCodegenError::FieldEmissionUnsupported {
            class: plan.class,
            field: field.field,
        });
    }

    if let Some(self_type) = class_info.self_type {
        return Err(JvmCodegenError::UnsupportedJvmSelfType {
            class: plan.class,
            self_type,
        });
    }

    let parent = class_info.parents.first().copied();
    let parent_symbol = parent.and_then(|parent| match store.types.try_get(parent) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) => Some(*symbol),
        _ => None,
    });
    let valid_object_definition = matches!(
        store.types.try_get(definitions.object_type),
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) if *symbol == definitions.object_class
    );
    if class_info.parents.len() != 1
        || superclass.class() != plan.class
        || superclass.class_internal_name() != metadata.identity().internal_name()
        || parent_symbol != Some(superclass.parent())
        || (superclass.parent() == definitions.object_class
            && (parent != Some(definitions.object_type)
                || superclass.parent_internal_name() != "java/lang/Object"
                || !valid_object_definition))
        || (superclass.parent() != definitions.object_class
            && superclass.parent_internal_name() == "java/lang/Object")
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
        .chain(
            metadata
                .accessors()
                .iter()
                .map(|accessor| accessor.source_field()),
        )
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
    for accessor in metadata.accessors() {
        if !declared.contains(&accessor.source_field()) || accessor.owner() != plan.class {
            return Err(JvmCodegenError::FieldEmissionUnsupported {
                class: plan.class,
                field: accessor.source_field(),
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
    ) && let Some(field) = metadata
        .fields()
        .first()
        .map(|field| field.symbol())
        .or_else(|| {
            metadata
                .accessors()
                .first()
                .map(|accessor| accessor.source_field())
        })
    {
        return Err(JvmCodegenError::FieldEmissionUnsupported {
            class: plan.class,
            field,
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
