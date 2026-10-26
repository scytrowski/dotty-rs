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
use dotty_classfile::reader::Reader as ClassFileReader;
use dotty_classfile::tasty_link::{TastyLinkError, TastyLinkId};
use dotty_core::ast::{AstArena, TreeKind, Untyped};
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_core::types::{MethodKind, MethodType, TermRefTarget, Type};
use dotty_core::{Definitions, SemanticStore, SourceId, SymbolId, TreeId, Typed};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassMetadata, JvmCodegenError, JvmCompilationUnit,
    JvmFieldSourceVisibility,
};
use dotty_tasty::tasty::{TastyFile, TastyFileError};
use dotty_tasty_pickler::{ClassMemberPicklePlan, PickleError, ValidatedClassStatePlan};

mod factory_reference;
pub use factory_reference::{
    SameBatchSourceEvidence, ValidatedSameBatchConstructorReference,
    ValidatedSameBatchReferenceReturn, validate_same_batch_factory_result,
    validate_same_batch_reference_return,
};

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
    /// The plans do not describe the complete ordered JVM compilation unit.
    BatchShape(&'static str),
    /// One class failed while validating the complete batch.
    BatchClass {
        class_index: usize,
        class: SymbolId,
        error: Box<JvmTastyContractError>,
    },
    /// A same-batch factory proof failed at a specific source/typed boundary.
    FactoryReferenceMismatch {
        class: SymbolId,
        method: Option<SymbolId>,
        constructor: Option<SymbolId>,
        source_tree: Option<TreeId<Untyped>>,
        typed_tree: Option<TreeId<Typed>>,
        stage: &'static str,
        reason: &'static str,
    },
    /// A batch artifact entry set has a duplicate, missing, or unexpected path.
    BatchArtifactPaths { reason: &'static str, path: String },
    /// One emitted class/sidecar pair failed byte-level batch validation.
    BatchArtifact {
        class_index: usize,
        class: SymbolId,
        phase: &'static str,
        reason: String,
    },
}

impl fmt::Display for JvmTastyContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JVM/TASTy class contract is invalid: {self:?}")
    }
}

impl Error for JvmTastyContractError {}

/// A class's immutable ABI proof retained by a whole-source batch receipt.
pub struct ValidatedJvmTastyBatchClass<'proof, 'unit, 'plan, 'session> {
    proof: ValidatedJvmTastyClass<'proof, 'unit, 'plan, 'session>,
    classfile_path: String,
}

impl ValidatedJvmTastyBatchClass<'_, '_, '_, '_> {
    /// Per-class source/TASTy/JVM semantic proof.
    pub fn proof(&self) -> &ValidatedJvmTastyClass<'_, '_, '_, '_> {
        &self.proof
    }
    /// Exact package-qualified class-file path required by this batch.
    pub fn classfile_path(&self) -> &str {
        &self.classfile_path
    }
}

/// Read-only proof that every planned class is present exactly once, ordered
/// consistently, and validated against the same complete JVM unit/session.
pub struct ValidatedJvmTastyBatch<'proof, 'unit, 'plan, 'session> {
    classes: Vec<ValidatedJvmTastyBatchClass<'proof, 'unit, 'plan, 'session>>,
    source_id: SourceId,
}

impl ValidatedJvmTastyBatch<'_, '_, '_, '_> {
    /// Source identity shared by every class in this one-session batch.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    /// Class proofs in canonical source declaration order.
    pub fn classes(&self) -> &[ValidatedJvmTastyBatchClass<'_, '_, '_, '_>] {
        &self.classes
    }
}

/// Owned class-file bytes supplied to the post-encoding batch checker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmTastyClassFileArtifact {
    /// Class-file output path.
    pub classfile_path: String,
    /// Complete class-file bytes.
    pub classfile_bytes: Vec<u8>,
}

/// Owned TASTy sidecar bytes supplied to the post-encoding batch checker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmTastySidecarArtifact {
    /// TASTy output path.
    pub tasty_path: String,
    /// Complete TASTy bytes.
    pub tasty_bytes: Vec<u8>,
}

/// Owned, independently keyed artifact collections. Independent path maps
/// allow orphan and extra sidecars to be rejected explicitly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JvmTastyBatchArtifacts {
    /// Class files in arbitrary input order.
    pub classfiles: Vec<JvmTastyClassFileArtifact>,
    /// TASTy sidecars in arbitrary input order.
    pub sidecars: Vec<JvmTastySidecarArtifact>,
}

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

    tasty_plan
        .validate_method_bodies()
        .map_err(JvmTastyContractError::TastyPlan)?;

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

    let all_jvm_metadata = jvm_unit
        .metadata(tasty.definitions)
        .map_err(JvmTastyContractError::JvmPlan)?;
    let jvm_metadata = all_jvm_metadata
        .iter()
        .into_iter()
        .find(|metadata| metadata.identity().symbol() == tasty.class)
        .cloned()
        .ok_or(JvmTastyContractError::JvmClassMissing { class: tasty.class })?;

    validate_constructor(jvm_unit.store, tasty, class_plan, &jvm_metadata)?;
    validate_fields(jvm_unit.store, tasty, class_plan, &jvm_metadata)?;
    validate_methods(
        jvm_unit.store,
        tasty,
        class_plan,
        &jvm_metadata,
        &all_jvm_metadata,
    )?;

    Ok(ValidatedJvmTastyClass {
        jvm_unit,
        tasty_plan,
        metadata: jvm_metadata,
        tasty_path,
    })
}

