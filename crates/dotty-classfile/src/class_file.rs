use crate::access_flags::{ACC_MODULE, ClassAccessFlags};
use crate::attribute::{
    Attribute, AttributeError, MODULE_ACC_MANDATED, MODULE_ACC_OPEN, MODULE_ACC_STATIC_PHASE,
    MODULE_ACC_SYNTHETIC, MODULE_ACC_TRANSITIVE, ModuleAttribute, decode_attributes,
    read_index_list, read_optional_index, validate_attributes,
};
use crate::constant_pool::{
    ConstantPool, ConstantPoolError, ConstantPoolIndex, PoolRefError, read_index,
};
use crate::encode_error::EncodeError;
use crate::field::FieldInfo;
use crate::method::MethodInfo;
use crate::reader::{ReadError, Reader};
use crate::writer::Writer;
use std::fmt;

/// The `ClassFile` magic number (JVMS §4.1).
pub const CLASS_MAGIC: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassFileHeaderError {
    Read(ReadError),
    InvalidMagic { actual: [u8; 4] },
}

impl fmt::Display for ClassFileHeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidMagic { actual } => write!(
                formatter,
                "invalid class file magic: {:02x} {:02x} {:02x} {:02x}",
                actual[0], actual[1], actual[2], actual[3]
            ),
        }
    }
}

impl std::error::Error for ClassFileHeaderError {}

impl From<ReadError> for ClassFileHeaderError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

/// The lowest `major_version` the JVMS has ever defined (JDK 1.0.2;
/// `docs/classfile-format-jdk25.md` §2.1).
pub const MIN_MAJOR_VERSION: u16 = 45;

/// This project's compatibility ceiling: JDK 26, the highest release covered
/// by a real compatibility corpus. A `major_version` above this is a JDK
/// newer than this decoder has been validated against, not necessarily an
/// invalid file.
pub const MAX_MAJOR_VERSION: u16 = 70;

/// The `minor_version` that marks a class as compiled against **preview
/// features** of its major version (JVMS §4.1, `major_version >= 56`).
/// Only the exact JDK that introduced those preview features can load
/// such a class; this decoder never can, since it has no notion of
/// "preview features enabled".
pub const PREVIEW_MINOR_VERSION: u16 = 0xFFFF;

/// The `minor_version`/`major_version` pair of a class file (JVMS §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassFileVersion {
    pub major: u16,
    pub minor: u16,
}

impl ClassFileVersion {
    /// Decodes the class file's magic number and version fields (the first
    /// eight bytes of a `ClassFile`, JVMS §4.1).
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, ClassFileHeaderError> {
        let magic_bytes = reader.read_bytes(CLASS_MAGIC.len())?;
        let mut actual = [0u8; CLASS_MAGIC.len()];
        actual.copy_from_slice(magic_bytes);

        if actual != CLASS_MAGIC {
            return Err(ClassFileHeaderError::InvalidMagic { actual });
        }

        let minor = reader.read_u16()?;
        let major = reader.read_u16()?;

        Ok(Self { major, minor })
    }

    /// Whether this decoder can safely give this version's class file a
    /// semantic reading, per `docs/classfile-format-jdk25.md` §2.1's
    /// compatible-range rules:
    ///
    /// - `major_version` must fall within the historically valid,
    ///   JDK-26-or-below range (`MIN_MAJOR_VERSION..=MAX_MAJOR_VERSION`);
    /// - `minor_version` must not be [`PREVIEW_MINOR_VERSION`] (this
    ///   decoder never has preview features enabled for any major
    ///   version, so a preview class is always incompatible);
    /// - for `major_version >= 56` (JDK 12+), `minor_version` must be `0`
    ///   (the only other value the JVMS allows there is
    ///   `PREVIEW_MINOR_VERSION`, already rejected above).
    pub fn is_compatible(&self) -> bool {
        if !(MIN_MAJOR_VERSION..=MAX_MAJOR_VERSION).contains(&self.major) {
            return false;
        }
        if self.minor == PREVIEW_MINOR_VERSION {
            return false;
        }
        if self.major >= 56 && self.minor != 0 {
            return false;
        }
        true
    }
}

impl fmt::Display for ClassFileVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// The full `ClassFile` structure (JVMS §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassFile<'a> {
    pub version: ClassFileVersion,
    pub constant_pool: ConstantPool,
    pub access_flags: ClassAccessFlags,
    pub this_class: ConstantPoolIndex,
    /// `None` only for `java.lang.Object`.
    pub super_class: Option<ConstantPoolIndex>,
    pub interfaces: Vec<ConstantPoolIndex>,
    pub fields: Vec<FieldInfo<'a>>,
    pub methods: Vec<MethodInfo<'a>>,
    pub attributes: Vec<Attribute<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassFileError {
    Read(ReadError),
    Header(ClassFileHeaderError),
    ConstantPool(ConstantPoolError),
    Attribute(AttributeError),
}

