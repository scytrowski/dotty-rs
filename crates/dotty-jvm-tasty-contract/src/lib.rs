//! Read-only consistency proofs for the audited JVM / Scala 3 TASTy overlap.
//!
//! This crate is the acyclic coordination layer between `dotty-jvm` and
//! `dotty-tasty-pickler`. It accepts validated private primitive class-state
//! plans and metadata-only private same-batch reference-field proofs from a
//! JVM unit in the same canonical session. A proof
//! checks semantic identities, source-to-typed roots, names, and the bounded
//! erased ABI; it does not encode, publish, or claim Scala-consumable artifacts.

use std::error::Error;
use std::fmt;

use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC, ACC_STATIC};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::PoolRefError;
use dotty_classfile::reader::Reader as ClassFileReader;
use dotty_classfile::tasty_link::{TastyLinkError, TastyLinkId};
use dotty_core::ast::{AstArena, AstPhase, TreeKind, Untyped};
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_core::types::{MethodKind, MethodType, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{Definitions, SemanticStore, SourceDefinition, SourceId, SymbolId, TreeId, Typed};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassMetadata, JvmCodegenError, JvmCompilationUnit,
    JvmFieldSourceVisibility,
};
use dotty_tasty::tasty::{TastyFile, TastyFileError};
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, FormalLocalConstructorCallArgumentOrigin, LocalReceiverUnitCallBinding,
    OrderedConstructorCallArgumentOrigin, PickleError, ValidatedClassStatePlan,
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
/// This intentionally supports the source-private primitive-field class-state
/// subset and read-only metadata proofs for private immutable same-batch
/// reference fields. Unsupported source constructs fail closed. The proof
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

    validate_constructor(
        jvm_unit.store,
        tasty,
        class_plan,
        &jvm_metadata,
        all_jvm_metadata.as_slice(),
    )?;
    validate_fields(
        jvm_unit.store,
        tasty_plan,
        class_plan,
        &jvm_metadata,
        all_jvm_metadata.as_slice(),
    )?;
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
        validate_stack_map_tables_against_canonical_emission(
            &class_file,
            expected,
            TastyLinkId::from_bytes(tasty.header().uuid),
        )
        .map_err(|reason| artifact_error("StackMapTable", reason))?;
        validate_decoded_class_abi(&class_file, &expected.proof)
            .map_err(|reason| artifact_error("member ABI", reason.into()))?;
        validate_reference_field_constructor_code(&class_file, &expected.proof)
            .map_err(|reason| artifact_error("reference-field initializer", reason))?;
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

type MethodStackMapPayloads = Vec<(String, String, Vec<Vec<u8>>)>;

fn validate_stack_map_tables_against_canonical_emission(
    actual: &ClassFile<'_>,
    proof: &ValidatedJvmTastyBatchClass<'_, '_, '_, '_>,
    tasty_link: TastyLinkId,
) -> Result<(), String> {
    let (jvm_unit, tasty_plan) = proof.proof.plans();
    let canonical = dotty_jvm::emit_class_with_tasty_link(
        jvm_unit,
        proof.proof.class(),
        tasty_plan.plan().definitions,
        tasty_link,
    )
    .map_err(|error| format!("canonical JVM emission failed: {error:?}"))?;
    let mut reader = ClassFileReader::new(canonical.bytes());
    let canonical = ClassFile::decode(&mut reader)
        .map_err(|error| format!("canonical class-file decode failed: {error:?}"))?;
    if !reader.is_at_end() {
        return Err("canonical class-file decoder left trailing bytes".into());
    }

    fn stack_map_payloads(class_file: &ClassFile<'_>) -> Result<MethodStackMapPayloads, String> {
        let mut methods = Vec::with_capacity(class_file.methods.len());
        for method in &class_file.methods {
            let name = class_file
                .constant_pool
                .utf8(method.name_index)
                .map_err(|error| format!("invalid method name: {error:?}"))?
                .to_owned();
            let descriptor = class_file
                .constant_pool
                .utf8(method.descriptor_index)
                .map_err(|error| format!("invalid method descriptor: {error:?}"))?
                .to_owned();
            let mut payloads = Vec::new();
            for attribute in &method.attributes {
                let Attribute::Code(code) = attribute else {
                    continue;
                };
                for nested in &code.attributes {
                    let Attribute::Other(raw) = nested else {
                        continue;
                    };
                    if class_file.constant_pool.utf8(raw.name_index).ok() == Some("StackMapTable") {
                        payloads.push(raw.bytes.to_vec());
                    }
                }
            }
            methods.push((name, descriptor, payloads));
        }
        methods.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
        Ok(methods)
    }

    let actual_frames = stack_map_payloads(actual)?;
    let canonical_frames = stack_map_payloads(&canonical)?;
    if actual_frames != canonical_frames {
        return Err(
            "decoded StackMapTable offsets, locals, or stacks differ from frames generated by the canonical CFG analyzer".into(),
        );
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

fn validate_reference_field_constructor_code(
    class_file: &ClassFile<'_>,
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
) -> Result<(), String> {
    let reference_fields = proof
        .tasty_plan
        .same_batch_reference_fields()
        .collect::<Vec<_>>();
    if reference_fields.is_empty() {
        return Ok(());
    }
    if reference_fields.len() == 2 {
        let aliases = reference_fields
            .iter()
            .map(|field| {
                (
                    field.field(),
                    field.target_class(),
                    field.template_parameter_alias(),
                )
            })
            .collect::<Vec<_>>();
        let [
            (first_field, first_target, Some(first_parameter)),
            (second_field, second_target, Some(second_parameter)),
        ] = aliases.as_slice()
        else {
            return Err(
                "two same-batch reference fields require distinct constructor aliases".into(),
            );
        };
        if first_target != second_target
            || first_field == second_field
            || first_parameter == second_parameter
        {
            return Err(
                "dual-reference aliases must target one class through distinct fields and formals"
                    .into(),
            );
        }
        let all_metadata = proof
            .jvm_unit
            .metadata(proof.tasty_plan.plan().definitions)
            .map_err(|error| format!("cannot derive planned classes: {error:?}"))?;
        let target = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == *first_target)
            .ok_or_else(|| {
                "reference initializer target is absent from the JVM batch".to_owned()
            })?;
        let fields = [*first_field, *second_field];
        let template_parameters = [*first_parameter, *second_parameter];
        return validate_dual_reference_field_alias_constructor_code(
            class_file,
            proof,
            target,
            fields,
            template_parameters,
        );
    }
    if reference_fields.len() != 1 {
        return Err("more than two same-batch reference field initializers are unsupported".into());
    }
    let reference_field = reference_fields[0];

    let field = proof
        .metadata
        .fields()
        .iter()
        .find(|field| field.symbol() == reference_field.field())
        .ok_or_else(|| {
            "reference initializer field is absent from checked JVM metadata".to_owned()
        })?;
    let all_metadata = proof
        .jvm_unit
        .metadata(proof.tasty_plan.plan().definitions)
        .map_err(|error| format!("cannot derive planned classes: {error:?}"))?;
    let target = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == reference_field.target_class())
        .ok_or_else(|| "reference initializer target is absent from the JVM batch".to_owned())?;
    if let Some(template_parameter) = reference_field.template_parameter_alias() {
        return validate_reference_field_alias_constructor_code(
            class_file,
            proof,
            field,
            target,
            template_parameter,
        );
    }
    let target_constructor = target
        .constructor()
        .filter(|constructor| constructor.descriptor().as_str() == "(I)V")
        .ok_or_else(|| "reference initializer target has no checked (I)V constructor".to_owned())?;
    let owner_constructor = proof
        .metadata
        .constructor()
        .filter(|constructor| constructor.descriptor().as_str() == "(I)V")
        .ok_or_else(|| "reference initializer owner has no checked (I)V constructor".to_owned())?;
    let constructor = class_file
        .methods
        .iter()
        .find(|method| {
            class_file.constant_pool.utf8(method.name_index).ok() == Some("<init>")
                && class_file.constant_pool.utf8(method.descriptor_index).ok()
                    == Some(owner_constructor.descriptor().as_str())
        })
        .ok_or_else(|| "reference initializer constructor is absent from class bytes".to_owned())?;
    let [dotty_classfile::attribute::Attribute::Code(code)] = constructor.attributes.as_slice()
    else {
        return Err("reference initializer constructor has no unique Code attribute".into());
    };

    // This batch proof currently admits one direct `new Target(seed)` field
    // initializer in a `(I)V` owner constructor. Check the entire method shape
    // so a valid ABI cannot conceal a changed argument or extra initialization.
    if code.code.len() != 17
        || code.code[0] != 0x2a
        || code.code[1] != 0xb7
        || code.code[4] != 0x2a
        || code.code[5] != 0xbb
        || code.code[8] != 0x59
        || code.code[9] != 0x1b
        || code.code[10] != 0xb7
        || code.code[13] != 0xb5
        || code.code[16] != 0xb1
    {
        return Err(format!(
            "constructor does not contain the bounded super/new/dup/iload_1/constructor/putfield/return shape: {:02x?}",
            code.code
        ));
    }

    let cp_index = |offset: usize| {
        dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]))
    };
    let method_ref = |index, expected_owner: &str, expected_method: (&str, &str)| {
        let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class_file.constant_pool.get(index)
        else {
            return Err("constructor invocation does not reference CONSTANT_Methodref".to_owned());
        };
        let owner = class_file
            .constant_pool
            .class_name(*class_index)
            .map_err(|error| format!("invalid constructor invocation owner: {error:?}"))?;
        let method = class_file
            .constant_pool
            .name_and_type(*name_and_type_index)
            .map_err(|error| {
                format!("invalid constructor invocation name/descriptor: {error:?}")
            })?;
        if owner != expected_owner || method != expected_method {
            return Err(format!(
                "constructor invocation targets {owner}.{}/{}, expected {}.{}{}",
                method.0, method.1, expected_owner, expected_method.0, expected_method.1
            ));
        }
        Ok(())
    };

    let expected_parent = proof
        .metadata
        .superclass()
        .ok_or_else(|| "checked owner ABI has no direct parent".to_owned())?
        .parent_internal_name();
    method_ref(cp_index(2), expected_parent, ("<init>", "()V"))?;

    let class_index = cp_index(6);
    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
        class_file.constant_pool.get(class_index)
    else {
        return Err("constructor new does not reference CONSTANT_Class".into());
    };
    let allocated_class = class_file
        .constant_pool
        .utf8(*name_index)
        .map_err(|error| format!("invalid constructed class name: {error:?}"))?;
    if allocated_class != target.identity().internal_name() {
        return Err(format!(
            "constructor allocates {allocated_class}, expected {}",
            target.identity().internal_name()
        ));
    }
    method_ref(
        cp_index(11),
        target.identity().internal_name(),
        ("<init>", target_constructor.descriptor().as_str()),
    )?;

    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    }) = class_file.constant_pool.get(cp_index(14))
    else {
        return Err("constructor putfield does not reference CONSTANT_Fieldref".into());
    };
    let owner = class_file
        .constant_pool
        .class_name(*class_index)
        .map_err(|error| format!("invalid initialized field owner: {error:?}"))?;
    let initialized_field = class_file
        .constant_pool
        .name_and_type(*name_and_type_index)
        .map_err(|error| format!("invalid initialized field name/descriptor: {error:?}"))?;
    if owner != proof.internal_name() || initialized_field != (field.name(), field.descriptor()) {
        return Err(format!(
            "constructor initializes {owner}.{}:{}, expected {}.{}:{}",
            initialized_field.0,
            initialized_field.1,
            proof.internal_name(),
            field.name(),
            field.descriptor()
        ));
    }
    Ok(())
}

fn validate_dual_reference_field_alias_constructor_code(
    class_file: &ClassFile<'_>,
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    target: &JvmClassMetadata,
    fields: [SymbolId; 2],
    template_parameters: [SymbolId; 2],
) -> Result<(), String> {
    let target_descriptor = format!("L{};", target.identity().internal_name());
    let expected_descriptor = format!("({target_descriptor}{target_descriptor})V");
    let constructor_metadata = proof
        .metadata
        .constructor()
        .filter(|constructor| constructor.descriptor().as_str() == expected_descriptor)
        .ok_or_else(|| {
            format!("dual-reference alias owner has no checked {expected_descriptor} constructor")
        })?;
    if constructor_metadata.parameters().len() != 2
        || !constructor_metadata.access_flags().is_public()
    {
        return Err(
            "dual-reference alias constructor must be public with two ordered formals".into(),
        );
    }
    let checked_fields = fields
        .iter()
        .map(|symbol| {
            proof
                .metadata
                .fields()
                .iter()
                .find(|field| field.symbol() == *symbol)
                .ok_or_else(|| {
                    format!(
                        "reference initializer field {symbol:?} is absent from checked JVM metadata"
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if checked_fields
        .iter()
        .any(|field| field.descriptor() != target_descriptor)
    {
        return Err(format!(
            "dual-reference alias field descriptor differs from {target_descriptor}"
        ));
    }

    let tasty = proof.tasty_plan.plan();
    let [constructor_clause] = tasty.constructor.parameter_clauses.as_slice() else {
        return Err("dual-reference TASTy plan must have one constructor clause".into());
    };
    let [first_derived, second_derived] = constructor_clause.as_slice() else {
        return Err("dual-reference TASTy plan must have two ordered constructor formals".into());
    };
    let TreeKind::DefDef(constructor_definition) =
        &tasty.source_ast.get(tasty.constructor.source_tree).kind
    else {
        return Err("dual-reference source constructor definition is absent".into());
    };
    let [source_clause] = constructor_definition.value_param_clauses.as_slice() else {
        return Err("dual-reference source constructor must have one clause".into());
    };
    let [first_source_tree, second_source_tree] = source_clause.as_slice() else {
        return Err("dual-reference source constructor must have two ordered formals".into());
    };
    for (index, (source_tree, derived, expected_template)) in [
        (first_source_tree, first_derived, template_parameters[0]),
        (second_source_tree, second_derived, template_parameters[1]),
    ]
    .into_iter()
    .enumerate()
    {
        let source_parameter = tasty
            .source_index
            .symbol_at(tasty.source.id, *source_tree)
            .ok_or_else(|| "dual-reference template formal has no canonical identity".to_owned())?;
        let template_record = tasty.store.symbols.get(source_parameter);
        let derived_record = tasty.store.symbols.get(*derived);
        if source_parameter != expected_template
            || tasty.source_index.derived_symbol_at(
                tasty.constructor.symbol,
                tasty.source.id,
                *source_tree,
            ) != Some(*derived)
            || constructor_metadata.parameters()[index].symbol() != *derived
            || template_record.kind != SymbolKind::Parameter
            || template_record.owner != Some(tasty.class)
            || template_record.origin != SymbolOrigin::Source(tasty.source.id)
            || derived_record.kind != SymbolKind::Parameter
            || derived_record.owner != Some(tasty.constructor.symbol)
            || derived_record.origin != SymbolOrigin::Source(tasty.source.id)
            || tasty.source_index.definition_of(source_parameter)
                != Some(SourceDefinition::Canonical {
                    source: tasty.source.id,
                    tree: *source_tree,
                })
        {
            return Err(format!(
                "dual-reference formal at JVM slot {} does not preserve its exact template/derived source identities",
                index + 1
            ));
        }
    }

    let constructor = class_file
        .methods
        .iter()
        .find(|method| {
            class_file.constant_pool.utf8(method.name_index).ok() == Some("<init>")
                && class_file.constant_pool.utf8(method.descriptor_index).ok()
                    == Some(expected_descriptor.as_str())
        })
        .ok_or_else(|| "dual-reference alias constructor is absent from class bytes".to_owned())?;
    let [dotty_classfile::attribute::Attribute::Code(code)] = constructor.attributes.as_slice()
    else {
        return Err("dual-reference alias constructor has no unique Code attribute".into());
    };
    if code.code.len() != 15
        || code.code[0] != 0x2a
        || code.code[1] != 0xb7
        || code.code[4] != 0x2a
        || code.code[5] != 0x2b
        || code.code[6] != 0xb5
        || code.code[9] != 0x2a
        || code.code[10] != 0x2c
        || code.code[11] != 0xb5
        || code.code[14] != 0xb1
        || code.max_locals != 3
        || code.max_stack != 2
        || !code.exception_table.is_empty()
    {
        return Err(format!(
            "constructor does not contain the bounded super/aload_0/aload_1/putfield/aload_0/aload_2/putfield/return shape: {:02x?}",
            code.code
        ));
    }
    let cp_index = |offset: usize| {
        dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]))
    };
    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    }) = class_file.constant_pool.get(cp_index(2))
    else {
        return Err("dual-reference alias super invocation is not CONSTANT_Methodref".into());
    };
    let parent = proof
        .metadata
        .superclass()
        .ok_or_else(|| "checked owner ABI has no direct parent".to_owned())?
        .parent_internal_name();
    if class_file
        .constant_pool
        .class_name(*class_index)
        .map_err(|error| format!("invalid dual-reference superclass reference: {error:?}"))?
        != parent
        || class_file
            .constant_pool
            .name_and_type(*name_and_type_index)
            .map_err(|error| format!("invalid dual-reference superclass constructor: {error:?}"))?
            != ("<init>", "()V")
    {
        return Err(
            "dual-reference constructor does not call its checked direct superclass first".into(),
        );
    }

    for (index, field) in checked_fields.into_iter().enumerate() {
        let offset = if index == 0 { 7 } else { 12 };
        let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Fieldref {
            class_index,
            name_and_type_index,
        }) = class_file.constant_pool.get(cp_index(offset))
        else {
            return Err(format!(
                "constructor putfield for slot {} is not CONSTANT_Fieldref",
                index + 1
            ));
        };
        let owner = class_file
            .constant_pool
            .class_name(*class_index)
            .map_err(|error| format!("invalid slot {} field owner: {error:?}", index + 1))?;
        let initialized_field = class_file
            .constant_pool
            .name_and_type(*name_and_type_index)
            .map_err(|error| format!("invalid slot {} field reference: {error:?}", index + 1))?;
        if owner != proof.internal_name() || initialized_field != (field.name(), field.descriptor())
        {
            return Err(format!(
                "constructor slot {} initializes {owner}.{}:{}, expected {}.{}:{}",
                index + 1,
                initialized_field.0,
                initialized_field.1,
                proof.internal_name(),
                field.name(),
                field.descriptor()
            ));
        }
    }
    Ok(())
}

fn validate_reference_field_alias_constructor_code(
    class_file: &ClassFile<'_>,
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    field: &dotty_jvm::JvmFieldMetadata,
    target: &JvmClassMetadata,
    template_parameter: SymbolId,
) -> Result<(), String> {
    let target_descriptor = format!("L{};", target.identity().internal_name());
    if field.descriptor() != target_descriptor {
        return Err(format!(
            "reference alias field descriptor is {}, expected {target_descriptor}",
            field.descriptor()
        ));
    }
    let expected_descriptor = format!("({target_descriptor})V");
    let constructor_metadata = proof
        .metadata
        .constructor()
        .filter(|constructor| constructor.descriptor().as_str() == expected_descriptor)
        .ok_or_else(|| {
            format!("reference alias owner has no checked {expected_descriptor} constructor")
        })?;
    if constructor_metadata.parameters().len() != 1
        || !constructor_metadata.access_flags().is_public()
    {
        return Err("reference alias owner constructor must be public with one formal".into());
    }

    let tasty = proof.tasty_plan.plan();
    let [constructor_clause] = tasty.constructor.parameter_clauses.as_slice() else {
        return Err("reference alias TASTy plan must have one constructor clause".into());
    };
    let [derived_parameter] = constructor_clause.as_slice() else {
        return Err("reference alias TASTy plan must have one constructor formal".into());
    };
    let TreeKind::DefDef(constructor_definition) =
        &tasty.source_ast.get(tasty.constructor.source_tree).kind
    else {
        return Err("reference alias source constructor definition is absent".into());
    };
    let [source_clause] = constructor_definition.value_param_clauses.as_slice() else {
        return Err("reference alias source constructor must have one clause".into());
    };
    let [source_parameter_tree] = source_clause.as_slice() else {
        return Err("reference alias source constructor must have one formal".into());
    };
    let source_parameter = tasty
        .source_index
        .symbol_at(tasty.source.id, *source_parameter_tree)
        .ok_or_else(|| "reference alias template formal has no canonical identity".to_owned())?;
    let template_record = tasty.store.symbols.get(source_parameter);
    let derived_record = tasty.store.symbols.get(*derived_parameter);
    if source_parameter != template_parameter
        || tasty.source_index.derived_symbol_at(
            tasty.constructor.symbol,
            tasty.source.id,
            *source_parameter_tree,
        ) != Some(*derived_parameter)
        || constructor_metadata.parameters()[0].symbol() != *derived_parameter
        || template_record.kind != SymbolKind::Parameter
        || template_record.owner != Some(tasty.class)
        || template_record.origin != SymbolOrigin::Source(tasty.source.id)
        || derived_record.kind != SymbolKind::Parameter
        || derived_record.owner != Some(tasty.constructor.symbol)
        || derived_record.origin != SymbolOrigin::Source(tasty.source.id)
        || tasty.source_index.definition_of(source_parameter)
            != Some(SourceDefinition::Canonical {
                source: tasty.source.id,
                tree: *source_parameter_tree,
            })
    {
        return Err(
            "reference alias constructor formal does not preserve the exact template/derived source identities"
                .into(),
        );
    }

    let constructor = class_file
        .methods
        .iter()
        .find(|method| {
            class_file.constant_pool.utf8(method.name_index).ok() == Some("<init>")
                && class_file.constant_pool.utf8(method.descriptor_index).ok()
                    == Some(expected_descriptor.as_str())
        })
        .ok_or_else(|| "reference alias constructor is absent from class bytes".to_owned())?;
    let [dotty_classfile::attribute::Attribute::Code(code)] = constructor.attributes.as_slice()
    else {
        return Err("reference alias constructor has no unique Code attribute".into());
    };
    // One reference formal occupies local 1 after `this`. This exact sequence
    // proves the already-initialized incoming object is copied into the field
    // once, after the superclass constructor, without allocating a target.
    if code.code.len() != 10
        || code.code[0] != 0x2a
        || code.code[1] != 0xb7
        || code.code[4] != 0x2a
        || code.code[5] != 0x2b
        || code.code[6] != 0xb5
        || code.code[9] != 0xb1
        || code.max_locals != 2
        || code.max_stack != 2
    {
        return Err(format!(
            "constructor does not contain the bounded super/aload_0/aload_1/putfield/return alias shape: {:02x?}",
            code.code
        ));
    }
    let cp_index = |offset: usize| {
        dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]))
    };
    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    }) = class_file.constant_pool.get(cp_index(2))
    else {
        return Err("reference alias super invocation is not CONSTANT_Methodref".into());
    };
    let parent = proof
        .metadata
        .superclass()
        .ok_or_else(|| "checked owner ABI has no direct parent".to_owned())?
        .parent_internal_name();
    if class_file
        .constant_pool
        .class_name(*class_index)
        .map_err(|error| format!("invalid alias superclass reference: {error:?}"))?
        != parent
        || class_file
            .constant_pool
            .name_and_type(*name_and_type_index)
            .map_err(|error| format!("invalid alias super constructor reference: {error:?}"))?
            != ("<init>", "()V")
    {
        return Err(
            "reference alias constructor does not call its checked direct superclass".into(),
        );
    }
    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    }) = class_file.constant_pool.get(cp_index(7))
    else {
        return Err("reference alias putfield is not CONSTANT_Fieldref".into());
    };
    let owner = class_file
        .constant_pool
        .class_name(*class_index)
        .map_err(|error| format!("invalid reference alias field owner: {error:?}"))?;
    let initialized_field = class_file
        .constant_pool
        .name_and_type(*name_and_type_index)
        .map_err(|error| format!("invalid reference alias field name/descriptor: {error:?}"))?;
    if owner != proof.internal_name() || initialized_field != (field.name(), field.descriptor()) {
        return Err(format!(
            "constructor initializes {owner}.{}:{}, expected {}.{}:{}",
            initialized_field.0,
            initialized_field.1,
            proof.internal_name(),
            field.name(),
            field.descriptor()
        ));
    }
    Ok(())
}

