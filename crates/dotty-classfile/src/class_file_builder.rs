//! Owned construction of JVM classes, fields, and method declarations.
//!
//! The builder owns names, descriptors, signatures, and method code. Typed
//! [`CodeBuilder`] closures provide symbolic references and infer stack/local
//! limits; [`MethodBuilder::code`] preserves an explicit raw-byte path.
//! At finalization it interns symbolic references with
//! [`crate::constant_pool_builder::ConstantPoolBuilder`] and creates a
//! temporary borrowed [`crate::class_file::ClassFile`] view for encoding.
//! This avoids self-referential storage. Typed code receives stack and local
//! analysis with exact checks for the supported reference subset; it does not
//! receive full JVM verification.
//!
//! ```
//! use dotty_classfile::classfile::{
//!     ClassFileBuilder, ClassFileVersion, FieldAccessFlags, ACC_PRIVATE,
//! };
//!
//! let mut class = ClassFileBuilder::new(
//!     "example/Calculator",
//!     ClassFileVersion { major: 65, minor: 0 },
//! )?;
//! class.add_field("answer", "I", FieldAccessFlags(ACC_PRIVATE))?;
//! class.add_static_method("answer", "()I", |code| {
//!     code.iconst(42)?;
//!     code.ireturn()?;
//!     Ok(())
//! })?;
//! let bytes = class.encode()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::access_flags::{
    ACC_ABSTRACT, ACC_ANNOTATION, ACC_BRIDGE, ACC_ENUM, ACC_FINAL, ACC_INTERFACE, ACC_MODULE,
    ACC_NATIVE, ACC_PRIVATE, ACC_PROTECTED, ACC_PUBLIC, ACC_STATIC, ACC_STRICT, ACC_SUPER,
    ACC_SYNCHRONIZED, ACC_SYNTHETIC, ACC_TRANSIENT, ACC_VARARGS, ACC_VOLATILE, ClassAccessFlags,
    FieldAccessFlags, MethodAccessFlags,
};
use crate::attribute::{Attribute, CodeAttribute, RawAttribute};
use crate::bytecode::code_builder::{
    self as code_builder, CodeBuilder, CodeBuilderError, FieldOperation, MethodOperation,
    SymbolicBranch, SymbolicCodeElement, SymbolicInstruction,
};
use crate::bytecode::{
    AnalysisError, AssembleError, BranchInstruction, CfgCodeAnalyzer, CodeElement, CodeSequence,
    ControlFlowGraph, Instruction, LdcConstant, StackMapFrameGenerationError, StackMapTable,
    generate_stack_map_frames_with_receiver,
};
use crate::class_file::{ClassFile, ClassFileVersion};
use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use crate::constant_pool_builder::{ConstantPoolBuildError, ConstantPoolBuilder};
use crate::descriptor::{DescriptorError, FieldType, MethodDescriptor};
use crate::encode_error::EncodeError;
use crate::field::FieldInfo;
use crate::method::MethodInfo;
use crate::signature::{ClassSignature, FieldSignature, MethodSignature, SignatureError};
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_SUPER_CLASS: &str = "java/lang/Object";
const METHOD_ACCESS_MASK: u16 = ACC_PUBLIC | ACC_PRIVATE | ACC_PROTECTED;
const FIELD_ACCESS_MASK: u16 = ACC_PUBLIC | ACC_PRIVATE | ACC_PROTECTED;
const CLASS_ALLOWED_FLAGS: u16 = ACC_PUBLIC
    | ACC_FINAL
    | ACC_SUPER
    | ACC_INTERFACE
    | ACC_ABSTRACT
    | ACC_SYNTHETIC
    | ACC_ANNOTATION
    | ACC_ENUM;
const FIELD_ALLOWED_FLAGS: u16 = FIELD_ACCESS_MASK
    | ACC_STATIC
    | ACC_FINAL
    | ACC_VOLATILE
    | ACC_TRANSIENT
    | ACC_SYNTHETIC
    | ACC_ENUM;
const METHOD_ALLOWED_FLAGS: u16 = METHOD_ACCESS_MASK
    | ACC_STATIC
    | ACC_FINAL
    | ACC_SYNCHRONIZED
    | ACC_BRIDGE
    | ACC_VARARGS
    | ACC_NATIVE
    | ACC_ABSTRACT
    | ACC_STRICT
    | ACC_SYNTHETIC;

/// A typed failure while constructing or encoding a class file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassFileBuildError {
    /// The class, superclass, interface, or descriptor object name is not a
    /// valid JVM internal name.
    InvalidInternalName,
    /// A field or method name is empty or violates the JVM unqualified-name
    /// rules for the relevant member kind.
    InvalidMemberName,
    /// The requested class-file version is outside the crate's supported
    /// non-preview range.
    UnsupportedVersion { major: u16, minor: u16 },
    /// An interface method reference requires class-file version 52 or newer.
    InterfaceMethodReferenceRequiresVersion { major: u16 },
    /// A field or method descriptor failed the existing descriptor parser.
    InvalidDescriptor(DescriptorError),
    /// A method descriptor exceeds the JVMS 255 parameter-slot limit.
    TooManyParameterSlots,
    /// Automatic branched-code generation requires class-file major version 65 or newer.
    BranchingRequiresVersion { major: u16 },
    /// A generic signature failed the existing signature parser.
    InvalidSignature(SignatureError),
    /// Class, field, or method flags contain a combination forbidden by the
    /// JVMS for this builder's supported class-file model.
    InvalidAccessFlags,
    /// The class-file superclass relation is invalid for the declared class.
    InvalidSuperclass,
    /// A field instruction's staticness does not match the declared field.
    FieldOperationStaticnessMismatch {
        /// Zero-based instruction position.
        instruction: usize,
        /// Referenced field name.
        field: String,
        /// Whether the declared field is static.
        field_is_static: bool,
        /// Whether the bytecode operation is static.
        operation_is_static: bool,
    },
    /// The same interface was added more than once.
    DuplicateInterface,
    /// A class cannot declare itself as an implemented interface.
    SelfInterface,
    /// The class already has a field with this name and descriptor.
    DuplicateField,
    /// The class already has a method with this name and descriptor.
    DuplicateMethod,
    /// A constructor descriptor must return void, and constructors cannot
    /// have static, final, synchronized, bridge, native, or abstract flags.
    InvalidConstructor,
    /// A class initializer must be static with descriptor `()V`.
    InvalidClassInitializer,
    /// A class, field, or method already has the requested singleton
    /// attribute.
    DuplicateAttribute,
    /// Abstract or native methods cannot contain a Code attribute.
    CodeNotAllowed,
    /// A method already has raw code attached.
    DuplicateCode,
    /// A concrete method must have raw code before class encoding.
    MissingCode,
    /// Raw method code must have a length in the JVM range 1..=65535.
    InvalidCodeLength { length: usize },
    /// A symbolic code builder operation used an invalid local or pool index.
    CodeBuilder(CodeBuilderError),
    /// Bytecode assembly rejected a generated instruction sequence.
    Assemble(AssembleError),
    /// Bytecode analysis rejected a generated instruction sequence.
    Analysis(AnalysisError),
    /// Control-flow graph construction rejected a generated branched method.
    ControlFlow(crate::bytecode::ControlFlowGraphError),
    /// Stack-map frame generation rejected a generated branched method.
    StackMapFrames(StackMapFrameGenerationError),
    /// Internal generated-code staging did not match its method declaration.
    InvalidCodePlan,
    /// Constant-pool construction failed.
    ConstantPool(ConstantPoolBuildError),
    /// The existing class-file encoder rejected the finalized model.
    Encode(EncodeError),
}