/// A violation found by the explicit `module-info.class` semantic validator.
/// Structural decoding remains available for ordinary and malformed class
/// files; callers opt into this stricter JPMS contract when cataloging a
/// module descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleDescriptorError {
    NotModuleDescriptor,
    ConstantPool(PoolRefError),
    UnsupportedVersion { major: u16, minor: u16 },
    InvalidAccessFlags { flags: u16 },
    WrongThisClass { name: String },
    HasSuperclass,
    HasInterfaces { count: usize },
    HasFields { count: usize },
    HasMethods { count: usize },
    MissingModuleAttribute,
    DuplicateAttribute { name: &'static str },
    DisallowedAttribute { name: String },
    InvalidModuleName { name: String },
    InvalidPackageName { name: String },
    InvalidDirective { reason: &'static str },
    DuplicateDirective { table: &'static str, name: String },
}

impl fmt::Display for ModuleDescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotModuleDescriptor => formatter.write_str("class file does not have ACC_MODULE"),
            Self::ConstantPool(error) => error.fmt(formatter),
            Self::UnsupportedVersion { major, minor } => write!(
                formatter,
                "module descriptor version {major}.{minor} is unsupported by the current class-file policy"
            ),
            Self::InvalidAccessFlags { flags } => write!(
                formatter,
                "module descriptor access flags {flags:#06x} must contain only ACC_MODULE"
            ),
            Self::WrongThisClass { name } => write!(
                formatter,
                "module descriptor this_class must be module-info, got {name:?}"
            ),
            Self::HasSuperclass => formatter.write_str("module descriptor must have no superclass"),
            Self::HasInterfaces { count } => {
                write!(formatter, "module descriptor has {count} interface(s)")
            }
            Self::HasFields { count } => {
                write!(formatter, "module descriptor has {count} field(s)")
            }
            Self::HasMethods { count } => {
                write!(formatter, "module descriptor has {count} method(s)")
            }
            Self::MissingModuleAttribute => {
                formatter.write_str("module descriptor is missing its Module attribute")
            }
            Self::DuplicateAttribute { name } => {
                write!(
                    formatter,
                    "module descriptor has duplicate {name} attributes"
                )
            }
            Self::DisallowedAttribute { name } => write!(
                formatter,
                "predefined attribute {name:?} is not permitted on module-info"
            ),
            Self::InvalidModuleName { name } => {
                write!(formatter, "invalid JVMS module name {name:?}")
            }
            Self::InvalidPackageName { name } => {
                write!(formatter, "invalid JVMS package name {name:?}")
            }
            Self::InvalidDirective { reason } => formatter.write_str(reason),
            Self::DuplicateDirective { table, name } => {
                write!(formatter, "duplicate {name:?} in Module {table} table")
            }
        }
    }
}

impl std::error::Error for ModuleDescriptorError {}