/// Validate the complete ordered class batch against one original JVM unit.
/// Every per-class proof is built before the receipt is returned; one failing
/// class rejects the entire batch. The plans must be the exact source-ordered
/// class set in `jvm_unit`, and all original symbols must have one owner.
pub fn validate_jvm_tasty_batch<'proof, 'unit, 'plan, 'session>(
    jvm_unit: &'proof JvmCompilationUnit<'unit>,
    tasty_plans: &'proof [&'proof ValidatedClassStatePlan<'plan, 'session>],
) -> Result<ValidatedJvmTastyBatch<'proof, 'unit, 'plan, 'session>, JvmTastyContractError> {
    if !(2..=32).contains(&tasty_plans.len()) || tasty_plans.len() != jvm_unit.classes.len() {
        return Err(JvmTastyContractError::BatchShape(
            "TASTy plans must cover every class in a multi-class JVM unit",
        ));
    }

    let first_plan = tasty_plans[0].plan();
    if !std::ptr::eq(jvm_unit.store, first_plan.store) {
        return Err(JvmTastyContractError::DifferentSession("semantic store"));
    }
    if !std::ptr::eq(jvm_unit.typed_ast, first_plan.typed_ast) {
        return Err(JvmTastyContractError::DifferentSession("typed AST"));
    }

    let source_id = first_plan.source.id;
    let source_packages = first_plan
        .source_ast
        .iter()
        .filter_map(|(_, tree)| match &tree.kind {
            TreeKind::PackageDef(package) if package.stats.contains(&first_plan.class_tree) => {
                Some(package)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [source_package] = source_packages.as_slice() else {
        return Err(JvmTastyContractError::BatchShape(
            "source class tree must belong to exactly one package declaration",
        ));
    };
    let mut source_classes = Vec::with_capacity(source_package.stats.len());
    for tree in &source_package.stats {
        if !matches!(
            first_plan.source_ast.try_get(*tree).map(|tree| &tree.kind),
            Some(TreeKind::TypeDef(_))
        ) {
            return Err(JvmTastyContractError::BatchShape(
                "source package contains an unsupported top-level sibling",
            ));
        }
        let class = first_plan.source_index.symbol_at(source_id, *tree).ok_or(
            JvmTastyContractError::BatchShape(
                "source class declaration is missing its canonical symbol",
            ),
        )?;
        if !first_plan.store.symbols.contains(class) {
            return Err(JvmTastyContractError::MemberProvenanceMismatch {
                member: class,
                source: source_id,
            });
        }
        let symbol = first_plan.store.symbols.get(class);
        if symbol.kind != SymbolKind::Class || symbol.origin != SymbolOrigin::Source(source_id) {
            return Err(JvmTastyContractError::MemberProvenanceMismatch {
                member: class,
                source: source_id,
            });
        }
        source_classes.push(class);
    }
    let jvm_classes = jvm_unit
        .classes
        .iter()
        .map(|class| class.class)
        .collect::<Vec<_>>();
    let tasty_classes = tasty_plans
        .iter()
        .map(|plan| plan.plan().class)
        .collect::<Vec<_>>();
    if source_classes != jvm_classes || source_classes != tasty_classes {
        return Err(JvmTastyContractError::BatchShape(
            "JVM and TASTy class lists must exactly match source package declarations in order",
        ));
    }

    let mut class_ids = Vec::with_capacity(tasty_plans.len());
    let mut internal_names = Vec::with_capacity(tasty_plans.len());
    let mut artifact_paths = Vec::with_capacity(tasty_plans.len() * 2);
    let mut original_symbols = Vec::new();
    let mut classes = Vec::with_capacity(tasty_plans.len());
    let mut source_owner: Option<SymbolId> = None;
    for (class_index, tasty_plan) in tasty_plans.iter().copied().enumerate() {
        let class = tasty_plan.plan().class;
        let plan = tasty_plan.plan();
        if jvm_unit.classes[class_index].class != class {
            return Err(JvmTastyContractError::BatchShape(
                "TASTy class order differs from the complete JVM unit",
            ));
        }
        if tasty_plan.plan().source.id != source_id {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::DifferentSession("source identity")),
            });
        }
        if !std::ptr::eq(first_plan.source_ast, plan.source_ast)
            || !std::ptr::eq(first_plan.source_index, plan.source_index)
            || !std::ptr::eq(first_plan.typed_ast, plan.typed_ast)
            || !std::ptr::eq(first_plan.store, plan.store)
            || !std::ptr::eq(first_plan.definitions, plan.definitions)
        {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::DifferentSession(
                    "source AST, semantic index, definitions, or typed AST",
                )),
            });
        }
        if class_ids.contains(&class) {
            return Err(JvmTastyContractError::BatchShape(
                "duplicate source class in TASTy batch",
            ));
        }
        let Some(owner) = jvm_unit.store.symbols.get(class).owner else {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::MemberProvenanceMismatch {
                    member: class,
                    source: source_id,
                }),
            });
        };
        if plan.source_index.symbol_at(source_id, plan.class_tree) != Some(class) {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::MemberProvenanceMismatch {
                    member: class,
                    source: source_id,
                }),
            });
        }
        if source_owner.is_some_and(|expected| expected != owner) {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::MemberProvenanceMismatch {
                    member: class,
                    source: source_id,
                }),
            });
        }
        source_owner = Some(owner);

        let proof = validate_jvm_tasty_class(jvm_unit, tasty_plan).map_err(|error| {
            JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(error),
            }
        })?;
        let classfile_path = format!("{}.class", proof.internal_name());
        let tasty_path = proof.tasty_path().to_owned();
        if internal_names
            .iter()
            .any(|name| name == proof.internal_name())
        {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::BatchShape(
                    "duplicate JVM internal name in source batch",
                )),
            });
        }
        for path in [&classfile_path, &tasty_path] {
            if artifact_paths.iter().any(|existing| existing == path) {
                return Err(JvmTastyContractError::BatchClass {
                    class_index,
                    class,
                    error: Box::new(JvmTastyContractError::BatchShape(
                        "duplicate class-file or TASTy artifact path in source batch",
                    )),
                });
            }
            artifact_paths.push(path.clone());
        }

        let mut owned_symbols = vec![class, plan.constructor.symbol];
        owned_symbols.extend(plan.constructor.parameter_clauses.iter().flatten().copied());
        owned_symbols.extend(plan.members.iter().map(|member| match member {
            ClassMemberPicklePlan::Field(field) => field.symbol,
            ClassMemberPicklePlan::Method { symbol, .. } => *symbol,
        }));
        if owned_symbols
            .iter()
            .any(|symbol| original_symbols.contains(symbol))
        {
            return Err(JvmTastyContractError::BatchClass {
                class_index,
                class,
                error: Box::new(JvmTastyContractError::BatchShape(
                    "original source symbol is represented by multiple classes",
                )),
            });
        }
        original_symbols.extend(owned_symbols);
        class_ids.push(class);
        internal_names.push(proof.internal_name().to_owned());
        classes.push(ValidatedJvmTastyBatchClass {
            proof,
            classfile_path,
        });
    }

    Ok(ValidatedJvmTastyBatch { classes, source_id })
}

