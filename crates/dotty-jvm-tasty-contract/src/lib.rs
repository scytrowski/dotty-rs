//! Read-only consistency proofs for the audited JVM / Scala 3 TASTy overlap.
//!
//! This crate is the acyclic coordination layer between `dotty-jvm` and
//! `dotty-tasty-pickler`. It accepts only a validated private primitive
//! class-state plan and a JVM unit from the same canonical session. A proof
//! checks semantic identities, source-to-typed roots, names, and the bounded
//! erased ABI; it does not encode, publish, or claim Scala-consumable artifacts.

use std::error::Error;
use std::fmt;

use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC, ACC_STATIC};
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::PoolRefError;
use dotty_classfile::tasty_link::{TastyLinkError, TastyLinkId};
use dotty_core::ast::{TreeKind, Untyped};
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_core::types::{MethodKind, MethodType, Type};
use dotty_core::{Definitions, SemanticStore, SourceId, SymbolId, TreeId, Typed};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassMetadata, JvmCodegenError, JvmCompilationUnit,
    JvmFieldSourceVisibility,
};
use dotty_tasty::tasty::{TastyFile, TastyFileError};
use dotty_tasty_pickler::{ClassMemberPicklePlan, PickleError, ValidatedClassStatePlan};

/// A typed, read-only failure while proving one JVM class and TASTy plan agree.
#[derive(Debug)]
pub enum JvmTastyContractError {
    /// The two producer plans do not borrow the same canonical source session.
    DifferentSession(&'static str),
    /// One of the producer plans failed its own validation.
    JvmPlan(JvmCodegenError),
    /// The TASTy plan was valid when created, but its projection failed.
    TastyPlan(PickleError),
    /// The TASTy class is not present exactly once in the JVM unit.
    JvmClassMissing { class: SymbolId },
    /// A JVM class identity does not match the canonical source declaration.
    ClassIdentityMismatch { expected: SymbolId, found: SymbolId },
    /// Scala's encoded `.tasty` path and the JVM internal name disagree.
    ClassPathMismatch {
        tasty_path: String,
        internal_name: String,
    },
    /// Constructor identity, formals, or descriptor differs between plans.
    ConstructorMismatch {
        constructor: SymbolId,
        expected: String,
        actual: String,
    },
    /// A declared source member is absent from a producer plan.
    MemberMissing {
        member: SymbolId,
        side: &'static str,
    },
    /// Producer member order or source identity differs.
    MemberOrderMismatch {
        expected: Vec<SymbolId>,
        actual: Vec<SymbolId>,
    },
    /// A member's canonical owner or source origin is wrong.
    MemberProvenanceMismatch { member: SymbolId, source: SourceId },
    /// The producer plans disagree on the exact typed RHS root of a source RHS.
    TypedRootMismatch {
        member: SymbolId,
        source_tree: TreeId<Untyped>,
        expected: Option<TreeId<Typed>>,
        actual: Option<TreeId<Typed>>,
    },
    /// JVM descriptor does not match the supported source-level type projection.
    DescriptorMismatch {
        member: SymbolId,
        expected: String,
        actual: String,
    },
    /// The class-state field is not the exact private mutable primitive field audited here.
    FieldShapeMismatch { field: SymbolId },
    /// The ABI contains an unsupported source type or method shape.
    UnsupportedSourceShape { symbol: SymbolId },
    /// Two methods occupy the same JVM `(name, descriptor)` slot.
    DuplicateJvmMethod { name: String, descriptor: String },
    /// Classfile is missing the class-level `TASTY` link.
    MissingTastyLink { internal_name: String },
    /// Classfile link UUID differs from the validated Scala 3.9 TASTy header.
    StaleTastyLink {
        internal_name: String,
        classfile_uuid: TastyLinkId,
        tasty_uuid: TastyLinkId,
    },
    /// The classfile's opt-in TASTY link could not be read or validated.
    InvalidTastyLink { error: TastyLinkError },
    /// The classfile's `this_class` did not resolve to a class name.
    InvalidClassName { error: PoolRefError },
    /// The sidecar is not a valid pinned Scala 3.9 TASTy file.
    InvalidTastyFile { error: TastyFileError },
}

impl fmt::Display for JvmTastyContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JVM/TASTy class contract is invalid: {self:?}")
    }
}

impl Error for JvmTastyContractError {}