fn validate_direct_formal_constructor_pair(
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    caller: SymbolId,
    (source_rhs, typed_rhs): (TreeId<Untyped>, TreeId<Typed>),
    outer_application: &dotty_core::ast::Apply<Typed>,
    outer_constructor_symbol: SymbolId,
    outer_metadata: &JvmClassMetadata,
) -> Result<[u16; 2], String> {
    let tasty = proof.tasty_plan.plan();
    let binding = tasty
        .typed_bindings
        .direct_formal_constructor_call_pair(tasty.source.id, caller)
        .ok_or_else(|| {
            "two-argument constructor has no canonical ordered-formal receipt".to_owned()
        })?;
    let source_outer_tree = tasty
        .source_ast
        .try_get(source_rhs)
        .ok_or_else(|| "direct-formal source RHS is stale".to_owned())?;
    let TreeKind::Apply(source_outer) = &source_outer_tree.kind else {
        return Err("direct-formal source RHS is not an Apply".into());
    };
    let typed_outer_tree = tasty
        .typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| "direct-formal typed RHS is stale".to_owned())?;
    let TreeKind::Apply(typed_outer) = &typed_outer_tree.kind else {
        return Err("direct-formal typed RHS is not an Apply".into());
    };
    if source_outer.kind != dotty_core::ast::ApplyKind::Regular
        || typed_outer.kind != dotty_core::ast::ApplyKind::Regular
        || outer_application.kind != dotty_core::ast::ApplyKind::Regular
        || source_outer.args.as_slice() != binding.source_arguments
        || typed_outer.args.as_slice() != binding.typed_arguments
        || outer_application.args.as_slice() != binding.typed_arguments
        || tasty.typed_bindings.typed_at(tasty.source.id, source_rhs) != Some(typed_rhs)
        || !matches!(
            tasty
                .source_ast
                .try_get(source_outer.function)
                .map(|tree| &tree.kind),
            Some(TreeKind::Select(_))
        )
        || binding.method_formals[0] == binding.method_formals[1]
        || binding.argument_formals != binding.method_formals
        || binding.source_method_formals[0] == binding.source_method_formals[1]
    {
        return Err(
            "direct-formal constructor roots or receipt differ from the ordered source call".into(),
        );
    }

    let caller_method = proof
        .metadata
        .methods()
        .iter()
        .find(|method| method.symbol() == caller)
        .ok_or_else(|| "direct-formal caller is absent from its JVM plan".to_owned())?;
    let outer_constructor = outer_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == outer_constructor_symbol)
        .ok_or_else(|| "direct-formal outer constructor is absent from its JVM plan".to_owned())?;
    if !outer_constructor.access_flags().is_public()
        || caller_method.parameters().len() != 2
        || outer_constructor.parameters().len() != 2
        || caller_method
            .parameters()
            .iter()
            .zip(binding.method_formals)
            .any(|(parameter, formal)| parameter.symbol() != formal || parameter.width() != 1)
        || caller_method.parameters()[0].slot() != 1
        || caller_method.parameters()[1].slot() != 2
        || outer_constructor
            .parameters()
            .iter()
            .zip(caller_method.parameters())
            .any(|(target, source)| target.ty() != source.ty())
    {
        return Err(
            "direct-formal caller and target do not have the same ordered reference ABI".into(),
        );
    }
    let source_method_tree = tasty
        .members
        .iter()
        .find_map(|member| match member {
            ClassMemberPicklePlan::Method {
                symbol,
                source_tree,
            } if *symbol == caller => Some(*source_tree),
            _ => None,
        })
        .ok_or_else(|| "direct-formal caller is absent from its TASTy plan".to_owned())?;
    let TreeKind::DefDef(source_method) = &tasty
        .source_ast
        .try_get(source_method_tree)
        .ok_or_else(|| "direct-formal caller source definition is stale".to_owned())?
        .kind
    else {
        return Err("direct-formal caller source definition is not a DefDef".into());
    };
    if source_method.value_param_clauses.len() != 1
        || source_method.value_param_clauses[0].as_slice() != binding.source_method_formals
    {
        return Err(
            "direct-formal receipt differs from the ordered source parameter clause".into(),
        );
    }
    for index in 0..2 {
        let source_argument = tasty
            .source_ast
            .try_get(binding.source_arguments[index])
            .ok_or_else(|| format!("direct source argument {index} is stale"))?;
        let TreeKind::Ident(source_ident) = &source_argument.kind else {
            return Err(format!("direct source argument {index} is not an Ident"));
        };
        let source_formal = tasty
            .source_ast
            .try_get(binding.source_method_formals[index])
            .ok_or_else(|| format!("caller source formal {index} is stale"))?;
        let TreeKind::ValDef(source_formal) = &source_formal.kind else {
            return Err(format!("caller source formal {index} is not a ValDef"));
        };
        let typed_argument = tasty
            .typed_ast
            .try_get(binding.typed_arguments[index])
            .ok_or_else(|| format!("direct typed argument {index} is stale"))?;
        if !matches!(typed_argument.kind, TreeKind::Ident(_))
            || source_ident.name != *source_formal.name.as_name()
            || tasty
                .typed_bindings
                .typed_at(tasty.source.id, binding.source_arguments[index])
                != Some(binding.typed_arguments[index])
            || !matches!(tasty.store.types.try_get(typed_argument.ty), Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }) if *prefix == tasty.definitions.no_prefix && *symbol == binding.method_formals[index])
            || !tasty.store.symbols.contains(binding.method_formals[index])
            || tasty.store.symbols.get(binding.method_formals[index]).kind != SymbolKind::Parameter
            || tasty.store.symbols.get(binding.method_formals[index]).owner != Some(caller)
            || tasty
                .store
                .symbols
                .get(binding.method_formals[index])
                .origin
                != SymbolOrigin::Source(tasty.source.id)
        {
            return Err(format!(
                "direct argument {index} does not name its ordered caller formal"
            ));
        }
    }
    let constructor_descriptor = outer_constructor.descriptor().as_str();
    let parsed_constructor =
        dotty_classfile::descriptor::MethodDescriptor::parse(constructor_descriptor)
            .map_err(|error| format!("outer constructor descriptor is malformed: {error}"))?;
    let caller_descriptor = caller_method.descriptor().as_str();
    let parsed_caller = dotty_classfile::descriptor::MethodDescriptor::parse(caller_descriptor)
        .map_err(|error| format!("direct-formal caller descriptor is malformed: {error}"))?;
    if parsed_constructor.parameters.len() != 2
        || parsed_constructor.parameters.iter().any(|parameter| {
            !matches!(parameter, dotty_classfile::descriptor::FieldType::Object(_))
        })
        || parsed_constructor.return_type.is_some()
        || parsed_caller.parameters != parsed_constructor.parameters
        || parsed_caller.return_type
            != Some(dotty_classfile::descriptor::FieldType::Object(
                outer_metadata.identity().internal_name().to_owned(),
            ))
    {
        return Err(
            "direct-formal caller descriptor differs from its selected two-reference constructor"
                .into(),
        );
    }
    Ok([
        caller_method.parameters()[0].slot(),
        caller_method.parameters()[1].slot(),
    ])
}

struct CrossClassMethodRef {
    caller: SymbolId,
    target: SymbolId,
    owner_internal_name: String,
    name: String,
    descriptor: String,
    constructor: bool,
    argument_load_opcode: Option<u8>,
    receiver_field: Option<SymbolId>,
    nested_constructor: Option<NestedConstructorMethodRef>,
    nested_constructor_pair: Option<[NestedConstructorMethodRef; 2]>,
    mixed_constructor_pair: Option<MixedConstructorPairRef>,
    direct_formal_constructor_pair: Option<[u16; 2]>,
    factory_method: Option<FactoryMethodRef>,
    factory_local_slot: Option<u16>,
    factory_local_max_locals: Option<u16>,
}

struct NestedConstructorMethodRef {
    owner_internal_name: String,
    descriptor: String,
    argument_local_slot: u16,
}

enum MixedConstructorArgumentRef {
    Formal { local_slot: u16 },
    LocalValue { local_slot: u16 },
    Nested(NestedConstructorMethodRef),
}

struct MixedConstructorPairRef {
    arguments: [MixedConstructorArgumentRef; 2],
    local_initializers: Vec<LocalCounterInitializerRef>,
    local_alias_copy: Option<LocalReferenceAliasCopyRef>,
    local_receiver_call: Option<LocalReceiverUnitCallRef>,
    local_receiver_call_pair: Option<[LocalReceiverUnitCallRef; 2]>,
    conditional_selection: Option<ConditionalSelectionRef>,
    max_stack: u16,
    max_locals: u16,
}

#[derive(Clone, Copy)]
struct ConditionalSelectionRef {
    condition_local_slot: u16,
    first_target_local_slot: u16,
    second_target_local_slot: u16,
    selected_local_slot: u16,
}

#[derive(Clone, Copy)]
struct LocalReferenceAliasCopyRef {
    source_local_slot: u16,
    alias_local_slot: u16,
}

struct LocalReceiverUnitCallRef {
    owner_internal_name: String,
    name: String,
    descriptor: String,
    receiver_local_slot: u16,
    argument_local_slot: u16,
}