/// Check a complete owned set of encoded class-file/TASTy pairs against an
/// immutable semantic batch proof. Entries may arrive in any order; paths and
/// members are checked in deterministic source order. The function consumes
/// the bytes and returns no successful subset.
pub fn validate_jvm_tasty_batch_artifacts(
    batch: &ValidatedJvmTastyBatch<'_, '_, '_, '_>,
    mut artifacts: JvmTastyBatchArtifacts,
) -> Result<(), JvmTastyContractError> {
    artifacts
        .classfiles
        .sort_by(|left, right| left.classfile_path.cmp(&right.classfile_path));
    artifacts
        .sidecars
        .sort_by(|left, right| left.tasty_path.cmp(&right.tasty_path));
    for paths in [
        artifacts
            .classfiles
            .iter()
            .map(|artifact| artifact.classfile_path.as_str())
            .collect::<Vec<_>>(),
        artifacts
            .sidecars
            .iter()
            .map(|artifact| artifact.tasty_path.as_str())
            .collect::<Vec<_>>(),
    ] {
        if let Some(pair) = paths.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(JvmTastyContractError::BatchArtifactPaths {
                reason: "duplicate artifact path",
                path: pair[0].to_owned(),
            });
        }
    }
    for classfile in &artifacts.classfiles {
        if artifacts
            .sidecars
            .binary_search_by(|sidecar| sidecar.tasty_path.cmp(&classfile.classfile_path))
            .is_ok()
        {
            return Err(JvmTastyContractError::BatchArtifactPaths {
                reason: "class-file and TASTy sidecar use the same artifact path",
                path: classfile.classfile_path.clone(),
            });
        }
    }

    let mut used_classfiles = vec![false; artifacts.classfiles.len()];
    let mut used_sidecars = vec![false; artifacts.sidecars.len()];
    for (class_index, expected) in batch.classes.iter().enumerate() {
        let class = expected.proof.class();
        let classfile_path = expected.classfile_path();
        let tasty_path = expected.proof.tasty_path();
        let Some((classfile_index, classfile)) = artifacts
            .classfiles
            .iter()
            .enumerate()
            .find(|(_, artifact)| artifact.classfile_path == classfile_path)
        else {
            return Err(JvmTastyContractError::BatchArtifactPaths {
                reason: "missing expected class-file path",
                path: classfile_path.to_owned(),
            });
        };
        used_classfiles[classfile_index] = true;
        let Some((sidecar_index, sidecar)) = artifacts
            .sidecars
            .iter()
            .enumerate()
            .find(|(_, artifact)| artifact.tasty_path == tasty_path)
        else {
            return Err(JvmTastyContractError::BatchArtifactPaths {
                reason: "missing expected TASTy sidecar path",
                path: tasty_path.to_owned(),
            });
        };
        used_sidecars[sidecar_index] = true;

        let artifact_error = |phase, reason: String| JvmTastyContractError::BatchArtifact {
            class_index,
            class,
            phase,
            reason,
        };
        let mut reader = ClassFileReader::new(&classfile.classfile_bytes);
        let class_file = ClassFile::decode(&mut reader)
            .map_err(|error| artifact_error("class-file decode", format!("{error:?}")))?;
        if !reader.is_at_end() {
            return Err(artifact_error(
                "class-file trailing bytes",
                "decoder did not consume the complete class file".into(),
            ));
        }
        class_file
            .validate_references()
            .map_err(|error| artifact_error("class-file references", format!("{error:?}")))?;
        if (class_file.version.major, class_file.version.minor) != (65, 0) {
            return Err(artifact_error(
                "class-file version",
                format!(
                    "expected 65.0, found {}.{}",
                    class_file.version.major, class_file.version.minor
                ),
            ));
        }
        let actual_name = class_file
            .constant_pool
            .class_name(class_file.this_class)
            .map_err(|error| artifact_error("class identity", format!("{error:?}")))?;
        if actual_name != expected.proof.internal_name() {
            return Err(artifact_error(
                "class identity",
                format!(
                    "expected {}, found {actual_name}",
                    expected.proof.internal_name()
                ),
            ));
        }
        let metadata = expected.proof.jvm_metadata();
        if class_file.access_flags != metadata.identity().access_flags() {
            return Err(artifact_error(
                "class ABI",
                "class access flags differ from the checked source ABI".into(),
            ));
        }
        if !class_file.interfaces.is_empty() {
            return Err(artifact_error(
                "class ABI",
                "decoded class has interfaces absent from the checked source ABI".into(),
            ));
        }
        let expected_parent = metadata
            .superclass()
            .ok_or_else(|| {
                artifact_error(
                    "class ABI",
                    "checked source ABI has no direct parent".into(),
                )
            })?
            .parent_internal_name();
        let actual_parent = class_file
            .super_class
            .map(|index| class_file.constant_pool.class_name(index))
            .transpose()
            .map_err(|error| artifact_error("class ABI", format!("{error:?}")))?;
        if actual_parent != Some(expected_parent) {
            return Err(artifact_error(
                "class ABI",
                "superclass differs from the checked source ABI".into(),
            ));
        }
        validate_classfile_tasty_link(&class_file, &sidecar.tasty_bytes)
            .map_err(|error| artifact_error("class-file/TASTy link", format!("{error:?}")))?;
        let tasty = TastyFile::parse_and_validate_scala_3_9(&sidecar.tasty_bytes)
            .map_err(|error| artifact_error("TASTy decode", format!("{error:?}")))?;
        if (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ) != (28, 9, 0)
        {
            return Err(artifact_error(
                "TASTy version",
                format!(
                    "expected 28.9.0, found {}.{}.{}",
                    tasty.header().major_version,
                    tasty.header().minor_version,
                    tasty.header().experimental_version
                ),
            ));
        }
        let expected_tasty = expected
            .proof
            .tasty_plan
            .pickle()
            .map_err(|error| artifact_error("TASTy plan encoding", format!("{error:?}")))?;
        if expected_tasty.bytes() != sidecar.tasty_bytes {
            return Err(artifact_error(
                "TASTy plan identity",
                "decoded sidecar bytes differ from the exact validated class plan output".into(),
            ));
        }
        validate_decoded_class_abi(&class_file, &expected.proof)
            .map_err(|reason| artifact_error("member ABI", reason.into()))?;
        validate_cross_class_call_code(&class_file, &expected.proof)
            .map_err(|reason| artifact_error("cross-class invocation", reason))?;
    }

    if let Some((index, _)) = used_classfiles.iter().enumerate().find(|(_, used)| !**used) {
        return Err(JvmTastyContractError::BatchArtifactPaths {
            reason: "unexpected extra class file",
            path: artifacts.classfiles[index].classfile_path.clone(),
        });
    }
    if let Some((index, _)) = used_sidecars.iter().enumerate().find(|(_, used)| !**used) {
        return Err(JvmTastyContractError::BatchArtifactPaths {
            reason: "unexpected extra TASTy sidecar",
            path: artifacts.sidecars[index].tasty_path.clone(),
        });
    }
    Ok(())
}