/// Check that a decoded classfile's `TASTY` UUID links to these exact
/// validated Scala 3.9 TASTy bytes. This is a byte-level pair-integrity check;
/// it does not establish semantic ABI agreement without
/// [`validate_jvm_tasty_class`].
pub fn validate_classfile_tasty_link(
    class_file: &ClassFile,
    tasty_bytes: &[u8],
) -> Result<(), JvmTastyContractError> {
    let internal_name = class_file
        .constant_pool
        .class_name(class_file.this_class)
        .map_err(|error| JvmTastyContractError::InvalidClassName { error })?
        .to_owned();
    let classfile_uuid = class_file
        .tasty_link_id()
        .map_err(|error| JvmTastyContractError::InvalidTastyLink { error })?
        .ok_or_else(|| JvmTastyContractError::MissingTastyLink {
            internal_name: internal_name.clone(),
        })?;
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes)
        .map_err(|error| JvmTastyContractError::InvalidTastyFile { error })?;
    let tasty_uuid = TastyLinkId::from_bytes(tasty.header().uuid);
    if classfile_uuid != tasty_uuid {
        return Err(JvmTastyContractError::StaleTastyLink {
            internal_name,
            classfile_uuid,
            tasty_uuid,
        });
    }
    Ok(())
}

/// Borrowing proof that one validated JVM class plan and one complete TASTy
/// class-state plan describe the same supported source class in one session.
///
/// The proof retains both input borrows, so callers cannot mutate either plan
/// while relying on the result. It contains no artifact bytes or UUID claim.
pub struct ValidatedJvmTastyClass<'proof, 'unit, 'plan, 'session> {
    jvm_unit: &'proof JvmCompilationUnit<'unit>,
    tasty_plan: &'proof ValidatedClassStatePlan<'plan, 'session>,
    metadata: JvmClassMetadata,
    tasty_path: String,
}

impl ValidatedJvmTastyClass<'_, '_, '_, '_> {
    /// Canonical source class identity proven by both producer plans.
    pub fn class(&self) -> SymbolId {
        self.tasty_plan.plan().class
    }

    /// Checked JVM internal name, equal to the TASTy artifact path without suffix.
    pub fn internal_name(&self) -> &str {
        self.metadata.identity().internal_name()
    }

    /// Exact encoded relative `.tasty` path produced by the validated pickler plan.
    pub fn tasty_path(&self) -> &str {
        &self.tasty_path
    }

    /// Validated JVM metadata snapshot for the paired class.
    pub fn jvm_metadata(&self) -> &JvmClassMetadata {
        &self.metadata
    }

    /// Borrowed input plans retained by this proof.
    pub fn plans(&self) -> (&JvmCompilationUnit<'_>, &ValidatedClassStatePlan<'_, '_>) {
        (self.jvm_unit, self.tasty_plan)
    }
}

/// Prove the audited class/member ABI agreement for the TASTy plan's class.
///
/// This intentionally supports the source-private mutable primitive-field
/// class-state subset. Unsupported source constructs fail closed. The proof
/// must be paired later with actual pickled bytes and a classfile TASTY link;
/// those byte-level checks belong to the artifact publication stage.
pub fn validate_jvm_tasty_class<'proof, 'unit, 'plan, 'session>(
    jvm_unit: &'proof JvmCompilationUnit<'unit>,
    tasty_plan: &'proof ValidatedClassStatePlan<'plan, 'session>,
) -> Result<ValidatedJvmTastyClass<'proof, 'unit, 'plan, 'session>, JvmTastyContractError> {
    let tasty = tasty_plan.plan();
    if !std::ptr::eq(jvm_unit.store, tasty.store) {
        return Err(JvmTastyContractError::DifferentSession("semantic store"));
    }
    if !std::ptr::eq(jvm_unit.typed_ast, tasty.typed_ast) {
        return Err(JvmTastyContractError::DifferentSession("typed AST"));
    }

    jvm_unit
        .validate()
        .map_err(JvmTastyContractError::JvmPlan)?;

    let class_plan = jvm_unit
        .classes
        .iter()
        .find(|plan| plan.class == tasty.class)
        .ok_or(JvmTastyContractError::JvmClassMissing { class: tasty.class })?;
    let source = tasty.source.id;
    let class_symbol = &jvm_unit.store.symbols.get(tasty.class);
    if class_symbol.kind != SymbolKind::Class
        || class_symbol.owner.is_none()
        || class_symbol.origin != SymbolOrigin::Source(source)
    {
        return Err(JvmTastyContractError::MemberProvenanceMismatch {
            member: tasty.class,
            source,
        });
    }
    if tasty.source_ast.try_get(tasty.class_tree).is_none() {
        return Err(JvmTastyContractError::MemberProvenanceMismatch {
            member: tasty.class,
            source,
        });
    }

    let identity = class_plan
        .identity(jvm_unit.store)
        .map_err(JvmTastyContractError::JvmPlan)?;
    if identity.symbol() != tasty.class {
        return Err(JvmTastyContractError::ClassIdentityMismatch {
            expected: tasty.class,
            found: identity.symbol(),
        });
    }
    let tasty_path = tasty_plan
        .intended_tasty_path()
        .map_err(JvmTastyContractError::TastyPlan)?;
    let expected_internal = tasty_path.strip_suffix(".tasty").unwrap_or_default();
    if expected_internal != identity.internal_name() {
        return Err(JvmTastyContractError::ClassPathMismatch {
            tasty_path,
            internal_name: identity.internal_name().to_owned(),
        });
    }

    let jvm_metadata = jvm_unit
        .metadata(tasty.definitions)
        .map_err(JvmTastyContractError::JvmPlan)?
        .into_iter()
        .find(|metadata| metadata.identity().symbol() == tasty.class)
        .ok_or(JvmTastyContractError::JvmClassMissing { class: tasty.class })?;

    validate_constructor(jvm_unit.store, tasty, class_plan, &jvm_metadata)?;
    validate_fields(jvm_unit.store, tasty, class_plan, &jvm_metadata)?;
    validate_methods(jvm_unit.store, tasty, class_plan, &jvm_metadata)?;

    Ok(ValidatedJvmTastyClass {
        jvm_unit,
        tasty_plan,
        metadata: jvm_metadata,
        tasty_path,
    })
}