struct LocalCounterInitializerRef {
    owner_internal_name: String,
    descriptor: String,
    argument_local_slot: u16,
    result_local_slot: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairedBlockShape {
    TwoLocals,
    TwoLocalsOneCall,
    TwoLocalsTwoCalls,
    TwoLocalsAliasTwoCalls,
    /// A same-count collision introduced by a third local and only one call.
    /// Keep this separate from the accepted two-local/two-call shape so the
    /// existing proof-cardinality failure remains the first reported failure.
    ThreeLocalsOneCall,
    ThreeLocalsConditionalSelection,
    ReceiptMismatch,
    Unclassified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairedStatementKind {
    ValDef,
    Apply,
    Other,
}

fn paired_statement_kinds<P: AstPhase>(
    stats: &[TreeId<P>],
    ast: &AstArena<P>,
) -> Option<Vec<PairedStatementKind>> {
    stats
        .iter()
        .map(|tree| {
            ast.try_get(*tree).map(|tree| match tree.kind {
                TreeKind::ValDef(_) => PairedStatementKind::ValDef,
                TreeKind::Apply(_) => PairedStatementKind::Apply,
                _ => PairedStatementKind::Other,
            })
        })
        .collect()
}

fn classify_paired_block_shape(
    source_kinds: &[PairedStatementKind],
    typed_kinds: &[PairedStatementKind],
    has_one_call_receipt: bool,
    has_two_call_receipt: bool,
    has_alias_receipt: bool,
    has_conditional_selection_receipt: bool,
) -> PairedBlockShape {
    if source_kinds.len() != typed_kinds.len() {
        return PairedBlockShape::ReceiptMismatch;
    }

    let has_expected_receipts = match source_kinds.len() {
        2 => !has_one_call_receipt && !has_two_call_receipt && !has_conditional_selection_receipt,
        3 => has_one_call_receipt && !has_two_call_receipt && !has_conditional_selection_receipt,
        4 => {
            (has_one_call_receipt
                && !has_two_call_receipt
                && !has_alias_receipt
                && has_conditional_selection_receipt)
                || (!has_one_call_receipt
                    && has_two_call_receipt
                    && !has_conditional_selection_receipt)
        }
        5 => {
            !has_one_call_receipt
                && has_two_call_receipt
                && has_alias_receipt
                && !has_conditional_selection_receipt
        }
        _ => false,
    };
    if !has_expected_receipts {
        return PairedBlockShape::ReceiptMismatch;
    }

    let matches_shape = |prefix: usize, call_count: usize| {
        source_kinds
            .iter()
            .zip(typed_kinds)
            .enumerate()
            .all(|(index, (source, typed))| {
                let expected_call = index >= prefix && index < prefix + call_count;
                let expected_kind = if expected_call {
                    PairedStatementKind::Apply
                } else {
                    PairedStatementKind::ValDef
                };
                *source == expected_kind && *typed == expected_kind
            })
    };

    match source_kinds.len() {
        2 if matches_shape(2, 0) && !has_alias_receipt => PairedBlockShape::TwoLocals,
        3 if matches_shape(2, 1) && !has_alias_receipt => PairedBlockShape::TwoLocalsOneCall,
        4 if matches_shape(2, 2) && !has_alias_receipt && !has_conditional_selection_receipt => {
            PairedBlockShape::TwoLocalsTwoCalls
        }
        4 if matches_shape(3, 1) && !has_alias_receipt && !has_conditional_selection_receipt => {
            PairedBlockShape::ThreeLocalsOneCall
        }
        4 if matches_shape(3, 1) && !has_alias_receipt && has_conditional_selection_receipt => {
            PairedBlockShape::ThreeLocalsConditionalSelection
        }
        5 if matches_shape(3, 2)
            && !has_one_call_receipt
            && has_two_call_receipt
            && has_alias_receipt =>
        {
            PairedBlockShape::TwoLocalsAliasTwoCalls
        }
        _ => PairedBlockShape::Unclassified,
    }
}

fn validate_two_local_constructor_pair(
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    caller: SymbolId,
    (source_rhs, typed_rhs): (TreeId<Untyped>, TreeId<Typed>),
    outer_application: &dotty_core::ast::Apply<Typed>,
    outer_constructor_symbol: SymbolId,
    outer_metadata: &JvmClassMetadata,
    all_metadata: &[JvmClassMetadata],
) -> Result<MixedConstructorPairRef, String> {
    use dotty_core::ast::TreeKind;

    let tasty = proof.tasty_plan.plan();
    let source_id = tasty.source.id;
    let pair = tasty
        .typed_bindings
        .formal_local_constructor_call_pair(source_id, caller)
        .ok_or_else(|| "two-local constructor has no ordered source receipt".to_owned())?;
    let Some(TreeKind::Block(source_block)) = tasty.source_ast.try_get(source_rhs).map(|t| &t.kind)
    else {
        return Err("two-local source RHS is not its original Block".into());
    };
    let Some(TreeKind::Block(typed_block)) = tasty.typed_ast.try_get(typed_rhs).map(|t| &t.kind)
    else {
        return Err("two-local typed RHS is not its original Block".into());
    };
    let source_declarations: [TreeId<Untyped>; 2] = source_block
        .stats
        .get(..2)
        .and_then(|stats| stats.try_into().ok())
        .ok_or_else(|| "two-local Block must begin with exactly two declarations".to_owned())?;
    let typed_declarations: [TreeId<Typed>; 2] = typed_block
        .stats
        .get(..2)
        .and_then(|stats| stats.try_into().ok())
        .ok_or_else(|| {
            "two-local typed Block must begin with exactly two declarations".to_owned()
        })?;
    let local_receiver_call = tasty
        .typed_bindings
        .local_receiver_unit_call(source_id, caller);
    let local_receiver_call_pair = tasty
        .typed_bindings
        .local_receiver_unit_call_pair(source_id, caller);
    let local_reference_alias = tasty
        .typed_bindings
        .local_reference_alias(source_id, caller);
    let source_statement_kinds = paired_statement_kinds(&source_block.stats, tasty.source_ast);
    let typed_statement_kinds = paired_statement_kinds(&typed_block.stats, tasty.typed_ast);
    let block_shape = match (
        source_statement_kinds.as_deref(),
        typed_statement_kinds.as_deref(),
    ) {
        (Some(source_kinds), Some(typed_kinds)) => classify_paired_block_shape(
            source_kinds,
            typed_kinds,
            local_receiver_call.is_some(),
            local_receiver_call_pair.is_some(),
            local_reference_alias.is_some(),
            tasty
                .typed_bindings
                .conditional_local_reference_selection(source_id, caller)
                .is_some(),
        ),
        _ => PairedBlockShape::Unclassified,
    };
    if matches!(block_shape, PairedBlockShape::ReceiptMismatch) {
        return Err(
            "two-local Block must contain exactly zero, one, or two Counter.set stats".into(),
        );
    }
    let initializers = tasty
        .typed_bindings
        .local_counter_initializers(source_id, caller);
    let locals = tasty.typed_bindings.local_value_bindings(source_id, caller);
    let Some(TreeKind::Apply(source_outer)) = tasty
        .source_ast
        .try_get(source_block.expr)
        .map(|tree| &tree.kind)
    else {
        return Err("two-local source Block result is not the original Pair Apply".into());
    };
    let Some(TreeKind::Apply(typed_outer)) = tasty
        .typed_ast
        .try_get(typed_block.expr)
        .map(|tree| &tree.kind)
    else {
        return Err("two-local typed Block result is not the original Pair Apply".into());
    };
    // Keep validating malformed five-stat alias candidates through the same
    // detailed checks as before; the final shape gate below prevents this
    // diagnostic-only fallback from authorizing a new shape.
    let conditional_selection = tasty
        .typed_bindings
        .conditional_local_reference_selection(source_id, caller);
    let has_conditional_selection = matches!(
        block_shape,
        PairedBlockShape::ThreeLocalsConditionalSelection
    );
    let has_alias = matches!(block_shape, PairedBlockShape::TwoLocalsAliasTwoCalls)
        || (matches!(block_shape, PairedBlockShape::Unclassified)
            && source_block.stats.len() == 5
            && local_reference_alias.is_some());
    if initializers.len() != 2
        || locals.len()
            != if has_alias || has_conditional_selection {
                3
            } else {
                2
            }
        || tasty.typed_bindings.typed_at(source_id, source_rhs) != Some(typed_rhs)
        || source_outer.kind != dotty_core::ast::ApplyKind::Regular
        || typed_outer.kind != dotty_core::ast::ApplyKind::Regular
        || source_outer.args.as_slice() != pair.arguments.map(|argument| argument.source_argument)
        || typed_outer.args.as_slice() != pair.arguments.map(|argument| argument.typed_argument)
        || tasty.typed_bindings.typed_at(source_id, source_block.expr) != Some(typed_block.expr)
    {
        return Err("two-local proof does not retain exactly two original declarations and initializer receipts".into());
    }
    if has_conditional_selection != conditional_selection.is_some() {
        return Err(
            "three-local conditional selection receipt does not match its exact source shape"
                .into(),
        );
    }
    if let Some(alias) = local_reference_alias {
        let source_declaration = *source_block
            .stats
            .get(2)
            .ok_or_else(|| "alias declaration is missing from the original Block".to_owned())?;
        let typed_declaration = *typed_block.stats.get(2).ok_or_else(|| {
            "typed alias declaration is missing from the original Block".to_owned()
        })?;
        let source_local = locals
            .get(2)
            .ok_or_else(|| "alias has no canonical local value binding".to_owned())?;
        if alias.source_block != source_rhs
            || alias.typed_block != typed_rhs
            || alias.source_declaration != source_declaration
            || alias.typed_declaration != typed_declaration
            || alias.local != source_local.symbol
            || alias.scope != source_local.scope
            || alias.local == alias.target
            || initializers
                .iter()
                .find(|initializer| initializer.argument_formal == pair.method_formals[0])
                .map(|initializer| initializer.local)
                != Some(alias.target)
            || !matches!(tasty.source_ast.try_get(source_declaration).map(|tree| &tree.kind), Some(TreeKind::ValDef(value)) if value.rhs == Some(alias.source_initializer) && value.tpt == alias.source_type_tree)
            || !matches!(tasty.typed_ast.try_get(typed_declaration).map(|tree| &tree.kind), Some(TreeKind::ValDef(value)) if value.rhs == Some(alias.typed_initializer) && value.tpt == alias.typed_type_tree)
            || tasty.typed_bindings.typed_at(source_id, source_declaration)
                != Some(typed_declaration)
            || tasty
                .typed_bindings
                .typed_at(source_id, alias.source_initializer)
                != Some(alias.typed_initializer)
            || tasty
                .typed_bindings
                .typed_at(source_id, alias.source_target)
                != Some(alias.typed_target)
            || !matches!(
                tasty
                    .source_ast
                    .try_get(alias.source_initializer)
                    .map(|tree| &tree.kind),
                Some(TreeKind::Ident(_))
            )
            || !matches!(
                tasty
                    .typed_ast
                    .try_get(alias.typed_target)
                    .map(|tree| &tree.kind),
                Some(TreeKind::Ident(_))
            )
            || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(alias.typed_target).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == alias.target)
        {
            return Err("alias declaration does not preserve the canonical first-local source/typed identity".into());
        }
    } else if has_alias {
        return Err("five-stat local Block requires its canonical alias receipt".into());
    }
    if pair.method_formals[0] == pair.method_formals[1]
        || pair.source_method_formals[0] == pair.source_method_formals[1]
        || outer_application.kind != dotty_core::ast::ApplyKind::Regular
        || outer_application.args.as_slice()
            != pair.arguments.map(|argument| argument.typed_argument)
        || tasty.typed_bindings.typed_at(source_id, source_block.expr) != Some(typed_block.expr)
    {
        return Err(
            "two-local final Pair application differs from the ordered source proof".into(),
        );
    }

    let method = proof
        .metadata
        .methods()
        .iter()
        .find(|method| method.symbol() == caller)
        .ok_or_else(|| "two-local caller is absent from JVM metadata".to_owned())?;
    let caller_plan = proof
        .jvm_unit
        .classes
        .iter()
        .find(|class| class.class == tasty.class)
        .and_then(|class| class.methods.iter().find(|method| method.method == caller))
        .ok_or_else(|| "two-local caller has no JVM method plan".to_owned())?;
    let conditional_selection_ref = if has_conditional_selection {
        let selection = conditional_selection
            .ok_or_else(|| "conditional selection proof is absent".to_owned())?;
        let source_declaration = source_block.stats[2];
        let typed_declaration = typed_block.stats[2];
        let selected_local = locals[2];
        let Some(TreeKind::ValDef(source_value)) = tasty
            .source_ast
            .try_get(source_declaration)
            .map(|tree| &tree.kind)
        else {
            return Err(
                "conditional selected declaration is not the original source ValDef".into(),
            );
        };
        let Some(TreeKind::ValDef(typed_value)) = tasty
            .typed_ast
            .try_get(typed_declaration)
            .map(|tree| &tree.kind)
        else {
            return Err("conditional selected declaration is not the original typed ValDef".into());
        };
        let Some(TreeKind::If(source_if)) = tasty
            .source_ast
            .try_get(selection.source_if)
            .map(|tree| &tree.kind)
        else {
            return Err("conditional selection source root is not its original If".into());
        };
        let Some(TreeKind::If(typed_if)) = tasty
            .typed_ast
            .try_get(selection.typed_if)
            .map(|tree| &tree.kind)
        else {
            return Err("conditional selection typed root is not its original If".into());
        };
        if !tasty.store.symbols.contains(selection.condition_formal)
            || !tasty.store.symbols.contains(selection.selected)
        {
            return Err(
                "conditional selection refers to a stale condition or selected-local symbol".into(),
            );
        }
        let condition_type = tasty.store.symbols.get(selection.condition_formal).info;
        let bool_type = tasty.definitions.boolean;
        let first_target = initializers[0].local;
        let second_target = initializers[1].local;
        let targets_match = selection.targets == [first_target, second_target]
            || selection.targets == [second_target, first_target];
        let typed_arms_match = selection.typed_arms.iter().zip(selection.targets).all(
            |(arm, target)| {
                matches!(
                    tasty.typed_ast.try_get(*arm).and_then(|tree| tasty.store.types.try_get(tree.ty)),
                    Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) })
                        if *prefix == tasty.definitions.no_prefix && *symbol == target
                )
            },
        );
        let source_arms_match = selection
            .source_arms
            .iter()
            .zip(selection.typed_arms)
            .zip(selection.targets)
            .all(|((arm, typed_arm), target)| {
                let Some(TreeKind::Ident(source_ident)) =
                    tasty.source_ast.try_get(*arm).map(|tree| &tree.kind)
                else {
                    return false;
                };
                let Some(TreeKind::Ident(typed_ident)) =
                    tasty.typed_ast.try_get(typed_arm).map(|tree| &tree.kind)
                else {
                    return false;
                };
                source_ident.name == tasty.store.symbols.get(target).name
                    && typed_ident.name == source_ident.name
                    && tasty.typed_bindings.typed_at(source_id, *arm) == Some(typed_arm)
            });
        let typed_condition_matches = matches!(
            tasty
                .typed_ast
                .try_get(selection.typed_condition)
                .and_then(|tree| tasty.store.types.try_get(tree.ty)),
            Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                if *symbol == selection.condition_formal
        );
        if selection.source_id != source_id
            || selection.owner != tasty.class
            || selection.method != caller
            || selection.scope != selected_local.scope
            || selection.source_block != source_rhs
            || selection.typed_block != typed_rhs
            || selection.source_declaration != source_declaration
            || selection.typed_declaration != typed_declaration
            || source_value.rhs != Some(selection.source_if)
            || typed_value.rhs != Some(selection.typed_if)
            || selection.selected != selected_local.symbol
            || selected_local.source_tree != source_declaration
            || selected_local.typed_declaration != typed_declaration
            || selected_local.scope != locals[0].scope
            || !matches!(tasty.store.symbols.get(selection.selected).info, SymbolInfo::Complete(ty) if ty == selection.local_type)
            || !matches!(tasty.store.types.try_get(selection.local_type), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == initializers[0].call.class)
            || !targets_match
            || source_if.cond != selection.source_condition
            || typed_if.cond != selection.typed_condition
            || [source_if.then_branch, source_if.else_branch] != selection.source_arms
            || [typed_if.then_branch, typed_if.else_branch] != selection.typed_arms
            || tasty
                .typed_bindings
                .typed_at(source_id, selection.source_if)
                != Some(selection.typed_if)
            || tasty
                .typed_bindings
                .typed_at(source_id, selection.source_condition)
                != Some(selection.typed_condition)
            || !typed_arms_match
            || !source_arms_match
            || !typed_condition_matches
            || !matches!(condition_type, SymbolInfo::Complete(ty) if ty == bool_type)
            || !tasty.store.symbols.contains(selection.condition_formal)
        {
            return Err("conditional selection receipt differs from the canonical original If and local bindings".into());
        }
        let condition_slot = method
            .parameters()
            .iter()
            .find(|parameter| parameter.symbol() == selection.condition_formal)
            .ok_or_else(|| {
                "conditional condition formal is absent from the factory method".to_owned()
            })?
            .slot();
        let conditional_layout = proof
            .jvm_unit
            .local_layout_for(method, &caller_plan.locals, tasty.definitions)
            .map_err(|error| format!("cannot derive conditional JVM layout: {error:?}"))?;
        let first_slot = conditional_layout
            .local(selection.targets[0])
            .map_err(|error| format!("first conditional branch target has no JVM slot: {error:?}"))?
            .slot();
        let second_slot = conditional_layout
            .local(selection.targets[1])
            .map_err(|error| {
                format!("second conditional branch target has no JVM slot: {error:?}")
            })?
            .slot();
        let selected_slot = conditional_layout
            .local(selected_local.symbol)
            .map_err(|error| format!("selected conditional local has no JVM slot: {error:?}"))?;
        if selected_slot.declaration() != typed_declaration
            || selected_slot.is_mutable()
            || selected_slot.ty() != dotty_jvm::JvmType::Reference(initializers[0].call.class)
        {
            return Err(
                "selected conditional local differs from its canonical immutable JVM slot".into(),
            );
        }
        Some(ConditionalSelectionRef {
            condition_local_slot: condition_slot,
            first_target_local_slot: first_slot,
            second_target_local_slot: second_slot,
            selected_local_slot: selected_slot.slot(),
        })
    } else {
        None
    };

    let conditional_formals_match = if let Some(selection) = conditional_selection {
        method.parameters().len() == 4
            && method.parameters()[2].symbol() == selection.condition_formal
            && method.parameters()[2].ty() == dotty_jvm::JvmType::Boolean
            && method.parameters()[2].slot() == 3
            && method.parameters()[3].ty() == dotty_jvm::JvmType::Int
            && method.parameters()[3].slot() == 4
            && tasty.store.symbols.contains(selection.condition_formal)
            && tasty.store.symbols.get(selection.condition_formal).kind == SymbolKind::Parameter
            && tasty.store.symbols.get(selection.condition_formal).owner == Some(caller)
            && tasty.store.symbols.get(selection.condition_formal).origin
                == SymbolOrigin::Source(source_id)
            && matches!(tasty.store.symbols.get(selection.condition_formal).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.boolean)
            && tasty
                .store
                .symbols
                .get(method.parameters()[3].symbol())
                .kind
                == SymbolKind::Parameter
            && tasty
                .store
                .symbols
                .get(method.parameters()[3].symbol())
                .owner
                == Some(caller)
            && tasty
                .store
                .symbols
                .get(method.parameters()[3].symbol())
                .origin
                == SymbolOrigin::Source(source_id)
            && matches!(tasty.store.symbols.get(method.parameters()[3].symbol()).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
    } else {
        method.parameters().len() == 2
    };
    if !conditional_formals_match
        || method.parameters()[0].symbol() != pair.method_formals[0]
        || method.parameters()[1].symbol() != pair.method_formals[1]
        || method.parameters()[0].slot() != 1
        || method.parameters()[1].slot() != 2
        || pair.method_formals.iter().any(|formal| {
            !tasty.store.symbols.contains(*formal)
                || tasty.store.symbols.get(*formal).kind != SymbolKind::Parameter
                || tasty.store.symbols.get(*formal).owner != Some(caller)
                || tasty.store.symbols.get(*formal).origin != SymbolOrigin::Source(source_id)
                || !matches!(tasty.store.symbols.get(*formal).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
        })
    {
        return Err("two-local seed formals do not preserve the original method ABI".into());
    }
    if initializers[0].argument_formal == initializers[1].argument_formal {
        return Err("two-local initializers must use distinct authorized seed formals".into());
    }
    let layout = proof
        .jvm_unit
        .local_layout_for(method, &caller_plan.locals, tasty.definitions)
        .map_err(|error| format!("cannot derive two-local JVM layout: {error:?}"))?;
    let mut local_slots = Vec::with_capacity(2);
    let mut local_class = None;
    let mut local_initializers = Vec::with_capacity(2);
    for index in 0..2 {
        let initializer = initializers[index];
        let local_binding = locals[index];
        let source_declaration = source_declarations[index];
        let typed_declaration = typed_declarations[index];
        let Some(TreeKind::ValDef(source_val)) = tasty
            .source_ast
            .try_get(source_declaration)
            .map(|t| &t.kind)
        else {
            return Err(format!(
                "two-local source declaration {index} is not a ValDef"
            ));
        };
        let Some(TreeKind::ValDef(typed_val)) =
            tasty.typed_ast.try_get(typed_declaration).map(|t| &t.kind)
        else {
            return Err(format!(
                "two-local typed declaration {index} is not a ValDef"
            ));
        };
        if initializer.source_block != source_rhs
            || initializer.typed_block != typed_rhs
            || initializer.source_declaration != source_declaration
            || initializer.typed_declaration != typed_declaration
            || local_binding.source_tree != source_declaration
            || local_binding.typed_declaration != typed_declaration
            || local_binding.symbol != initializer.local
            || local_binding.scope != initializer.scope
            || source_val.rhs != Some(initializer.source_initializer)
            || typed_val.rhs != Some(initializer.typed_initializer)
            || source_val.tpt != initializer.source_type_tree
            || typed_val.tpt != initializer.typed_type_tree
            || tasty.typed_bindings.typed_at(source_id, source_declaration)
                != Some(typed_declaration)
            || tasty
                .typed_bindings
                .typed_at(source_id, initializer.source_initializer)
                != Some(initializer.typed_initializer)
            || tasty
                .typed_bindings
                .typed_at(source_id, initializer.source_type_tree)
                != Some(initializer.typed_type_tree)
            || !tasty.store.symbols.contains(initializer.local)
            || tasty.store.symbols.get(initializer.local).kind != SymbolKind::Local
            || tasty.store.symbols.get(initializer.local).owner != Some(caller)
            || tasty.store.symbols.get(initializer.local).origin != SymbolOrigin::Source(source_id)
            || !tasty.store.symbols.get(initializer.local).flags.is_empty()
            || !matches!(tasty.store.symbols.get(initializer.local).info, SymbolInfo::Complete(ty) if ty == initializer.local_type)
            || tasty.typed_ast.get(typed_declaration).ty != initializer.local_type
            || !pair.method_formals.contains(&initializer.argument_formal)
            || initializer.call.formal_type != tasty.definitions.int
            || !matches!(tasty.store.types.try_get(initializer.local_type), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == initializer.call.class)
        {
            return Err(format!(
                "two-local declaration/initializer {index} has stale or foreign source identity"
            ));
        }
        if local_class
            .replace(initializer.call.class)
            .is_some_and(|previous| previous != initializer.call.class)
        {
            return Err("two-local declarations do not share one canonical Counter class".into());
        }
        let local_slot = layout
            .local(initializer.local)
            .map_err(|error| format!("two-local binding has no allocator slot: {error:?}"))?;
        if local_slot.declaration() != typed_declaration
            || local_slot.is_mutable()
            || local_slot.ty() != dotty_jvm::JvmType::Reference(initializer.call.class)
        {
            return Err(format!(
                "two-local declaration {index} differs from its canonical JVM slot"
            ));
        }
        local_slots.push(local_slot.slot());

        let call = initializer.call;
        let Some(TreeKind::Apply(source_apply)) = tasty
            .source_ast
            .try_get(initializer.source_initializer)
            .map(|t| &t.kind)
        else {
            return Err(format!(
                "two-local initializer {index} is not a source Apply"
            ));
        };
        let Some(TreeKind::Apply(typed_apply)) =
            tasty.typed_ast.try_get(call.typed_apply).map(|t| &t.kind)
        else {
            return Err(format!(
                "two-local initializer {index} is not a typed Apply"
            ));
        };
        if source_apply.kind != dotty_core::ast::ApplyKind::Regular
            || source_apply.args.as_slice() != [call.source_argument]
            || source_apply.function != call.source_select
            || typed_apply.kind != dotty_core::ast::ApplyKind::Regular
            || typed_apply.function != call.typed_select
            || typed_apply.args.as_slice() != [call.typed_argument]
            || !matches!(tasty.source_ast.try_get(call.source_select).map(|t| &t.kind), Some(TreeKind::Select(selection)) if selection.qualifier == call.source_new)
            || !matches!(
                tasty.source_ast.try_get(call.source_new).map(|t| &t.kind),
                Some(TreeKind::New(_))
            )
            || !matches!(tasty.typed_ast.try_get(call.typed_select).map(|t| &t.kind), Some(TreeKind::Select(selection)) if selection.qualifier == call.typed_new)
            || !matches!(
                tasty.typed_ast.try_get(call.typed_new).map(|t| &t.kind),
                Some(TreeKind::New(_))
            )
            || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(call.typed_argument).ty), Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }) if *prefix == tasty.definitions.no_prefix && *symbol == initializer.argument_formal)
            || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(call.typed_new).ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == call.class)
            || tasty.typed_ast.get(call.typed_apply).ty != call.result_type
            || tasty.typed_bindings.typed_at(source_id, call.source_select)
                != Some(call.typed_select)
            || tasty.typed_bindings.typed_at(source_id, call.source_new) != Some(call.typed_new)
            || tasty
                .typed_bindings
                .typed_at(source_id, call.source_argument)
                != Some(call.typed_argument)
            || !matches!(tasty.typed_ast.try_get(call.typed_select).and_then(|t| tasty.store.types.try_get(t.ty)), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == call.constructor)
        {
            return Err(format!(
                "two-local initializer {index} does not authenticate its original Counter(seed) source/typed path"
            ));
        }
        let nested_metadata = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == call.class)
            .ok_or_else(|| format!("two-local Counter class {index} is outside the batch"))?;
        let nested_constructor = nested_metadata
            .constructor()
            .filter(|constructor| constructor.symbol() == call.constructor)
            .ok_or_else(|| {
                format!("two-local Counter constructor {index} is absent from the JVM plan")
            })?;
        if nested_constructor.descriptor().as_str() != "(I)V"
            || !nested_constructor.access_flags().is_public()
            || !tasty.store.symbols.contains(call.formal)
            || tasty.store.symbols.get(call.formal).kind != SymbolKind::Parameter
            || tasty.store.symbols.get(call.formal).owner != Some(call.constructor)
            || !nested_constructor.parameters().iter().any(|parameter| {
                parameter.symbol() == call.formal && parameter.ty() == dotty_jvm::JvmType::Int
            })
        {
            return Err(format!(
                "two-local Counter initializer {index} has a foreign constructor ABI"
            ));
        }
        local_initializers.push(LocalCounterInitializerRef {
            owner_internal_name: nested_metadata.identity().internal_name().to_owned(),
            descriptor: nested_constructor.descriptor().as_str().to_owned(),
            argument_local_slot: method
                .parameters()
                .iter()
                .find(|parameter| parameter.symbol() == initializer.argument_formal)
                .ok_or_else(|| {
                    format!(
                        "two-local initializer {index} formal is absent from the authenticated JVM method"
                    )
                })?
                .slot(),
            result_local_slot: local_slot.slot(),
        });
    }

    let local_symbols = [initializers[0].local, initializers[1].local];
    let conditional_selected = conditional_selection.map(|selection| selection.selected);
    let counter_class =
        local_class.ok_or_else(|| "two-local Counter class is absent".to_owned())?;
    let local_alias_copy = local_reference_alias
        .map(|alias| {
            let source_index = local_symbols
                .iter()
                .position(|symbol| *symbol == alias.target)
                .ok_or_else(|| {
                    "alias target is not one of the original Counter locals".to_owned()
                })?;
            let source_local_slot = *local_slots
                .get(source_index)
                .ok_or_else(|| "alias target has no authenticated JVM local slot".to_owned())?;
            let alias_slot = layout
                .local(alias.local)
                .map_err(|error| format!("alias has no canonical JVM local slot: {error:?}"))?;
            if alias_slot.declaration() != alias.typed_declaration
                || alias_slot.is_mutable()
                || alias_slot.ty() != dotty_jvm::JvmType::Reference(counter_class)
            {
                return Err(
                    "alias JVM slot differs from its canonical immutable reference declaration"
                        .into(),
                );
            }
            Ok::<LocalReferenceAliasCopyRef, String>(LocalReferenceAliasCopyRef {
                source_local_slot,
                alias_local_slot: alias_slot.slot(),
            })
        })
        .transpose()?;
    let pair_first_receiver = match pair.arguments[0].origin {
        FormalLocalConstructorCallArgumentOrigin::LocalValue { symbol, .. } => symbol,
        _ => return Err("first Pair argument is not an authenticated local value".into()),
    };
    let validate_receiver_call = |binding: LocalReceiverUnitCallBinding,
                                  call_index: usize,
                                  required_receiver: Option<SymbolId>,
                                  required_formal: Option<SymbolId>|
     -> Result<LocalReceiverUnitCallRef, String> {
        let call_prefix = if has_alias || has_conditional_selection {
            3
        } else {
            2
        };
        let source_call_tree = *source_block
            .stats
            .get(call_index + call_prefix)
            .ok_or_else(|| format!("Counter.set source stat {call_index} is missing"))?;
        let typed_call_tree = *typed_block
            .stats
            .get(call_index + call_prefix)
            .ok_or_else(|| format!("Counter.set typed stat {call_index} is missing"))?;
        let source_apply = tasty
            .source_ast
            .try_get(binding.source_apply)
            .ok_or_else(|| format!("Counter.set source Apply {call_index} is stale"))?;
        let typed_apply = tasty
            .typed_ast
            .try_get(binding.typed_apply)
            .ok_or_else(|| format!("Counter.set typed Apply {call_index} is stale"))?;
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source_apply.kind, &typed_apply.kind)
        else {
            return Err("Counter.set receipt roots must remain Apply trees".into());
        };
        if source_call_tree != binding.source_apply
            || typed_call_tree != binding.typed_apply
            || source_apply.kind != dotty_core::ast::ApplyKind::Regular
            || typed_apply.kind != dotty_core::ast::ApplyKind::Regular
            || source_apply.function != binding.source_select
            || source_apply.args.as_slice() != [binding.source_argument]
            || typed_apply.function != binding.typed_select
            || typed_apply.args.as_slice() != [binding.typed_argument]
            || tasty
                .typed_bindings
                .typed_at(source_id, binding.source_apply)
                != Some(binding.typed_apply)
            || tasty
                .typed_bindings
                .typed_at(source_id, binding.source_select)
                != Some(binding.typed_select)
            || tasty
                .typed_bindings
                .typed_at(source_id, binding.source_receiver)
                != Some(binding.typed_receiver)
            || tasty
                .typed_bindings
                .typed_at(source_id, binding.source_argument)
                != Some(binding.typed_argument)
            || !matches!(tasty.source_ast.try_get(binding.source_select).map(|tree| &tree.kind), Some(TreeKind::Select(select)) if select.qualifier == binding.source_receiver)
            || !matches!(tasty.typed_ast.try_get(binding.typed_select).map(|tree| &tree.kind), Some(TreeKind::Select(select)) if select.qualifier == binding.typed_receiver)
            || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(binding.typed_receiver).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == binding.receiver && (local_symbols.contains(symbol) || local_reference_alias.is_some_and(|alias| alias.local == *symbol) || conditional_selected == Some(*symbol)) && required_receiver.is_none_or(|expected| expected == *symbol))
            || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(binding.typed_argument).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == binding.argument_formal && method.parameters().iter().any(|parameter| parameter.symbol() == *symbol) && required_formal.is_none_or(|expected| expected == *symbol))
            || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(binding.typed_select).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == binding.method)
            || tasty.typed_ast.get(binding.typed_apply).ty != tasty.definitions.unit
        {
            return Err(
                "Counter.set source/typed call roots differ from the canonical two-local receipt"
                    .into(),
            );
        }
        let counter_metadata = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == counter_class)
            .ok_or_else(|| "Counter.set receiver class is outside the batch".to_owned())?;
        let set_method = counter_metadata
            .methods()
            .iter()
            .find(|method| method.symbol() == binding.method)
            .ok_or_else(|| "Counter.set selected method is absent from the JVM plan".to_owned())?;
        if set_method.name() != "set"
            || set_method.descriptor().as_str() != "(I)V"
            || set_method.access_flags().contains(ACC_STATIC)
            || tasty.store.symbols.get(binding.method).owner != Some(counter_class)
        {
            return Err("Counter.set JVM target must be the selected instance set(I)V".into());
        }
        let receiver_scope = local_reference_alias
            .filter(|alias| alias.local == binding.receiver)
            .map(|alias| alias.scope)
            .or_else(|| {
                initializers
                    .iter()
                    .find(|initializer| initializer.local == binding.receiver)
                    .map(|initializer| initializer.scope)
            })
            .or_else(|| {
                conditional_selected
                    .filter(|symbol| *symbol == binding.receiver)
                    .and_then(|symbol| locals.iter().find(|local| local.symbol == symbol))
                    .map(|local| local.scope)
            })
            .ok_or_else(|| "Counter.set receiver has no authenticated local binding".to_owned())?;
        if binding.receiver_scope != receiver_scope {
            return Err("Counter.set receiver scope differs from its initialized local".into());
        }
        let receiver_local_slot = if let Some(alias) =
            local_reference_alias.filter(|alias| alias.local == binding.receiver)
        {
            let slot = layout
                .local(alias.local)
                .map_err(|error| format!("alias has no canonical JVM local slot: {error:?}"))?;
            if slot.declaration() != alias.typed_declaration
                || slot.is_mutable()
                || slot.ty() != dotty_jvm::JvmType::Reference(counter_class)
            {
                return Err(
                    "alias JVM slot differs from its canonical immutable declaration".into(),
                );
            }
            slot.slot()
        } else if conditional_selected == Some(binding.receiver) {
            conditional_selection_ref
                .ok_or_else(|| {
                    "conditional receiver has no authenticated selected slot".to_owned()
                })?
                .selected_local_slot
        } else {
            let receiver_index = local_symbols
                .iter()
                .position(|symbol| *symbol == binding.receiver)
                .ok_or_else(|| {
                    "Counter.set receiver is not one of the authenticated locals".to_owned()
                })?;
            *local_slots.get(receiver_index).ok_or_else(|| {
                "Counter.set receiver has no authenticated JVM local slot".to_owned()
            })?
        };
        let argument_local_slot = method
            .parameters()
            .iter()
            .find(|parameter| parameter.symbol() == binding.argument_formal)
            .ok_or_else(|| "Counter.set formal is absent from the factory method".to_owned())?
            .slot();
        Ok(LocalReceiverUnitCallRef {
            owner_internal_name: counter_metadata.identity().internal_name().to_owned(),
            name: set_method.name().to_owned(),
            descriptor: set_method.descriptor().as_str().to_owned(),
            receiver_local_slot,
            argument_local_slot,
        })
    };
    let local_receiver_call = local_receiver_call
        .map(|binding| {
            validate_receiver_call(
                binding,
                0,
                Some(conditional_selected.unwrap_or(pair_first_receiver)),
                Some(if has_conditional_selection {
                    method.parameters()[3].symbol()
                } else {
                    pair.method_formals[1]
                }),
            )
        })
        .transpose()?;
    let local_receiver_call_pair = local_receiver_call_pair
        .map(|bindings| {
            if bindings[0].argument_formal == bindings[1].argument_formal {
                return Err("Counter.set pair must use distinct factory formals".to_owned());
            }
            Ok([
                validate_receiver_call(bindings[0], 0, None, None)?,
                validate_receiver_call(bindings[1], 1, None, None)?,
            ])
        })
        .transpose()?;
    if local_receiver_call.is_some() && local_receiver_call_pair.is_some() {
        return Err("two-local Block has both single-call and paired-call receipts".into());
    };

    let outer_constructor = outer_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == outer_constructor_symbol)
        .ok_or_else(|| "two-local Pair constructor is absent from the JVM plan".to_owned())?;
    let mut arguments = Vec::with_capacity(2);
    for (index, argument) in pair.arguments.iter().enumerate() {
        if tasty
            .typed_bindings
            .typed_at(source_id, argument.source_argument)
            != Some(argument.typed_argument)
        {
            return Err(format!(
                "two-local Pair argument {index} has a stale source-to-typed edge"
            ));
        }
        let source_argument = tasty
            .source_ast
            .try_get(argument.source_argument)
            .ok_or_else(|| format!("two-local source argument {index} is stale"))?;
        let typed_argument = tasty
            .typed_ast
            .try_get(argument.typed_argument)
            .ok_or_else(|| format!("two-local typed argument {index} is stale"))?;
        let FormalLocalConstructorCallArgumentOrigin::LocalValue {
            symbol,
            result_type,
        } = argument.origin
        else {
            return Err("two-local Pair arguments must be original local references".into());
        };
        let local_index = local_symbols
            .iter()
            .position(|candidate| *candidate == symbol)
            .ok_or_else(|| format!("two-local Pair argument {index} names a foreign local"))?;
        let local_name = tasty.store.symbols.get(symbol).name;
        if !matches!(tasty.store.types.try_get(typed_argument.ty), Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(target) }) if *prefix == tasty.definitions.no_prefix && *target == symbol)
            || typed_argument.ty != result_type
            || !matches!(source_argument.kind, TreeKind::Ident(ident) if ident.name == local_name)
            || !matches!(typed_argument.kind, TreeKind::Ident(ident) if ident.name == local_name)
            || outer_constructor
                .parameters()
                .get(index)
                .map(|parameter| parameter.ty())
                != Some(dotty_jvm::JvmType::Reference(counter_class))
        {
            return Err(format!(
                "two-local Pair argument {index} does not target the exact initialized source local"
            ));
        }
        arguments.push(MixedConstructorArgumentRef::LocalValue {
            local_slot: local_slots[local_index],
        });
    }
    let counter_name = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == counter_class)
        .ok_or_else(|| "two-local Counter class is outside the batch".to_owned())?
        .identity()
        .internal_name();
    if outer_constructor.descriptor().as_str() != format!("(L{counter_name};L{counter_name};)V")
        || method.descriptor().as_str()
            != if has_conditional_selection {
                format!("(IIZI)L{};", outer_metadata.identity().internal_name())
            } else {
                format!("(II)L{};", outer_metadata.identity().internal_name())
            }
        || !matches!(tasty.typed_ast.try_get(typed_rhs).and_then(|tree| tasty.store.types.try_get(tree.ty)), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == outer_metadata.identity().symbol())
    {
        return Err(
            "two-local source method and selected Pair constructor do not match the original ABI"
                .into(),
        );
    }
    if !matches!(
        block_shape,
        PairedBlockShape::TwoLocals
            | PairedBlockShape::TwoLocalsOneCall
            | PairedBlockShape::TwoLocalsTwoCalls
            | PairedBlockShape::TwoLocalsAliasTwoCalls
            | PairedBlockShape::ThreeLocalsConditionalSelection
    ) {
        return Err(
            "two-local proof does not retain an accepted ordered statement/proof shape".into(),
        );
    }
    Ok(MixedConstructorPairRef {
        arguments: arguments
            .try_into()
            .map_err(|_| "two-local Pair argument receipt is incomplete")?,
        local_initializers,
        local_alias_copy,
        local_receiver_call,
        local_receiver_call_pair,
        conditional_selection: conditional_selection_ref,
        max_stack: 4,
        max_locals: layout.max_locals(),
    })
}

