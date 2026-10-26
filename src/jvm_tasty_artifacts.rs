//! Owned, validated JVM and TASTy artifacts emitted from one source session.
//!
//! This bounded adapter consumes the paired plans and cross-producer proof in
//! [`crate::jvm_tasty_plan`]. It validates the complete TASTy bytes first,
//! links the JVM class to that exact header identifier, then independently
//! decodes both artifacts before returning either one.

use std::error::Error;
use std::fmt;

use dotty_classfile::class_file::{ClassFile, ClassFileError};
use dotty_classfile::constant_pool::PoolRefError;
use dotty_classfile::reader::Reader as ClassFileReader;
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::{SourceId, SymbolId};
use dotty_jvm::{GeneratedClass, JvmCodegenError, emit_class_with_tasty_link};
use dotty_jvm_tasty_contract::{
    JvmTastyBatchArtifacts, JvmTastyClassFileArtifact, JvmTastyContractError,
    JvmTastySidecarArtifact, ValidatedJvmTastyClass, validate_classfile_tasty_link,
    validate_jvm_tasty_batch, validate_jvm_tasty_batch_artifacts,
};
use dotty_tasty::tasty::{TastyFile, TastyFileError};
use dotty_tasty_pickler::{PickleError, ValidatedClassStatePlan};

use crate::jvm_tasty_plan::{
    PairedPlanError, with_paired_jvm_tasty_batch_plan, with_paired_jvm_tasty_class_plan,
};

/// Failures while producing and validating a linked in-memory artifact pair.
#[derive(Debug)]
pub enum PairedArtifactError {
    /// The source could not produce the one-session plans and ABI proof.
    Plan(PairedPlanError),
    /// The complete TASTy output could not be encoded.
    TastyPickle(PickleError),
    /// The produced TASTy bytes failed pinned 3.9 validation.
    TastyFile(TastyFileError),
    /// The linked JVM class could not be emitted.
    Jvm(JvmCodegenError),
    /// The produced JVM bytes failed class-file decoding.
    ClassFile(ClassFileError),
    /// A class-file name or member reference did not resolve.
    ClassFileReference(PoolRefError),
    /// The pair failed its source or byte-level contract.
    Contract(JvmTastyContractError),
    /// An emitted artifact disagreed with the canonical paired plan.
    ArtifactMismatch(&'static str),
}

impl fmt::Display for PairedArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot emit a validated JVM/TASTy pair: {self:?}")
    }
}

impl Error for PairedArtifactError {}

/// An owned `.class`/`.tasty` pair whose source plans and binary link agree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedJvmTastyArtifacts {
    class: SymbolId,
    source_id: SourceId,
    source_member_order: Vec<SymbolId>,
    classfile_path: String,
    classfile_bytes: Vec<u8>,
    tasty_path: String,
    tasty_bytes: Vec<u8>,
    tasty_link_id: TastyLinkId,
}

impl PairedJvmTastyArtifacts {
    /// Canonical source class identity used by both producer plans.
    pub const fn class(&self) -> SymbolId {
        self.class
    }

    /// Source identity of the single session used to build both artifacts.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// Canonical field and method symbols in source declaration order.
    pub fn source_member_order(&self) -> &[SymbolId] {
        &self.source_member_order
    }

    /// Package-qualified relative JVM artifact path.
    pub fn classfile_path(&self) -> &str {
        &self.classfile_path
    }

    /// Complete encoded JVM class-file bytes.
    pub fn classfile_bytes(&self) -> &[u8] {
        &self.classfile_bytes
    }

    /// Package-qualified relative TASTy artifact path.
    pub fn tasty_path(&self) -> &str {
        &self.tasty_path
    }

    /// Complete encoded TASTy bytes.
    pub fn tasty_bytes(&self) -> &[u8] {
        &self.tasty_bytes
    }

    /// Exact TASTy header identifier mirrored in the class-level `TASTY` link.
    pub const fn tasty_link_id(&self) -> TastyLinkId {
        self.tasty_link_id
    }
}

/// Complete immutable artifact batch emitted from one frontend session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedJvmTastyBatchArtifacts {
    source_id: SourceId,
    classes: Vec<PairedJvmTastyArtifacts>,
}

impl PairedJvmTastyBatchArtifacts {
    /// Source identity shared by all class artifacts in the batch.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// Complete class/TASTy pairs in canonical source declaration order.
    pub fn classes(&self) -> &[PairedJvmTastyArtifacts] {
        &self.classes
    }
}

/// Failures while emitting and validating a complete JVM/TASTy source batch.
#[derive(Debug)]
pub enum PairedBatchArtifactError {
    /// The source could not produce the one-session plans and ABI proof.
    Plan(PairedPlanError),
    /// Batch provenance or whole-batch artifact validation failed.
    Contract(JvmTastyContractError),
    /// One class failed before a complete batch could be returned.
    ClassArtifact {
        class_index: usize,
        class: SymbolId,
        error: Box<PairedArtifactError>,
    },
}