fn validate_constructor(
    store: &SemanticStore,
    tasty: &dotty_tasty_pickler::ClassStatePicklePlan<'_>,
    jvm_class: &dotty_jvm::JvmClassPlan,
    metadata: &JvmClassMetadata,
) -> Result<(), JvmTastyContractError> {
    let constructor = tasty.constructor.symbol;
    let JvmClassInitializationPlan::PrimaryConstructor(jvm_constructor) = &jvm_class.initialization
    else {
        return Err(JvmTastyContractError::ConstructorMismatch {
            constructor,
            expected: "source primary constructor".into(),
            actual: "implicit or unsupported initialization".into(),
        });
    };
    let Some(binary_constructor) = metadata.constructor() else {
        return Err(JvmTastyContractError::ConstructorMismatch {
            constructor,
            expected: "primary constructor metadata".into(),
            actual: "missing".into(),
        });
    };
    let expected_parameters = tasty
        .constructor
        .parameter_clauses
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let actual_parameters = jvm_constructor.parameters().to_vec();
    let metadata_parameters = binary_constructor
        .parameters()
        .iter()
        .map(|parameter| parameter.symbol())
        .collect::<Vec<_>>();
    let expected_descriptor =
        source_constructor_descriptor(store, tasty.definitions, &expected_parameters).ok_or(
            JvmTastyContractError::UnsupportedSourceShape {
                symbol: constructor,
            },
        )?;
    let actual_descriptor = binary_constructor.descriptor().as_str().to_owned();
    if jvm_constructor.symbol() != constructor
        || actual_parameters != expected_parameters
        || metadata_parameters != expected_parameters
        || expected_descriptor != actual_descriptor
    {
        return Err(JvmTastyContractError::ConstructorMismatch {
            constructor,
            expected: expected_descriptor,
            actual: actual_descriptor,
        });
    }
    for parameter in &expected_parameters {
        validate_symbol_source(store, *parameter, tasty.source.id, Some(constructor))?;
    }
    Ok(())
}