fn validate_formal_local_constructor_pair(
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    caller: SymbolId,
    (source_rhs, typed_rhs): (TreeId<Untyped>, TreeId<Typed>),
    outer_application: &dotty_core::ast::Apply<Typed>,
    outer_constructor_symbol: SymbolId,
    outer_metadata: &JvmClassMetadata,
    all_metadata: &[JvmClassMetadata],
) -> Result<MixedConstructorPairRef, String> {
    use dotty_core::ast::TreeKind;

    let tasty = proof.tasty_plan.plan();
    let source_id = tasty.source.id;
    let pair = tasty
        .typed_bindings
        .formal_local_constructor_call_pair(source_id, caller)
        .ok_or_else(|| {
            "two-argument local constructor has no FormalLocalPair receipt".to_owned()
        })?;
    let initializer = tasty
        .typed_bindings
        .local_counter_initializer(source_id, caller)
        .ok_or_else(|| {
            "FormalLocalPair has no direct local Counter initializer receipt".to_owned()
        })?;
    let Some(TreeKind::Block(source_block)) =
        tasty.source_ast.try_get(source_rhs).map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair source RHS is not the original Block".into());
    };
    let Some(TreeKind::Block(typed_block)) =
        tasty.typed_ast.try_get(typed_rhs).map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair typed RHS is not the original Block".into());
    };
    let [source_declaration] = source_block.stats.as_slice() else {
        return Err("FormalLocalPair Block must contain exactly one local declaration".into());
    };
    let [typed_declaration] = typed_block.stats.as_slice() else {
        return Err(
            "FormalLocalPair typed Block must contain exactly one local declaration".into(),
        );
    };
    if initializer.source_block != source_rhs
        || initializer.typed_block != typed_rhs
        || initializer.source_declaration != *source_declaration
        || initializer.typed_declaration != *typed_declaration
        || tasty.typed_bindings.typed_at(source_id, source_rhs) != Some(typed_rhs)
    {
        return Err(
            "FormalLocalPair initializer roots differ from the original source/typed Block".into(),
        );
    }
    let Some(TreeKind::ValDef(source_val)) = tasty
        .source_ast
        .try_get(*source_declaration)
        .map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair declaration is not a source ValDef".into());
    };
    let Some(TreeKind::ValDef(typed_val)) = tasty
        .typed_ast
        .try_get(*typed_declaration)
        .map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair declaration is not a typed ValDef".into());
    };
    if source_val.rhs != Some(initializer.source_initializer)
        || typed_val.rhs != Some(initializer.typed_initializer)
        || source_val.tpt != initializer.source_type_tree
        || typed_val.tpt != initializer.typed_type_tree
        || tasty
            .typed_bindings
            .typed_at(source_id, initializer.source_declaration)
            != Some(initializer.typed_declaration)
        || tasty
            .typed_bindings
            .typed_at(source_id, initializer.source_initializer)
            != Some(initializer.typed_initializer)
        || tasty
            .typed_bindings
            .typed_at(source_id, initializer.source_type_tree)
            != Some(initializer.typed_type_tree)
    {
        return Err(
            "FormalLocalPair declaration or initializer has stale/mutable source bindings".into(),
        );
    }
    let local_bindings = tasty.typed_bindings.local_value_bindings(source_id, caller);
    let [local_binding] = local_bindings.as_slice() else {
        return Err("FormalLocalPair must retain exactly one canonical local binding".into());
    };
    if local_binding.source_tree != initializer.source_declaration
        || local_binding.typed_declaration != initializer.typed_declaration
        || local_binding.symbol != initializer.local
        || local_binding.scope != initializer.scope
        || !tasty.store.symbols.contains(initializer.local)
        || tasty.store.symbols.get(initializer.local).kind != SymbolKind::Local
        || tasty.store.symbols.get(initializer.local).owner != Some(caller)
        || tasty.store.symbols.get(initializer.local).origin != SymbolOrigin::Source(source_id)
        || !tasty.store.symbols.get(initializer.local).flags.is_empty()
        || !matches!(tasty.store.symbols.get(initializer.local).info, SymbolInfo::Complete(ty) if ty == initializer.local_type)
        || !matches!(tasty.store.types.try_get(initializer.local_type), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == initializer.call.class)
        || tasty.typed_ast.get(*typed_declaration).ty != initializer.local_type
    {
        return Err(
            "FormalLocalPair local symbol/type/scope is not canonical for this method".into(),
        );
    }
    let Some(TreeKind::Apply(source_outer)) = tasty
        .source_ast
        .try_get(source_block.expr)
        .map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair source Block result is not a constructor Apply".into());
    };
    let Some(TreeKind::Apply(typed_outer)) = tasty
        .typed_ast
        .try_get(typed_block.expr)
        .map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair typed Block result is not a constructor Apply".into());
    };
    if source_outer.kind != dotty_core::ast::ApplyKind::Regular
        || typed_outer.kind != dotty_core::ast::ApplyKind::Regular
        || outer_application.kind != dotty_core::ast::ApplyKind::Regular
        || source_outer.args.as_slice() != pair.arguments.map(|argument| argument.source_argument)
        || typed_outer.args.as_slice() != pair.arguments.map(|argument| argument.typed_argument)
        || outer_application.args.as_slice()
            != pair.arguments.map(|argument| argument.typed_argument)
        || !matches!(
            tasty
                .source_ast
                .try_get(source_outer.function)
                .map(|tree| &tree.kind),
            Some(TreeKind::Select(_))
        )
        || !matches!(
            tasty
                .typed_ast
                .try_get(typed_outer.function)
                .map(|tree| &tree.kind),
            Some(TreeKind::Select(_))
        )
        || !matches!(
            tasty.source_ast.try_get(source_outer.function).map(|tree| &tree.kind),
            Some(TreeKind::Select(selection))
                if matches!(tasty.source_ast.try_get(selection.qualifier).map(|tree| &tree.kind), Some(TreeKind::New(_)))
        )
        || !matches!(
            tasty.typed_ast.try_get(typed_outer.function).map(|tree| &tree.kind),
            Some(TreeKind::Select(selection))
                if matches!(tasty.typed_ast.try_get(selection.qualifier).map(|tree| &tree.kind), Some(TreeKind::New(_)))
        )
        || !matches!(
            tasty.typed_ast.try_get(typed_outer.function).and_then(|tree| tasty.store.types.try_get(tree.ty)),
            Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                if *symbol == outer_constructor_symbol
        )
        || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(typed_rhs).ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == outer_metadata.identity().symbol())
        || tasty.store.symbols.get(outer_constructor_symbol).kind != SymbolKind::Constructor
        || tasty.store.symbols.get(outer_constructor_symbol).owner
            != Some(outer_metadata.identity().symbol())
    {
        return Err(
            "FormalLocalPair outer constructor arguments differ from the ordered original call"
                .into(),
        );
    }
    let method = proof
        .metadata
        .methods()
        .iter()
        .find(|method| method.symbol() == caller)
        .ok_or_else(|| "FormalLocalPair caller is absent from the JVM plan".to_owned())?;
    if pair.method_formals[0] == pair.method_formals[1]
        || pair.source_method_formals[0] == pair.source_method_formals[1]
        || method.parameters().len() != 2
        || method.parameters()[0].symbol() != pair.method_formals[0]
        || method.parameters()[1].symbol() != pair.method_formals[1]
        || method.parameters()[0].slot() != 1
        || method.parameters()[1].slot() != 2
        || tasty
            .source_index
            .symbol_at(source_id, pair.source_method_formals[0])
            != Some(pair.method_formals[0])
        || tasty
            .source_index
            .symbol_at(source_id, pair.source_method_formals[1])
            != Some(pair.method_formals[1])
        || pair.method_formals.iter().any(|formal| {
            !tasty.store.symbols.contains(*formal)
                || tasty.store.symbols.get(*formal).kind != SymbolKind::Parameter
                || tasty.store.symbols.get(*formal).owner != Some(caller)
                || tasty.store.symbols.get(*formal).origin != SymbolOrigin::Source(source_id)
        })
        || !matches!(tasty.store.symbols.get(pair.method_formals[0]).info, SymbolInfo::Complete(ty) if matches!(tasty.store.types.try_get(ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == initializer.call.class))
        || !matches!(tasty.store.symbols.get(pair.method_formals[1]).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
        || method.descriptor().as_str()
            != format!(
                "(L{};I)L{};",
                all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == initializer.call.class)
                    .ok_or_else(|| "FormalLocalPair Counter class is outside the batch".to_owned())?
                    .identity()
                    .internal_name(),
                outer_metadata.identity().internal_name()
            )
    {
        return Err("FormalLocalPair method formals do not preserve the source JVM ABI".into());
    }
    let outer_constructor = outer_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == outer_constructor_symbol)
        .ok_or_else(|| "FormalLocalPair Pair constructor is absent from the JVM plan".to_owned())?;
    if outer_constructor.parameters().len() != 2
        || outer_constructor.descriptor().as_str()
            != format!(
                "(L{};L{};)V",
                all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == initializer.call.class)
                    .ok_or_else(|| "FormalLocalPair Counter class is outside the batch".to_owned())?
                    .identity()
                    .internal_name(),
                all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == initializer.call.class)
                    .ok_or_else(|| "FormalLocalPair Counter class is outside the batch".to_owned())?
                    .identity()
                    .internal_name()
            )
    {
        return Err(
            "FormalLocalPair Pair constructor descriptor is not the exact Counter pair ABI".into(),
        );
    }
    let caller_plan = proof
        .jvm_unit
        .classes
        .iter()
        .find(|class| class.class == tasty.class)
        .and_then(|class| class.methods.iter().find(|method| method.method == caller))
        .ok_or_else(|| "FormalLocalPair caller has no JVM method plan".to_owned())?;
    let layout = proof
        .jvm_unit
        .local_layout_for(method, &caller_plan.locals, tasty.definitions)
        .map_err(|error| format!("cannot derive FormalLocalPair local layout: {error:?}"))?;
    let local_slot = layout
        .local(initializer.local)
        .map_err(|error| format!("FormalLocalPair local has no checked slot: {error:?}"))?;
    if local_slot.declaration() != initializer.typed_declaration
        || local_slot.is_mutable()
        || local_slot.ty() != dotty_jvm::JvmType::Reference(initializer.call.class)
    {
        return Err(
            "FormalLocalPair local slot differs from the exact immutable Counter declaration"
                .into(),
        );
    }
    let mut arguments = Vec::with_capacity(2);
    let mut formal_count = 0;
    let mut local_count = 0;
    for (index, argument) in pair.arguments.iter().enumerate() {
        if tasty
            .typed_bindings
            .typed_at(source_id, argument.source_argument)
            != Some(argument.typed_argument)
        {
            return Err(format!(
                "FormalLocalPair argument {index} has a stale source-to-typed edge"
            ));
        }
        let source_argument = tasty
            .source_ast
            .try_get(argument.source_argument)
            .ok_or_else(|| format!("FormalLocalPair source argument {index} is stale"))?;
        let typed_argument = tasty
            .typed_ast
            .try_get(argument.typed_argument)
            .ok_or_else(|| format!("FormalLocalPair typed argument {index} is stale"))?;
        match argument.origin {
            FormalLocalConstructorCallArgumentOrigin::Formal {
                formal,
                result_type,
                ..
            } => {
                let Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                }) = tasty.store.types.try_get(typed_argument.ty)
                else {
                    return Err(format!(
                        "FormalLocalPair formal argument {index} is not a canonical TermRef"
                    ));
                };
                let Some(formal_index) = pair
                    .method_formals
                    .iter()
                    .position(|candidate| *candidate == formal)
                else {
                    return Err(format!(
                        "FormalLocalPair formal argument {index} is not an original method formal"
                    ));
                };
                let Some(TreeKind::Ident(source_ident)) = Some(&source_argument.kind) else {
                    return Err(format!(
                        "FormalLocalPair formal source argument {index} is not an Ident"
                    ));
                };
                let Some(TreeKind::ValDef(source_formal)) = tasty
                    .source_ast
                    .try_get(pair.source_method_formals[formal_index])
                    .map(|tree| &tree.kind)
                else {
                    return Err(format!(
                        "FormalLocalPair source formal {formal_index} is not a ValDef"
                    ));
                };
                if *prefix != tasty.definitions.no_prefix
                    || *target != formal
                    || typed_argument.ty != result_type
                    || !matches!(source_argument.kind, TreeKind::Ident(_))
                    || !matches!(typed_argument.kind, TreeKind::Ident(_))
                    || source_ident.name != *source_formal.name.as_name()
                    || formal_index != 0
                    || outer_constructor.parameters()[index].ty()
                        != dotty_jvm::JvmType::Reference(initializer.call.class)
                {
                    return Err(format!(
                        "FormalLocalPair formal argument {index} is not the original first formal"
                    ));
                }
                formal_count += 1;
                arguments.push(MixedConstructorArgumentRef::Formal {
                    local_slot: method.parameters()[0].slot(),
                });
            }
            FormalLocalConstructorCallArgumentOrigin::LocalValue {
                symbol,
                result_type,
            } => {
                let Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                }) = tasty.store.types.try_get(typed_argument.ty)
                else {
                    return Err(format!(
                        "FormalLocalPair local argument {index} is not a canonical TermRef"
                    ));
                };
                if symbol != initializer.local
                    || *prefix != tasty.definitions.no_prefix
                    || *target != symbol
                    || typed_argument.ty != result_type
                    || !matches!(source_argument.kind, TreeKind::Ident(_))
                    || !matches!(typed_argument.kind, TreeKind::Ident(_))
                    || !matches!(&source_argument.kind, TreeKind::Ident(ident) if ident.name == *source_val.name.as_name())
                    || outer_constructor.parameters()[index].ty()
                        != dotty_jvm::JvmType::Reference(initializer.call.class)
                {
                    return Err(format!(
                        "FormalLocalPair local argument {index} does not name the exact initialized local"
                    ));
                }
                local_count += 1;
                arguments.push(MixedConstructorArgumentRef::LocalValue {
                    local_slot: local_slot.slot(),
                });
            }
        }
    }
    if formal_count != 1
        || local_count != 1
        || arguments.len() != 2
        || initializer.call.formal_type != tasty.definitions.int
    {
        return Err(
            "FormalLocalPair must contain one formal and one initialized local argument".into(),
        );
    }
    let Some(TreeKind::Apply(source_initializer)) = tasty
        .source_ast
        .try_get(initializer.source_initializer)
        .map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair local initializer is not a source Apply".into());
    };
    if source_initializer.kind != dotty_core::ast::ApplyKind::Regular
        || source_initializer.args.as_slice() != [initializer.call.source_argument]
    {
        return Err(
            "FormalLocalPair initializer is not the direct one-argument Counter constructor call"
                .into(),
        );
    }
    let Some(TreeKind::Apply(typed_initializer)) = tasty
        .typed_ast
        .try_get(initializer.call.typed_apply)
        .map(|tree| &tree.kind)
    else {
        return Err("FormalLocalPair typed initializer is not an Apply".into());
    };
    if typed_initializer.kind != dotty_core::ast::ApplyKind::Regular
        || typed_initializer.function != initializer.call.typed_select
        || typed_initializer.args.as_slice() != [initializer.call.typed_argument]
        || source_initializer.function != initializer.call.source_select
        || !matches!(tasty.typed_ast.try_get(initializer.call.typed_select).map(|tree| &tree.kind), Some(TreeKind::Select(selection)) if selection.qualifier == initializer.call.typed_new)
        || !matches!(
            tasty
                .typed_ast
                .try_get(initializer.call.typed_new)
                .map(|tree| &tree.kind),
            Some(TreeKind::New(_))
        )
        || !matches!(
            tasty
                .typed_ast
                .try_get(initializer.call.typed_argument)
                .map(|tree| &tree.kind),
            Some(TreeKind::Ident(_))
        )
        || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(initializer.call.typed_argument).ty), Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }) if *prefix == tasty.definitions.no_prefix && *symbol == pair.method_formals[1])
        || !matches!(tasty.store.types.try_get(tasty.typed_ast.get(initializer.call.typed_new).ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == initializer.call.class)
        || tasty.typed_ast.get(initializer.call.typed_apply).ty != initializer.call.result_type
    {
        return Err("FormalLocalPair Counter initializer does not bind new Counter(seed) to the original seed formal".into());
    }
    let nested_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == initializer.call.class)
        .ok_or_else(|| "FormalLocalPair Counter class is outside the batch".to_owned())?;
    let nested_constructor = nested_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == initializer.call.constructor)
        .ok_or_else(|| {
            "FormalLocalPair Counter constructor is absent from the JVM plan".to_owned()
        })?;
    if nested_constructor.descriptor().as_str() != "(I)V"
        || !nested_constructor.access_flags().is_public()
        || !tasty.store.symbols.contains(initializer.call.formal)
        || tasty.store.symbols.get(initializer.call.formal).kind != SymbolKind::Parameter
        || tasty.store.symbols.get(initializer.call.formal).owner
            != Some(initializer.call.constructor)
        || tasty.store.symbols.get(initializer.call.formal).origin
            != SymbolOrigin::Source(source_id)
        || !matches!(tasty.store.symbols.get(initializer.call.formal).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
        || !nested_constructor.parameters().iter().any(|parameter| {
            parameter.symbol() == initializer.call.formal
                && parameter.ty() == dotty_jvm::JvmType::Int
        })
        || tasty
            .typed_bindings
            .typed_at(source_id, initializer.call.source_select)
            != Some(initializer.call.typed_select)
        || tasty
            .typed_bindings
            .typed_at(source_id, initializer.call.source_new)
            != Some(initializer.call.typed_new)
        || tasty
            .typed_bindings
            .typed_at(source_id, initializer.call.source_argument)
            != Some(initializer.call.typed_argument)
        || !matches!(
            tasty
                .source_ast
                .try_get(initializer.call.source_new)
                .map(|tree| &tree.kind),
            Some(TreeKind::New(_))
        )
        || !matches!(tasty.source_ast.try_get(initializer.call.source_select).map(|tree| &tree.kind), Some(TreeKind::Select(selection)) if selection.qualifier == initializer.call.source_new)
        || !matches!(tasty.typed_ast.try_get(initializer.call.typed_select).and_then(|tree| tasty.store.types.try_get(tree.ty)), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == initializer.call.constructor)
    {
        return Err("FormalLocalPair initializer differs from its canonical Counter(seed) constructor receipt".into());
    }
    Ok(MixedConstructorPairRef {
        arguments: arguments
            .try_into()
            .map_err(|_| "FormalLocalPair argument receipt is incomplete")?,
        local_initializers: vec![LocalCounterInitializerRef {
            owner_internal_name: nested_metadata.identity().internal_name().to_owned(),
            descriptor: nested_constructor.descriptor().as_str().to_owned(),
            argument_local_slot: method.parameters()[1].slot(),
            result_local_slot: local_slot.slot(),
        }],
        local_alias_copy: None,
        local_receiver_call: None,
        local_receiver_call_pair: None,
        conditional_selection: None,
        max_stack: 4,
        max_locals: layout.max_locals(),
    })
}