impl fmt::Display for ClassFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Header(error) => error.fmt(formatter),
            Self::ConstantPool(error) => error.fmt(formatter),
            Self::Attribute(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ClassFileError {}

impl From<ReadError> for ClassFileError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<ClassFileHeaderError> for ClassFileError {
    fn from(error: ClassFileHeaderError) -> Self {
        Self::Header(error)
    }
}

impl From<ConstantPoolError> for ClassFileError {
    fn from(error: ConstantPoolError) -> Self {
        Self::ConstantPool(error)
    }
}

impl From<AttributeError> for ClassFileError {
    fn from(error: AttributeError) -> Self {
        Self::Attribute(error)
    }
}

impl<'a> ClassFile<'a> {
    /// Encodes a complete class file in JVMS §4.1 order.
    ///
    /// This structural serializer preserves constant-pool slots, duplicate
    /// entries, flags, indices, and attribute order. It accepts the project's
    /// supported non-preview versions (major 45 through 70), and does not
    /// synthesize constants or JVM metadata.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        if !self.version.is_compatible() {
            return Err(EncodeError::UnsupportedVersion {
                major: self.version.major,
                minor: self.version.minor,
            });
        }

        crate::writer::checked_u16_len(self.interfaces.len())?;
        crate::writer::checked_u16_len(self.fields.len())?;
        crate::writer::checked_u16_len(self.methods.len())?;
        self.validate_references()
            .map_err(|error| encode_pool_ref_error(error, self.constant_pool.entry_count()))?;

        // Prepare every fallible section before assembling the final bytes.
        let constant_pool = self.constant_pool.encode()?;
        let fields = self
            .fields
            .iter()
            .map(|field| field.encode(&self.constant_pool))
            .collect::<Result<Vec<_>, _>>()?;
        let methods = self
            .methods
            .iter()
            .map(|method| method.encode(&self.constant_pool))
            .collect::<Result<Vec<_>, _>>()?;
        let attributes =
            crate::attribute::encode_attributes(&self.attributes, &self.constant_pool)?;

        let mut writer = Writer::new();
        writer.write_bytes(&CLASS_MAGIC);
        writer.write_u2(self.version.minor);
        writer.write_u2(self.version.major);
        writer.write_bytes(&constant_pool);
        writer.write_u2(self.access_flags.0);
        writer.write_u2(self.this_class.0);
        writer.write_u2(self.super_class.map_or(0, |index| index.0));
        writer.write_u2_length(self.interfaces.len())?;
        for interface in &self.interfaces {
            writer.write_u2(interface.0);
        }
        writer.write_u2_length(fields.len())?;
        for field in fields {
            writer.write_bytes(&field);
        }
        writer.write_u2_length(methods.len())?;
        for method in methods {
            writer.write_bytes(&method);
        }
        writer.write_bytes(&attributes);
        Ok(writer.into_bytes())
    }

    /// Decodes a complete `ClassFile` (JVMS §4.1): header, constant pool,
    /// access flags, `this_class`/`super_class`/interfaces, fields, methods,
    /// and top-level attributes.
    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, ClassFileError> {
        let version = ClassFileVersion::decode(reader)?;
        let constant_pool = ConstantPool::decode(reader)?;

        let access_flags = ClassAccessFlags(reader.read_u16()?);
        let this_class = read_index(reader)?;
        let super_class = read_optional_index(reader)?;
        let interfaces = read_index_list(reader)?;

        let fields_count = reader.read_u16()?;
        let mut fields = Vec::with_capacity(usize::from(fields_count));
        for _ in 0..fields_count {
            fields.push(FieldInfo::decode(reader, &constant_pool)?);
        }

        let methods_count = reader.read_u16()?;
        let mut methods = Vec::with_capacity(usize::from(methods_count));
        for _ in 0..methods_count {
            methods.push(MethodInfo::decode(reader, &constant_pool)?);
        }

        let attributes = decode_attributes(reader, &constant_pool)?;

        Ok(Self {
            version,
            constant_pool,
            access_flags,
            this_class,
            super_class,
            interfaces,
            fields,
            methods,
            attributes,
        })
    }

    /// Validates that every constant pool reference reachable from this
    /// `ClassFile` — `this_class`, `super_class`, `interfaces`, and every
    /// field/method/attribute's indices, recursively — resolves to the
    /// expected entry kind (JVMS §4.1). Stops at the first bad reference.
    pub fn validate_references(&self) -> Result<(), PoolRefError> {
        let pool = &self.constant_pool;

        pool.class_name(self.this_class)?;
        if let Some(super_class) = self.super_class {
            pool.class_name(super_class)?;
        }
        for interface in &self.interfaces {
            pool.class_name(*interface)?;
        }
        for field in &self.fields {
            field.validate_references(pool)?;
        }
        for method in &self.methods {
            method.validate_references(pool)?;
        }
        validate_attributes(&self.attributes, pool)
    }

    /// Validates this class file as a JPMS module descriptor (JVMS §4.1 and
    /// §4.7.25–§4.7.27). This is deliberately separate from [`Self::decode`]
    /// and [`Self::validate_references`], so callers can continue to inspect
    /// ordinary class files without applying module-only shape rules.
    ///
    /// The check is bounded by the already-decoded classfile tables and the
    /// existing attribute nesting limit. It validates module directive
    /// references, names, cardinality, and the Java 21 `java.base` rules used
    /// by this crate's pinned JPMS oracle; it does not resolve modules or
    /// verify referenced classes exist.
    pub fn validate_module_descriptor(&self) -> Result<(), ModuleDescriptorError> {
        if !self.access_flags.contains(ACC_MODULE) {
            return Err(ModuleDescriptorError::NotModuleDescriptor);
        }
        if self.version.major < 53 || !self.version.is_compatible() {
            return Err(ModuleDescriptorError::UnsupportedVersion {
                major: self.version.major,
                minor: self.version.minor,
            });
        }
        if self.access_flags.0 != ACC_MODULE {
            return Err(ModuleDescriptorError::InvalidAccessFlags {
                flags: self.access_flags.0,
            });
        }
        self.validate_references()
            .map_err(ModuleDescriptorError::ConstantPool)?;

        let this_name = self
            .constant_pool
            .class_name(self.this_class)
            .map_err(ModuleDescriptorError::ConstantPool)?;
        if this_name != "module-info" {
            return Err(ModuleDescriptorError::WrongThisClass {
                name: this_name.to_owned(),
            });
        }
        if self.super_class.is_some() {
            return Err(ModuleDescriptorError::HasSuperclass);
        }
        if !self.interfaces.is_empty() {
            return Err(ModuleDescriptorError::HasInterfaces {
                count: self.interfaces.len(),
            });
        }
        if !self.fields.is_empty() {
            return Err(ModuleDescriptorError::HasFields {
                count: self.fields.len(),
            });
        }
        if !self.methods.is_empty() {
            return Err(ModuleDescriptorError::HasMethods {
                count: self.methods.len(),
            });
        }

        let mut module_count = 0;
        let mut packages_count = 0;
        let mut main_class_count = 0;
        let mut module = None;
        for attribute in &self.attributes {
            let name = descriptor_attribute_name(attribute, &self.constant_pool)?;
            match attribute {
                Attribute::Module(value) => {
                    module_count += 1;
                    module = Some(value);
                }
                Attribute::ModulePackages(_) => packages_count += 1,
                Attribute::ModuleMainClass(_) => main_class_count += 1,
                _ => {}
            }

            if is_predefined_attribute(&name)
                && !matches!(
                    name.as_str(),
                    "Module"
                        | "ModulePackages"
                        | "ModuleMainClass"
                        | "InnerClasses"
                        | "SourceFile"
                        | "SourceDebugExtension"
                        | "RuntimeVisibleAnnotations"
                        | "RuntimeInvisibleAnnotations"
                )
            {
                return Err(ModuleDescriptorError::DisallowedAttribute { name });
            }
        }
        if module_count == 0 {
            return Err(ModuleDescriptorError::MissingModuleAttribute);
        }
        if module_count > 1 {
            return Err(ModuleDescriptorError::DuplicateAttribute { name: "Module" });
        }
        if packages_count > 1 {
            return Err(ModuleDescriptorError::DuplicateAttribute {
                name: "ModulePackages",
            });
        }
        if main_class_count > 1 {
            return Err(ModuleDescriptorError::DuplicateAttribute {
                name: "ModuleMainClass",
            });
        }

        let module = module.ok_or(ModuleDescriptorError::MissingModuleAttribute)?;
        validate_module_metadata(self, module)
    }
}