fn validate_fields(
    store: &SemanticStore,
    tasty: &dotty_tasty_pickler::ClassStatePicklePlan<'_>,
    jvm_class: &dotty_jvm::JvmClassPlan,
    metadata: &JvmClassMetadata,
) -> Result<(), JvmTastyContractError> {
    let tasty_fields = tasty
        .members
        .iter()
        .filter_map(|member| match member {
            ClassMemberPicklePlan::Field(field) => Some(*field),
            ClassMemberPicklePlan::Method { .. } => None,
        })
        .collect::<Vec<_>>();
    let jvm_fields = jvm_class.fields.iter().collect::<Vec<_>>();
    let metadata_fields = metadata.fields();
    let expected = tasty_fields
        .iter()
        .map(|field| field.symbol)
        .collect::<Vec<_>>();
    let actual = jvm_fields
        .iter()
        .map(|field| field.field)
        .collect::<Vec<_>>();
    if expected != actual {
        return Err(JvmTastyContractError::MemberOrderMismatch { expected, actual });
    }
    for field in tasty_fields {
        let Some(jvm_field) = jvm_fields
            .iter()
            .find(|candidate| candidate.field == field.symbol)
        else {
            return Err(JvmTastyContractError::MemberMissing {
                member: field.symbol,
                side: "JVM field plan",
            });
        };
        let Some(binary_field) = metadata_fields
            .iter()
            .find(|candidate| candidate.symbol() == field.symbol)
        else {
            return Err(JvmTastyContractError::MemberMissing {
                member: field.symbol,
                side: "JVM field metadata",
            });
        };
        let symbol = store.symbols.get(field.symbol);
        let mutable = symbol.flags.contains(SymbolFlags::MUTABLE);
        validate_symbol_source(store, field.symbol, tasty.source.id, Some(tasty.class))?;
        let rhs = match &tasty.source_ast.get(field.source_tree).kind {
            TreeKind::ValDef(definition) => definition.rhs,
            _ => None,
        };
        let mapped_rhs = rhs.and_then(|tree| tasty.typed_bindings.typed_at(tasty.source.id, tree));
        if mapped_rhs != Some(field.typed_initializer)
            || jvm_field.initializer != Some(field.typed_initializer)
        {
            return Err(JvmTastyContractError::TypedRootMismatch {
                member: field.symbol,
                source_tree: field.source_tree,
                expected: Some(field.typed_initializer),
                actual: jvm_field.initializer,
            });
        }
        let expected_descriptor = primitive_descriptor(tasty.definitions, field.declared_type)
            .ok_or(JvmTastyContractError::UnsupportedSourceShape {
                symbol: field.symbol,
            })?;
        let flags = binary_field.access_flags();
        let mutable_field = !flags.contains(ACC_FINAL);
        if symbol.owner != Some(tasty.class)
            || symbol.kind != SymbolKind::Field
            || symbol.visibility != Visibility::Private
            || !mutable
            || jvm_field.source_visibility != JvmFieldSourceVisibility::PlainPrivate
            || binary_field.descriptor() != expected_descriptor
            || binary_field.owner() != tasty.class
            || !flags.contains(ACC_PRIVATE)
            || flags.contains(ACC_STATIC)
            || mutable_field != mutable
            || metadata
                .accessors()
                .iter()
                .any(|accessor| accessor.source_field() == field.symbol)
        {
            return Err(JvmTastyContractError::FieldShapeMismatch {
                field: field.symbol,
            });
        }
    }
    Ok(())
}