impl fmt::Display for ClassFileBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInternalName => formatter.write_str("invalid JVM internal class name"),
            Self::InvalidMemberName => formatter.write_str("invalid JVM member name"),
            Self::UnsupportedVersion { major, minor } => write!(
                formatter,
                "class-file version {major}.{minor} is unsupported"
            ),
            Self::InterfaceMethodReferenceRequiresVersion { major } => write!(
                formatter,
                "interface method references for invokestatic and invokespecial require class-file version 52 or newer (found {major})"
            ),
            Self::InvalidDescriptor(error) => write!(formatter, "invalid JVM descriptor: {error}"),
            Self::TooManyParameterSlots => {
                formatter.write_str("method descriptor exceeds 255 parameter slots")
            }
            Self::BranchingRequiresVersion { major } => write!(
                formatter,
                "automatic branched code generation requires class-file major version 65 or newer (found {major})"
            ),
            Self::InvalidSignature(error) => write!(formatter, "invalid JVM signature: {error}"),
            Self::InvalidAccessFlags => {
                formatter.write_str("invalid combination of JVM access flags")
            }
            Self::InvalidSuperclass => formatter.write_str("invalid JVM superclass relation"),
            Self::FieldOperationStaticnessMismatch {
                instruction,
                field,
                field_is_static,
                operation_is_static,
            } => write!(
                formatter,
                "instruction {instruction} uses {} access for field {field}, which is {}",
                if *operation_is_static {
                    "static"
                } else {
                    "instance"
                },
                if *field_is_static {
                    "static"
                } else {
                    "an instance field"
                }
            ),
            Self::DuplicateInterface => formatter.write_str("duplicate interface declaration"),
            Self::SelfInterface => formatter.write_str("a class cannot implement itself"),
            Self::DuplicateField => formatter.write_str("duplicate field name and descriptor"),
            Self::DuplicateMethod => formatter.write_str("duplicate method name and descriptor"),
            Self::InvalidConstructor => formatter.write_str("invalid constructor declaration"),
            Self::InvalidClassInitializer => {
                formatter.write_str("class initializer must be static with descriptor ()V")
            }
            Self::DuplicateAttribute => formatter.write_str("duplicate singleton attribute"),
            Self::CodeNotAllowed => {
                formatter.write_str("abstract and native methods cannot have Code")
            }
            Self::DuplicateCode => formatter.write_str("method already has Code"),
            Self::MissingCode => formatter.write_str("concrete method is missing Code"),
            Self::InvalidCodeLength { length } => write!(
                formatter,
                "raw method code length {length} is outside the JVM range 1..=65535"
            ),
            Self::CodeBuilder(error) => error.fmt(formatter),
            Self::Assemble(error) => error.fmt(formatter),
            Self::Analysis(error) => error.fmt(formatter),
            Self::ControlFlow(error) => error.fmt(formatter),
            Self::StackMapFrames(error) => error.fmt(formatter),
            Self::InvalidCodePlan => formatter.write_str("invalid generated method code plan"),
            Self::ConstantPool(error) => error.fmt(formatter),
            Self::Encode(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ClassFileBuildError {}

impl From<ConstantPoolBuildError> for ClassFileBuildError {
    fn from(error: ConstantPoolBuildError) -> Self {
        Self::ConstantPool(error)
    }
}

impl From<EncodeError> for ClassFileBuildError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

#[derive(Debug, Clone)]
struct OwnedField {
    access_flags: FieldAccessFlags,
    name: String,
    descriptor: String,
    signature: Option<String>,
}

#[derive(Debug, Clone)]
struct OwnedMethod {
    access_flags: MethodAccessFlags,
    name: String,
    descriptor: String,
    signature: Option<String>,
    code: Option<OwnedMethodCode>,
}

#[derive(Debug, Clone)]
enum OwnedMethodCode {
    Raw(OwnedCode),
    Built(CodeBuilder),
}

enum ResolvedMethodCode {
    Linear(Vec<Instruction>),
    Branched(CodeSequence),
}

#[derive(Debug, Clone)]
struct OwnedCode {
    max_stack: u16,
    max_locals: u16,
    bytes: Vec<u8>,
    attributes: Vec<OwnedCodeAttribute>,
}

#[derive(Debug, Clone)]
struct OwnedCodeAttribute {
    name_index: ConstantPoolIndex,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
enum OwnedClassAttribute {
    SourceFile(String),
    Signature(String),
}

/// An owned builder for a new class file.
///
/// The constructor requires an explicit [`ClassFileVersion`]. Ordinary
/// classes default to `java/lang/Object` as superclass and carry `ACC_SUPER`;
/// use [`set_super_class`](Self::set_super_class) to select another superclass
/// or to declare `java/lang/Object` itself. Members are emitted in declaration
/// order. The builder preserves the supplied flags and does not synthesize
/// members or bytecode.
///
/// [`encode`](Self::encode) preserves raw code bytes with explicit stack/local
/// limits and also supports typed methods through [`CodeBuilder`]. Typed code
/// resolves symbolic references, assembles instructions, and computes stack
/// and local bounds before the constant pool is frozen. The analysis checks
/// supported instruction categories and straight-line bounds. Branched code
/// uses the bounded CFG analyzer, including exact initialized references, and
/// emits `StackMapTable` frames for the supported class-file version 65+
/// subset and rejects unreachable blocks whose verifier state cannot be
/// inferred; neither path is full JVM
/// verification. A typed-code closure commits its method only after the
/// closure and label validation succeed; failures during final encoding return
/// an error and produce no class bytes.
pub struct ClassFileBuilder {
    class_name: String,
    version: ClassFileVersion,
    access_flags: ClassAccessFlags,
    super_class: Option<String>,
    verification_hierarchy: crate::bytecode::analysis::ClassHierarchy,
    interfaces: Vec<String>,
    fields: Vec<OwnedField>,
    methods: Vec<OwnedMethod>,
    attributes: Vec<OwnedClassAttribute>,
}

impl ClassFileBuilder {
    /// Creates a class builder with an explicit class internal name and
    /// class-file version.
    pub fn new(
        internal_name: impl Into<String>,
        version: ClassFileVersion,
    ) -> Result<Self, ClassFileBuildError> {
        let class_name = internal_name.into();
        validate_internal_name(&class_name)?;
        if !version.is_compatible() {
            return Err(ClassFileBuildError::UnsupportedVersion {
                major: version.major,
                minor: version.minor,
            });
        }
        let super_class = (class_name != DEFAULT_SUPER_CLASS).then(|| DEFAULT_SUPER_CLASS.into());
        Ok(Self {
            class_name,
            version,
            access_flags: ClassAccessFlags(ACC_SUPER),
            super_class,
            verification_hierarchy: crate::bytecode::analysis::ClassHierarchy::default(),
            interfaces: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            attributes: Vec::new(),
        })
    }

    /// Replaces the class access flags without modifying their bits.
    pub fn set_access_flags(
        &mut self,
        access_flags: ClassAccessFlags,
    ) -> Result<&mut Self, ClassFileBuildError> {
        validate_class_flags(access_flags)?;
        self.access_flags = access_flags;
        Ok(self)
    }

    /// Sets the superclass internal name. `None` is allowed only for
    /// `java/lang/Object`.
    pub fn set_super_class(
        &mut self,
        internal_name: Option<&str>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        if let Some(name) = internal_name {
            validate_internal_name(name)?;
            if self.class_name == DEFAULT_SUPER_CLASS
                || name == self.class_name
                || (self.access_flags.is_interface() && name != DEFAULT_SUPER_CLASS)
            {
                return Err(ClassFileBuildError::InvalidSuperclass);
            }
        } else if self.class_name != DEFAULT_SUPER_CLASS {
            return Err(ClassFileBuildError::InvalidSuperclass);
        }
        self.super_class = internal_name.map(str::to_owned);
        Ok(self)
    }

    /// Sets the explicit JVM superclass proof graph used by bounded bytecode
    /// reference verification and stack-map frame generation.
    pub fn set_verification_hierarchy(
        &mut self,
        hierarchy: crate::bytecode::analysis::ClassHierarchy,
    ) -> &mut Self {
        self.verification_hierarchy = hierarchy;
        self
    }

    /// Adds an implemented interface in declaration order.
    pub fn add_interface(
        &mut self,
        internal_name: impl Into<String>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        let internal_name = internal_name.into();
        validate_internal_name(&internal_name)?;
        if internal_name == self.class_name {
            return Err(ClassFileBuildError::SelfInterface);
        }
        if self.interfaces.iter().any(|name| name == &internal_name) {
            return Err(ClassFileBuildError::DuplicateInterface);
        }
        self.interfaces.push(internal_name);
        Ok(self)
    }

    /// Adds a class-level `SourceFile` attribute.
    pub fn source_file(
        &mut self,
        file_name: impl Into<String>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        if self
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, OwnedClassAttribute::SourceFile(_)))
        {
            return Err(ClassFileBuildError::DuplicateAttribute);
        }
        self.attributes
            .push(OwnedClassAttribute::SourceFile(file_name.into()));
        Ok(self)
    }

    /// Adds the generic class `Signature` attribute after parsing it with the
    /// existing JVMS signature parser.
    pub fn signature(
        &mut self,
        signature: impl Into<String>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        if self
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, OwnedClassAttribute::Signature(_)))
        {
            return Err(ClassFileBuildError::DuplicateAttribute);
        }
        let signature = signature.into();
        ClassSignature::parse(&signature).map_err(ClassFileBuildError::InvalidSignature)?;
        self.attributes
            .push(OwnedClassAttribute::Signature(signature));
        Ok(self)
    }

    /// Adds a field declaration, validating its name and descriptor before
    /// storing it. Fields retain their insertion order.
    pub fn add_field(
        &mut self,
        name: impl Into<String>,
        descriptor: impl Into<String>,
        access_flags: FieldAccessFlags,
    ) -> Result<FieldBuilder<'_>, ClassFileBuildError> {
        let name = name.into();
        let descriptor = descriptor.into();
        validate_member_name(&name, false)?;
        validate_field_descriptor(&descriptor)?;
        validate_field_flags_for_class(access_flags, self.access_flags.is_interface())?;
        if self
            .fields
            .iter()
            .any(|field| field.name == name && field.descriptor == descriptor)
        {
            return Err(ClassFileBuildError::DuplicateField);
        }
        self.fields.push(OwnedField {
            access_flags,
            name,
            descriptor,
            signature: None,
        });
        let field = self.fields.last_mut().expect("field was just appended");
        Ok(FieldBuilder { field })
    }

    /// Adds a method declaration and returns a handle for attaching an
    /// optional generic signature and owned raw Code body.
    pub fn add_method(
        &mut self,
        name: impl Into<String>,
        descriptor: impl Into<String>,
        access_flags: MethodAccessFlags,
    ) -> Result<MethodBuilder<'_>, ClassFileBuildError> {
        let name = name.into();
        let descriptor = descriptor.into();
        validate_member_name(&name, true)?;
        let parsed_descriptor = validate_method_descriptor(&descriptor, !access_flags.is_static())?;
        validate_method_flags_for_class(
            access_flags,
            self.access_flags.is_interface(),
            self.version.major,
            &name,
        )?;
        validate_special_method(&name, &parsed_descriptor, access_flags)?;
        if self
            .methods
            .iter()
            .any(|method| method.name == name && method.descriptor == descriptor)
        {
            return Err(ClassFileBuildError::DuplicateMethod);
        }
        self.methods.push(OwnedMethod {
            access_flags,
            name,
            descriptor,
            signature: None,
            code: None,
        });
        let method = self.methods.last_mut().expect("method was just appended");
        Ok(MethodBuilder { method })
    }

    /// Adds a public static method whose typed code is assembled and analyzed
    /// automatically during [`encode`](Self::encode).
    pub fn add_static_method<F>(
        &mut self,
        name: impl Into<String>,
        descriptor: impl Into<String>,
        code: F,
    ) -> Result<&mut Self, ClassFileBuildError>
    where
        F: FnOnce(&mut CodeBuilder) -> Result<(), CodeBuilderError>,
    {
        self.add_code_method(
            name,
            descriptor,
            MethodAccessFlags(ACC_PUBLIC | ACC_STATIC),
            code,
        )
    }

    /// Adds a public instance method whose typed code is assembled and
    /// analyzed automatically during [`encode`](Self::encode).
    pub fn add_instance_method<F>(
        &mut self,
        name: impl Into<String>,
        descriptor: impl Into<String>,
        code: F,
    ) -> Result<&mut Self, ClassFileBuildError>
    where
        F: FnOnce(&mut CodeBuilder) -> Result<(), CodeBuilderError>,
    {
        self.add_code_method(name, descriptor, MethodAccessFlags(ACC_PUBLIC), code)
    }

    /// Adds a method with caller-selected flags and a typed code-building
    /// closure. The method is appended only after the closure succeeds.
    pub fn add_code_method<F>(
        &mut self,
        name: impl Into<String>,
        descriptor: impl Into<String>,
        access_flags: MethodAccessFlags,
        code: F,
    ) -> Result<&mut Self, ClassFileBuildError>
    where
        F: FnOnce(&mut CodeBuilder) -> Result<(), CodeBuilderError>,
    {
        let name = name.into();
        let descriptor = descriptor.into();
        validate_member_name(&name, true)?;
        let parsed_descriptor = validate_method_descriptor(&descriptor, !access_flags.is_static())?;
        validate_method_flags_for_class(
            access_flags,
            self.access_flags.is_interface(),
            self.version.major,
            &name,
        )?;
        validate_special_method(&name, &parsed_descriptor, access_flags)?;
        if access_flags.is_abstract() || access_flags.is_native() {
            return Err(ClassFileBuildError::CodeNotAllowed);
        }
        if self
            .methods
            .iter()
            .any(|method| method.name == name && method.descriptor == descriptor)
        {
            return Err(ClassFileBuildError::DuplicateMethod);
        }

        let mut code_builder = CodeBuilder::new();
        code(&mut code_builder).map_err(ClassFileBuildError::CodeBuilder)?;
        code_builder
            .validate()
            .map_err(ClassFileBuildError::CodeBuilder)?;
        if code_builder.has_branches() && self.version.major < 65 {
            return Err(ClassFileBuildError::BranchingRequiresVersion {
                major: self.version.major,
            });
        }
        self.methods.push(OwnedMethod {
            access_flags,
            name,
            descriptor,
            signature: None,
            code: Some(OwnedMethodCode::Built(code_builder)),
        });
        Ok(self)
    }

    /// Adds a method from an already-built symbolic instruction sequence.
    ///
    /// The code is validated and analyzed by the same path as
    /// [`add_code_method`](Self::add_code_method) during class encoding.
    /// This is useful when a higher-level backend lowers typed trees before it
    /// creates the class builder.
    pub fn add_prebuilt_code_method(
        &mut self,
        name: impl Into<String>,
        descriptor: impl Into<String>,
        access_flags: MethodAccessFlags,
        code: code_builder::CodeBuilder,
    ) -> Result<&mut Self, ClassFileBuildError> {
        self.add_code_method(name, descriptor, access_flags, move |builder| {
            *builder = code;
            Ok(())
        })
    }

    /// Finalizes and encodes the class by interning all symbolic references
    /// before freezing the constant pool, then using the existing class-file
    /// encoder.
    pub fn encode(self) -> Result<Vec<u8>, ClassFileBuildError> {
        validate_class_flags(self.access_flags)?;
        validate_superclass(
            &self.class_name,
            self.super_class.as_deref(),
            self.access_flags.is_interface(),
        )?;
        let verification_hierarchy = if let Some(super_class) = self.super_class.as_deref() {
            self.verification_hierarchy
                .with_edge(&self.class_name, super_class)
                .map_err(crate::bytecode::analysis::AnalysisError::InvalidClassHierarchy)
                .map_err(ClassFileBuildError::Analysis)?
        } else {
            self.verification_hierarchy.clone()
        };
        if self
            .methods
            .iter()
            .any(|method| method.access_flags.is_abstract())
            && !self.access_flags.is_abstract()
        {
            return Err(ClassFileBuildError::InvalidAccessFlags);
        }
        for field in &self.fields {
            validate_field_flags_for_class(field.access_flags, self.access_flags.is_interface())?;
        }
        for method in &self.methods {
            validate_method_flags_for_class(
                method.access_flags,
                self.access_flags.is_interface(),
                self.version.major,
                &method.name,
            )?;
            let parsed =
                validate_method_descriptor(&method.descriptor, !method.access_flags.is_static())?;
            validate_special_method(&method.name, &parsed, method.access_flags)?;
        }
        for method in &self.methods {
            if method.access_flags.is_abstract() || method.access_flags.is_native() {
                if method.code.is_some() {
                    return Err(ClassFileBuildError::CodeNotAllowed);
                }
            } else if method.code.is_none() {
                return Err(ClassFileBuildError::MissingCode);
            }
        }

        let mut pool_builder = ConstantPoolBuilder::new();
        let this_class = pool_builder.class(&self.class_name)?;
        let super_class = self
            .super_class
            .as_deref()
            .map(|name| pool_builder.class(name))
            .transpose()?;
        let interfaces = self
            .interfaces
            .iter()
            .map(|name| pool_builder.class(name))
            .collect::<Result<Vec<_>, _>>()?;

        let mut fields = Vec::with_capacity(self.fields.len());
        for field in &self.fields {
            fields.push(FieldIndices {
                name: pool_builder.utf8(&field.name)?,
                descriptor: pool_builder.utf8(&field.descriptor)?,
                signature: field
                    .signature
                    .as_deref()
                    .map(|signature| pool_builder.utf8(signature))
                    .transpose()?,
            });
        }

        let mut methods = Vec::with_capacity(self.methods.len());
        for method in &self.methods {
            methods.push(MethodIndices {
                name: pool_builder.utf8(&method.name)?,
                descriptor: pool_builder.utf8(&method.descriptor)?,
                signature: method
                    .signature
                    .as_deref()
                    .map(|signature| pool_builder.utf8(signature))
                    .transpose()?,
            });
        }

        // Resolve every symbolic code reference while the pool builder is
        // still mutable. The resulting operand indices remain stable when the
        // pool is finalized below.
        let resolved_code = self
            .methods
            .iter()
            .map(|method| match &method.code {
                Some(OwnedMethodCode::Built(code)) => {
                    resolve_code(code, &mut pool_builder, self.version.major).map(Some)
                }
                Some(OwnedMethodCode::Raw(_)) | None => Ok(None),
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Analyze branched code against a snapshot while the builder remains
        // mutable. Exact reference classes needed by StackMapTable must be
        // interned before the final pool is frozen; the append-only builder
        // guarantees these indices remain stable afterward.
        let preview_pool = pool_builder.clone().finish()?;
        if !self.access_flags.contains(ACC_SUPER) {
            for resolved in resolved_code.iter().flatten() {
                for (instruction, operation) in resolved_code_instructions(resolved)? {
                    if let Instruction::InvokeSpecial(reference) = operation
                        && special_reference_is_ordinary_method(
                            reference.index().get(),
                            &preview_pool,
                        )
                    {
                        return Err(ClassFileBuildError::Analysis(
                            AnalysisError::DirectSuperMethodRequiresAccSuper { instruction },
                        ));
                    }
                }
            }
        }
        for resolved in resolved_code.iter().flatten() {
            validate_field_operation_staticness(
                resolved,
                &self.class_name,
                &self.fields,
                &preview_pool,
            )?;
        }
        let mut frame_class_identities = std::collections::BTreeSet::new();
        for (method, resolved) in self.methods.iter().zip(&resolved_code) {
            let Some(ResolvedMethodCode::Branched(sequence)) = resolved else {
                continue;
            };
            if method.name == "<init>" {
                // Preserve the explicit constructor-frame guard below.
                continue;
            }
            let graph =
                ControlFlowGraph::build(sequence).map_err(ClassFileBuildError::ControlFlow)?;
            CfgCodeAnalyzer::validate_all_instructions_with_references(&graph)
                .map_err(ClassFileBuildError::Analysis)?;
            let flow = CfgCodeAnalyzer::analyze_validated_with_current_class_and_hierarchy(
                &graph,
                &method.name,
                &method.descriptor,
                method.access_flags.is_static(),
                &preview_pool,
                Some(this_class),
                &verification_hierarchy,
            )
            .map_err(ClassFileBuildError::Analysis)?;
            frame_class_identities.extend(
                crate::bytecode::stack_map_frames::frame_class_identities(&flow),
            );
        }
        let frame_classes = frame_class_identities
            .into_iter()
            .map(|identity| {
                pool_builder
                    .class(identity.internal_name())
                    .map(|index| (identity, index))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        if self.methods.iter().any(|method| method.code.is_some()) {
            pool_builder.utf8("Code")?;
        }
        let stack_map_table_name = self
            .methods
            .iter()
            .any(|method| {
                matches!(
                    &method.code,
                    Some(OwnedMethodCode::Built(code)) if code.has_branches()
                )
            })
            .then(|| pool_builder.utf8("StackMapTable"))
            .transpose()?;
        if self
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, OwnedClassAttribute::SourceFile(_)))
        {
            pool_builder.utf8("SourceFile")?;
        }
        if self
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, OwnedClassAttribute::Signature(_)))
            || fields.iter().any(|field| field.signature.is_some())
            || methods.iter().any(|method| method.signature.is_some())
        {
            pool_builder.utf8("Signature")?;
        }

        let class_attributes = self
            .attributes
            .iter()
            .map(|attribute| match attribute {
                OwnedClassAttribute::SourceFile(file_name) => {
                    pool_builder.utf8(file_name).map(Attribute::SourceFile)
                }
                OwnedClassAttribute::Signature(signature) => {
                    pool_builder.utf8(signature).map(Attribute::Signature)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let constant_pool = pool_builder.finish()?;

        let finalized_code = self
            .methods
            .iter()
            .zip(resolved_code)
            .map(|(method, resolved)| match (&method.code, resolved) {
                (Some(OwnedMethodCode::Raw(code)), None) => Ok(Some(code.clone())),
                (
                    Some(OwnedMethodCode::Built(_)),
                    Some(ResolvedMethodCode::Linear(instructions)),
                ) => {
                    let bytes = crate::bytecode::assemble_validated(&instructions, &constant_pool)
                        .map_err(ClassFileBuildError::Assemble)?;
                    let descriptor = crate::descriptor::MethodDescriptor::parse(&method.descriptor)
                        .map_err(AnalysisError::InvalidMethodDescriptor)
                        .map_err(ClassFileBuildError::Analysis)?;
                    let constructor_body = if method.name == "<init>" {
                        &instructions[2.min(instructions.len())..]
                    } else {
                        &instructions[..]
                    };
                    let requires_exact_references = method_descriptor_has_references(&descriptor)
                        || constructor_body.iter().copied().any(|instruction| {
                            instruction_requires_exact_references(instruction, &constant_pool)
                        });
                    let analysis = if method.name == "<init>" {
                        let super_class = super_class
                            .ok_or(AnalysisError::InvalidConstructorSuperCall { instruction: 0 })
                            .map_err(ClassFileBuildError::Analysis)?;
                        crate::bytecode::analyze_validated_constructor_body_with_hierarchy(
                            &method.descriptor,
                            &instructions,
                            &constant_pool,
                            this_class,
                            super_class,
                            &verification_hierarchy,
                        )
                    } else if requires_exact_references {
                        crate::bytecode::analyze_validated_with_current_class_and_hierarchy(
                            &method.name,
                            &method.descriptor,
                            method.access_flags.is_static(),
                            &instructions,
                            &constant_pool,
                            Some(this_class),
                            &verification_hierarchy,
                        )
                    } else {
                        crate::bytecode::analyze_validated(
                            &method.descriptor,
                            method.access_flags.is_static(),
                            &instructions,
                            &constant_pool,
                        )
                    }
                    .map_err(ClassFileBuildError::Analysis)?;
                    Ok(Some(OwnedCode {
                        max_stack: analysis.max_stack,
                        max_locals: analysis.max_locals,
                        bytes,
                        attributes: Vec::new(),
                    }))
                }
                (Some(OwnedMethodCode::Built(_)), Some(ResolvedMethodCode::Branched(sequence))) => {
                    let name_index =
                        stack_map_table_name.ok_or(ClassFileBuildError::InvalidCodePlan)?;
                    let graph = ControlFlowGraph::build(&sequence)
                        .map_err(ClassFileBuildError::ControlFlow)?;
                    if method.name == "<init>" {
                        // The integer-only flow model does not track the
                        // transition from `uninitializedThis` to the class
                        // type after an invokespecial constructor call.
                        return Err(ClassFileBuildError::StackMapFrames(
                            StackMapFrameGenerationError::UnsupportedFlow {
                                block: graph.entry_block(),
                                state: crate::bytecode::UnsupportedStackMapState::InstanceReceiver,
                            },
                        ));
                    }
                    CfgCodeAnalyzer::validate_all_instructions_with_references(&graph)
                        .map_err(ClassFileBuildError::Analysis)?;
                    let flow = CfgCodeAnalyzer::analyze_validated_with_current_class_and_hierarchy(
                        &graph,
                        &method.name,
                        &method.descriptor,
                        method.access_flags.is_static(),
                        &constant_pool,
                        Some(this_class),
                        &verification_hierarchy,
                    )
                    .map_err(ClassFileBuildError::Analysis)?;
                    for (block, entry) in graph.blocks().iter().zip(flow.block_entries()) {
                        if entry.is_none() {
                            return Err(ClassFileBuildError::Analysis(
                                AnalysisError::UnreachableBlock { block: block.id },
                            ));
                        }
                    }
                    let assembled = crate::bytecode::assembler::assemble_labeled_pool_validated(
                        &sequence,
                        &constant_pool,
                    )
                    .map_err(ClassFileBuildError::Assemble)?;
                    let generated = generate_stack_map_frames_with_receiver(
                        &flow,
                        &graph,
                        &assembled,
                        &constant_pool,
                        this_class,
                        &frame_classes,
                    )
                    .map_err(ClassFileBuildError::StackMapFrames)?;
                    let payload = StackMapTable {
                        frames: generated.frames,
                    }
                    .encode_payload_with_layout(&constant_pool, &assembled.bytes, &assembled.layout)
                    .map_err(ClassFileBuildError::Encode)?;
                    Ok(Some(OwnedCode {
                        max_stack: generated.code_analysis.max_stack,
                        max_locals: generated.code_analysis.max_locals,
                        bytes: assembled.bytes,
                        attributes: vec![OwnedCodeAttribute {
                            name_index,
                            payload,
                        }],
                    }))
                }
                (None, None) => Ok(None),
                _ => Err(ClassFileBuildError::InvalidCodePlan),
            })
            .collect::<Result<Vec<_>, ClassFileBuildError>>()?;

        let fields = self
            .fields
            .iter()
            .zip(fields)
            .map(|(field, indices)| FieldInfo {
                access_flags: field.access_flags,
                name_index: indices.name,
                descriptor_index: indices.descriptor,
                attributes: indices
                    .signature
                    .into_iter()
                    .map(Attribute::Signature)
                    .collect(),
            })
            .collect();
        let methods = self
            .methods
            .iter()
            .zip(methods)
            .zip(finalized_code.iter())
            .map(|((method, indices), code)| {
                let mut attributes = Vec::with_capacity(
                    code.as_ref().map_or(1, |code| code.attributes.len() + 1)
                        + usize::from(code.is_some()),
                );
                if let Some(signature) = indices.signature {
                    attributes.push(Attribute::Signature(signature));
                }
                if let Some(code) = code {
                    attributes.push(Attribute::Code(CodeAttribute {
                        max_stack: code.max_stack,
                        max_locals: code.max_locals,
                        code: &code.bytes,
                        exception_table: vec![],
                        attributes: code
                            .attributes
                            .iter()
                            .map(|attribute| {
                                Attribute::Other(RawAttribute {
                                    name_index: attribute.name_index,
                                    bytes: &attribute.payload,
                                })
                            })
                            .collect(),
                    }));
                }
                MethodInfo {
                    access_flags: method.access_flags,
                    name_index: indices.name,
                    descriptor_index: indices.descriptor,
                    attributes,
                }
            })
            .collect();
        let class_file = ClassFile {
            version: self.version,
            constant_pool,
            access_flags: self.access_flags,
            this_class,
            super_class,
            interfaces,
            fields,
            methods,
            attributes: class_attributes,
        };
        class_file.encode().map_err(ClassFileBuildError::Encode)
    }
}

fn validate_field_operation_staticness(
    code: &ResolvedMethodCode,
    current_class: &str,
    fields: &[OwnedField],
    pool: &ConstantPool,
) -> Result<(), ClassFileBuildError> {
    let instructions: Vec<(usize, Instruction)> =
        match code {
            ResolvedMethodCode::Linear(instructions) => {
                instructions.iter().copied().enumerate().collect()
            }
            ResolvedMethodCode::Branched(sequence) => {
                let graph =
                    ControlFlowGraph::build(sequence).map_err(ClassFileBuildError::ControlFlow)?;
                graph
                    .blocks()
                    .iter()
                    .flat_map(|block| {
                        block.instructions.iter().enumerate().filter_map(
                            move |(offset, element)| match element {
                                crate::bytecode::CodeElement::Instruction(instruction) => {
                                    Some((block.instruction_range.start + offset, *instruction))
                                }
                                crate::bytecode::CodeElement::Branch(_)
                                | crate::bytecode::CodeElement::Label(_) => None,
                            },
                        )
                    })
                    .collect()
            }
        };
    for (instruction, operation) in instructions {
        let (reference, operation_is_static) = match operation {
            Instruction::GetField(reference) | Instruction::PutField(reference) => {
                (reference, false)
            }
            Instruction::GetStatic(reference) | Instruction::PutStatic(reference) => {
                (reference, true)
            }
            _ => continue,
        };
        let Some(ConstantPoolEntry::Fieldref {
            class_index,
            name_and_type_index,
        }) = pool.get(reference.index().get())
        else {
            continue;
        };
        if pool.class_name(*class_index).ok() != Some(current_class) {
            continue;
        }
        let Ok((name, descriptor)) = pool.name_and_type(*name_and_type_index) else {
            continue;
        };
        let Some(field) = fields
            .iter()
            .find(|field| field.name == name && field.descriptor == descriptor)
        else {
            continue;
        };
        let field_is_static = field.access_flags.0 & ACC_STATIC != 0;
        if field_is_static != operation_is_static {
            return Err(ClassFileBuildError::FieldOperationStaticnessMismatch {
                instruction,
                field: name.to_owned(),
                field_is_static,
                operation_is_static,
            });
        }
    }
    Ok(())
}

fn method_descriptor_has_references(descriptor: &MethodDescriptor) -> bool {
    descriptor
        .parameters
        .iter()
        .chain(descriptor.return_type.iter())
        .any(|ty| matches!(ty, FieldType::Object(_) | FieldType::Array(_)))
}

fn instruction_requires_exact_references(instruction: Instruction, pool: &ConstantPool) -> bool {
    match instruction {
        Instruction::AConstNull
        | Instruction::AStore(_)
        | Instruction::AReturn
        | Instruction::GetField(_)
        | Instruction::PutField(_)
        | Instruction::InvokeVirtual(_)
        | Instruction::New(_)
        | Instruction::Ldc(LdcConstant::Reference(_)) => true,
        Instruction::InvokeSpecial(reference) => {
            special_reference_is_constructor(reference.index().get(), pool)
                || special_reference_is_ordinary_method(reference.index().get(), pool)
        }
        Instruction::GetStatic(reference) | Instruction::PutStatic(reference) => {
            field_reference_has_reference_type(reference.index().get(), pool)
        }
        Instruction::InvokeStatic(reference) => {
            method_reference_has_reference_type(reference.index().get(), pool)
        }
        _ => false,
    }
}

fn resolved_code_instructions(
    code: &ResolvedMethodCode,
) -> Result<Vec<(usize, Instruction)>, ClassFileBuildError> {
    match code {
        ResolvedMethodCode::Linear(instructions) => {
            Ok(instructions.iter().copied().enumerate().collect())
        }
        ResolvedMethodCode::Branched(sequence) => {
            let graph =
                ControlFlowGraph::build(sequence).map_err(ClassFileBuildError::ControlFlow)?;
            Ok(graph
                .blocks()
                .iter()
                .flat_map(|block| {
                    block
                        .instructions
                        .iter()
                        .enumerate()
                        .filter_map(move |(offset, element)| match element {
                            CodeElement::Instruction(instruction) => {
                                Some((block.instruction_range.start + offset, *instruction))
                            }
                            CodeElement::Branch(_) | CodeElement::Label(_) => None,
                        })
                })
                .collect())
        }
    }
}

fn special_reference_is_ordinary_method(index: ConstantPoolIndex, pool: &ConstantPool) -> bool {
    let Some(ConstantPoolEntry::Methodref {
        name_and_type_index,
        ..
    }) = pool.get(index)
    else {
        return false;
    };
    pool.name_and_type(*name_and_type_index)
        .is_ok_and(|(name, _)| name != "<init>")
}

fn special_reference_is_constructor(index: ConstantPoolIndex, pool: &ConstantPool) -> bool {
    match pool.get(index) {
        Some(
            ConstantPoolEntry::Methodref {
                name_and_type_index,
                ..
            }
            | ConstantPoolEntry::InterfaceMethodref {
                name_and_type_index,
                ..
            },
        ) => pool
            .name_and_type(*name_and_type_index)
            .map(|(name, _)| name == "<init>")
            .unwrap_or(true),
        _ => true,
    }
}

fn field_reference_has_reference_type(index: ConstantPoolIndex, pool: &ConstantPool) -> bool {
    let Some(ConstantPoolEntry::Fieldref {
        name_and_type_index,
        ..
    }) = pool.get(index)
    else {
        return true;
    };
    let Ok((_, descriptor)) = pool.name_and_type(*name_and_type_index) else {
        return true;
    };
    FieldType::parse(descriptor)
        .map(|ty| matches!(ty, FieldType::Object(_) | FieldType::Array(_)))
        .unwrap_or(true)
}

fn method_reference_has_reference_type(index: ConstantPoolIndex, pool: &ConstantPool) -> bool {
    let Some(
        ConstantPoolEntry::Methodref {
            name_and_type_index,
            ..
        }
        | ConstantPoolEntry::InterfaceMethodref {
            name_and_type_index,
            ..
        },
    ) = pool.get(index)
    else {
        return true;
    };
    let Ok((_, descriptor)) = pool.name_and_type(*name_and_type_index) else {
        return true;
    };
    MethodDescriptor::parse(descriptor)
        .map(|descriptor| method_descriptor_has_references(&descriptor))
        .unwrap_or(true)
}

fn resolve_code(
    code: &CodeBuilder,
    pool: &mut ConstantPoolBuilder,
    major_version: u16,
) -> Result<ResolvedMethodCode, ClassFileBuildError> {
    if code.has_branches() {
        let mut sequence = CodeSequence::new();
        let mut labels = Vec::with_capacity(code.label_count());
        for _ in 0..code.label_count() {
            labels.push(
                sequence
                    .new_label()
                    .map_err(|_| ClassFileBuildError::InvalidCodePlan)?,
            );
        }
        for element in &code.elements {
            match element {
                SymbolicCodeElement::Instruction(instruction) => {
                    sequence.instruction(resolve_instruction(instruction, pool, major_version)?);
                }
                SymbolicCodeElement::Label(label) => {
                    let index = code_builder::CodeBuilder::label_index(label);
                    let label = labels
                        .get(index)
                        .cloned()
                        .ok_or(ClassFileBuildError::InvalidCodePlan)?;
                    sequence
                        .bind(label)
                        .map_err(|_| ClassFileBuildError::InvalidCodePlan)?;
                }
                SymbolicCodeElement::Branch { operation, target } => {
                    let index = code_builder::CodeBuilder::label_index(target);
                    let target = labels
                        .get(index)
                        .cloned()
                        .ok_or(ClassFileBuildError::InvalidCodePlan)?;
                    sequence.branch(match operation {
                        SymbolicBranch::Goto => BranchInstruction::Goto(target),
                        SymbolicBranch::GotoWide => BranchInstruction::GotoWide(target),
                        SymbolicBranch::IfEq => BranchInstruction::IfEq(target),
                        SymbolicBranch::IfNe => BranchInstruction::IfNe(target),
                        SymbolicBranch::IfICmpEq => BranchInstruction::IfICmpEq(target),
                        SymbolicBranch::IfICmpNe => BranchInstruction::IfICmpNe(target),
                        SymbolicBranch::IfICmpLt => BranchInstruction::IfICmpLt(target),
                        SymbolicBranch::IfICmpGe => BranchInstruction::IfICmpGe(target),
                        SymbolicBranch::IfICmpGt => BranchInstruction::IfICmpGt(target),
                        SymbolicBranch::IfICmpLe => BranchInstruction::IfICmpLe(target),
                    });
                }
            }
        }
        return Ok(ResolvedMethodCode::Branched(sequence));
    }

    let mut instructions = Vec::new();
    for element in &code.elements {
        match element {
            SymbolicCodeElement::Instruction(instruction) => {
                instructions.push(resolve_instruction(instruction, pool, major_version)?);
            }
            SymbolicCodeElement::Label(_) => {}
            SymbolicCodeElement::Branch { .. } => {
                return Err(ClassFileBuildError::InvalidCodePlan);
            }
        }
    }
    Ok(ResolvedMethodCode::Linear(instructions))
}

fn resolve_instruction(
    instruction: &SymbolicInstruction,
    pool: &mut ConstantPoolBuilder,
    major_version: u16,
) -> Result<Instruction, ClassFileBuildError> {
    match instruction {
        SymbolicInstruction::Typed(instruction) => Ok(*instruction),
        SymbolicInstruction::Integer(value) => {
            let index = pool.integer(*value)?;
            code_builder::integer_load(index).map_err(ClassFileBuildError::CodeBuilder)
        }
        SymbolicInstruction::Long(value) => {
            let index = pool.long(*value)?;
            code_builder::long_load(index).map_err(ClassFileBuildError::CodeBuilder)
        }
        SymbolicInstruction::New(owner) => {
            validate_internal_name(owner)?;
            let index = pool.class(owner)?;
            let reference = code_builder::typed_class_reference(index)
                .map_err(ClassFileBuildError::CodeBuilder)?;
            Ok(Instruction::New(reference))
        }
        SymbolicInstruction::Field {
            operation,
            owner,
            name,
            descriptor,
        } => {
            validate_internal_name(owner)?;
            validate_member_name(name, false)?;
            validate_field_descriptor(descriptor)?;
            let index = pool.field_ref(owner, name, descriptor)?;
            let reference = code_builder::typed_field_reference(index)
                .map_err(ClassFileBuildError::CodeBuilder)?;
            Ok(match operation {
                FieldOperation::Get => Instruction::GetField(reference),
                FieldOperation::Put => Instruction::PutField(reference),
                FieldOperation::GetStatic => Instruction::GetStatic(reference),
                FieldOperation::PutStatic => Instruction::PutStatic(reference),
            })
        }
        SymbolicInstruction::Method {
            operation,
            owner,
            name,
            descriptor,
        } => {
            validate_internal_name(owner)?;
            validate_member_name(name, true)?;
            let has_receiver = !matches!(
                operation,
                MethodOperation::Static | MethodOperation::StaticInterface
            );
            validate_method_descriptor(descriptor, has_receiver)?;
            if matches!(
                operation,
                MethodOperation::StaticInterface | MethodOperation::SpecialInterface
            ) && major_version < 52
            {
                return Err(
                    ClassFileBuildError::InterfaceMethodReferenceRequiresVersion {
                        major: major_version,
                    },
                );
            }
            let index = match operation {
                MethodOperation::StaticInterface | MethodOperation::SpecialInterface => {
                    pool.interface_method_ref(owner, name, descriptor)?
                }
                MethodOperation::Static | MethodOperation::Virtual | MethodOperation::Special => {
                    pool.method_ref(owner, name, descriptor)?
                }
            };
            code_builder::typed_method_reference(*operation, index)
                .map_err(ClassFileBuildError::CodeBuilder)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FieldIndices {
    name: ConstantPoolIndex,
    descriptor: ConstantPoolIndex,
    signature: Option<ConstantPoolIndex>,
}

#[derive(Debug, Clone, Copy)]
struct MethodIndices {
    name: ConstantPoolIndex,
    descriptor: ConstantPoolIndex,
    signature: Option<ConstantPoolIndex>,
}

/// A short-lived handle for attaching supported attributes to a newly added
/// field. The field itself is owned by its [`ClassFileBuilder`].
pub struct FieldBuilder<'a> {
    field: &'a mut OwnedField,
}

impl FieldBuilder<'_> {
    /// Adds a parsed generic field `Signature` attribute.
    pub fn signature(
        &mut self,
        signature: impl Into<String>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        if self.field.signature.is_some() {
            return Err(ClassFileBuildError::DuplicateAttribute);
        }
        let signature = signature.into();
        FieldSignature::parse(&signature).map_err(ClassFileBuildError::InvalidSignature)?;
        self.field.signature = Some(signature);
        Ok(self)
    }
}

/// A short-lived handle for attaching supported attributes and raw code to a
/// newly added method. The method declaration is owned by its
/// [`ClassFileBuilder`].
pub struct MethodBuilder<'a> {
    method: &'a mut OwnedMethod,
}

impl MethodBuilder<'_> {
    /// Adds a parsed generic method `Signature` attribute.
    pub fn signature(
        &mut self,
        signature: impl Into<String>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        if self.method.signature.is_some() {
            return Err(ClassFileBuildError::DuplicateAttribute);
        }
        let signature = signature.into();
        MethodSignature::parse(&signature).map_err(ClassFileBuildError::InvalidSignature)?;
        self.method.signature = Some(signature);
        Ok(self)
    }

    /// Attaches owned, opaque JVM bytecode with explicit `max_stack` and
    /// `max_locals` values. The builder checks only the JVMS Code length and
    /// method Code rules; it does not decode or verify instructions.
    pub fn code(
        &mut self,
        max_stack: u16,
        max_locals: u16,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<&mut Self, ClassFileBuildError> {
        if self.method.access_flags.is_abstract() || self.method.access_flags.is_native() {
            return Err(ClassFileBuildError::CodeNotAllowed);
        }
        if self.method.code.is_some() {
            return Err(ClassFileBuildError::DuplicateCode);
        }
        let bytes = bytes.into();
        if !(1..=usize::from(u16::MAX)).contains(&bytes.len()) {
            return Err(ClassFileBuildError::InvalidCodeLength {
                length: bytes.len(),
            });
        }
        self.method.code = Some(OwnedMethodCode::Raw(OwnedCode {
            max_stack,
            max_locals,
            bytes,
            attributes: Vec::new(),
        }));
        Ok(self)
    }
}

fn validate_internal_name(name: &str) -> Result<(), ClassFileBuildError> {
    if name.is_empty()
        || name
            .split('/')
            .any(|part| part.is_empty() || part.chars().any(|c| matches!(c, '.' | ';' | '[')))
    {
        return Err(ClassFileBuildError::InvalidInternalName);
    }
    Ok(())
}

fn validate_member_name(name: &str, method: bool) -> Result<(), ClassFileBuildError> {
    if name.is_empty() || name.chars().any(|c| matches!(c, '.' | ';' | '[' | '/')) {
        return Err(ClassFileBuildError::InvalidMemberName);
    }
    if method
        && name.chars().any(|c| matches!(c, '<' | '>'))
        && !matches!(name, "<init>" | "<clinit>")
    {
        return Err(ClassFileBuildError::InvalidMemberName);
    }
    Ok(())
}

fn validate_descriptor_names(field_type: &FieldType) -> Result<(), ClassFileBuildError> {
    match field_type {
        FieldType::Object(name) => validate_internal_name(name),
        FieldType::Array(component) => validate_descriptor_names(component),
        FieldType::Byte
        | FieldType::Char
        | FieldType::Double
        | FieldType::Float
        | FieldType::Int
        | FieldType::Long
        | FieldType::Short
        | FieldType::Boolean => Ok(()),
    }
}

fn validate_field_descriptor(descriptor: &str) -> Result<FieldType, ClassFileBuildError> {
    let field_type =
        FieldType::parse(descriptor).map_err(ClassFileBuildError::InvalidDescriptor)?;
    validate_descriptor_names(&field_type)?;
    Ok(field_type)
}

fn validate_method_descriptor(
    descriptor: &str,
    has_receiver: bool,
) -> Result<MethodDescriptor, ClassFileBuildError> {
    let parsed =
        MethodDescriptor::parse(descriptor).map_err(ClassFileBuildError::InvalidDescriptor)?;
    let mut slots = usize::from(has_receiver);
    for parameter in &parsed.parameters {
        validate_descriptor_names(parameter)?;
        slots += match parameter {
            FieldType::Long | FieldType::Double => 2,
            _ => 1,
        };
    }
    if let Some(return_type) = &parsed.return_type {
        validate_descriptor_names(return_type)?;
    }
    if slots > 255 {
        return Err(ClassFileBuildError::TooManyParameterSlots);
    }
    Ok(parsed)
}

fn validate_class_flags(flags: ClassAccessFlags) -> Result<(), ClassFileBuildError> {
    let visibility = (flags.0 & METHOD_ACCESS_MASK).count_ones();
    if flags.0 & !CLASS_ALLOWED_FLAGS != 0
        || visibility > 1
        || (flags.contains(ACC_FINAL) && flags.contains(ACC_ABSTRACT))
        || (flags.contains(ACC_INTERFACE) && !flags.contains(ACC_ABSTRACT))
        || (flags.contains(ACC_INTERFACE) && flags.contains(ACC_SUPER))
        || (flags.contains(ACC_ANNOTATION) && !flags.contains(ACC_INTERFACE))
        || (flags.contains(ACC_ENUM) && flags.contains(ACC_INTERFACE))
        || flags.contains(ACC_MODULE)
    {
        return Err(ClassFileBuildError::InvalidAccessFlags);
    }
    Ok(())
}

fn validate_field_flags(flags: FieldAccessFlags) -> Result<(), ClassFileBuildError> {
    if flags.0 & !FIELD_ALLOWED_FLAGS != 0
        || (flags.0 & FIELD_ACCESS_MASK).count_ones() > 1
        || (flags.contains(ACC_FINAL) && flags.contains(ACC_VOLATILE))
    {
        return Err(ClassFileBuildError::InvalidAccessFlags);
    }
    Ok(())
}

fn validate_field_flags_for_class(
    flags: FieldAccessFlags,
    is_interface: bool,
) -> Result<(), ClassFileBuildError> {
    validate_field_flags(flags)?;
    if is_interface {
        let required = ACC_PUBLIC | ACC_STATIC | ACC_FINAL;
        let forbidden = ACC_PRIVATE | ACC_PROTECTED | ACC_VOLATILE | ACC_TRANSIENT;
        if flags.0 & required != required || flags.0 & forbidden != 0 {
            return Err(ClassFileBuildError::InvalidAccessFlags);
        }
    }
    Ok(())
}

fn validate_method_flags(flags: MethodAccessFlags) -> Result<(), ClassFileBuildError> {
    let abstract_conflicts =
        ACC_PRIVATE | ACC_STATIC | ACC_FINAL | ACC_SYNCHRONIZED | ACC_NATIVE | ACC_STRICT;
    if (flags.0 & METHOD_ACCESS_MASK).count_ones() > 1
        || (flags.is_abstract() && flags.0 & abstract_conflicts != 0)
    {
        return Err(ClassFileBuildError::InvalidAccessFlags);
    }
    Ok(())
}

fn validate_method_flags_for_class(
    flags: MethodAccessFlags,
    is_interface: bool,
    major_version: u16,
    name: &str,
) -> Result<(), ClassFileBuildError> {
    validate_method_flags(flags)?;
    if flags.0 & !METHOD_ALLOWED_FLAGS != 0
        || (major_version >= 61 && flags.contains(ACC_STRICT))
        || (is_interface
            && (name == "<init>"
                || flags.contains(ACC_PROTECTED)
                || flags.contains(ACC_FINAL)
                || flags.contains(ACC_SYNCHRONIZED)
                || flags.contains(ACC_NATIVE)
                || (flags.contains(ACC_PRIVATE) && major_version < 53)))
    {
        return Err(ClassFileBuildError::InvalidAccessFlags);
    }
    if is_interface && name != "<clinit>" {
        let required = if major_version < 52 {
            ACC_PUBLIC | ACC_ABSTRACT
        } else if major_version < 53 {
            ACC_PUBLIC
        } else {
            0
        };
        if flags.0 & required != required
            || (major_version >= 52 && (flags.contains(ACC_PUBLIC) == flags.contains(ACC_PRIVATE)))
        {
            return Err(ClassFileBuildError::InvalidAccessFlags);
        }
    }
    Ok(())
}

fn validate_special_method(
    name: &str,
    descriptor: &MethodDescriptor,
    flags: MethodAccessFlags,
) -> Result<(), ClassFileBuildError> {
    if name == "<init>" {
        let forbidden = ACC_STATIC
            | ACC_FINAL
            | ACC_SYNCHRONIZED
            | crate::access_flags::ACC_BRIDGE
            | ACC_NATIVE
            | ACC_ABSTRACT;
        if descriptor.return_type.is_some() || flags.0 & forbidden != 0 {
            return Err(ClassFileBuildError::InvalidConstructor);
        }
    } else if name == "<clinit>"
        && (!flags.contains(ACC_STATIC)
            || !descriptor.parameters.is_empty()
            || descriptor.return_type.is_some())
    {
        return Err(ClassFileBuildError::InvalidClassInitializer);
    }
    Ok(())
}

fn validate_superclass(
    class_name: &str,
    super_class: Option<&str>,
    is_interface: bool,
) -> Result<(), ClassFileBuildError> {
    match (class_name, super_class) {
        (DEFAULT_SUPER_CLASS, None) => Ok(()),
        (DEFAULT_SUPER_CLASS, Some(_)) | (_, None) => Err(ClassFileBuildError::InvalidSuperclass),
        (_, Some(super_name)) if is_interface && super_name != DEFAULT_SUPER_CLASS => {
            Err(ClassFileBuildError::InvalidSuperclass)
        }
        (_, Some(super_name)) if class_name == super_name => {
            Err(ClassFileBuildError::InvalidSuperclass)
        }
        (_, Some(_)) => Ok(()),
    }
}