fn descriptor_attribute_name(
    attribute: &Attribute<'_>,
    pool: &ConstantPool,
) -> Result<String, ModuleDescriptorError> {
    let name = match attribute {
        Attribute::Other(raw) => pool
            .utf8(raw.name_index)
            .map_err(ModuleDescriptorError::ConstantPool)?,
        Attribute::ConstantValue(_) => "ConstantValue",
        Attribute::Signature(_) => "Signature",
        Attribute::Exceptions(_) => "Exceptions",
        Attribute::InnerClasses(_) => "InnerClasses",
        Attribute::EnclosingMethod { .. } => "EnclosingMethod",
        Attribute::SourceFile(_) => "SourceFile",
        Attribute::Deprecated => "Deprecated",
        Attribute::Synthetic => "Synthetic",
        Attribute::NestHost(_) => "NestHost",
        Attribute::NestMembers(_) => "NestMembers",
        Attribute::Record(_) => "Record",
        Attribute::PermittedSubclasses(_) => "PermittedSubclasses",
        Attribute::Code(_) => "Code",
        Attribute::BootstrapMethods(_) => "BootstrapMethods",
        Attribute::MethodParameters(_) => "MethodParameters",
        Attribute::Module(_) => "Module",
        Attribute::ModulePackages(_) => "ModulePackages",
        Attribute::ModuleMainClass(_) => "ModuleMainClass",
        Attribute::RuntimeVisibleAnnotations(_) => "RuntimeVisibleAnnotations",
        Attribute::RuntimeInvisibleAnnotations(_) => "RuntimeInvisibleAnnotations",
    };
    Ok(name.to_owned())
}

/// All predefined classfile attribute names in the Java 21 JVMS. Known
/// attributes decoded as `Other` (including nested/method-only attributes)
/// still need location checks; vendor attributes remain opaque and allowed.
fn is_predefined_attribute(name: &str) -> bool {
    matches!(
        name,
        "ConstantValue"
            | "Code"
            | "StackMapTable"
            | "Exceptions"
            | "InnerClasses"
            | "EnclosingMethod"
            | "Synthetic"
            | "Signature"
            | "SourceFile"
            | "SourceDebugExtension"
            | "LineNumberTable"
            | "LocalVariableTable"
            | "LocalVariableTypeTable"
            | "Deprecated"
            | "RuntimeVisibleAnnotations"
            | "RuntimeInvisibleAnnotations"
            | "RuntimeVisibleParameterAnnotations"
            | "RuntimeInvisibleParameterAnnotations"
            | "RuntimeVisibleTypeAnnotations"
            | "RuntimeInvisibleTypeAnnotations"
            | "AnnotationDefault"
            | "BootstrapMethods"
            | "MethodParameters"
            | "Module"
            | "ModulePackages"
            | "ModuleMainClass"
            | "NestHost"
            | "NestMembers"
            | "Record"
            | "PermittedSubclasses"
    )
}