fn validate_methods(
    store: &SemanticStore,
    tasty: &dotty_tasty_pickler::ClassStatePicklePlan<'_>,
    jvm_class: &dotty_jvm::JvmClassPlan,
    metadata: &JvmClassMetadata,
) -> Result<(), JvmTastyContractError> {
    let mut expected = Vec::new();
    for member in tasty.members {
        let ClassMemberPicklePlan::Method {
            symbol,
            source_tree,
        } = *member
        else {
            continue;
        };
        expected.push(symbol);
        validate_symbol_source(store, symbol, tasty.source.id, Some(tasty.class))?;
        let TreeKind::DefDef(definition) = &tasty.source_ast.get(source_tree).kind else {
            return Err(JvmTastyContractError::MemberProvenanceMismatch {
                member: symbol,
                source: tasty.source.id,
            });
        };
        let parameters = definition
            .value_param_clauses
            .iter()
            .flat_map(|clause| clause.iter())
            .map(|tree| {
                tasty.source_index.symbol_at(tasty.source.id, *tree).ok_or(
                    JvmTastyContractError::MemberProvenanceMismatch {
                        member: symbol,
                        source: tasty.source.id,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected_root = definition
            .rhs
            .map(|rhs| {
                tasty.typed_bindings.typed_at(tasty.source.id, rhs).ok_or(
                    JvmTastyContractError::TypedRootMismatch {
                        member: symbol,
                        source_tree,
                        expected: None,
                        actual: None,
                    },
                )
            })
            .transpose()?;
        let jvm_plan = jvm_class
            .methods
            .iter()
            .find(|candidate| candidate.method == symbol)
            .ok_or(JvmTastyContractError::MemberMissing {
                member: symbol,
                side: "JVM method plan",
            })?;
        let binary = metadata
            .methods()
            .iter()
            .find(|candidate| candidate.symbol() == symbol)
            .ok_or(JvmTastyContractError::MemberMissing {
                member: symbol,
                side: "JVM method metadata",
            })?;
        let actual_parameters = binary
            .parameters()
            .iter()
            .map(|parameter| parameter.symbol())
            .collect::<Vec<_>>();
        let expected_descriptor = source_method_descriptor(store, tasty.definitions, symbol)
            .ok_or(JvmTastyContractError::UnsupportedSourceShape { symbol })?;
        if parameters != jvm_plan.parameters
            || parameters != actual_parameters
            || expected_descriptor != binary.descriptor().as_str()
        {
            return Err(JvmTastyContractError::DescriptorMismatch {
                member: symbol,
                expected: expected_descriptor,
                actual: binary.descriptor().as_str().to_owned(),
            });
        }
        if jvm_plan.body != expected_root {
            return Err(JvmTastyContractError::TypedRootMismatch {
                member: symbol,
                source_tree,
                expected: expected_root,
                actual: jvm_plan.body,
            });
        }
        if !binary.access_flags().contains(ACC_PUBLIC) {
            return Err(JvmTastyContractError::UnsupportedSourceShape { symbol });
        }
        let slot = (
            binary.name().to_owned(),
            binary.descriptor().as_str().to_owned(),
        );
        if metadata
            .methods()
            .iter()
            .filter(|candidate| {
                candidate.name() == slot.0 && candidate.descriptor().as_str() == slot.1
            })
            .count()
            != 1
        {
            return Err(JvmTastyContractError::DuplicateJvmMethod {
                name: slot.0,
                descriptor: slot.1,
            });
        }
    }
    let actual = jvm_class
        .methods
        .iter()
        .map(|method| method.method)
        .collect::<Vec<_>>();
    if expected != actual {
        return Err(JvmTastyContractError::MemberOrderMismatch { expected, actual });
    }
    Ok(())
}

fn validate_symbol_source(
    store: &SemanticStore,
    symbol: SymbolId,
    source: SourceId,
    owner: Option<SymbolId>,
) -> Result<(), JvmTastyContractError> {
    let record = &store.symbols.get(symbol);
    if record.origin != SymbolOrigin::Source(source)
        || owner.is_some_and(|owner| record.owner != Some(owner))
    {
        return Err(JvmTastyContractError::MemberProvenanceMismatch {
            member: symbol,
            source,
        });
    }
    Ok(())
}

fn primitive_descriptor(definitions: &Definitions, ty: dotty_core::TypeId) -> Option<&'static str> {
    match ty {
        ty if ty == definitions.int => Some("I"),
        ty if ty == definitions.boolean => Some("Z"),
        _ => None,
    }
}

fn source_method_descriptor(
    store: &SemanticStore,
    definitions: &Definitions,
    method: SymbolId,
) -> Option<String> {
    let SymbolInfo::Complete(mut ty) = store.symbols.get(method).info else {
        return None;
    };
    let mut parameters = Vec::new();
    loop {
        match store.types.try_get(ty)? {
            Type::Method(MethodType {
                params,
                result,
                kind: MethodKind::Plain,
            }) => {
                if params
                    .iter()
                    .any(|parameter| parameter.erased || parameter.varargs)
                {
                    return None;
                }
                for parameter in params {
                    parameters.push(primitive_descriptor(definitions, parameter.ty)?);
                }
                ty = *result;
            }
            Type::Method(_) => return None,
            _ => break,
        }
    }
    let result = match primitive_descriptor(definitions, ty) {
        Some("I") => "I",
        Some("Z") => "Z",
        _ if is_unit_type(definitions, ty) => "V",
        _ => return None,
    };
    Some(format!("({}){result}", parameters.concat()))
}

fn source_constructor_descriptor(
    store: &SemanticStore,
    definitions: &Definitions,
    parameters: &[SymbolId],
) -> Option<String> {
    let mut descriptors = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        let SymbolInfo::Complete(ty) = store.symbols.get(*parameter).info else {
            return None;
        };
        descriptors.push(primitive_descriptor(definitions, ty)?);
    }
    // JVM constructors always return void, even if the source constructor
    // symbol's semantic result records its owning class.
    Some(format!("({})V", descriptors.concat()))
}

fn is_unit_type(definitions: &Definitions, ty: dotty_core::TypeId) -> bool {
    ty == definitions.unit
}