fn validate_mixed_constructor_pair(
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    caller: SymbolId,
    (source_rhs, typed_rhs): (TreeId<Untyped>, TreeId<Typed>),
    outer_application: &dotty_core::ast::Apply<Typed>,
    outer_constructor_symbol: SymbolId,
    outer_metadata: &JvmClassMetadata,
    all_metadata: &[JvmClassMetadata],
) -> Result<MixedConstructorPairRef, String> {
    use dotty_core::ast::TreeKind;

    let tasty = proof.tasty_plan.plan();
    let source_id = tasty.source.id;
    let binding = tasty
        .typed_bindings
        .ordered_constructor_call_argument_pair(source_id, caller)
        .ok_or_else(|| "two-argument constructor has no ordered mixed-origin receipt".to_owned())?;
    let source_outer = tasty
        .source_ast
        .try_get(source_rhs)
        .and_then(|tree| match &tree.kind {
            TreeKind::Apply(application) => Some(application),
            _ => None,
        })
        .ok_or_else(|| "mixed constructor source RHS is not an Apply".to_owned())?;
    let typed_outer = tasty
        .typed_ast
        .try_get(typed_rhs)
        .and_then(|tree| match &tree.kind {
            TreeKind::Apply(application) => Some(application),
            _ => None,
        })
        .ok_or_else(|| "mixed constructor typed RHS is not an Apply".to_owned())?;
    if source_outer.kind != dotty_core::ast::ApplyKind::Regular
        || typed_outer.kind != dotty_core::ast::ApplyKind::Regular
        || outer_application.kind != dotty_core::ast::ApplyKind::Regular
        || source_outer.args.as_slice()
            != binding.arguments.map(|argument| argument.source_argument)
        || typed_outer.args.as_slice() != binding.arguments.map(|argument| argument.typed_argument)
        || outer_application.args.as_slice()
            != binding.arguments.map(|argument| argument.typed_argument)
        || tasty.typed_bindings.typed_at(source_id, source_rhs) != Some(typed_rhs)
        || !matches!(
            tasty
                .source_ast
                .try_get(source_outer.function)
                .map(|tree| &tree.kind),
            Some(TreeKind::Select(_))
        )
        || binding.method_formals[0] == binding.method_formals[1]
        || binding.source_method_formals[0] == binding.source_method_formals[1]
        || binding.arguments[0].source_argument == binding.arguments[1].source_argument
        || binding.arguments[0].typed_argument == binding.arguments[1].typed_argument
    {
        return Err(
            "mixed constructor roots or receipt differ from the ordered source call".into(),
        );
    }

    let caller_method = proof
        .metadata
        .methods()
        .iter()
        .find(|method| method.symbol() == caller)
        .ok_or_else(|| "mixed constructor caller is absent from its JVM plan".to_owned())?;
    let source_method_tree = tasty
        .members
        .iter()
        .find_map(|member| match member {
            ClassMemberPicklePlan::Method {
                symbol,
                source_tree,
            } if *symbol == caller => Some(*source_tree),
            _ => None,
        })
        .ok_or_else(|| "mixed constructor caller is absent from its TASTy plan".to_owned())?;
    let source_method = tasty
        .source_ast
        .try_get(source_method_tree)
        .and_then(|tree| match &tree.kind {
            TreeKind::DefDef(method) => Some(method),
            _ => None,
        })
        .ok_or_else(|| "mixed constructor caller source definition is not a DefDef".to_owned())?;
    if source_method.value_param_clauses.len() != 1
        || source_method.value_param_clauses[0].as_slice() != binding.source_method_formals
        || caller_method.parameters().len() != 2
        || caller_method.parameters()[0].symbol() != binding.method_formals[0]
        || caller_method.parameters()[1].symbol() != binding.method_formals[1]
        || caller_method.parameters()[0].slot() != 1
        || caller_method.parameters()[1].slot() != 2
        || caller_method
            .parameters()
            .iter()
            .any(|parameter| parameter.width() != 1)
    {
        return Err("mixed constructor method formals do not preserve the ordered JVM ABI".into());
    }
    if binding
        .method_formals
        .iter()
        .any(|formal| !tasty.store.symbols.contains(*formal))
    {
        return Err("mixed constructor receipt contains a stale method formal".into());
    }
    for index in 0..2 {
        let source_formal = binding.source_method_formals[index];
        let formal = binding.method_formals[index];
        if tasty.source_index.symbol_at(source_id, source_formal) != Some(formal)
            || tasty.source_index.definition_of(formal)
                != Some(SourceDefinition::Canonical {
                    source: source_id,
                    tree: source_formal,
                })
            || !tasty.store.symbols.contains(formal)
            || tasty.store.symbols.get(formal).origin != SymbolOrigin::Source(source_id)
        {
            return Err(format!(
                "mixed method formal {index} does not retain its canonical source identity"
            ));
        }
    }
    let typed_body_type = tasty
        .typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| "mixed typed RHS is stale".to_owned())?
        .ty;
    if !matches!(tasty.store.types.try_get(typed_body_type), Some(Type::TypeRef { prefix, target: TypeRefTarget::Symbol(class) }) if *prefix == tasty.definitions.no_prefix && *class == outer_metadata.identity().symbol())
    {
        return Err("mixed typed method body does not return the exact planned Pair class".into());
    }

    let outer_constructor = outer_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == outer_constructor_symbol)
        .ok_or_else(|| "mixed outer constructor is absent from its JVM plan".to_owned())?;
    if !outer_constructor.access_flags().is_public() || outer_constructor.parameters().len() != 2 {
        return Err("mixed outer constructor does not have two public parameters".into());
    }

    let counter_class = match tasty.store.symbols.get(binding.method_formals[0]).info {
        SymbolInfo::Complete(ty) => match tasty.store.types.try_get(ty) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) => *class,
            _ => {
                return Err("mixed method formal 0 is not the canonical Counter class type".into());
            }
        },
        _ => return Err("mixed method formal 0 has incomplete type information".into()),
    };
    if !matches!(tasty.store.symbols.get(binding.method_formals[1]).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
        || caller_method.descriptor().as_str()
            != format!(
                "(L{};I)L{};",
                all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == counter_class)
                    .ok_or_else(|| "mixed Counter class is outside the batch".to_owned())?
                    .identity()
                    .internal_name(),
                outer_metadata.identity().internal_name()
            )
    {
        return Err("mixed method ABI is not exactly (Counter, Int) => Pair".into());
    }
    let mut nested_count = 0;
    let mut arguments = Vec::with_capacity(2);
    for index in 0..2 {
        let argument = binding.arguments[index];
        let typed_argument = tasty
            .typed_ast
            .try_get(argument.typed_argument)
            .ok_or_else(|| format!("mixed typed argument {index} is stale"))?;
        if tasty
            .typed_bindings
            .typed_at(source_id, argument.source_argument)
            != Some(argument.typed_argument)
        {
            return Err(format!(
                "mixed argument {index} has a stale source-to-typed edge"
            ));
        }
        let (argument_class, argument_ref) = match argument.origin {
            OrderedConstructorCallArgumentOrigin::Formal {
                formal,
                formal_type,
                result_type,
            } => {
                if !tasty.store.symbols.contains(formal) {
                    return Err(format!(
                        "mixed direct argument {index} has a stale formal symbol"
                    ));
                }
                let formal_index = binding
                    .method_formals
                    .iter()
                    .position(|candidate| *candidate == formal)
                    .ok_or_else(|| {
                        format!("mixed direct argument {index} names a foreign caller formal")
                    })?;
                let source_argument = tasty
                    .source_ast
                    .try_get(argument.source_argument)
                    .ok_or_else(|| format!("mixed formal source argument {index} is stale"))?;
                let TreeKind::Ident(source_ident) = &source_argument.kind else {
                    return Err(format!(
                        "mixed formal source argument {index} is not an Ident"
                    ));
                };
                let TreeKind::Ident(_) = typed_argument.kind else {
                    return Err(format!(
                        "mixed formal typed argument {index} is not an Ident"
                    ));
                };
                let formal_info = tasty.store.symbols.get(formal);
                let source_formal = tasty
                    .source_ast
                    .try_get(binding.source_method_formals[formal_index])
                    .ok_or_else(|| format!("mixed source formal {formal_index} is stale"))?;
                let TreeKind::ValDef(source_formal) = &source_formal.kind else {
                    return Err(format!("mixed source formal {index} is not a ValDef"));
                };
                let Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                }) = tasty.store.types.try_get(typed_argument.ty)
                else {
                    return Err(format!(
                        "mixed formal typed argument {index} is not a canonical TermRef"
                    ));
                };
                let formal_class = match tasty.store.types.try_get(formal_type) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(class),
                        ..
                    }) => *class,
                    _ => return Err(format!("mixed formal type {index} is not a class TypeRef")),
                };
                if formal_index != 0
                    || *prefix != tasty.definitions.no_prefix
                    || *target != formal
                    || typed_argument.ty != result_type
                    || formal_info.kind != SymbolKind::Parameter
                    || formal_info.owner != Some(caller)
                    || formal_info.origin != SymbolOrigin::Source(source_id)
                    || !matches!(formal_info.info, SymbolInfo::Complete(ty) if ty == formal_type)
                    || formal_class != counter_class
                    || caller_method.parameters()[formal_index].ty()
                        != dotty_jvm::JvmType::Reference(formal_class)
                    || outer_constructor.parameters()[index].ty()
                        != dotty_jvm::JvmType::Reference(formal_class)
                    || tasty
                        .store
                        .symbols
                        .get(binding.method_formals[formal_index])
                        .kind
                        != SymbolKind::Parameter
                    || tasty
                        .store
                        .symbols
                        .get(binding.method_formals[formal_index])
                        .owner
                        != Some(caller)
                    || source_ident.name != *source_formal.name.as_name()
                {
                    return Err(format!(
                        "mixed direct argument {index} does not name its exact ordered caller formal"
                    ));
                }
                (
                    formal_class,
                    MixedConstructorArgumentRef::Formal {
                        local_slot: caller_method.parameters()[formal_index].slot(),
                    },
                )
            }
            OrderedConstructorCallArgumentOrigin::NestedNew {
                call,
                argument_formal,
            } => {
                if argument.source_argument != call.source_apply
                    || argument.typed_argument != call.typed_apply
                    || argument_formal != binding.method_formals[1]
                    || tasty.typed_bindings.typed_at(source_id, call.source_select)
                        != Some(call.typed_select)
                    || tasty.typed_bindings.typed_at(source_id, call.source_new)
                        != Some(call.typed_new)
                    || tasty
                        .typed_bindings
                        .typed_at(source_id, call.source_argument)
                        != Some(call.typed_argument)
                {
                    return Err(format!(
                        "mixed nested argument {index} has stale or mismatched receipt roots"
                    ));
                }
                let source_call = tasty
                    .source_ast
                    .try_get(call.source_apply)
                    .and_then(|tree| match &tree.kind {
                        TreeKind::Apply(node) => Some(node),
                        _ => None,
                    })
                    .ok_or_else(|| format!("mixed nested source call {index} is not an Apply"))?;
                let typed_call = tasty
                    .typed_ast
                    .try_get(call.typed_apply)
                    .and_then(|tree| match &tree.kind {
                        TreeKind::Apply(node) => Some(node),
                        _ => None,
                    })
                    .ok_or_else(|| format!("mixed nested typed call {index} is not an Apply"))?;
                let source_select = tasty
                    .source_ast
                    .try_get(call.source_select)
                    .and_then(|tree| match &tree.kind {
                        TreeKind::Select(node) => Some(node),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!("mixed nested source selection {index} is not a Select")
                    })?;
                let typed_select = tasty
                    .typed_ast
                    .try_get(call.typed_select)
                    .and_then(|tree| match &tree.kind {
                        TreeKind::Select(node) => Some(node),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!("mixed nested typed selection {index} is not a Select")
                    })?;
                let source_argument = tasty
                    .source_ast
                    .try_get(call.source_argument)
                    .ok_or_else(|| format!("mixed nested source leaf {index} is stale"))?;
                let typed_leaf = tasty
                    .typed_ast
                    .try_get(call.typed_argument)
                    .ok_or_else(|| format!("mixed nested typed leaf {index} is stale"))?;
                let TreeKind::Ident(_) = source_argument.kind else {
                    return Err(format!("mixed nested source leaf {index} is not an Ident"));
                };
                let Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                }) = tasty.store.types.try_get(typed_leaf.ty)
                else {
                    return Err(format!("mixed nested typed leaf {index} is not a TermRef"));
                };
                let formal = caller_method
                    .parameters()
                    .iter()
                    .find(|parameter| parameter.symbol() == *target)
                    .ok_or_else(|| {
                        format!("mixed nested leaf {index} does not name a caller formal")
                    })?;
                let nested_metadata = all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == call.class)
                    .ok_or_else(|| {
                        format!("mixed nested constructor owner {index} is outside the batch")
                    })?;
                let nested_constructor = nested_metadata
                    .constructor()
                    .filter(|constructor| constructor.symbol() == call.constructor)
                    .ok_or_else(|| {
                        format!("mixed nested constructor {index} is absent from its JVM plan")
                    })?;
                if source_call.kind != dotty_core::ast::ApplyKind::Regular
                    || typed_call.kind != dotty_core::ast::ApplyKind::Regular
                    || source_call.function != call.source_select
                    || source_call.args.as_slice() != [call.source_argument]
                    || typed_call.function != call.typed_select
                    || typed_call.args.as_slice() != [call.typed_argument]
                    || source_select.qualifier != call.source_new
                    || typed_select.qualifier != call.typed_new
                    || !matches!(
                        tasty
                            .source_ast
                            .try_get(call.source_new)
                            .map(|tree| &tree.kind),
                        Some(TreeKind::New(_))
                    )
                    || !matches!(
                        tasty
                            .typed_ast
                            .try_get(call.typed_new)
                            .map(|tree| &tree.kind),
                        Some(TreeKind::New(_))
                    )
                    || !matches!(tasty.typed_ast.try_get(call.typed_select).and_then(|tree| tasty.store.types.try_get(tree.ty)), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == call.constructor)
                    || *target != argument_formal
                    || *prefix != tasty.definitions.no_prefix
                    || call.formal_type != tasty.definitions.int
                    || call.result_type != typed_argument.ty
                    || !matches!(tasty.store.types.try_get(call.result_type), Some(Type::TypeRef { prefix, target: TypeRefTarget::Symbol(class) }) if *prefix == tasty.definitions.no_prefix && *class == call.class)
                    || nested_constructor.descriptor().as_str() != "(I)V"
                    || !nested_constructor.access_flags().is_public()
                    || !tasty.store.symbols.contains(call.formal)
                    || !nested_constructor
                        .parameters()
                        .iter()
                        .any(|parameter| parameter.symbol() == call.formal)
                    || tasty.store.symbols.get(call.formal).owner != Some(call.constructor)
                    || tasty.store.symbols.get(call.formal).kind != SymbolKind::Parameter
                    || tasty.store.symbols.get(call.formal).origin
                        != SymbolOrigin::Source(source_id)
                    || !matches!(tasty.store.symbols.get(call.formal).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
                    || !tasty.store.symbols.contains(call.constructor)
                    || tasty.store.symbols.get(call.constructor).kind != SymbolKind::Constructor
                    || tasty.store.symbols.get(call.constructor).owner != Some(call.class)
                    || tasty.store.symbols.get(call.constructor).origin
                        != SymbolOrigin::Source(source_id)
                    || !tasty.store.symbols.contains(argument_formal)
                    || tasty.store.symbols.get(argument_formal).kind != SymbolKind::Parameter
                    || tasty.store.symbols.get(argument_formal).owner != Some(caller)
                    || tasty.store.symbols.get(argument_formal).origin
                        != SymbolOrigin::Source(source_id)
                    || !matches!(tasty.store.symbols.get(argument_formal).info, SymbolInfo::Complete(ty) if ty == tasty.definitions.int)
                    || formal.width() != 1
                    || outer_constructor.parameters()[index].ty()
                        != dotty_jvm::JvmType::Reference(call.class)
                    || call.class != counter_class
                {
                    return Err(format!(
                        "mixed nested argument {index} differs from its source-backed constructor receipt"
                    ));
                }
                nested_count += 1;
                (
                    call.class,
                    MixedConstructorArgumentRef::Nested(NestedConstructorMethodRef {
                        owner_internal_name: nested_metadata.identity().internal_name().to_owned(),
                        descriptor: nested_constructor.descriptor().as_str().to_owned(),
                        argument_local_slot: formal.slot(),
                    }),
                )
            }
        };
        if argument_class != counter_class {
            return Err("mixed arguments do not share the exact canonical reference class".into());
        }
        if outer_constructor.parameters()[index].ty()
            != dotty_jvm::JvmType::Reference(argument_class)
        {
            return Err(format!(
                "mixed Pair constructor formal {index} has the wrong reference class"
            ));
        }
        arguments.push(argument_ref);
    }
    if nested_count != 1 {
        return Err(
            "mixed constructor receipt must contain exactly one Formal and one NestedNew argument"
                .into(),
        );
    }
    let [first, second] = arguments
        .try_into()
        .map_err(|_| "mixed constructor receipt is incomplete")?;
    let counter_name = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == counter_class)
        .ok_or_else(|| "mixed Counter class is outside the batch".to_owned())?
        .identity()
        .internal_name();
    if outer_constructor.descriptor().as_str() != format!("(L{counter_name};L{counter_name};)V") {
        return Err("mixed caller and selected Pair constructor descriptors do not match the canonical Counter/Int => Pair ABI".into());
    }
    let max_stack = if matches!(
        binding.arguments[0].origin,
        OrderedConstructorCallArgumentOrigin::Formal { .. }
    ) {
        6
    } else {
        5
    };
    Ok(MixedConstructorPairRef {
        arguments: [first, second],
        local_initializers: Vec::new(),
        local_alias_copy: None,
        local_receiver_call: None,
        local_receiver_call_pair: None,
        conditional_selection: None,
        max_stack,
        max_locals: 3,
    })
}

fn validate_nested_constructor_pair(
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    caller: SymbolId,
    (source_rhs, typed_rhs): (TreeId<Untyped>, TreeId<Typed>),
    outer_application: &dotty_core::ast::Apply<Typed>,
    outer_constructor_symbol: SymbolId,
    outer_metadata: &JvmClassMetadata,
    all_metadata: &[JvmClassMetadata],
) -> Result<[NestedConstructorMethodRef; 2], String> {
    let tasty = proof.tasty_plan.plan();
    let source_id = tasty.source.id;
    let pair = tasty
        .typed_bindings
        .nested_constructor_call_pair(source_id, caller)
        .ok_or_else(|| "two-argument constructor has no canonical paired receipt".to_owned())?;
    let source_rhs_tree = tasty
        .source_ast
        .try_get(source_rhs)
        .ok_or_else(|| "paired constructor source RHS is stale".to_owned())?;
    let TreeKind::Apply(source_outer) = &source_rhs_tree.kind else {
        return Err("paired constructor source RHS is not an Apply".into());
    };
    let typed_rhs_tree = tasty
        .typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| "paired constructor typed RHS is stale".to_owned())?;
    let TreeKind::Apply(typed_outer) = &typed_rhs_tree.kind else {
        return Err("paired constructor typed RHS is not an Apply".into());
    };
    if source_outer.kind != dotty_core::ast::ApplyKind::Regular
        || typed_outer.kind != dotty_core::ast::ApplyKind::Regular
        || outer_application.kind != dotty_core::ast::ApplyKind::Regular
    {
        return Err("paired constructor call uses a non-regular Apply".into());
    }
    if source_outer.args.as_slice() != pair.calls.map(|call| call.source_apply) {
        return Err(
            "paired constructor arguments differ from the original source call roots".into(),
        );
    }
    if typed_outer.args.as_slice() != pair.calls.map(|call| call.typed_apply)
        || outer_application.args.as_slice() != pair.calls.map(|call| call.typed_apply)
    {
        return Err(
            "paired constructor arguments differ from the original typed call roots".into(),
        );
    }
    if !matches!(
        tasty
            .source_ast
            .try_get(source_outer.function)
            .map(|tree| &tree.kind),
        Some(TreeKind::Select(_))
    ) {
        return Err("paired constructor source function is not a Select".into());
    }

    let caller_method = proof
        .metadata
        .methods()
        .iter()
        .find(|method| method.symbol() == caller)
        .ok_or_else(|| "paired constructor caller is absent from its JVM plan".to_owned())?;
    if caller_method.descriptor().as_str()
        != format!("(II)L{};", outer_metadata.identity().internal_name())
        || caller_method.parameters().len() != 2
        || caller_method.parameters()[0].slot() != 1
        || caller_method.parameters()[1].slot() != 2
        || caller_method
            .parameters()
            .iter()
            .any(|parameter| parameter.width() != 1)
    {
        return Err("paired constructor caller must have two ordered Int locals".into());
    }
    let outer_constructor = outer_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == outer_constructor_symbol)
        .ok_or_else(|| "paired outer constructor is absent from its JVM plan".to_owned())?;
    if !outer_constructor.access_flags().is_public() {
        return Err("paired outer constructor is not public".into());
    }

    let mut nested_proofs = Vec::with_capacity(2);
    for index in 0..2 {
        let binding = pair.calls[index];
        if pair.calls[0].source_apply == pair.calls[1].source_apply
            || pair.calls[0].typed_apply == pair.calls[1].typed_apply
        {
            return Err("paired constructor receipts do not retain distinct sibling calls".into());
        }
        let source_apply = tasty
            .source_ast
            .try_get(binding.source_apply)
            .ok_or_else(|| format!("nested source call {index} is stale"))?;
        let TreeKind::Apply(source_call) = &source_apply.kind else {
            return Err(format!("nested source call {index} is not an Apply"));
        };
        let typed_apply = tasty
            .typed_ast
            .try_get(binding.typed_apply)
            .ok_or_else(|| format!("nested typed call {index} is stale"))?;
        let TreeKind::Apply(typed_call) = &typed_apply.kind else {
            return Err(format!("nested typed call {index} is not an Apply"));
        };
        let source_select = tasty
            .source_ast
            .try_get(binding.source_select)
            .ok_or_else(|| format!("nested source selection {index} is stale"))?;
        let TreeKind::Select(source_selection) = &source_select.kind else {
            return Err(format!("nested source selection {index} is not a Select"));
        };
        let typed_select = tasty
            .typed_ast
            .try_get(binding.typed_select)
            .ok_or_else(|| format!("nested typed selection {index} is stale"))?;
        let TreeKind::Select(typed_selection) = &typed_select.kind else {
            return Err(format!("nested typed selection {index} is not a Select"));
        };
        let source_new = tasty
            .source_ast
            .try_get(binding.source_new)
            .ok_or_else(|| format!("nested source allocation {index} is stale"))?;
        let typed_new = tasty
            .typed_ast
            .try_get(binding.typed_new)
            .ok_or_else(|| format!("nested typed allocation {index} is stale"))?;
        let source_argument = tasty
            .source_ast
            .try_get(binding.source_argument)
            .ok_or_else(|| format!("nested source leaf {index} is stale"))?;
        let typed_argument = tasty
            .typed_ast
            .try_get(binding.typed_argument)
            .ok_or_else(|| format!("nested typed leaf {index} is stale"))?;
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(nested_constructor_symbol),
            ..
        }) = tasty.store.types.try_get(typed_select.ty)
        else {
            return Err(format!(
                "nested typed selection {index} has no constructor target"
            ));
        };
        if !tasty.store.symbols.contains(*nested_constructor_symbol) {
            return Err(format!("nested constructor symbol {index} is stale"));
        }
        let nested_info = tasty.store.symbols.get(*nested_constructor_symbol);
        let Some(nested_owner) = nested_info.owner else {
            return Err(format!("nested constructor {index} has no owner"));
        };
        let nested_metadata = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == nested_owner)
            .ok_or_else(|| format!("nested constructor owner {index} is outside the batch"))?;
        let nested_constructor = nested_metadata
            .constructor()
            .filter(|constructor| constructor.symbol() == *nested_constructor_symbol)
            .ok_or_else(|| format!("nested constructor {index} is absent from its JVM plan"))?;
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(argument_formal),
            ..
        }) = tasty.store.types.try_get(typed_argument.ty)
        else {
            return Err(format!(
                "nested typed leaf {index} is not a formal reference"
            ));
        };
        let parameter = caller_method
            .parameters()
            .iter()
            .find(|parameter| parameter.symbol() == *argument_formal)
            .ok_or_else(|| format!("nested leaf {index} does not name a caller formal"))?;
        if !tasty.store.symbols.contains(*argument_formal)
            || !tasty.store.symbols.contains(binding.constructor)
        {
            return Err(format!(
                "nested constructor receipt {index} contains a stale symbol"
            ));
        }
        let formal_info = tasty.store.symbols.get(*argument_formal);
        let constructor_info = tasty.store.symbols.get(binding.constructor);
        if *argument_formal != pair.argument_formals[index]
            || !matches!(
                tasty.store.types.try_get(binding.result_type),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(class),
                    ..
                }) if *class == nested_owner
            )
            || constructor_info.owner != Some(nested_owner)
            || binding.class != nested_owner
            || binding.constructor != *nested_constructor_symbol
            || nested_info.kind != SymbolKind::Constructor
            || nested_info.origin != SymbolOrigin::Source(source_id)
            || formal_info.kind != SymbolKind::Parameter
            || formal_info.owner != Some(caller)
            || !nested_constructor
                .parameters()
                .iter()
                .any(|parameter| parameter.symbol() == binding.formal)
        {
            return Err(format!(
                "nested constructor receipt {index} has mismatched source symbols"
            ));
        }
        if binding.formal_type != tasty.definitions.int || binding.result_type != typed_apply.ty {
            return Err(format!(
                "nested constructor receipt {index} has mismatched semantic types (formal={}, result/apply={})",
                binding.formal_type == tasty.definitions.int,
                binding.result_type == typed_apply.ty
            ));
        }
        if source_call.kind != dotty_core::ast::ApplyKind::Regular
            || typed_call.kind != dotty_core::ast::ApplyKind::Regular
            || source_call.function != binding.source_select
            || source_call.args.as_slice() != [binding.source_argument]
            || typed_call.function != binding.typed_select
            || typed_call.args.as_slice() != [binding.typed_argument]
        {
            return Err(format!(
                "nested constructor receipt {index} has mismatched Apply shape"
            ));
        }
        if source_selection.qualifier != binding.source_new
            || typed_selection.qualifier != binding.typed_new
            || !matches!(source_new.kind, TreeKind::New(_))
            || !matches!(typed_new.kind, TreeKind::New(_))
            || !matches!(source_argument.kind, TreeKind::Ident(_))
        {
            return Err(format!(
                "nested constructor receipt {index} has mismatched Select/New/leaf shape"
            ));
        }
        if tasty
            .typed_bindings
            .typed_at(source_id, binding.source_apply)
            != Some(binding.typed_apply)
            || tasty
                .typed_bindings
                .typed_at(source_id, binding.source_select)
                != Some(binding.typed_select)
            || tasty.typed_bindings.typed_at(source_id, binding.source_new)
                != Some(binding.typed_new)
            || tasty
                .typed_bindings
                .typed_at(source_id, binding.source_argument)
                != Some(binding.typed_argument)
        {
            return Err(format!(
                "nested constructor receipt {index} has stale source-to-typed bindings (apply={}, select={}, new={}, leaf={})",
                tasty
                    .typed_bindings
                    .typed_at(source_id, binding.source_apply)
                    == Some(binding.typed_apply),
                tasty
                    .typed_bindings
                    .typed_at(source_id, binding.source_select)
                    == Some(binding.typed_select),
                tasty.typed_bindings.typed_at(source_id, binding.source_new)
                    == Some(binding.typed_new),
                tasty
                    .typed_bindings
                    .typed_at(source_id, binding.source_argument)
                    == Some(binding.typed_argument)
            ));
        }
        if nested_constructor.descriptor().as_str() != "(I)V"
            || !nested_constructor.access_flags().is_public()
        {
            return Err(format!(
                "nested constructor receipt {index} has an unsupported JVM constructor"
            ));
        }
        nested_proofs.push(NestedConstructorMethodRef {
            owner_internal_name: nested_metadata.identity().internal_name().to_owned(),
            descriptor: nested_constructor.descriptor().as_str().to_owned(),
            argument_local_slot: parameter.slot(),
        });
        if index == 0 && nested_owner != pair.calls[1].class {
            return Err("sibling constructors do not target the same source class".into());
        }
    }
    let [first, second] = nested_proofs.as_slice() else {
        return Err("paired constructor receipt is incomplete".into());
    };
    if first.owner_internal_name != second.owner_internal_name
        || outer_constructor.descriptor().as_str()
            != format!(
                "(L{};L{};)V",
                first.owner_internal_name, second.owner_internal_name
            )
    {
        return Err(
            "paired constructor descriptors do not match the two sibling allocations".into(),
        );
    }
    nested_proofs
        .try_into()
        .map_err(|_| "paired constructor receipt is incomplete".to_owned())
}