impl fmt::Display for PairedBatchArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot emit a validated JVM/TASTy batch: {self:?}")
    }
}

impl Error for PairedBatchArtifactError {}

/// Parse, name and type once, encode both producer outputs, and validate the
/// exact class-file/TASTy pair before returning owned bytes.
///
/// This is intentionally limited to the source forms accepted by
/// [`with_paired_jvm_tasty_class_plan`]. It does not write files or provide a
/// general source compiler API.
pub fn emit_paired_jvm_tasty_class(
    source_text: &str,
    source_name: &str,
    source_id: SourceId,
) -> Result<PairedJvmTastyArtifacts, PairedArtifactError> {
    with_paired_jvm_tasty_class_plan(source_text, source_name, source_id, |plan| {
        emit_validated_pair(ValidatedPairInput {
            jvm_unit: plan.jvm_unit,
            class: plan.class,
            source_id: plan.source_id,
            source_member_order: plan.source_member_order,
            classfile_path: plan.classfile_path,
            tasty_path: plan.tasty_path,
            tasty_plan: plan.tasty_plan,
            compatibility: &plan.compatibility,
        })
    })
    .map_err(PairedArtifactError::Plan)?
}

/// Emit every class and its source-defined TASTy sidecar from one shared
/// frontend session. No class artifacts escape unless the complete batch has
/// passed semantic and byte-level validation.
pub fn emit_paired_jvm_tasty_batch(
    source_text: &str,
    source_name: &str,
    source_id: SourceId,
) -> Result<PairedJvmTastyBatchArtifacts, PairedBatchArtifactError> {
    with_paired_jvm_tasty_batch_plan(source_text, source_name, source_id, |plan| {
        let tasty_plans = plan
            .classes
            .iter()
            .map(|class| class.tasty_plan)
            .collect::<Vec<_>>();
        let validated_batch = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans)
            .map_err(PairedBatchArtifactError::Contract)?;

        let mut classes = Vec::with_capacity(plan.classes.len());
        for (class_index, class) in plan.classes.iter().enumerate() {
            let artifacts = emit_validated_pair(ValidatedPairInput {
                jvm_unit: plan.jvm_unit,
                class: class.class,
                source_id: plan.source_id,
                source_member_order: class.source_member_order.clone(),
                classfile_path: class.classfile_path.clone(),
                tasty_path: class.tasty_path.clone(),
                tasty_plan: class.tasty_plan,
                compatibility: class.compatibility,
            })
            .map_err(|error| PairedBatchArtifactError::ClassArtifact {
                class_index,
                class: class.class,
                error: Box::new(error),
            })?;
            classes.push(artifacts);
        }

        let owned_artifacts = JvmTastyBatchArtifacts {
            classfiles: classes
                .iter()
                .map(|class| JvmTastyClassFileArtifact {
                    classfile_path: class.classfile_path.clone(),
                    classfile_bytes: class.classfile_bytes.clone(),
                })
                .collect(),
            sidecars: classes
                .iter()
                .map(|class| JvmTastySidecarArtifact {
                    tasty_path: class.tasty_path.clone(),
                    tasty_bytes: class.tasty_bytes.clone(),
                })
                .collect(),
        };
        validate_jvm_tasty_batch_artifacts(&validated_batch, owned_artifacts)
            .map_err(PairedBatchArtifactError::Contract)?;

        Ok(PairedJvmTastyBatchArtifacts { source_id, classes })
    })
    .map_err(PairedBatchArtifactError::Plan)?
}

struct ValidatedPairInput<'session> {
    jvm_unit: &'session dotty_jvm::JvmCompilationUnit<'session>,
    class: SymbolId,
    source_id: SourceId,
    source_member_order: Vec<SymbolId>,
    classfile_path: String,
    tasty_path: String,
    tasty_plan: &'session ValidatedClassStatePlan<'session, 'session>,
    compatibility: &'session ValidatedJvmTastyClass<'session, 'session, 'session, 'session>,
}