fn validate_decoded_class_abi(
    class_file: &ClassFile<'_>,
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
) -> Result<(), &'static str> {
    let metadata = proof.jvm_metadata();
    let mut expected_fields = metadata
        .fields()
        .iter()
        .map(|field| {
            (
                field.name().to_owned(),
                field.descriptor().to_owned(),
                field.access_flags().0,
            )
        })
        .chain(metadata.accessors().iter().map(|field| {
            (
                field.field_name().to_owned(),
                field.field_descriptor().to_owned(),
                field.field_access_flags().0,
            )
        }))
        .collect::<Vec<_>>();
    let mut actual_fields = class_file
        .fields
        .iter()
        .map(|field| {
            Ok((
                class_file
                    .constant_pool
                    .utf8(field.name_index)
                    .map_err(|_| "decoded field name does not resolve")?
                    .to_owned(),
                class_file
                    .constant_pool
                    .utf8(field.descriptor_index)
                    .map_err(|_| "decoded field descriptor does not resolve")?
                    .to_owned(),
                field.access_flags.0,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    expected_fields.sort();
    actual_fields.sort();
    if actual_fields != expected_fields {
        return Err("decoded fields differ from checked source ABI");
    }

    let constructor = metadata
        .constructor()
        .ok_or("checked ABI has no primary constructor")?;
    let mut expected_methods = vec![(
        "<init>".to_owned(),
        constructor.descriptor().as_str().to_owned(),
        constructor.access_flags().0,
    )];
    expected_methods.extend(
        metadata
            .accessors()
            .iter()
            .flat_map(|accessor| {
                std::iter::once((
                    accessor.getter().name().to_owned(),
                    accessor.getter().descriptor().as_str().to_owned(),
                    accessor.getter().access_flags().0,
                ))
                .chain(accessor.setter().map(|setter| {
                    (
                        setter.name().to_owned(),
                        setter.descriptor().as_str().to_owned(),
                        setter.access_flags().0,
                    )
                }))
            })
            .chain(metadata.methods().iter().map(|method| {
                (
                    method.name().to_owned(),
                    method.descriptor().as_str().to_owned(),
                    method.access_flags().0,
                )
            })),
    );
    let mut actual_methods = class_file
        .methods
        .iter()
        .map(|method| {
            Ok((
                class_file
                    .constant_pool
                    .utf8(method.name_index)
                    .map_err(|_| "decoded method name does not resolve")?
                    .to_owned(),
                class_file
                    .constant_pool
                    .utf8(method.descriptor_index)
                    .map_err(|_| "decoded method descriptor does not resolve")?
                    .to_owned(),
                method.access_flags.0,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    expected_methods.sort();
    actual_methods.sort();
    if actual_methods != expected_methods {
        return Err("decoded methods differ from checked source ABI");
    }
    Ok(())
}

struct CrossClassMethodRef {
    caller: SymbolId,
    target: SymbolId,
    owner_internal_name: String,
    name: String,
    descriptor: String,
}

fn validate_cross_class_call_code(
    class_file: &ClassFile<'_>,
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
) -> Result<(), String> {
    // The paired pickler admits the direct same-batch zero-argument call only.
    // Keep the binary check equally narrow instead of accepting a plausible
    // but differently owned or shaped invocation from classfile bytes.
    let tasty = proof.tasty_plan.plan();
    let all_metadata = proof
        .jvm_unit
        .metadata(tasty.definitions)
        .map_err(|error| format!("cannot derive planned classes: {error:?}"))?;
    let mut calls = Vec::new();

    for member in tasty.members {
        let ClassMemberPicklePlan::Method {
            symbol: caller,
            source_tree,
        } = *member
        else {
            continue;
        };
        let TreeKind::DefDef(definition) = &tasty.source_ast.get(source_tree).kind else {
            return Err(format!("method {caller:?} has no source definition"));
        };
        let Some(rhs) = definition.rhs else {
            continue;
        };
        let root = tasty
            .typed_bindings
            .typed_at(tasty.source.id, rhs)
            .ok_or_else(|| format!("method {caller:?} has no canonical typed body"))?;
        let Some(root_tree) = tasty.typed_ast.try_get(root) else {
            return Err(format!("method {caller:?} has a stale typed body"));
        };
        let TreeKind::Apply(application) = &root_tree.kind else {
            if typed_tree_has_sibling_call(
                root,
                tasty.typed_ast,
                tasty.store,
                tasty.class,
                &all_metadata,
                &mut Vec::new(),
            ) {
                return Err(format!(
                    "method {caller:?} contains a cross-class call outside the bounded direct-call shape"
                ));
            }
            continue;
        };
        let Some(function_tree) = tasty.typed_ast.try_get(application.function) else {
            return Err(format!("method {caller:?} has a stale selected function"));
        };
        let TreeKind::Select(_) = &function_tree.kind else {
            if typed_tree_has_sibling_call(
                root,
                tasty.typed_ast,
                tasty.store,
                tasty.class,
                &all_metadata,
                &mut Vec::new(),
            ) {
                return Err(format!(
                    "method {caller:?} contains a cross-class call outside the bounded direct-call shape"
                ));
            }
            continue;
        };
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(target),
            ..
        }) = tasty.store.types.try_get(function_tree.ty)
        else {
            if typed_tree_has_sibling_call(
                root,
                tasty.typed_ast,
                tasty.store,
                tasty.class,
                &all_metadata,
                &mut Vec::new(),
            ) {
                return Err(format!(
                    "method {caller:?} contains a cross-class call outside the bounded direct-call shape"
                ));
            }
            return Err(format!(
                "method {caller:?} selection has no canonical method target"
            ));
        };
        let target_info = tasty.store.symbols.get(*target);
        let Some(owner) = target_info.owner else {
            return Err(format!("selected method {target:?} has no declaring owner"));
        };
        if owner == tasty.class {
            if application.args.iter().any(|argument| {
                typed_tree_has_sibling_call(
                    *argument,
                    tasty.typed_ast,
                    tasty.store,
                    tasty.class,
                    &all_metadata,
                    &mut Vec::new(),
                )
            }) || typed_tree_has_sibling_call(
                application.function,
                tasty.typed_ast,
                tasty.store,
                tasty.class,
                &all_metadata,
                &mut Vec::new(),
            ) {
                return Err(format!(
                    "method {caller:?} contains a cross-class call outside the bounded direct-call shape"
                ));
            }
            continue;
        }
        // Primitive operations and other external calls may appear at the
        // root of a method body. This batch contract only narrows and checks
        // calls whose declaring class is another class in this batch.
        let Some(target_class_metadata) = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == owner)
        else {
            if typed_tree_has_sibling_call(
                root,
                tasty.typed_ast,
                tasty.store,
                tasty.class,
                &all_metadata,
                &mut Vec::new(),
            ) {
                return Err(format!(
                    "method {caller:?} contains a cross-class call outside the bounded direct-call shape"
                ));
            }
            continue;
        };
        if application.kind != dotty_core::ast::ApplyKind::Regular || !application.args.is_empty() {
            return Err(format!(
                "cross-class method {target:?} is not a direct zero-argument call"
            ));
        }
        if target_info.kind != SymbolKind::Method
            || target_info.origin != SymbolOrigin::Source(tasty.source.id)
            || !tasty.store.symbols.contains(owner)
            || tasty.store.symbols.get(owner).kind != SymbolKind::Class
        {
            return Err(format!(
                "cross-class target {target:?} is not a canonical planned source method"
            ));
        }
        let target_metadata = target_class_metadata
            .methods()
            .iter()
            .find(|method| method.symbol() == *target)
            .map(|method| (target_class_metadata, method))
            .ok_or_else(|| format!("cross-class target {target:?} is absent from its JVM plan"))?;
        let expected_descriptor =
            source_method_descriptor(tasty.store, tasty.definitions, *target, &all_metadata)
                .ok_or_else(|| {
                    format!("cross-class target {target:?} has an unsupported signature")
                })?;
        if target_metadata.1.descriptor().as_str() != expected_descriptor {
            return Err(format!(
                "cross-class target {target:?} descriptor differs from its source signature"
            ));
        }
        calls.push(CrossClassMethodRef {
            caller,
            target: *target,
            owner_internal_name: target_metadata.0.identity().internal_name().to_owned(),
            name: target_metadata.1.name().to_owned(),
            descriptor: expected_descriptor,
        });
    }

    if calls.is_empty() {
        return Ok(());
    }
    for call in calls {
        let caller_metadata = proof
            .metadata
            .methods()
            .iter()
            .find(|method| method.symbol() == call.caller)
            .ok_or_else(|| {
                format!(
                    "caller method {:?} is absent from its JVM plan",
                    call.caller
                )
            })?;
        let method = class_file
            .methods
            .iter()
            .find(|method| {
                class_file.constant_pool.utf8(method.name_index).ok()
                    == Some(caller_metadata.name())
                    && class_file.constant_pool.utf8(method.descriptor_index).ok()
                        == Some(caller_metadata.descriptor().as_str())
            })
            .ok_or_else(|| format!("caller method {:?} is absent from class bytes", call.caller))?;
        let [dotty_classfile::attribute::Attribute::Code(code)] = method.attributes.as_slice()
        else {
            return Err(format!(
                "caller method {:?} has no unique Code attribute",
                call.caller
            ));
        };
        if code.code.len() != 5
            || code.code[0] != 0x2b
            || code.code[1] != 0xb6
            || code.code[4] != 0xac
        {
            return Err(format!(
                "caller method {:?} does not contain the bounded aload_1/invokevirtual/ireturn shape",
                call.caller
            ));
        }
        let reference = dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
            code.code[2],
            code.code[3],
        ]));
        let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class_file.constant_pool.get(reference)
        else {
            return Err(format!(
                "caller method {:?} invokevirtual does not reference CONSTANT_Methodref",
                call.caller
            ));
        };
        let actual_owner = class_file
            .constant_pool
            .class_name(*class_index)
            .map_err(|error| format!("invalid invokevirtual owner: {error:?}"))?;
        let actual_method = class_file
            .constant_pool
            .name_and_type(*name_and_type_index)
            .map_err(|error| format!("invalid invokevirtual name/descriptor: {error:?}"))?;
        if actual_owner != call.owner_internal_name
            || actual_method != (call.name.as_str(), call.descriptor.as_str())
        {
            return Err(format!(
                "caller method {:?} targets {:?}, expected {}.{}{}",
                call.caller, call.target, call.owner_internal_name, call.name, call.descriptor
            ));
        }
    }
    Ok(())
}

fn typed_tree_has_sibling_call(
    root: TreeId<Typed>,
    ast: &AstArena<Typed>,
    store: &SemanticStore,
    caller_class: SymbolId,
    planned_classes: &[JvmClassMetadata],
    visited: &mut Vec<TreeId<Typed>>,
) -> bool {
    if visited.contains(&root) {
        return false;
    }
    visited.push(root);
    let Some(tree) = ast.try_get(root) else {
        return false;
    };
    if let TreeKind::Apply(application) = &tree.kind
        && let Some(function) = ast.try_get(application.function)
        && let TreeKind::Select(_) = &function.kind
        && let Some(Type::TermRef {
            target: TermRefTarget::Symbol(target),
            ..
        }) = store.types.try_get(function.ty)
        && let Some(owner) = store.symbols.get(*target).owner
        && owner != caller_class
        && planned_classes
            .iter()
            .any(|metadata| metadata.identity().symbol() == owner)
    {
        return true;
    }
    typed_tree_children(&tree.kind).into_iter().any(|child| {
        typed_tree_has_sibling_call(child, ast, store, caller_class, planned_classes, visited)
    })
}

fn typed_tree_children(kind: &TreeKind<Typed>) -> Vec<TreeId<Typed>> {
    match kind {
        TreeKind::Select(selection) => vec![selection.qualifier],
        TreeKind::Super(super_tree) => vec![super_tree.qual],
        TreeKind::Apply(application) => std::iter::once(application.function)
            .chain(application.args.iter().copied())
            .collect(),
        TreeKind::TypeApply(application) => std::iter::once(application.function)
            .chain(application.args.iter().copied())
            .collect(),
        TreeKind::New(new_tree) => vec![new_tree.tpt],
        TreeKind::Typed(expression) => vec![expression.expr, expression.tpt],
        TreeKind::NamedArg(argument) => vec![argument.arg],
        TreeKind::Assign(assignment) => vec![assignment.lhs, assignment.rhs],
        TreeKind::Block(block) => block
            .stats
            .iter()
            .copied()
            .chain(std::iter::once(block.expr))
            .collect(),
        TreeKind::If(expression) => vec![
            expression.cond,
            expression.then_branch,
            expression.else_branch,
        ],
        TreeKind::Match(expression) => std::iter::once(expression.selector)
            .chain(expression.cases.iter().copied())
            .collect(),
        TreeKind::CaseDef(case) => std::iter::once(case.pattern)
            .chain(case.guard)
            .chain(std::iter::once(case.body))
            .collect(),
        TreeKind::Return(expression) => expression.expr.into_iter().collect(),
        TreeKind::While(expression) => vec![expression.cond, expression.body],
        TreeKind::Try(expression) => std::iter::once(expression.expr)
            .chain(expression.cases.iter().copied())
            .chain(expression.finalizer)
            .collect(),
        TreeKind::Closure(closure) => closure
            .env
            .iter()
            .copied()
            .chain(std::iter::once(closure.method))
            .chain(closure.tpt)
            .collect(),
        TreeKind::ValDef(definition) => definition.rhs.into_iter().collect(),
        TreeKind::DefDef(definition) => definition.rhs.into_iter().collect(),
        TreeKind::TypeDef(definition) => vec![definition.rhs],
        TreeKind::Template(template) => std::iter::once(template.constructor)
            .chain(template.parents.iter().copied())
            .chain(template.self_val)
            .chain(template.body.iter().copied())
            .collect(),
        TreeKind::PackageDef(package) => std::iter::once(package.name)
            .chain(package.stats.iter().copied())
            .collect(),
        TreeKind::Import(import) => std::iter::once(import.expr)
            .chain(
                import
                    .selectors
                    .iter()
                    .flat_map(|selector| selector.renamed),
            )
            .chain(import.selectors.iter().flat_map(|selector| selector.bound))
            .collect(),
        TreeKind::Export(export) => std::iter::once(export.expr)
            .chain(
                export
                    .selectors
                    .iter()
                    .flat_map(|selector| selector.renamed),
            )
            .chain(export.selectors.iter().flat_map(|selector| selector.bound))
            .collect(),
        TreeKind::SingletonTypeTree(singleton) => vec![singleton.reference],
        TreeKind::AppliedTypeTree(applied) => std::iter::once(applied.tpt)
            .chain(applied.args.iter().copied())
            .collect(),
        TreeKind::RefinedTypeTree(refined) => std::iter::once(refined.tpt)
            .chain(refined.refinements.iter().copied())
            .collect(),
        TreeKind::LambdaTypeTree(lambda) => lambda
            .type_params
            .iter()
            .copied()
            .chain(std::iter::once(lambda.body))
            .collect(),
        TreeKind::MatchTypeTree(matched) => matched
            .bound
            .into_iter()
            .chain(std::iter::once(matched.selector))
            .chain(matched.cases.iter().copied())
            .collect(),
        TreeKind::ByNameTypeTree(by_name) => vec![by_name.result],
        TreeKind::TypeBoundsTree(bounds) => bounds
            .low
            .into_iter()
            .chain(bounds.high)
            .chain(bounds.alias)
            .collect(),
        TreeKind::Bind(binding) => vec![binding.body],
        TreeKind::Alternative(alternative) => alternative.alternatives.clone(),
        TreeKind::UnApply(unapply) => std::iter::once(unapply.function)
            .chain(unapply.implicits.iter().copied())
            .chain(unapply.patterns.iter().copied())
            .collect(),
        TreeKind::Annotated(annotated) => vec![annotated.expr, annotated.annotation],
        TreeKind::Quote(quote) => std::iter::once(quote.body)
            .chain(quote.tags.iter().copied())
            .collect(),
        TreeKind::Splice(splice) => vec![splice.expr],
        TreeKind::QuotePattern(pattern) => pattern
            .bindings
            .iter()
            .copied()
            .chain(std::iter::once(pattern.body))
            .chain(std::iter::once(pattern.quotes))
            .collect(),
        TreeKind::SplicePattern(pattern) => std::iter::once(pattern.body)
            .chain(pattern.type_args.iter().copied())
            .chain(pattern.args.iter().copied())
            .collect(),
        TreeKind::Inlined(inlined) => inlined
            .bindings
            .iter()
            .copied()
            .chain(std::iter::once(inlined.expansion))
            .collect(),
        TreeKind::Ident(_)
        | TreeKind::This(_)
        | TreeKind::Literal(_)
        | TreeKind::TypeTree(_)
        | TreeKind::PhaseSpecific(_) => Vec::new(),
    }
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
    all_metadata: &[JvmClassMetadata],
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
        let expected_descriptor =
            source_method_descriptor(store, tasty.definitions, symbol, all_metadata)
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
    planned_classes: &[JvmClassMetadata],
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
                    let descriptor = primitive_descriptor(definitions, parameter.ty)
                        .map(str::to_owned)
                        .or_else(|| {
                            planned_class_descriptor(
                                store,
                                definitions,
                                parameter.ty,
                                planned_classes,
                            )
                        })?;
                    parameters.push(descriptor);
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

fn planned_class_descriptor(
    store: &SemanticStore,
    definitions: &Definitions,
    ty: dotty_core::TypeId,
    planned_classes: &[JvmClassMetadata],
) -> Option<String> {
    let Some(Type::TypeRef {
        prefix,
        target: dotty_core::TypeRefTarget::Symbol(class),
    }) = store.types.try_get(ty)
    else {
        return None;
    };
    let Some(Type::TypeRef {
        prefix: package_prefix,
        target: dotty_core::TypeRefTarget::Symbol(package),
    }) = store.types.try_get(*prefix)
    else {
        return None;
    };
    if *package_prefix != definitions.no_prefix
        || !store.symbols.contains(*class)
        || !store.symbols.contains(*package)
    {
        return None;
    }
    let class_symbol = store.symbols.get(*class);
    if class_symbol.kind != SymbolKind::Class
        || class_symbol.owner != Some(*package)
        || class_symbol.origin == SymbolOrigin::Builtin
    {
        return None;
    }
    let metadata = planned_classes
        .iter()
        .find(|metadata| metadata.identity().symbol() == *class)?;
    Some(format!("L{};", metadata.identity().internal_name()))
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