struct FactoryMethodRef {
    owner_internal_name: String,
    name: String,
    descriptor: String,
}

fn reference_local_factory_call(
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
    caller: SymbolId,
    source_rhs: dotty_core::TreeId<dotty_core::Untyped>,
    typed_rhs: dotty_core::TreeId<dotty_core::Typed>,
    all_metadata: &[JvmClassMetadata],
) -> Result<Option<CrossClassMethodRef>, String> {
    use dotty_core::ast::{ApplyKind, TreeKind};

    let tasty = proof.tasty_plan.plan();
    let Some(TreeKind::Block(source_block)) =
        tasty.source_ast.try_get(source_rhs).map(|tree| &tree.kind)
    else {
        return Ok(None);
    };
    let Some(TreeKind::Block(typed_block)) =
        tasty.typed_ast.try_get(typed_rhs).map(|tree| &tree.kind)
    else {
        return Ok(None);
    };
    let [source_declaration] = source_block.stats.as_slice() else {
        return Ok(None);
    };
    let [typed_declaration] = typed_block.stats.as_slice() else {
        return Ok(None);
    };
    let bindings = tasty
        .typed_bindings
        .local_value_bindings(tasty.source.id, caller);
    let [binding] = bindings.as_slice() else {
        return Ok(None);
    };
    if binding.source_tree != *source_declaration || binding.typed_declaration != *typed_declaration
    {
        return Err("reference-local declaration differs from the canonical TASTy binding".into());
    }
    let Some(TreeKind::ValDef(source_val)) = tasty
        .source_ast
        .try_get(*source_declaration)
        .map(|tree| &tree.kind)
    else {
        return Err("reference-local source declaration is not a ValDef".into());
    };
    let Some(TreeKind::ValDef(typed_val)) = tasty
        .typed_ast
        .try_get(*typed_declaration)
        .map(|tree| &tree.kind)
    else {
        return Err("reference-local typed declaration is not a ValDef".into());
    };
    let Some(initializer) = source_val.rhs else {
        return Err("reference-local ValDef has no initializer".into());
    };
    let Some(typed_initializer) = typed_val.rhs else {
        return Err("reference-local typed ValDef has no initializer".into());
    };
    if tasty
        .typed_bindings
        .formal_local_constructor_call_pair(tasty.source.id, caller)
        .is_some()
    {
        return Ok(None);
    }
    let Some(call) = tasty.typed_bindings.planned_call(tasty.source.id, caller) else {
        return Err("reference-local initializer has no planned helper receipt".into());
    };
    if call.source_apply != initializer
        || call.typed_apply != typed_initializer
        || tasty.typed_bindings.typed_at(tasty.source.id, initializer) != Some(typed_initializer)
    {
        return Err("reference-local initializer differs from its planned helper receipt".into());
    }
    let Some(TreeKind::Apply(outer_source)) = tasty
        .source_ast
        .try_get(source_block.expr)
        .map(|tree| &tree.kind)
    else {
        return Err("reference-local final expression is not a constructor Apply".into());
    };
    let Some(TreeKind::Apply(outer_typed)) = tasty
        .typed_ast
        .try_get(typed_block.expr)
        .map(|tree| &tree.kind)
    else {
        return Err("reference-local typed final expression is not a constructor Apply".into());
    };
    if outer_source.kind != ApplyKind::Regular
        || outer_typed.kind != ApplyKind::Regular
        || outer_source.args.len() != 1
        || outer_typed.args.len() != 1
    {
        return Err("reference-local constructor must have one regular argument".into());
    }
    let Some(TreeKind::Ident(source_use)) = tasty
        .source_ast
        .try_get(
            *outer_source
                .args
                .first()
                .ok_or_else(|| "reference-local constructor has no local argument".to_owned())?,
        )
        .map(|tree| &tree.kind)
    else {
        return Err("reference-local constructor argument is not an Ident".into());
    };
    let source_use_name_matches = source_use.name == *source_val.name.as_name();
    let source_typed_matches = tasty
        .typed_bindings
        .typed_at(tasty.source.id, outer_source.args[0])
        == Some(outer_typed.args[0]);
    let typed_local_matches = matches!(
        tasty
            .typed_ast
            .try_get(outer_typed.args[0])
            .map(|tree| &tree.kind),
        Some(TreeKind::Ident(_))
    );
    let typed_symbol_matches = matches!(tasty.store.types.try_get(tasty.typed_ast.get(outer_typed.args[0]).ty), Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }) if *prefix == tasty.definitions.no_prefix && *symbol == binding.symbol);
    if !source_use_name_matches
        || !source_typed_matches
        || !typed_local_matches
        || !typed_symbol_matches
    {
        return Err(format!(
            "reference-local use identity mismatch: name={source_use_name_matches}, source-typed={source_typed_matches}, typed-ident={typed_local_matches}, typed-symbol={typed_symbol_matches}"
        ));
    }
    let Some(TreeKind::Select(outer_function)) = tasty
        .typed_ast
        .try_get(outer_typed.function)
        .map(|tree| &tree.kind)
    else {
        return Err("reference-local constructor function is not Select".into());
    };
    let Some(Type::TermRef {
        target: TermRefTarget::Symbol(target_constructor),
        ..
    }) = tasty
        .typed_ast
        .try_get(outer_typed.function)
        .and_then(|tree| tasty.store.types.try_get(tree.ty))
    else {
        return Err("reference-local constructor has no canonical constructor target".into());
    };
    if !matches!(
        tasty
            .typed_ast
            .try_get(outer_function.qualifier)
            .map(|tree| &tree.kind),
        Some(TreeKind::New(_))
    ) {
        return Err("reference-local constructor receiver is not New".into());
    }
    let target_owner = tasty
        .store
        .symbols
        .get(*target_constructor)
        .owner
        .ok_or_else(|| "reference-local constructor has no owner".to_owned())?;
    let target_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == target_owner)
        .ok_or_else(|| "reference-local target is absent from the JVM batch".to_owned())?;
    let target_constructor_metadata = target_metadata
        .constructor()
        .filter(|metadata| metadata.symbol() == *target_constructor)
        .ok_or_else(|| "reference-local constructor is absent from its JVM plan".to_owned())?;
    let caller_plan = proof
        .jvm_unit
        .classes
        .iter()
        .find(|class| class.class == tasty.class)
        .and_then(|class| class.methods.iter().find(|method| method.method == caller))
        .ok_or_else(|| "reference-local caller has no JVM method plan".to_owned())?;
    let caller_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == tasty.class)
        .and_then(|metadata| {
            metadata
                .methods()
                .iter()
                .find(|method| method.symbol() == caller)
        })
        .ok_or_else(|| "reference-local caller has no checked JVM metadata".to_owned())?;
    let layout = proof
        .jvm_unit
        .local_layout_for(caller_metadata, &caller_plan.locals, tasty.definitions)
        .map_err(|error| format!("cannot derive reference-local slot layout: {error:?}"))?;
    let local_slot = layout
        .local(binding.symbol)
        .map_err(|error| format!("reference-local symbol has no checked slot: {error:?}"))?;
    if local_slot.declaration() != binding.typed_declaration || local_slot.is_mutable() {
        return Err("reference-local slot differs from the exact immutable declaration".into());
    }
    let helper_owner = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == call.owner)
        .ok_or_else(|| "reference-local helper owner is absent from the JVM batch".to_owned())?;
    let helper = helper_owner
        .methods()
        .iter()
        .find(|metadata| metadata.symbol() == call.method)
        .ok_or_else(|| "reference-local helper is absent from its JVM plan".to_owned())?;
    let caller_descriptor =
        source_method_descriptor(tasty.store, tasty.definitions, caller, all_metadata)
            .ok_or_else(|| "reference-local caller has an unsupported JVM signature".to_owned())?;
    let constructor_descriptor = target_constructor_metadata.descriptor().as_str();
    let helper_descriptor = helper.descriptor().as_str();
    let argument_descriptor = constructor_descriptor
        .strip_prefix('(')
        .and_then(|descriptor| descriptor.split_once(')'))
        .map(|(arguments, _)| arguments)
        .ok_or_else(|| "reference-local constructor descriptor is malformed".to_owned())?;
    let local_type = match tasty.store.symbols.get(binding.symbol).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err("reference-local semantic type is incomplete".into()),
    };
    let helper_result_class = match tasty.store.types.try_get(call.result_type) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) => *class,
        _ => return Err("reference-local helper result is not a class TypeRef".into()),
    };
    let local_package = match tasty.store.types.try_get(local_type) {
        Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) if *class == helper_result_class => match tasty.store.types.try_get(*prefix) {
            Some(Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(package),
            }) if *package_prefix == tasty.definitions.no_prefix => Some(*package),
            _ => None,
        },
        _ => None,
    };
    let helper_package = match tasty.store.types.try_get(call.result_type) {
        Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) if *class == helper_result_class => match tasty.store.types.try_get(*prefix) {
            Some(Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(package),
            }) if *package_prefix == tasty.definitions.no_prefix => Some(*package),
            _ => None,
        },
        _ => None,
    };
    let local_type_matches_helper = local_package.is_some() && local_package == helper_package;
    let constructor_result_is_no_prefix = matches!(
        tasty.store.types.try_get(
            tasty
                .typed_ast
                .try_get(typed_block.expr)
                .ok_or_else(|| "reference-local constructor result is stale".to_owned())?
                .ty
        ),
        Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) if *prefix == tasty.definitions.no_prefix && *class == target_owner
    );
    if call.owner != tasty.class
        || call.method == caller
        || !local_type_matches_helper
        || !constructor_result_is_no_prefix
        || local_slot.ty() != dotty_jvm::JvmType::Reference(helper_result_class)
        || helper_descriptor != format!("(I){argument_descriptor}")
        || constructor_descriptor != format!("({argument_descriptor})V")
        || caller_descriptor != format!("(I)L{};", target_metadata.identity().internal_name())
        || tasty.store.symbols.get(*target_constructor).kind != SymbolKind::Constructor
        || tasty.store.symbols.get(*target_constructor).origin
            != SymbolOrigin::Source(tasty.source.id)
    {
        return Err(
            "reference-local helper, local TypeRef, constructor, and method ABI do not agree"
                .into(),
        );
    }
    Ok(Some(CrossClassMethodRef {
        caller,
        target: *target_constructor,
        owner_internal_name: target_metadata.identity().internal_name().to_owned(),
        name: "<init>".to_owned(),
        descriptor: constructor_descriptor.to_owned(),
        constructor: true,
        argument_load_opcode: None,
        receiver_field: None,
        nested_constructor: None,
        nested_constructor_pair: None,
        mixed_constructor_pair: None,
        direct_formal_constructor_pair: None,
        factory_method: Some(FactoryMethodRef {
            owner_internal_name: helper_owner.identity().internal_name().to_owned(),
            name: helper.name().to_owned(),
            descriptor: helper_descriptor.to_owned(),
        }),
        factory_local_slot: Some(local_slot.slot()),
        factory_local_max_locals: Some(layout.max_locals()),
    }))
}