fn emit_validated_pair<'session>(
    input: ValidatedPairInput<'session>,
) -> Result<PairedJvmTastyArtifacts, PairedArtifactError> {
    let ValidatedPairInput {
        jvm_unit,
        class,
        source_id,
        source_member_order,
        classfile_path,
        tasty_path,
        tasty_plan,
        compatibility,
    } = input;
    let tasty = tasty_plan
        .pickle()
        .map_err(PairedArtifactError::TastyPickle)?;
    if tasty.intended_tasty_path() != tasty_path {
        return Err(PairedArtifactError::ArtifactMismatch(
            "TASTy producer path differs from the paired plan",
        ));
    }

    let tasty_file = TastyFile::parse_and_validate_scala_3_9(tasty.bytes())
        .map_err(PairedArtifactError::TastyFile)?;
    if (
        tasty_file.header().major_version,
        tasty_file.header().minor_version,
    ) != (28, 9)
        || tasty_file.header().experimental_version != 0
    {
        return Err(PairedArtifactError::ArtifactMismatch(
            "TASTy producer did not emit format 28.9.0",
        ));
    }
    let tasty_link_id = TastyLinkId::from_bytes(tasty_file.header().uuid);

    let generated: GeneratedClass = emit_class_with_tasty_link(
        jvm_unit,
        class,
        tasty_plan.plan().definitions,
        tasty_link_id,
    )
    .map_err(PairedArtifactError::Jvm)?;
    let expected_classfile_path = format!("{}.class", compatibility.internal_name());
    if generated.internal_name() != compatibility.internal_name()
        || classfile_path != expected_classfile_path
    {
        return Err(PairedArtifactError::ArtifactMismatch(
            "JVM producer path differs from the paired source identity",
        ));
    }

    let mut reader = ClassFileReader::new(generated.bytes());
    let class_file = ClassFile::decode(&mut reader).map_err(PairedArtifactError::ClassFile)?;
    if !reader.is_at_end() {
        return Err(PairedArtifactError::ArtifactMismatch(
            "JVM class-file decoder left trailing bytes",
        ));
    }
    class_file
        .validate_references()
        .map_err(PairedArtifactError::ClassFileReference)?;
    if class_file.version.major != 65 || class_file.version.minor != 0 {
        return Err(PairedArtifactError::ArtifactMismatch(
            "JVM producer did not emit class-file version 65.0",
        ));
    }
    let internal_name = class_file
        .constant_pool
        .class_name(class_file.this_class)
        .map_err(PairedArtifactError::ClassFileReference)?;
    if internal_name != compatibility.internal_name() {
        return Err(PairedArtifactError::ArtifactMismatch(
            "decoded JVM class name differs from the paired source identity",
        ));
    }
    let metadata = compatibility.jvm_metadata();
    if class_file.access_flags != metadata.identity().access_flags() {
        return Err(PairedArtifactError::ArtifactMismatch(
            "decoded JVM class flags differ from the checked source ABI",
        ));
    }
    let expected_parent = metadata
        .superclass()
        .ok_or(PairedArtifactError::ArtifactMismatch(
            "checked JVM ABI has no direct parent",
        ))?
        .parent_internal_name();
    let actual_parent = class_file
        .super_class
        .map(|index| {
            class_file
                .constant_pool
                .class_name(index)
                .map_err(PairedArtifactError::ClassFileReference)
        })
        .transpose()?;
    if actual_parent != Some(expected_parent) {
        return Err(PairedArtifactError::ArtifactMismatch(
            "decoded JVM superclass differs from the checked source ABI",
        ));
    }
    validate_classfile_tasty_link(&class_file, tasty.bytes())
        .map_err(PairedArtifactError::Contract)?;
    validate_decoded_members(&class_file, compatibility)
        .map_err(PairedArtifactError::ArtifactMismatch)?;

    Ok(PairedJvmTastyArtifacts {
        class,
        source_id,
        source_member_order,
        classfile_path,
        classfile_bytes: generated.into_bytes(),
        tasty_path,
        tasty_bytes: tasty.bytes().to_vec(),
        tasty_link_id,
    })
}

fn validate_decoded_members(
    class_file: &ClassFile<'_>,
    proof: &dotty_jvm_tasty_contract::ValidatedJvmTastyClass<'_, '_, '_, '_>,
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
                    .map_err(|_| "decoded JVM field name does not resolve")?
                    .to_owned(),
                class_file
                    .constant_pool
                    .utf8(field.descriptor_index)
                    .map_err(|_| "decoded JVM field descriptor does not resolve")?
                    .to_owned(),
                field.access_flags.0,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    expected_fields.sort();
    actual_fields.sort();
    if actual_fields != expected_fields {
        return Err("decoded JVM fields differ from the checked source ABI");
    }

    let constructor = metadata
        .constructor()
        .ok_or("checked source ABI has no primary constructor")?;
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
                    .map_err(|_| "decoded JVM method name does not resolve")?
                    .to_owned(),
                class_file
                    .constant_pool
                    .utf8(method.descriptor_index)
                    .map_err(|_| "decoded JVM method descriptor does not resolve")?
                    .to_owned(),
                method.access_flags.0,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    expected_methods.sort();
    actual_methods.sort();
    if actual_methods != expected_methods {
        return Err("decoded JVM methods differ from the checked source ABI");
    }
    Ok(())
}