fn validate_module_metadata(
    class_file: &ClassFile<'_>,
    module: &ModuleAttribute,
) -> Result<(), ModuleDescriptorError> {
    let pool = &class_file.constant_pool;
    let module_name = pool
        .module_name(module.module_name_index)
        .map_err(ModuleDescriptorError::ConstantPool)?;
    if !is_valid_module_name(module_name) {
        return Err(ModuleDescriptorError::InvalidModuleName {
            name: module_name.to_owned(),
        });
    }

    validate_directive_flags(module)?;
    ensure_unique(
        "requires",
        module
            .requires
            .iter()
            .map(|row| pool.module_name(row.requires_index)),
    )?;
    validate_module_names(
        module
            .requires
            .iter()
            .map(|row| pool.module_name(row.requires_index)),
    )?;
    ensure_unique(
        "exports",
        module
            .exports
            .iter()
            .map(|row| pool.package_name(row.exports_index)),
    )?;
    ensure_unique(
        "opens",
        module
            .opens
            .iter()
            .map(|row| pool.package_name(row.opens_index)),
    )?;
    ensure_unique(
        "uses",
        module.uses.iter().map(|index| pool.class_name(*index)),
    )?;
    ensure_unique(
        "provides",
        module
            .provides
            .iter()
            .map(|row| pool.class_name(row.provides_index)),
    )?;

    for row in &module.exports {
        validate_module_names(row.exports_to.iter().map(|index| pool.module_name(*index)))?;
        ensure_unique(
            "exports_to",
            row.exports_to.iter().map(|index| pool.module_name(*index)),
        )?;
    }
    for row in &module.opens {
        validate_module_names(row.opens_to.iter().map(|index| pool.module_name(*index)))?;
        ensure_unique(
            "opens_to",
            row.opens_to.iter().map(|index| pool.module_name(*index)),
        )?;
    }
    for row in &module.provides {
        if row.provides_with.is_empty() {
            return Err(ModuleDescriptorError::InvalidDirective {
                reason: "Module provides row must contain at least one provider",
            });
        }
        ensure_unique(
            "provides_with",
            row.provides_with
                .iter()
                .map(|index| pool.class_name(*index)),
        )?;
    }

    if module.module_flags & MODULE_ACC_OPEN != 0 && !module.opens.is_empty() {
        return Err(ModuleDescriptorError::InvalidDirective {
            reason: "open module must not have opens rows",
        });
    }
    if module_name == "java.base" {
        if !module.requires.is_empty() {
            return Err(ModuleDescriptorError::InvalidDirective {
                reason: "java.base must not have requires rows",
            });
        }
    } else {
        let java_base = module
            .requires
            .iter()
            .filter(|row| pool.module_name(row.requires_index) == Ok("java.base"))
            .collect::<Vec<_>>();
        if java_base.len() != 1 {
            return Err(ModuleDescriptorError::InvalidDirective {
                reason: "non-java.base module must require java.base exactly once",
            });
        }
        let row = java_base[0];
        if row.requires_flags & MODULE_ACC_SYNTHETIC != 0 {
            return Err(ModuleDescriptorError::InvalidDirective {
                reason: "java.base requires row must not be synthetic",
            });
        }
        // Java 21's JVMS forbids these bits for java.base in version 54.0+
        // (the Java 26 specification relaxes the transitive bit).
        if (54..=65).contains(&class_file.version.major)
            && row.requires_flags & (MODULE_ACC_TRANSITIVE | MODULE_ACC_STATIC_PHASE) != 0
        {
            return Err(ModuleDescriptorError::InvalidDirective {
                reason: "java.base requires row has forbidden transitive/static-phase flags",
            });
        }
    }

    let mut listed_packages = std::collections::HashSet::new();
    let mut has_module_packages = false;
    for attribute in &class_file.attributes {
        if let Attribute::ModulePackages(packages) = attribute {
            has_module_packages = true;
            for index in &packages.package_indices {
                let name = pool
                    .package_name(*index)
                    .map_err(ModuleDescriptorError::ConstantPool)?;
                if !is_valid_package_name(name) {
                    return Err(ModuleDescriptorError::InvalidPackageName {
                        name: name.to_owned(),
                    });
                }
                if !listed_packages.insert(name.to_owned()) {
                    return Err(ModuleDescriptorError::DuplicateDirective {
                        table: "ModulePackages",
                        name: name.to_owned(),
                    });
                }
            }
        }
    }
    for index in module
        .exports
        .iter()
        .map(|row| row.exports_index)
        .chain(module.opens.iter().map(|row| row.opens_index))
    {
        validate_package_name(pool, index)?;
        if has_module_packages {
            let name = pool
                .package_name(index)
                .map_err(ModuleDescriptorError::ConstantPool)?;
            if !listed_packages.contains(name) {
                return Err(ModuleDescriptorError::InvalidDirective {
                    reason: "ModulePackages omits an exported or opened package",
                });
            }
        }
    }
    if has_module_packages {
        for row in &module.provides {
            for provider in &row.provides_with {
                let name = pool
                    .class_name(*provider)
                    .map_err(ModuleDescriptorError::ConstantPool)?;
                if let Some((package, _)) = name.rsplit_once('/')
                    && !listed_packages.contains(package)
                {
                    return Err(ModuleDescriptorError::InvalidDirective {
                        reason: "ModulePackages omits a provider package",
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_directive_flags(module: &ModuleAttribute) -> Result<(), ModuleDescriptorError> {
    let valid_module = MODULE_ACC_OPEN | MODULE_ACC_SYNTHETIC | MODULE_ACC_MANDATED;
    if module.module_flags & !valid_module != 0 {
        return Err(ModuleDescriptorError::InvalidDirective {
            reason: "Module has reserved module_flags bits",
        });
    }
    if module.requires.iter().any(|row| {
        row.requires_flags
            & !(MODULE_ACC_TRANSITIVE
                | MODULE_ACC_STATIC_PHASE
                | MODULE_ACC_SYNTHETIC
                | MODULE_ACC_MANDATED)
            != 0
    }) {
        return Err(ModuleDescriptorError::InvalidDirective {
            reason: "Module has reserved requires_flags bits",
        });
    }
    if module
        .exports
        .iter()
        .any(|row| row.exports_flags & !(MODULE_ACC_SYNTHETIC | MODULE_ACC_MANDATED) != 0)
        || module
            .opens
            .iter()
            .any(|row| row.opens_flags & !(MODULE_ACC_SYNTHETIC | MODULE_ACC_MANDATED) != 0)
    {
        return Err(ModuleDescriptorError::InvalidDirective {
            reason: "Module has reserved exports_flags or opens_flags bits",
        });
    }
    Ok(())
}

fn ensure_unique<'a>(
    table: &'static str,
    names: impl Iterator<Item = Result<&'a str, PoolRefError>>,
) -> Result<(), ModuleDescriptorError> {
    let mut seen = std::collections::HashSet::new();
    for name in names {
        let name = name.map_err(ModuleDescriptorError::ConstantPool)?;
        if !seen.insert(name) {
            return Err(ModuleDescriptorError::DuplicateDirective {
                table,
                name: name.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_module_names<'a>(
    names: impl Iterator<Item = Result<&'a str, PoolRefError>>,
) -> Result<(), ModuleDescriptorError> {
    for name in names {
        let name = name.map_err(ModuleDescriptorError::ConstantPool)?;
        if !is_valid_module_name(name) {
            return Err(ModuleDescriptorError::InvalidModuleName {
                name: name.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_package_name(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
) -> Result<(), ModuleDescriptorError> {
    let name = pool
        .package_name(index)
        .map_err(ModuleDescriptorError::ConstantPool)?;
    if !is_valid_package_name(name) {
        return Err(ModuleDescriptorError::InvalidPackageName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn is_valid_module_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    while let Some(ch) = chars.next() {
        if ('\u{0000}'..='\u{001f}').contains(&ch) {
            return false;
        }
        if ch == '\\' && !matches!(chars.next(), Some('\\' | ':' | '@')) {
            return false;
        }
        if matches!(ch, ':' | '@') {
            return false;
        }
    }
    true
}

fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('/').all(|part| {
            !part.is_empty() && !part.chars().any(|ch| matches!(ch, '.' | ';' | '[' | '/'))
        })
}

fn encode_pool_ref_error(error: PoolRefError, constant_pool_count: usize) -> EncodeError {
    match error {
        PoolRefError::InvalidIndex { index } => EncodeError::InvalidIndex {
            index: usize::from(index.0),
            count: constant_pool_count,
        },
        PoolRefError::WrongKind { index, .. } => EncodeError::InvalidReference {
            index: usize::from(index.0),
        },
        PoolRefError::TooDeeplyNestedAttribute => EncodeError::TooDeeplyNestedAttribute,
        PoolRefError::TooDeeplyNestedAnnotation => EncodeError::TooDeeplyNestedAnnotation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::AttributeError;
    use crate::constant_pool::ConstantPoolEntry;

    /// A minimal but complete class file: one class named via constant pool
    /// entries `#1` (`Utf8 "Foo"`) / `#2` (`Class -> #1`), `PUBLIC | SUPER`
    /// access flags, `super_class = 0` (`java.lang.Object`), and no
    /// interfaces/fields/methods/attributes.
    const MINIMAL_CLASS_FILE: &[u8] = &[
        0xCA, 0xFE, 0xBA, 0xBE, // magic
        0x00, 0x00, // minor
        0x00, 0x45, // major = 69
        0x00, 0x03, // constant_pool_count = 3
        0x01, 0x00, 0x03, b'F', b'o', b'o', // #1 Utf8 "Foo"
        0x07, 0x00, 0x01, // #2 Class -> #1
        0x00, 0x21, // access_flags = PUBLIC | SUPER
        0x00, 0x02, // this_class = #2
        0x00, 0x00, // super_class = 0 (Object)
        0x00, 0x00, // interfaces_count = 0
        0x00, 0x00, // fields_count = 0
        0x00, 0x00, // methods_count = 0
        0x00, 0x00, // attributes_count = 0
    ];

    #[test]
    fn decodes_a_minimal_complete_class_file() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);

        let class_file = ClassFile::decode(&mut reader).unwrap();

        assert_eq!(
            class_file.version,
            ClassFileVersion {
                major: 69,
                minor: 0
            }
        );
        assert_eq!(class_file.access_flags, ClassAccessFlags(0x0021));
        assert_eq!(class_file.this_class, ConstantPoolIndex(2));
        assert_eq!(class_file.super_class, None);
        assert_eq!(class_file.interfaces, Vec::<ConstantPoolIndex>::new());
        assert_eq!(class_file.fields, Vec::new());
        assert_eq!(class_file.methods, Vec::new());
        assert_eq!(class_file.attributes, Vec::new());
        assert_eq!(
            class_file.constant_pool.get(ConstantPoolIndex(1)),
            Some(&ConstantPoolEntry::Utf8("Foo".to_owned()))
        );
        assert!(reader.is_at_end());
    }

    #[test]
    fn reports_an_error_from_a_class_file_truncated_before_attributes_count() {
        let mut reader = Reader::new(&MINIMAL_CLASS_FILE[..31]);

        assert_eq!(
            ClassFile::decode(&mut reader),
            Err(ClassFileError::Attribute(AttributeError::Read(
                ReadError::UnexpectedEof {
                    offset: 31,
                    needed: 2,
                    remaining: 0,
                }
            )))
        );
    }

    #[test]
    fn validate_references_accepts_the_minimal_class_file() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let class_file = ClassFile::decode(&mut reader).unwrap();

        assert_eq!(class_file.validate_references(), Ok(()));
    }

    #[test]
    fn encodes_a_minimal_complete_class_file_exactly() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let class_file = ClassFile::decode(&mut reader).unwrap();

        assert_eq!(class_file.encode().unwrap(), MINIMAL_CLASS_FILE);
    }

    #[test]
    fn encode_rejects_an_invalid_mandatory_class_index() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.this_class = ConstantPoolIndex(0);

        assert_eq!(
            class_file.encode(),
            Err(EncodeError::InvalidIndex { index: 0, count: 2 })
        );
    }

    #[test]
    fn encode_rejects_invalid_superclass_and_member_references() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.super_class = Some(ConstantPoolIndex(0));
        assert_eq!(
            class_file.encode(),
            Err(EncodeError::InvalidIndex { index: 0, count: 2 })
        );

        class_file.super_class = None;
        class_file.fields.push(FieldInfo {
            access_flags: crate::access_flags::FieldAccessFlags(0),
            name_index: ConstantPoolIndex(0),
            descriptor_index: ConstantPoolIndex(1),
            attributes: vec![],
        });
        assert_eq!(
            class_file.encode(),
            Err(EncodeError::InvalidIndex { index: 0, count: 2 })
        );

        class_file.fields.clear();
        class_file.methods.push(MethodInfo {
            access_flags: crate::access_flags::MethodAccessFlags(0),
            name_index: ConstantPoolIndex(1),
            descriptor_index: ConstantPoolIndex(0),
            attributes: vec![],
        });
        assert_eq!(
            class_file.encode(),
            Err(EncodeError::InvalidIndex { index: 0, count: 2 })
        );
    }

    #[test]
    fn duplicate_attribute_name_entries_remain_structurally_equivalent() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.constant_pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
            Some(ConstantPoolEntry::Utf8("Deprecated".to_owned())),
            Some(ConstantPoolEntry::Utf8("Deprecated".to_owned())),
        ]);
        class_file.attributes = vec![Attribute::Deprecated];

        let encoded = class_file.encode().unwrap();
        let mut encoded_reader = Reader::new(&encoded);
        let decoded = ClassFile::decode(&mut encoded_reader).unwrap();
        assert_eq!(decoded, class_file);
    }

    #[test]
    fn encode_rejects_a_preview_or_out_of_range_version() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.version.minor = PREVIEW_MINOR_VERSION;
        assert_eq!(
            class_file.encode(),
            Err(EncodeError::UnsupportedVersion {
                major: 69,
                minor: PREVIEW_MINOR_VERSION,
            })
        );

        class_file.version = ClassFileVersion {
            major: 71,
            minor: 0,
        };
        assert_eq!(
            class_file.encode(),
            Err(EncodeError::UnsupportedVersion {
                major: 71,
                minor: 0
            })
        );
    }

    #[test]
    fn encode_rejects_oversized_interface_lists() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.interfaces = vec![class_file.this_class; usize::from(u16::MAX) + 1];

        assert_eq!(
            class_file.encode(),
            Err(EncodeError::CountOverflow {
                value: usize::from(u16::MAX) + 1,
                width: crate::encode_error::LengthWidth::U16,
            })
        );
    }

    #[test]
    fn validate_references_rejects_a_this_class_that_is_not_a_class_entry() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
                "Foo".to_owned(),
            ))]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(1),
            super_class: None,
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_references_rejects_a_super_class_that_is_not_a_class_entry() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: Some(ConstantPoolIndex(3)),
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_references_rejects_an_interface_that_is_not_a_class_entry() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: None,
            interfaces: vec![ConstantPoolIndex(3)],
            fields: vec![],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_references_propagates_a_bad_field_reference() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: None,
            interfaces: vec![],
            fields: vec![FieldInfo {
                access_flags: crate::access_flags::FieldAccessFlags(0),
                name_index: ConstantPoolIndex(3),
                descriptor_index: ConstantPoolIndex(1),
                attributes: vec![],
            }],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn validate_references_propagates_a_bad_top_level_attribute_reference() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: None,
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attributes: vec![Attribute::SourceFile(ConstantPoolIndex(3))],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn decodes_a_valid_magic_and_version() {
        let bytes = [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x45];
        let mut reader = Reader::new(&bytes);

        let version = ClassFileVersion::decode(&mut reader).unwrap();

        assert_eq!(
            version,
            ClassFileVersion {
                major: 69,
                minor: 0,
            }
        );
        assert_eq!(reader.position(), 8);
    }

    #[test]
    fn rejects_an_invalid_magic() {
        let bytes = [0x00, 0x00, 0x00, 0x00];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            ClassFileVersion::decode(&mut reader),
            Err(ClassFileHeaderError::InvalidMagic { actual: bytes })
        );
    }

    #[test]
    fn reports_a_magic_truncated_mid_sequence() {
        let bytes = [0xCA, 0xFE];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            ClassFileVersion::decode(&mut reader),
            Err(ClassFileHeaderError::Read(ReadError::UnexpectedEof {
                offset: 0,
                needed: CLASS_MAGIC.len(),
                remaining: 2,
            }))
        );
    }

    #[test]
    fn reports_a_version_truncated_after_a_valid_magic() {
        let bytes = [0xCA, 0xFE, 0xBA, 0xBE, 0x00];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            ClassFileVersion::decode(&mut reader),
            Err(ClassFileHeaderError::Read(ReadError::UnexpectedEof {
                offset: 4,
                needed: 2,
                remaining: 1,
            }))
        );
    }

    #[test]
    fn a_jdk_25_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 69,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_jdk_26_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 70,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_jdk_24_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 68,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_jdk_23_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 67,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn the_oldest_historical_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 45,
                minor: 3
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_major_version_below_the_historical_minimum_is_incompatible() {
        assert!(
            !ClassFileVersion {
                major: 44,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_major_version_above_the_jdk_26_ceiling_is_incompatible() {
        assert!(
            !ClassFileVersion {
                major: 71,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_preview_minor_version_is_incompatible() {
        assert!(
            !ClassFileVersion {
                major: 70,
                minor: PREVIEW_MINOR_VERSION,
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_nonzero_non_preview_minor_version_is_incompatible_from_jdk_12_onward() {
        assert!(
            !ClassFileVersion {
                major: 56,
                minor: 1
            }
            .is_compatible()
        );
    }

    #[test]
    fn any_minor_version_is_compatible_before_jdk_12() {
        assert!(
            ClassFileVersion {
                major: 55,
                minor: 1234
            }
            .is_compatible()
        );
    }

    #[test]
    fn version_displays_as_major_dot_minor() {
        let version = ClassFileVersion {
            major: 70,
            minor: 0,
        };

        assert_eq!(version.to_string(), "70.0");
    }
}