fn validate_cross_class_call_code(
    class_file: &ClassFile<'_>,
    proof: &ValidatedJvmTastyClass<'_, '_, '_, '_>,
) -> Result<(), String> {
    // The paired pickler admits a direct same-batch zero-argument call through
    // a method parameter or an exact validated reference field, plus the
    // bounded primary-constructor application. Keep the binary check equally
    // narrow instead of accepting a differently owned or shaped invocation.
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
        if matches!(root_tree.kind, TreeKind::Block(_)) {
            if let Some(call) =
                reference_local_factory_call(proof, caller, rhs, root, &all_metadata)?
            {
                calls.push(call);
                continue;
            }
            if tasty
                .typed_bindings
                .formal_local_constructor_call_pair(tasty.source.id, caller)
                .is_some()
            {
                let TreeKind::Block(block) = &root_tree.kind else {
                    return Err(format!(
                        "method {caller:?} changed from Block after typed-root validation"
                    ));
                };
                let Some(TreeKind::Apply(application)) =
                    tasty.typed_ast.try_get(block.expr).map(|tree| &tree.kind)
                else {
                    return Err("FormalLocalPair Block result is not a constructor Apply".into());
                };
                let Some(TreeKind::Select(_)) = tasty
                    .typed_ast
                    .try_get(application.function)
                    .map(|tree| &tree.kind)
                else {
                    return Err("FormalLocalPair constructor function is not a Select".into());
                };
                let Some(Type::TermRef {
                    target: TermRefTarget::Symbol(target),
                    ..
                }) = tasty
                    .typed_ast
                    .try_get(application.function)
                    .and_then(|tree| tasty.store.types.try_get(tree.ty))
                else {
                    return Err(
                        "FormalLocalPair constructor selection has no canonical target".into(),
                    );
                };
                let target_info = tasty.store.symbols.get(*target);
                let Some(owner) = target_info.owner else {
                    return Err("FormalLocalPair constructor has no canonical owner".into());
                };
                let target_metadata = all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == owner)
                    .ok_or_else(|| "FormalLocalPair Pair class is outside the batch".to_owned())?;
                let mixed_pair = if matches!(
                    tasty.source_ast.try_get(rhs).map(|tree| &tree.kind),
                    Some(TreeKind::Block(block)) if matches!(block.stats.len(), 2..=5)
                ) {
                    validate_two_local_constructor_pair(
                        proof,
                        caller,
                        (rhs, root),
                        application,
                        *target,
                        target_metadata,
                        &all_metadata,
                    )?
                } else {
                    validate_formal_local_constructor_pair(
                        proof,
                        caller,
                        (rhs, root),
                        application,
                        *target,
                        target_metadata,
                        &all_metadata,
                    )?
                };
                let constructor = target_metadata
                    .constructor()
                    .filter(|constructor| constructor.symbol() == *target)
                    .ok_or_else(|| {
                        "FormalLocalPair Pair constructor is absent from its JVM plan".to_owned()
                    })?;
                calls.push(CrossClassMethodRef {
                    caller,
                    target: *target,
                    owner_internal_name: target_metadata.identity().internal_name().to_owned(),
                    name: "<init>".to_owned(),
                    descriptor: constructor.descriptor().as_str().to_owned(),
                    constructor: true,
                    argument_load_opcode: None,
                    receiver_field: None,
                    nested_constructor: None,
                    nested_constructor_pair: None,
                    mixed_constructor_pair: Some(mixed_pair),
                    direct_formal_constructor_pair: None,
                    factory_method: None,
                    factory_local_slot: None,
                    factory_local_max_locals: None,
                });
                continue;
            }
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
        }
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
            let constructor_receiver = match &function_tree.kind {
                TreeKind::Select(selection) => tasty
                    .typed_ast
                    .try_get(selection.qualifier)
                    .is_some_and(|receiver| matches!(receiver.kind, TreeKind::New(_))),
                _ => false,
            };
            if target_info.kind == SymbolKind::Constructor
                && constructor_receiver
                && application.kind == dotty_core::ast::ApplyKind::Regular
                && application.args.len() == 2
            {
                let mixed_constructor_pair = if tasty
                    .typed_bindings
                    .ordered_constructor_call_argument_pair(tasty.source.id, caller)
                    .is_some()
                {
                    Some(validate_mixed_constructor_pair(
                        proof,
                        caller,
                        (rhs, root),
                        application,
                        *target,
                        target_class_metadata,
                        &all_metadata,
                    )?)
                } else {
                    None
                };
                let direct_formal_constructor_pair = if mixed_constructor_pair.is_none()
                    && tasty
                        .typed_bindings
                        .direct_formal_constructor_call_pair(tasty.source.id, caller)
                        .is_some()
                {
                    Some(validate_direct_formal_constructor_pair(
                        proof,
                        caller,
                        (rhs, root),
                        application,
                        *target,
                        target_class_metadata,
                    )?)
                } else {
                    None
                };
                let nested_constructors = if mixed_constructor_pair.is_none()
                    && direct_formal_constructor_pair.is_none()
                {
                    Some(validate_nested_constructor_pair(
                        proof,
                        caller,
                        (rhs, root),
                        application,
                        *target,
                        target_class_metadata,
                        &all_metadata,
                    )?)
                } else {
                    None
                };
                let outer_constructor = target_class_metadata
                    .constructor()
                    .filter(|constructor| constructor.symbol() == *target)
                    .ok_or_else(|| "outer constructor is absent from its JVM plan".to_owned())?;
                calls.push(CrossClassMethodRef {
                    caller,
                    target: *target,
                    owner_internal_name: target_class_metadata
                        .identity()
                        .internal_name()
                        .to_owned(),
                    name: "<init>".to_owned(),
                    descriptor: outer_constructor.descriptor().as_str().to_owned(),
                    constructor: true,
                    argument_load_opcode: None,
                    receiver_field: None,
                    nested_constructor: None,
                    nested_constructor_pair: nested_constructors,
                    mixed_constructor_pair,
                    direct_formal_constructor_pair,
                    factory_method: None,
                    factory_local_slot: None,
                    factory_local_max_locals: None,
                });
                continue;
            }
            if target_info.kind == SymbolKind::Constructor
                && constructor_receiver
                && application.kind == dotty_core::ast::ApplyKind::Regular
                && application.args.len() == 1
            {
                let nested_argument = application.args[0];
                if let Some(nested_tree) = tasty.typed_ast.try_get(nested_argument)
                    && let TreeKind::Apply(nested_application) = &nested_tree.kind
                {
                    if let Some(binding) =
                        tasty.typed_bindings.planned_call(tasty.source.id, caller)
                    {
                        if nested_application.kind != dotty_core::ast::ApplyKind::Regular
                            || nested_application.args.len() != 1
                        {
                            return Err(
                                "planned factory call has an unsupported application shape".into(),
                            );
                        }
                        let source_call = tasty
                            .source_ast
                            .try_get(binding.source_apply)
                            .ok_or_else(|| "planned factory source call is stale".to_owned())?;
                        let (
                            TreeKind::Apply(source_application),
                            TreeKind::Apply(typed_application),
                        ) = (&source_call.kind, &nested_tree.kind)
                        else {
                            return Err("planned factory call does not retain an Apply".into());
                        };
                        let source_function = tasty
                            .source_ast
                            .try_get(binding.source_function)
                            .ok_or_else(|| "planned factory source function is stale".to_owned())?;
                        let source_rhs = tasty
                            .source_ast
                            .try_get(rhs)
                            .ok_or_else(|| "planned caller source body is stale".to_owned())?;
                        let source_outer_argument = matches!(
                            &source_rhs.kind,
                            TreeKind::Apply(source_outer)
                                if source_outer.kind == dotty_core::ast::ApplyKind::Regular
                                    && source_outer.args.as_slice() == [binding.source_apply]
                        );
                        let typed_function = tasty
                            .typed_ast
                            .try_get(binding.typed_function)
                            .ok_or_else(|| "planned factory function is stale".to_owned())?;
                        let Some(Type::TermRef {
                            prefix: helper_prefix,
                            target: TermRefTarget::Symbol(helper_symbol),
                        }) = tasty.store.types.try_get(typed_function.ty)
                        else {
                            return Err("planned factory function has no canonical target".into());
                        };
                        let source_receiver_valid = match (
                            &source_function.kind,
                            binding.source_receiver,
                        ) {
                            (TreeKind::Ident(_), None) => binding.typed_receiver.is_none(),
                            (TreeKind::Select(selection), Some(receiver))
                                if selection.qualifier == receiver =>
                            {
                                match (&typed_function.kind, binding.typed_receiver) {
                                    (TreeKind::Select(typed_selection), Some(typed_receiver))
                                        if typed_selection.qualifier == typed_receiver => tasty
                                            .source_ast
                                            .try_get(receiver)
                                            .is_some_and(|tree| matches!(tree.kind, TreeKind::This(_)))
                                            && tasty.typed_ast.try_get(typed_receiver).is_some_and(
                                                |tree| matches!(tree.kind, TreeKind::This(_))
                                                    && matches!(tasty.store.types.try_get(tree.ty), Some(Type::ThisType { class }) if *class == binding.owner),
                                            ),
                                    _ => false,
                                }
                            }
                            _ => false,
                        };
                        let helper_source = tasty
                            .source_ast
                            .try_get(binding.helper_source_tree)
                            .ok_or_else(|| {
                                "planned helper source definition is stale".to_owned()
                            })?;
                        let TreeKind::DefDef(helper_definition) = &helper_source.kind else {
                            return Err("planned helper has no source method definition".into());
                        };
                        let helper_record = tasty.store.symbols.get(binding.method);
                        let helper_metadata = all_metadata
                            .iter()
                            .find(|metadata| metadata.identity().symbol() == binding.owner)
                            .ok_or_else(|| {
                                "planned helper owner is absent from the JVM batch".to_owned()
                            })?;
                        let helper_method = helper_metadata
                            .methods()
                            .iter()
                            .find(|method| method.symbol() == binding.method)
                            .ok_or_else(|| {
                                "planned helper method is absent from the JVM plan".to_owned()
                            })?;
                        let helper_descriptor = source_method_descriptor(
                            tasty.store,
                            tasty.definitions,
                            binding.method,
                            &all_metadata,
                        )
                        .ok_or_else(|| {
                            "planned helper has an unsupported source signature".to_owned()
                        })?;
                        let outer_constructor = target_class_metadata
                            .constructor()
                            .filter(|constructor| constructor.symbol() == *target)
                            .ok_or_else(|| {
                                "outer constructor is absent from its JVM plan".to_owned()
                            })?;
                        let caller_descriptor = source_method_descriptor(
                            tasty.store,
                            tasty.definitions,
                            caller,
                            &all_metadata,
                        )
                        .ok_or_else(|| {
                            "planned factory caller has an unsupported signature".to_owned()
                        })?;
                        let result_class = match tasty.store.types.try_get(binding.result_type) {
                            Some(Type::TypeRef {
                                target: TypeRefTarget::Symbol(class),
                                ..
                            }) => *class,
                            _ => return Err("planned factory result is not a class type".into()),
                        };
                        let result_metadata = all_metadata
                            .iter()
                            .find(|metadata| metadata.identity().symbol() == result_class)
                            .ok_or_else(|| {
                                "planned factory result class is absent from the JVM batch"
                                    .to_owned()
                            })?;
                        let result_descriptor =
                            format!("L{};", result_metadata.identity().internal_name());
                        let outer_descriptor = outer_constructor.descriptor().as_str();
                        let outer_parameter = outer_descriptor
                            .strip_prefix('(')
                            .and_then(|descriptor| descriptor.split_once(')'))
                            .map(|(parameters, _)| parameters)
                            .ok_or_else(|| {
                                "outer constructor has an invalid descriptor".to_owned()
                            })?;
                        let expected_caller_descriptor =
                            format!("(I)L{};", target_class_metadata.identity().internal_name());
                        if binding.typed_apply != nested_argument
                            || binding.typed_function != nested_application.function
                            || binding.typed_argument != nested_application.args[0]
                            || source_application.function != binding.source_function
                            || source_application.args.as_slice() != [binding.source_argument]
                            || typed_application.function != binding.typed_function
                            || typed_application.args.as_slice() != [binding.typed_argument]
                            || source_application.kind != dotty_core::ast::ApplyKind::Regular
                            || typed_application.kind != dotty_core::ast::ApplyKind::Regular
                            || !source_outer_argument
                            || application.args.as_slice() != [binding.typed_apply]
                            || *helper_symbol != binding.method
                            || binding.owner != tasty.class
                            || binding.result_type != nested_tree.ty
                            || binding.formal_type != tasty.definitions.int
                            || !source_receiver_valid
                            || helper_record.kind != SymbolKind::Method
                            || helper_record.owner != Some(tasty.class)
                            || helper_record.origin != SymbolOrigin::Source(tasty.source.id)
                            || helper_record.name != *helper_definition.name.as_name()
                            || !helper_method.access_flags().is_public()
                            || helper_method.descriptor().as_str() != helper_descriptor
                            || helper_descriptor != format!("(I){result_descriptor}")
                            || outer_parameter != result_descriptor
                            || caller_descriptor != expected_caller_descriptor
                            || helper_definition.rhs != Some(binding.helper_source_rhs)
                            || tasty.source_index.definition_of(binding.method)
                                != Some(SourceDefinition::Canonical {
                                    source: tasty.source.id,
                                    tree: binding.helper_source_tree,
                                })
                            || !tasty.members.iter().any(|member| {
                                matches!(member,
                                    ClassMemberPicklePlan::Method { symbol, source_tree }
                                        if *symbol == binding.method
                                            && *source_tree == binding.helper_source_tree)
                            })
                            || tasty
                                .typed_bindings
                                .typed_at(tasty.source.id, binding.source_apply)
                                != Some(binding.typed_apply)
                            || tasty
                                .typed_bindings
                                .typed_at(tasty.source.id, binding.source_function)
                                != Some(binding.typed_function)
                            || tasty
                                .typed_bindings
                                .typed_at(tasty.source.id, binding.source_argument)
                                != Some(binding.typed_argument)
                            || tasty
                                .typed_bindings
                                .typed_at(tasty.source.id, binding.helper_source_rhs)
                                != Some(binding.helper_typed_rhs)
                            || !matches!(tasty.store.types.try_get(*helper_prefix), Some(Type::ThisType { class }) if *class == tasty.class)
                        {
                            return Err(
                                "planned factory call differs from its source/JVM receipt".into()
                            );
                        }
                        calls.push(CrossClassMethodRef {
                            caller,
                            target: *target,
                            owner_internal_name: target_class_metadata
                                .identity()
                                .internal_name()
                                .to_owned(),
                            name: "<init>".to_owned(),
                            descriptor: outer_descriptor.to_owned(),
                            constructor: true,
                            argument_load_opcode: None,
                            receiver_field: None,
                            nested_constructor: None,
                            nested_constructor_pair: None,
                            mixed_constructor_pair: None,
                            direct_formal_constructor_pair: None,
                            factory_method: Some(FactoryMethodRef {
                                owner_internal_name: helper_metadata
                                    .identity()
                                    .internal_name()
                                    .to_owned(),
                                name: helper_method.name().to_owned(),
                                descriptor: helper_descriptor,
                            }),
                            factory_local_slot: None,
                            factory_local_max_locals: None,
                        });
                        continue;
                    }
                    if nested_application.kind != dotty_core::ast::ApplyKind::Regular
                        || nested_application.args.len() != 1
                    {
                        return Err(format!(
                            "cross-class constructor {target:?} has an unsupported nested argument shape"
                        ));
                    }
                    let nested_function = tasty
                        .typed_ast
                        .try_get(nested_application.function)
                        .ok_or_else(|| "nested constructor selection is stale".to_owned())?;
                    let TreeKind::Select(nested_selection) = &nested_function.kind else {
                        return Err("nested constructor function is not a Select".into());
                    };
                    let Some(Type::TermRef {
                        target: TermRefTarget::Symbol(nested_target),
                        ..
                    }) = tasty.store.types.try_get(nested_function.ty)
                    else {
                        return Err("nested constructor selection has no canonical target".into());
                    };
                    let nested_info = tasty.store.symbols.get(*nested_target);
                    if nested_info.kind != SymbolKind::Constructor {
                        return Err("nested constructor selection is not a constructor".into());
                    }
                    let nested_owner = nested_info
                        .owner
                        .ok_or_else(|| "nested constructor has no declaring owner".to_owned())?;
                    let nested_metadata = all_metadata
                        .iter()
                        .find(|metadata| metadata.identity().symbol() == nested_owner)
                        .ok_or_else(|| {
                            "nested constructor owner is absent from the JVM batch".to_owned()
                        })?;
                    let nested_receiver_is_new = tasty
                        .typed_ast
                        .try_get(nested_selection.qualifier)
                        .is_some_and(|receiver| matches!(receiver.kind, TreeKind::New(_)));
                    if !nested_receiver_is_new {
                        return Err("nested constructor receiver is not New".into());
                    }
                    let nested_constructor = nested_metadata
                        .constructor()
                        .filter(|constructor| constructor.symbol() == *nested_target)
                        .ok_or_else(|| {
                            "nested constructor is absent from its JVM plan".to_owned()
                        })?;
                    let outer_constructor = target_class_metadata
                        .constructor()
                        .filter(|constructor| constructor.symbol() == *target)
                        .ok_or_else(|| {
                            "outer constructor is absent from its JVM plan".to_owned()
                        })?;
                    let caller_descriptor = source_method_descriptor(
                        tasty.store,
                        tasty.definitions,
                        caller,
                        &all_metadata,
                    )
                    .ok_or_else(|| {
                        "nested constructor caller has an unsupported signature".to_owned()
                    })?;
                    let nested_descriptor = nested_constructor.descriptor().as_str();
                    let outer_descriptor = outer_constructor.descriptor().as_str();
                    let nested_owner_descriptor =
                        format!("L{};", nested_metadata.identity().internal_name());
                    let outer_result_descriptor =
                        format!("L{};", target_class_metadata.identity().internal_name());
                    if !nested_constructor.access_flags().is_public()
                        || nested_descriptor != "(I)V"
                        || outer_descriptor != format!("({nested_owner_descriptor})V")
                        || caller_descriptor != format!("(I){outer_result_descriptor}")
                        || nested_info.origin != SymbolOrigin::Source(tasty.source.id)
                    {
                        return Err(
                            "nested constructor chain differs from its checked source/JVM ABI"
                                .into(),
                        );
                    }
                    calls.push(CrossClassMethodRef {
                        caller,
                        target: *target,
                        owner_internal_name: target_class_metadata
                            .identity()
                            .internal_name()
                            .to_owned(),
                        name: "<init>".to_owned(),
                        descriptor: outer_descriptor.to_owned(),
                        constructor: true,
                        argument_load_opcode: None,
                        receiver_field: None,
                        nested_constructor: Some(NestedConstructorMethodRef {
                            owner_internal_name: nested_metadata
                                .identity()
                                .internal_name()
                                .to_owned(),
                            descriptor: nested_descriptor.to_owned(),
                            argument_local_slot: 1,
                        }),
                        nested_constructor_pair: None,
                        mixed_constructor_pair: None,
                        direct_formal_constructor_pair: None,
                        factory_method: None,
                        factory_local_slot: None,
                        factory_local_max_locals: None,
                    });
                    continue;
                }
                let constructor = target_class_metadata
                    .constructor()
                    .filter(|constructor| constructor.symbol() == *target)
                    .ok_or_else(|| {
                        format!("cross-class constructor {target:?} is absent from its JVM plan")
                    })?;
                let descriptor = constructor.descriptor().as_str();
                let caller_descriptor =
                    source_method_descriptor(tasty.store, tasty.definitions, caller, &all_metadata)
                        .ok_or_else(|| {
                            format!(
                                "cross-class caller {caller:?} has an unsupported source signature"
                            )
                        })?;
                let (constructor_parameters, constructor_result) = descriptor
                    .strip_prefix('(')
                    .and_then(|descriptor| descriptor.split_once(")"))
                    .ok_or_else(|| {
                        format!("cross-class constructor {target:?} has an invalid descriptor")
                    })?;
                let (caller_parameters, caller_result) = caller_descriptor
                    .strip_prefix('(')
                    .and_then(|descriptor| descriptor.split_once(")"))
                    .ok_or_else(|| {
                        format!("cross-class caller {caller:?} has an invalid descriptor")
                    })?;
                let expected_result =
                    format!("L{};", target_class_metadata.identity().internal_name());
                let supported_argument = constructor_parameters == "I"
                    || (constructor_parameters.starts_with('L')
                        && constructor_parameters.ends_with(';')
                        && constructor_parameters.matches(';').count() == 1);
                if !constructor.access_flags().is_public()
                    || !supported_argument
                    || constructor_result != "V"
                    || caller_parameters != constructor_parameters
                    || caller_result != expected_result
                {
                    return Err(format!(
                        "cross-class constructor {target:?} does not match the accessible source formal and result signature"
                    ));
                }
                let argument_load_opcode = match constructor_parameters {
                    "I" => 0x1b,
                    descriptor if descriptor.starts_with('L') => 0x2b,
                    _ => {
                        return Err(
                            "cross-class constructor argument category is unsupported".into()
                        );
                    }
                };
                calls.push(CrossClassMethodRef {
                    caller,
                    target: *target,
                    owner_internal_name: target_class_metadata
                        .identity()
                        .internal_name()
                        .to_owned(),
                    name: "<init>".to_owned(),
                    descriptor: descriptor.to_owned(),
                    constructor: true,
                    argument_load_opcode: Some(argument_load_opcode),
                    receiver_field: None,
                    nested_constructor: None,
                    nested_constructor_pair: None,
                    mixed_constructor_pair: None,
                    direct_formal_constructor_pair: None,
                    factory_method: None,
                    factory_local_slot: None,
                    factory_local_max_locals: None,
                });
                continue;
            }
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
            constructor: false,
            argument_load_opcode: None,
            receiver_field: match &function_tree.kind {
                TreeKind::Select(selection) => tasty
                    .typed_ast
                    .try_get(selection.qualifier)
                    .and_then(|receiver| match tasty.store.types.try_get(receiver.ty) {
                        Some(Type::TermRef {
                            target: TermRefTarget::Symbol(symbol),
                            ..
                        }) => Some(*symbol),
                        _ => None,
                    })
                    .filter(|receiver| {
                        proof
                            .tasty_plan
                            .same_batch_reference_fields()
                            .any(|field| field.field() == *receiver)
                    }),
                _ => None,
            },
            nested_constructor: None,
            nested_constructor_pair: None,
            mixed_constructor_pair: None,
            direct_formal_constructor_pair: None,
            factory_method: None,
            factory_local_slot: None,
            factory_local_max_locals: None,
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
        if let Some(factory) = &call.factory_method {
            if let Some(local_slot) = call.factory_local_slot {
                let mut pc = 0;
                take_factory_call_opcode(code.code, &mut pc, 0x2a, "aload_0")?;
                take_factory_call_opcode(code.code, &mut pc, 0x1b, "iload_1")?;
                let helper_call_index =
                    take_factory_call_operand(code.code, &mut pc, 0xb6, "invokevirtual")?;
                take_factory_local(code.code, &mut pc, true, local_slot)?;
                let outer_allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
                take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
                take_factory_local(code.code, &mut pc, false, local_slot)?;
                let outer_call_index =
                    take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
                take_factory_call_opcode(code.code, &mut pc, 0xb0, "areturn")?;
                if pc != code.code.len()
                    || code.max_stack != 3
                    || Some(code.max_locals) != call.factory_local_max_locals
                    || !code.exception_table.is_empty()
                {
                    return Err(format!(
                        "caller method {:?} has an invalid local factory sequence, stack/local layout, or exception metadata",
                        call.caller
                    ));
                }
                let class_name = |index| -> Result<&str, String> {
                    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class {
                        name_index,
                    }) = class_file.constant_pool.get(index)
                    else {
                        return Err("reference-local new does not reference CONSTANT_Class".into());
                    };
                    class_file
                        .constant_pool
                        .utf8(*name_index)
                        .map_err(|error| format!("invalid reference-local class name: {error:?}"))
                };
                let method_ref = |index| -> Result<(&str, (&str, &str)), String> {
                    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    }) = class_file.constant_pool.get(index)
                    else {
                        return Err(
                            "reference-local invocation does not reference CONSTANT_Methodref"
                                .into(),
                        );
                    };
                    let owner =
                        class_file
                            .constant_pool
                            .class_name(*class_index)
                            .map_err(|error| {
                                format!("invalid reference-local method owner: {error:?}")
                            })?;
                    let method = class_file
                        .constant_pool
                        .name_and_type(*name_and_type_index)
                        .map_err(|error| format!("invalid reference-local Methodref: {error:?}"))?;
                    Ok((owner, method))
                };
                if class_name(outer_allocation)? != call.owner_internal_name
                    || method_ref(helper_call_index)?
                        != (
                            factory.owner_internal_name.as_str(),
                            (factory.name.as_str(), factory.descriptor.as_str()),
                        )
                    || method_ref(outer_call_index)?
                        != (
                            call.owner_internal_name.as_str(),
                            ("<init>", call.descriptor.as_str()),
                        )
                {
                    return Err(format!(
                        "caller method {:?} does not invoke one exact factory and pass the stored local to its constructor",
                        call.caller
                    ));
                }
                continue;
            }
            let mut pc = 0;
            let allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
            take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
            take_factory_call_opcode(code.code, &mut pc, 0x2a, "aload_0")?;
            take_factory_call_opcode(code.code, &mut pc, 0x1b, "iload_1")?;
            let helper_call_index =
                take_factory_call_operand(code.code, &mut pc, 0xb6, "invokevirtual")?;
            let outer_call_index =
                take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
            take_factory_call_opcode(code.code, &mut pc, 0xb0, "areturn")?;
            if pc != code.code.len() {
                return Err(format!(
                    "caller method {:?} has trailing instructions after the bounded factory-result sequence",
                    call.caller
                ));
            }
            if code.max_stack < 4 || code.max_locals != 2 || !code.exception_table.is_empty() {
                return Err(format!(
                    "caller method {:?} has invalid stack, locals, or exception metadata for the bounded factory-result sequence",
                    call.caller
                ));
            }
            let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
                class_file.constant_pool.get(allocation)
            else {
                return Err("factory-result allocation does not reference CONSTANT_Class".into());
            };
            let allocation_owner = class_file
                .constant_pool
                .utf8(*name_index)
                .map_err(|error| format!("invalid factory-result allocation class: {error:?}"))?;
            let method_ref = |index| -> Result<(&str, (&str, &str)), String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class_file.constant_pool.get(index)
                else {
                    return Err(
                        "factory-result invocation does not reference CONSTANT_Methodref".into(),
                    );
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .map_err(|error| format!("invalid factory-result method owner: {error:?}"))?;
                let method = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .map_err(|error| {
                        format!("invalid factory-result method reference: {error:?}")
                    })?;
                Ok((owner, method))
            };
            let helper_call = method_ref(helper_call_index)?;
            let outer_call = method_ref(outer_call_index)?;
            if allocation_owner != call.owner_internal_name
                || helper_call
                    != (
                        factory.owner_internal_name.as_str(),
                        (factory.name.as_str(), factory.descriptor.as_str()),
                    )
                || outer_call
                    != (
                        call.owner_internal_name.as_str(),
                        ("<init>", call.descriptor.as_str()),
                    )
            {
                return Err(format!(
                    "caller method {:?} does not invoke the exact same-instance factory and outer constructor",
                    call.caller
                ));
            }
            continue;
        }
        if let Some(mixed_pair) = &call.mixed_constructor_pair {
            let mut pc = 0;
            for local in &mixed_pair.local_initializers {
                let allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
                take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
                take_int_local(code.code, &mut pc, local.argument_local_slot)?;
                let initializer =
                    take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
                take_factory_local(code.code, &mut pc, true, local.result_local_slot)?;
                let class_name = |index| -> Result<&str, String> {
                    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class {
                        name_index,
                    }) = class_file.constant_pool.get(index)
                    else {
                        return Err(
                            "local Counter allocation does not reference CONSTANT_Class".into()
                        );
                    };
                    class_file.constant_pool.utf8(*name_index).map_err(|error| {
                        format!("invalid local Counter allocation class: {error:?}")
                    })
                };
                let method_ref = |index| -> Result<(&str, (&str, &str)), String> {
                    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    }) = class_file.constant_pool.get(index)
                    else {
                        return Err(
                            "local Counter initializer does not reference CONSTANT_Methodref"
                                .into(),
                        );
                    };
                    let owner =
                        class_file
                            .constant_pool
                            .class_name(*class_index)
                            .map_err(|error| {
                                format!("invalid local Counter initializer owner: {error:?}")
                            })?;
                    let method = class_file
                        .constant_pool
                        .name_and_type(*name_and_type_index)
                        .map_err(|error| {
                            format!("invalid local Counter initializer Methodref: {error:?}")
                        })?;
                    Ok((owner, method))
                };
                if class_name(allocation)? != local.owner_internal_name
                    || method_ref(initializer)?
                        != (
                            local.owner_internal_name.as_str(),
                            ("<init>", local.descriptor.as_str()),
                        )
                {
                    return Err(format!(
                        "caller method {:?} does not initialize the exact local Counter before Pair construction",
                        call.caller
                    ));
                }
            }
            if let Some(alias) = mixed_pair.local_alias_copy {
                take_factory_local(code.code, &mut pc, false, alias.source_local_slot)?;
                take_factory_local(code.code, &mut pc, true, alias.alias_local_slot)?;
            }
            if let Some(selection) = mixed_pair.conditional_selection {
                take_int_local(code.code, &mut pc, selection.condition_local_slot)?;
                let ifeq_pc = pc;
                take_factory_call_opcode(code.code, &mut pc, 0x99, "ifeq")?;
                let else_target = take_factory_branch_target(code.code, &mut pc, ifeq_pc)?;
                take_factory_local(code.code, &mut pc, false, selection.first_target_local_slot)?;
                let goto_pc = pc;
                take_factory_call_opcode(code.code, &mut pc, 0xa7, "goto")?;
                let join_target = take_factory_branch_target(code.code, &mut pc, goto_pc)?;
                if else_target != pc {
                    return Err("conditional branch does not target the original else arm".into());
                }
                take_factory_local(
                    code.code,
                    &mut pc,
                    false,
                    selection.second_target_local_slot,
                )?;
                if join_target != pc {
                    return Err(
                        "conditional branch join does not follow the original else arm".into(),
                    );
                }
                take_factory_local(code.code, &mut pc, true, selection.selected_local_slot)?;
            }
            let local_receiver_calls = if let Some(calls) = &mixed_pair.local_receiver_call_pair {
                calls.iter().collect::<Vec<_>>()
            } else {
                mixed_pair.local_receiver_call.iter().collect::<Vec<_>>()
            };
            for call in local_receiver_calls {
                take_factory_local(code.code, &mut pc, false, call.receiver_local_slot)?;
                take_int_local(code.code, &mut pc, call.argument_local_slot)?;
                let invocation =
                    take_factory_call_operand(code.code, &mut pc, 0xb6, "invokevirtual")?;
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class_file.constant_pool.get(invocation)
                else {
                    return Err(
                        "Counter.set invocation does not reference CONSTANT_Methodref".into(),
                    );
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .map_err(|error| format!("invalid Counter.set owner: {error:?}"))?;
                let (name, descriptor) = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .map_err(|error| format!("invalid Counter.set Methodref: {error:?}"))?;
                if owner != call.owner_internal_name
                    || name != call.name
                    || descriptor != call.descriptor
                    || call.name != "set"
                    || call.descriptor != "(I)V"
                {
                    return Err(
                        "Counter.set invokevirtual differs from its canonical instance Methodref"
                            .into(),
                    );
                }
            }
            let outer_allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
            take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
            let mut nested_allocations = Vec::new();
            let mut nested_initializers = Vec::new();
            for argument in &mixed_pair.arguments {
                match argument {
                    MixedConstructorArgumentRef::Formal { local_slot } => {
                        take_factory_local(code.code, &mut pc, false, *local_slot)?;
                    }
                    MixedConstructorArgumentRef::LocalValue { local_slot } => {
                        take_factory_local(code.code, &mut pc, false, *local_slot)?;
                    }
                    MixedConstructorArgumentRef::Nested(nested) => {
                        let allocation =
                            take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
                        take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
                        take_int_local(code.code, &mut pc, nested.argument_local_slot)?;
                        let initializer =
                            take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
                        nested_allocations.push((allocation, nested));
                        nested_initializers.push((initializer, nested));
                    }
                }
            }
            let outer_initializer =
                take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
            take_factory_call_opcode(code.code, &mut pc, 0xb0, "areturn")?;
            if pc != code.code.len()
                || code.max_stack != mixed_pair.max_stack
                || code.max_locals != mixed_pair.max_locals
                || !code.exception_table.is_empty()
                || (!mixed_pair.local_initializers.is_empty()
                    && (!nested_allocations.is_empty() || !nested_initializers.is_empty()))
                || (mixed_pair.local_initializers.is_empty()
                    && (nested_allocations.len() != 1 || nested_initializers.len() != 1))
            {
                return Err(format!(
                    "caller method {:?} has invalid mixed constructor instructions, stack/local bounds, or exception metadata",
                    call.caller
                ));
            }
            let class_name = |index| -> Result<&str, String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
                    class_file.constant_pool.get(index)
                else {
                    return Err(
                        "mixed constructor allocation does not reference CONSTANT_Class".into(),
                    );
                };
                class_file
                    .constant_pool
                    .utf8(*name_index)
                    .map_err(|error| format!("invalid mixed allocation class: {error:?}"))
            };
            let method_ref = |index| -> Result<(&str, (&str, &str)), String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class_file.constant_pool.get(index)
                else {
                    return Err(
                        "mixed constructor invocation does not reference CONSTANT_Methodref".into(),
                    );
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .map_err(|error| format!("invalid mixed constructor owner: {error:?}"))?;
                let method = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .map_err(|error| format!("invalid mixed constructor Methodref: {error:?}"))?;
                Ok((owner, method))
            };
            if class_name(outer_allocation)? != call.owner_internal_name
                || method_ref(outer_initializer)?
                    != (
                        call.owner_internal_name.as_str(),
                        ("<init>", call.descriptor.as_str()),
                    )
            {
                return Err(format!(
                    "caller method {:?} does not allocate and invoke its exact mixed Pair constructor",
                    call.caller
                ));
            }
            for ((allocation, nested), (initializer, _)) in
                nested_allocations.into_iter().zip(nested_initializers)
            {
                if class_name(allocation)? != nested.owner_internal_name
                    || method_ref(initializer)?
                        != (
                            nested.owner_internal_name.as_str(),
                            ("<init>", nested.descriptor.as_str()),
                        )
                {
                    return Err(format!(
                        "caller method {:?} does not allocate and initialize the exact nested Counter before continuing",
                        call.caller
                    ));
                }
            }
            continue;
        }
        if let Some(nested_pair) = &call.nested_constructor_pair {
            let mut pc = 0;
            let outer_allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
            take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
            let mut inner_allocations = Vec::with_capacity(2);
            let mut inner_calls = Vec::with_capacity(2);
            for nested in nested_pair {
                let allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
                take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
                take_int_local(code.code, &mut pc, nested.argument_local_slot)?;
                let constructor =
                    take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
                inner_allocations.push(allocation);
                inner_calls.push(constructor);
            }
            let outer_call = take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
            take_factory_call_opcode(code.code, &mut pc, 0xb0, "areturn")?;
            if pc != code.code.len()
                || code.max_stack != 6
                || code.max_locals != 3
                || !code.exception_table.is_empty()
            {
                return Err(format!(
                    "caller method {:?} has invalid paired constructor instructions, stack/local bounds, or exception metadata",
                    call.caller
                ));
            }
            let class_name = |index| -> Result<&str, String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
                    class_file.constant_pool.get(index)
                else {
                    return Err(
                        "paired constructor allocation does not reference CONSTANT_Class".into(),
                    );
                };
                class_file
                    .constant_pool
                    .utf8(*name_index)
                    .map_err(|error| format!("invalid paired allocation class: {error:?}"))
            };
            let method_ref = |index| -> Result<(&str, (&str, &str)), String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class_file.constant_pool.get(index)
                else {
                    return Err(
                        "paired constructor invocation does not reference CONSTANT_Methodref"
                            .into(),
                    );
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .map_err(|error| format!("invalid paired constructor owner: {error:?}"))?;
                let method = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .map_err(|error| format!("invalid paired constructor Methodref: {error:?}"))?;
                Ok((owner, method))
            };
            if class_name(outer_allocation)? != call.owner_internal_name
                || method_ref(outer_call)?
                    != (
                        call.owner_internal_name.as_str(),
                        ("<init>", call.descriptor.as_str()),
                    )
            {
                return Err(format!(
                    "caller method {:?} does not allocate and invoke its exact paired outer constructor",
                    call.caller
                ));
            }
            for ((allocation, constructor), expected) in inner_allocations
                .into_iter()
                .zip(inner_calls)
                .zip(nested_pair)
            {
                if class_name(allocation)? != expected.owner_internal_name
                    || method_ref(constructor)?
                        != (
                            expected.owner_internal_name.as_str(),
                            ("<init>", expected.descriptor.as_str()),
                        )
                {
                    return Err(format!(
                        "caller method {:?} does not allocate and invoke the exact ordered sibling constructors",
                        call.caller
                    ));
                }
            }
            continue;
        }
        if let Some(formal_slots) = call.direct_formal_constructor_pair {
            let mut pc = 0;
            let allocation = take_factory_call_operand(code.code, &mut pc, 0xbb, "new")?;
            take_factory_call_opcode(code.code, &mut pc, 0x59, "dup")?;
            for slot in formal_slots {
                take_factory_local(code.code, &mut pc, false, slot)?;
            }
            let constructor = take_factory_call_operand(code.code, &mut pc, 0xb7, "invokespecial")?;
            take_factory_call_opcode(code.code, &mut pc, 0xb0, "areturn")?;
            if pc != code.code.len()
                || code.max_stack != 4
                || code.max_locals != 3
                || !code.exception_table.is_empty()
            {
                return Err(format!(
                    "caller method {:?} has invalid direct-formal constructor instructions, stack/local bounds, or exception metadata",
                    call.caller
                ));
            }
            let class_name = |index| -> Result<&str, String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
                    class_file.constant_pool.get(index)
                else {
                    return Err("direct-formal allocation does not reference CONSTANT_Class".into());
                };
                class_file
                    .constant_pool
                    .utf8(*name_index)
                    .map_err(|error| format!("invalid direct-formal allocation class: {error:?}"))
            };
            let method_ref = |index| -> Result<(&str, (&str, &str)), String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class_file.constant_pool.get(index)
                else {
                    return Err(
                        "direct-formal invocation does not reference CONSTANT_Methodref".into(),
                    );
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .map_err(|error| {
                        format!("invalid direct-formal constructor owner: {error:?}")
                    })?;
                let method = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .map_err(|error| {
                        format!("invalid direct-formal constructor reference: {error:?}")
                    })?;
                Ok((owner, method))
            };
            if class_name(allocation)? != call.owner_internal_name
                || method_ref(constructor)?
                    != (
                        call.owner_internal_name.as_str(),
                        ("<init>", call.descriptor.as_str()),
                    )
            {
                return Err(format!(
                    "caller method {:?} does not allocate and invoke its exact direct-formal constructor",
                    call.caller
                ));
            }
            continue;
        }
        if let Some(nested) = &call.nested_constructor {
            if code.code.len() != 16
                || code.code[0] != 0xbb
                || code.code[3] != 0x59
                || code.code[4] != 0xbb
                || code.code[7] != 0x59
                || code.code[8] != 0x1b
                || code.code[9] != 0xb7
                || code.code[12] != 0xb7
                || code.code[15] != 0xb0
                || code.max_stack < 5
                || code.max_locals != 2
                || !code.exception_table.is_empty()
            {
                return Err(format!(
                    "caller method {:?} does not contain the bounded two-allocation nested-constructor sequence: {:02x?}",
                    call.caller, code.code
                ));
            }
            let cp_index = |offset: usize| {
                dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
                    code.code[offset],
                    code.code[offset + 1],
                ]))
            };
            let allocation_owner = |offset: usize| -> Result<&str, String> {
                let index = cp_index(offset);
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
                    class_file.constant_pool.get(index)
                else {
                    return Err(
                        "nested constructor allocation does not reference CONSTANT_Class".into(),
                    );
                };
                class_file
                    .constant_pool
                    .utf8(*name_index)
                    .map_err(|error| format!("invalid nested allocation class name: {error:?}"))
            };
            let method_ref = |offset: usize| -> Result<(&str, (&str, &str)), String> {
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class_file.constant_pool.get(cp_index(offset))
                else {
                    return Err(
                        "nested constructor invocation does not reference CONSTANT_Methodref"
                            .into(),
                    );
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .map_err(|error| format!("invalid nested constructor owner: {error:?}"))?;
                let method = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .map_err(|error| {
                        format!("invalid nested constructor name/descriptor: {error:?}")
                    })?;
                Ok((owner, method))
            };
            let actual_outer_allocation = allocation_owner(1)?;
            let actual_inner_allocation = allocation_owner(5)?;
            if actual_outer_allocation != call.owner_internal_name
                || actual_inner_allocation != nested.owner_internal_name
            {
                return Err(format!(
                    "caller method {:?} allocates ({actual_outer_allocation}, {actual_inner_allocation}), expected ({}, {})",
                    call.caller, call.owner_internal_name, nested.owner_internal_name
                ));
            }
            let inner_call = method_ref(10)?;
            let outer_call = method_ref(13)?;
            if inner_call
                != (
                    nested.owner_internal_name.as_str(),
                    ("<init>", nested.descriptor.as_str()),
                )
                || outer_call
                    != (
                        call.owner_internal_name.as_str(),
                        ("<init>", call.descriptor.as_str()),
                    )
            {
                return Err(format!(
                    "caller method {:?} does not invoke the exact nested and outer constructors in evaluation order",
                    call.caller
                ));
            }
            continue;
        }
        let reference = if call.constructor {
            let load_argument = call.argument_load_opcode.ok_or_else(|| {
                "cross-class constructor has no validated argument load".to_owned()
            })?;
            if code.code.len() != 9
                || code.code[0] != 0xbb
                || code.code[3] != 0x59
                || code.code[4] != load_argument
                || code.code[5] != 0xb7
                || code.code[8] != 0xb0
            {
                return Err(format!(
                    "caller method {:?} does not contain the bounded new/dup/aload_1/invokespecial/areturn shape: {:02x?}",
                    call.caller, code.code
                ));
            }
            let class_reference =
                dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
                    code.code[1],
                    code.code[2],
                ]));
            let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Class { name_index }) =
                class_file.constant_pool.get(class_reference)
            else {
                return Err(format!(
                    "caller method {:?} new does not reference CONSTANT_Class",
                    call.caller
                ));
            };
            let actual_new_owner = class_file
                .constant_pool
                .utf8(*name_index)
                .map_err(|error| format!("invalid new class name: {error:?}"))?;
            if actual_new_owner != call.owner_internal_name {
                return Err(format!(
                    "caller method {:?} allocates {}, expected {}",
                    call.caller, actual_new_owner, call.owner_internal_name
                ));
            }
            dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
                code.code[6],
                code.code[7],
            ]))
        } else if let Some(field_symbol) = call.receiver_field {
            if code.code.len() != 8
                || code.code[0] != 0x2a
                || code.code[1] != 0xb4
                || code.code[4] != 0xb6
                || code.code[7] != 0xac
            {
                return Err(format!(
                    "caller method {:?} does not contain the bounded aload_0/getfield/invokevirtual/ireturn shape",
                    call.caller
                ));
            }
            let field_reference =
                dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
                    code.code[2],
                    code.code[3],
                ]));
            let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            }) = class_file.constant_pool.get(field_reference)
            else {
                return Err(format!(
                    "caller method {:?} getfield does not reference CONSTANT_Fieldref",
                    call.caller
                ));
            };
            let actual_field_owner = class_file
                .constant_pool
                .class_name(*class_index)
                .map_err(|error| format!("invalid getfield owner: {error:?}"))?;
            let actual_field = class_file
                .constant_pool
                .name_and_type(*name_and_type_index)
                .map_err(|error| format!("invalid getfield name/descriptor: {error:?}"))?;
            let expected_field = proof
                .metadata
                .fields()
                .iter()
                .find(|field| field.symbol() == field_symbol)
                .ok_or_else(|| {
                    format!("receiver field {field_symbol:?} is absent from checked JVM metadata")
                })?;
            if actual_field_owner != proof.internal_name()
                || actual_field != (expected_field.name(), expected_field.descriptor())
            {
                return Err(format!(
                    "caller method {:?} loads field {:?} through {:?}, expected {}.{}:{}",
                    call.caller,
                    field_symbol,
                    actual_field,
                    proof.internal_name(),
                    expected_field.name(),
                    expected_field.descriptor()
                ));
            }
            dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
                code.code[5],
                code.code[6],
            ]))
        } else {
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
            dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
                code.code[2],
                code.code[3],
            ]))
        };
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

/// Consume one instruction from a deliberately bounded source-bound call
/// sequence. This is not a general bytecode decoder: callers enumerate every
/// supported instruction and require complete consumption of the method body.
fn take_factory_call_opcode(
    code: &[u8],
    pc: &mut usize,
    expected: u8,
    mnemonic: &str,
) -> Result<(), String> {
    let Some(actual) = code.get(*pc).copied() else {
        return Err(format!(
            "truncated bounded call bytecode while reading {mnemonic}"
        ));
    };
    if actual != expected {
        return Err(format!(
            "expected {mnemonic} in bounded call bytecode, found opcode 0x{actual:02x}"
        ));
    }
    *pc += 1;
    Ok(())
}

fn take_factory_local(code: &[u8], pc: &mut usize, store: bool, slot: u16) -> Result<(), String> {
    let (compact_base, generic_opcode, mnemonic) = if store {
        (0x4b_u16, 0x3a_u8, "astore")
    } else {
        (0x2a_u16, 0x19_u8, "aload")
    };
    if slot <= 3 {
        return take_factory_call_opcode(
            code,
            pc,
            (compact_base + slot) as u8,
            &format!("{mnemonic}_{slot}"),
        );
    }
    take_factory_call_opcode(code, pc, generic_opcode, mnemonic)?;
    let actual = code
        .get(*pc)
        .copied()
        .ok_or_else(|| format!("truncated {mnemonic} local index"))?;
    if u16::from(actual) != slot {
        return Err(format!(
            "expected {mnemonic} local slot {slot}, found {actual}"
        ));
    }
    *pc += 1;
    Ok(())
}

fn take_int_local(code: &[u8], pc: &mut usize, slot: u16) -> Result<(), String> {
    if slot <= 3 {
        return take_factory_call_opcode(code, pc, 0x1a + slot as u8, &format!("iload_{slot}"));
    }
    take_factory_call_opcode(code, pc, 0x15, "iload")?;
    let actual = code
        .get(*pc)
        .copied()
        .ok_or_else(|| "truncated iload local index".to_owned())?;
    if u16::from(actual) != slot {
        return Err(format!("expected iload local slot {slot}, found {actual}"));
    }
    *pc += 1;
    Ok(())
}

fn take_factory_branch_target(
    code: &[u8],
    pc: &mut usize,
    branch_pc: usize,
) -> Result<usize, String> {
    let bytes = code
        .get(*pc..*pc + 2)
        .ok_or_else(|| "truncated bounded conditional branch offset".to_owned())?;
    let offset = i16::from_be_bytes([bytes[0], bytes[1]]);
    *pc += 2;
    let target = i64::try_from(branch_pc)
        .map_err(|_| "conditional branch offset exceeds address range".to_owned())?
        + i64::from(offset);
    usize::try_from(target).map_err(|_| "conditional branch target is outside code".to_owned())
}

fn take_factory_call_operand(
    code: &[u8],
    pc: &mut usize,
    opcode: u8,
    mnemonic: &str,
) -> Result<dotty_classfile::constant_pool::ConstantPoolIndex, String> {
    take_factory_call_opcode(code, pc, opcode, mnemonic)?;
    let operand = code
        .get(*pc..pc.saturating_add(2))
        .ok_or_else(|| format!("truncated constant-pool operand for factory-result {mnemonic}"))?;
    let [high, low] = operand else {
        return Err(format!(
            "truncated constant-pool operand for factory-result {mnemonic}"
        ));
    };
    *pc += 2;
    Ok(dotty_classfile::constant_pool::ConstantPoolIndex(
        u16::from_be_bytes([*high, *low]),
    ))
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
    planned_classes: &[JvmClassMetadata],
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
    let expected_descriptor = source_constructor_descriptor(
        store,
        tasty.definitions,
        &expected_parameters,
        planned_classes,
        tasty.source.id,
        store.symbols.get(tasty.class).owner,
    )
    .ok_or(JvmTastyContractError::UnsupportedSourceShape {
        symbol: constructor,
    })?;
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
    tasty_plan: &ValidatedClassStatePlan<'_, '_>,
    jvm_class: &dotty_jvm::JvmClassPlan,
    metadata: &JvmClassMetadata,
    planned_class_metadata: &[JvmClassMetadata],
) -> Result<(), JvmTastyContractError> {
    let tasty = tasty_plan.plan();
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
        let mutable = symbol.flags == SymbolFlags::MUTABLE;
        let immutable_reference = symbol.flags.is_empty()
            && tasty_plan
                .same_batch_reference_fields()
                .any(|proof| proof.field() == field.symbol);
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
            .map(str::to_owned)
            .or_else(|| {
                immutable_reference.then(|| {
                    planned_class_descriptor(
                        store,
                        tasty.definitions,
                        field.declared_type,
                        planned_class_metadata,
                    )
                })?
            })
            .ok_or(JvmTastyContractError::UnsupportedSourceShape {
                symbol: field.symbol,
            })?;
        let flags = binary_field.access_flags();
        let mutable_field = !flags.contains(ACC_FINAL);
        if symbol.owner != Some(tasty.class)
            || symbol.kind != SymbolKind::Field
            || symbol.visibility != Visibility::Private
            || (!mutable && !immutable_reference)
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
    let result = primitive_descriptor(definitions, ty)
        .map(str::to_owned)
        .or_else(|| is_unit_type(definitions, ty).then(|| "V".to_owned()))
        .or_else(|| planned_class_descriptor(store, definitions, ty, planned_classes))?;
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
    planned_classes: &[JvmClassMetadata],
    source: dotty_core::SourceId,
    owner_package: Option<SymbolId>,
) -> Option<String> {
    let mut descriptors = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        let SymbolInfo::Complete(ty) = store.symbols.get(*parameter).info else {
            return None;
        };
        if let Some(descriptor) = primitive_descriptor(definitions, ty) {
            descriptors.push(descriptor.to_owned());
            continue;
        }
        let descriptor = planned_class_descriptor(store, definitions, ty, planned_classes)?;
        let Type::TypeRef {
            target: dotty_core::TypeRefTarget::Symbol(target),
            ..
        } = store.types.try_get(ty)?
        else {
            descriptors.push(descriptor);
            continue;
        };
        if !store.symbols.contains(*target) {
            return None;
        }
        let target = store.symbols.get(*target);
        if target.kind != SymbolKind::Class
            || target.origin != SymbolOrigin::Source(source)
            || target.owner != owner_package
        {
            return None;
        }
        descriptors.push(descriptor);
    }
    // JVM constructors always return void, even if the source constructor
    // symbol's semantic result records its owning class.
    Some(format!("({})V", descriptors.concat()))
}

fn is_unit_type(definitions: &Definitions, ty: dotty_core::TypeId) -> bool {
    ty == definitions.unit
}

#[cfg(test)]
mod paired_block_shape_tests {
    use super::{PairedBlockShape, PairedStatementKind, classify_paired_block_shape};

    use PairedStatementKind::{Apply, Other, ValDef};

    fn classify(
        source: &[PairedStatementKind],
        typed: &[PairedStatementKind],
        one_call: bool,
        two_calls: bool,
        alias: bool,
    ) -> PairedBlockShape {
        classify_paired_block_shape(source, typed, one_call, two_calls, alias, false)
    }

    #[test]
    fn recognizes_only_the_ordered_legacy_statement_and_receipt_shapes() {
        assert_eq!(
            classify(&[ValDef, ValDef], &[ValDef, ValDef], false, false, false),
            PairedBlockShape::TwoLocals
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, Apply],
                &[ValDef, ValDef, Apply],
                true,
                false,
                false
            ),
            PairedBlockShape::TwoLocalsOneCall
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, Apply, Apply],
                &[ValDef, ValDef, Apply, Apply],
                false,
                true,
                false
            ),
            PairedBlockShape::TwoLocalsTwoCalls
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, ValDef, Apply, Apply],
                &[ValDef, ValDef, ValDef, Apply, Apply],
                false,
                true,
                true
            ),
            PairedBlockShape::TwoLocalsAliasTwoCalls
        );
    }

    #[test]
    fn rejects_same_count_collisions_wrong_order_and_missing_proofs() {
        assert_eq!(
            classify(
                &[ValDef, ValDef, ValDef, Apply],
                &[ValDef, ValDef, ValDef, Apply],
                false,
                true,
                false
            ),
            PairedBlockShape::ThreeLocalsOneCall
        );
        assert_eq!(
            classify_paired_block_shape(
                &[ValDef, ValDef, ValDef, Apply],
                &[ValDef, ValDef, ValDef, Apply],
                true,
                false,
                false,
                true,
            ),
            PairedBlockShape::ThreeLocalsConditionalSelection
        );
        assert_eq!(
            classify_paired_block_shape(
                &[ValDef, ValDef, ValDef, Apply],
                &[ValDef, ValDef, ValDef, Apply],
                false,
                false,
                false,
                true,
            ),
            PairedBlockShape::ReceiptMismatch
        );
        assert_eq!(
            classify(
                &[ValDef, Apply, ValDef, Apply],
                &[ValDef, Apply, ValDef, Apply],
                false,
                true,
                false
            ),
            PairedBlockShape::Unclassified
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, ValDef, Apply, Apply],
                &[ValDef, ValDef, ValDef, Apply, Apply],
                false,
                true,
                false
            ),
            PairedBlockShape::ReceiptMismatch
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, Other, Apply],
                &[ValDef, ValDef, Apply, Apply],
                false,
                true,
                false
            ),
            PairedBlockShape::Unclassified
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, Apply, Apply],
                &[ValDef, ValDef, ValDef, Apply],
                false,
                true,
                false
            ),
            PairedBlockShape::Unclassified
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, Apply],
                &[ValDef, ValDef, Apply],
                false,
                false,
                false
            ),
            PairedBlockShape::ReceiptMismatch
        );
        assert_eq!(
            classify(
                &[ValDef, ValDef, Apply, Apply],
                &[ValDef, ValDef, Apply, Apply],
                false,
                false,
                false
            ),
            PairedBlockShape::ReceiptMismatch
        );
    }
}
