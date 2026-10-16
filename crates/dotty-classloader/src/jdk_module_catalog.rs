use crate::class_path::{ClassOrigin, ClassPathError};
use crate::jmod_class_path::JmodClassPath;
use dotty_classfile::attribute::{Attribute, ModuleAttribute};
use dotty_classfile::class_file::{ClassFile, ClassFileError, ModuleDescriptorError};
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolIndex, PoolRefError};
use dotty_classfile::reader::Reader;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

/// An owned, human-readable `requires` row from a JVM module descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkModuleRequire {
    /// Required JVM module name.
    pub module: String,
    /// JVMS `requires_flags`, preserved without interpreting or flattening.
    pub flags: u16,
    /// `None` represents a zero `requires_version_index`.
    pub version: Option<String>,
}

/// An owned `exports` row. An empty `targets` list means unqualified export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkModuleExport {
    /// Dotted package name, such as `java.lang`.
    pub package: String,
    /// JVMS `exports_flags`, preserved as encoded.
    pub flags: u16,
    /// Ordered target module names; empty for an unqualified export.
    pub targets: Vec<String>,
}

/// An owned `opens` row. An empty `targets` list means unqualified open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkModuleOpen {
    /// Dotted package name, such as `java.lang`.
    pub package: String,
    /// JVMS `opens_flags`, preserved as encoded.
    pub flags: u16,
    /// Ordered target module names; empty for an unqualified open.
    pub targets: Vec<String>,
}

/// An owned `provides` row with its service and ordered implementation names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkModuleProvide {
    /// Dotted binary class name, such as `java.lang.Runnable`.
    pub service: String,
    /// Dotted binary class names in encoded order.
    pub providers: Vec<String>,
}

/// Owned metadata projected from one validated JMOD `module-info.class`.
///
/// This describes a JVM JPMS module. It is not a Scala `ModuleClass` or a
/// `ModuleIdentity` used by source or symbol resolution. Module/package/class
/// names use dotted Java spelling; directive and package-list order is kept
/// as encoded in the class file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkModuleDescriptor {
    /// Authoritative module name decoded from the `Module` attribute.
    pub name: String,
    /// `None` represents a zero `module_version_index`.
    pub version: Option<String>,
    /// JVMS `module_flags`, preserved as encoded.
    pub flags: u16,
    /// Ordered `requires` rows; no implicit rows are synthesized.
    pub requires: Vec<JdkModuleRequire>,
    /// Ordered exported packages and their optional qualified targets.
    pub exports: Vec<JdkModuleExport>,
    /// Ordered opened packages and their optional qualified targets.
    pub opens: Vec<JdkModuleOpen>,
    /// Dotted binary class names, in encoded order.
    pub uses: Vec<String>,
    /// Ordered service-to-provider declarations.
    pub provides: Vec<JdkModuleProvide>,
    /// `None` means the optional `ModulePackages` attribute was absent;
    /// `Some([])` means it was present with an empty table.
    pub packages: Option<Vec<String>>,
    /// Dotted binary class name from the optional `ModuleMainClass` attribute.
    pub main_class: Option<String>,
    /// Origin of the exact JMOD containing this descriptor.
    pub origin: ClassOrigin,
}

/// A deterministic snapshot of all validated module descriptors in a JDK's
/// `jmods` directory.
///
/// Entries are sorted by descriptor name and names are unique. The snapshot
/// owns its strings, so it remains valid independently of the JDK classpath.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkModuleCatalog {
    modules: Vec<JdkModuleDescriptor>,
}

impl JdkModuleCatalog {
    pub(crate) fn read(jmods: &[JmodClassPath]) -> Result<Self, JdkModuleCatalogError> {
        let mut modules = Vec::with_capacity(jmods.len());
        for jmod in jmods {
            modules.push(read_descriptor(jmod)?);
        }

        let mut first_origins: HashMap<String, ClassOrigin> = HashMap::with_capacity(modules.len());
        for module in &modules {
            if let Some(first_origin) = first_origins.get(&module.name) {
                return Err(JdkModuleCatalogError::DuplicateModuleName {
                    name: module.name.clone(),
                    first_origin: first_origin.clone(),
                    duplicate_origin: module.origin.clone(),
                });
            }
            first_origins.insert(module.name.clone(), module.origin.clone());
        }

        for module in &modules {
            let path = jmod_path(&module.origin).ok_or_else(|| {
                JdkModuleCatalogError::InvalidJmodFilename {
                    origin: module.origin.clone(),
                }
            })?;
            let filename = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| JdkModuleCatalogError::InvalidJmodFilename {
                    origin: module.origin.clone(),
                })?;
            if filename != module.name {
                return Err(JdkModuleCatalogError::ModuleNameMismatch {
                    origin: module.origin.clone(),
                    filename: filename.to_owned(),
                    descriptor_name: module.name.clone(),
                });
            }
        }

        modules.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        Ok(Self { modules })
    }

    /// The catalog's modules in deterministic descriptor-name order.
    pub fn modules(&self) -> &[JdkModuleDescriptor] {
        &self.modules
    }

    /// Looks up a module by its authoritative descriptor name.
    pub fn get(&self, name: &str) -> Option<&JdkModuleDescriptor> {
        self.modules
            .binary_search_by(|module| module.name.as_str().cmp(name))
            .ok()
            .map(|index| &self.modules[index])
    }
}

/// Why the requested JDK module catalog could not be read.
#[derive(Debug)]
pub enum JdkModuleCatalogError {
    /// Reading the module-info entry failed in the JMOD archive.
    Archive {
        origin: ClassOrigin,
        source: ClassPathError,
    },
    /// The JMOD has no `classes/module-info.class` entry.
    MissingDescriptor { origin: ClassOrigin },
    /// The module-info entry is not a structurally decodable class file.
    InvalidClassFile {
        origin: ClassOrigin,
        source: ClassFileError,
    },
    /// The class file decoded successfully but did not consume the full entry.
    TrailingClassFileBytes {
        origin: ClassOrigin,
        remaining: usize,
    },
    /// The class file is not a valid JPMS module descriptor.
    InvalidDescriptor {
        origin: ClassOrigin,
        source: ModuleDescriptorError,
    },
    /// A projected constant-pool index did not resolve to its expected kind.
    InvalidReference {
        origin: ClassOrigin,
        source: PoolRefError,
    },
    /// The validator accepted a descriptor with no typed `Module` attribute.
    MissingModuleAttribute { origin: ClassOrigin },
    /// The JMOD path is not a UTF-8 filename that can be compared to a module.
    InvalidJmodFilename { origin: ClassOrigin },
    /// The JMOD filename stem differs from the authoritative descriptor name.
    ModuleNameMismatch {
        origin: ClassOrigin,
        filename: String,
        descriptor_name: String,
    },
    /// More than one JMOD declares the same module name.
    DuplicateModuleName {
        name: String,
        first_origin: ClassOrigin,
        duplicate_origin: ClassOrigin,
    },
}

impl fmt::Display for JdkModuleCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive { origin, source } => {
                write!(
                    formatter,
                    "could not read module-info from {origin:?}: {source}"
                )
            }
            Self::MissingDescriptor { origin } => {
                write!(
                    formatter,
                    "JMOD {origin:?} has no classes/module-info.class"
                )
            }
            Self::InvalidClassFile { origin, source } => {
                write!(
                    formatter,
                    "invalid module-info class file in {origin:?}: {source}"
                )
            }
            Self::TrailingClassFileBytes { origin, remaining } => write!(
                formatter,
                "module-info class file in {origin:?} has {remaining} trailing byte(s)"
            ),
            Self::InvalidDescriptor { origin, source } => {
                write!(
                    formatter,
                    "invalid module descriptor in {origin:?}: {source}"
                )
            }
            Self::InvalidReference { origin, source } => {
                write!(
                    formatter,
                    "invalid module metadata reference in {origin:?}: {source}"
                )
            }
            Self::MissingModuleAttribute { origin } => write!(
                formatter,
                "validated module descriptor in {origin:?} has no Module attribute"
            ),
            Self::InvalidJmodFilename { origin } => {
                write!(formatter, "JMOD filename is not valid UTF-8: {origin:?}")
            }
            Self::ModuleNameMismatch {
                origin,
                filename,
                descriptor_name,
            } => write!(
                formatter,
                "JMOD filename stem {filename:?} in {origin:?} does not match descriptor module {descriptor_name:?}"
            ),
            Self::DuplicateModuleName {
                name,
                first_origin,
                duplicate_origin,
            } => write!(
                formatter,
                "module name {name:?} is declared by both {first_origin:?} and {duplicate_origin:?}"
            ),
        }
    }
}

impl Error for JdkModuleCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Archive { source, .. } => Some(source),
            Self::InvalidClassFile { source, .. } => Some(source),
            Self::InvalidDescriptor { source, .. } => Some(source),
            Self::InvalidReference { source, .. } => Some(source),
            Self::MissingDescriptor { .. }
            | Self::TrailingClassFileBytes { .. }
            | Self::MissingModuleAttribute { .. }
            | Self::InvalidJmodFilename { .. }
            | Self::ModuleNameMismatch { .. }
            | Self::DuplicateModuleName { .. } => None,
        }
    }
}

fn read_descriptor(jmod: &JmodClassPath) -> Result<JdkModuleDescriptor, JdkModuleCatalogError> {
    let origin = jmod.origin();
    let bytes = jmod
        .read_module_info()
        .map_err(|source| JdkModuleCatalogError::Archive {
            origin: origin.clone(),
            source,
        })?
        .ok_or_else(|| JdkModuleCatalogError::MissingDescriptor {
            origin: origin.clone(),
        })?;

    let mut reader = Reader::new(&bytes);
    let class_file = ClassFile::decode(&mut reader).map_err(|source| {
        JdkModuleCatalogError::InvalidClassFile {
            origin: origin.clone(),
            source,
        }
    })?;
    if !reader.is_at_end() {
        return Err(JdkModuleCatalogError::TrailingClassFileBytes {
            origin: origin.clone(),
            remaining: reader.remaining(),
        });
    }
    class_file.validate_module_descriptor().map_err(|source| {
        JdkModuleCatalogError::InvalidDescriptor {
            origin: origin.clone(),
            source,
        }
    })?;

    let module = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Module(module) => Some(module),
            _ => None,
        })
        .ok_or_else(|| JdkModuleCatalogError::MissingModuleAttribute {
            origin: origin.clone(),
        })?;

    project_module(module, &class_file, origin)
}

fn project_module(
    module: &ModuleAttribute,
    class_file: &ClassFile<'_>,
    origin: ClassOrigin,
) -> Result<JdkModuleDescriptor, JdkModuleCatalogError> {
    let pool = &class_file.constant_pool;
    let name = resolve_module(pool, module.module_name_index, &origin)?;
    let version = module
        .module_version_index
        .map(|index| resolve_utf8(pool, index, &origin))
        .transpose()?;

    let mut requires = Vec::with_capacity(module.requires.len());
    for row in &module.requires {
        requires.push(JdkModuleRequire {
            module: resolve_module(pool, row.requires_index, &origin)?,
            flags: row.requires_flags,
            version: row
                .requires_version_index
                .map(|index| resolve_utf8(pool, index, &origin))
                .transpose()?,
        });
    }

    let mut exports = Vec::with_capacity(module.exports.len());
    for row in &module.exports {
        exports.push(JdkModuleExport {
            package: dotted(resolve_package(pool, row.exports_index, &origin)?),
            flags: row.exports_flags,
            targets: row
                .exports_to
                .iter()
                .map(|index| resolve_module(pool, *index, &origin))
                .collect::<Result<_, _>>()?,
        });
    }

    let mut opens = Vec::with_capacity(module.opens.len());
    for row in &module.opens {
        opens.push(JdkModuleOpen {
            package: dotted(resolve_package(pool, row.opens_index, &origin)?),
            flags: row.opens_flags,
            targets: row
                .opens_to
                .iter()
                .map(|index| resolve_module(pool, *index, &origin))
                .collect::<Result<_, _>>()?,
        });
    }

    let uses = module
        .uses
        .iter()
        .map(|index| resolve_class(pool, *index, &origin).map(dotted))
        .collect::<Result<_, _>>()?;

    let mut provides = Vec::with_capacity(module.provides.len());
    for row in &module.provides {
        provides.push(JdkModuleProvide {
            service: dotted(resolve_class(pool, row.provides_index, &origin)?),
            providers: row
                .provides_with
                .iter()
                .map(|index| resolve_class(pool, *index, &origin).map(dotted))
                .collect::<Result<_, _>>()?,
        });
    }

    let packages = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::ModulePackages(packages) => Some(
                packages
                    .package_indices
                    .iter()
                    .map(|index| resolve_package(pool, *index, &origin).map(dotted))
                    .collect::<Result<Vec<_>, _>>(),
            ),
            _ => None,
        });
    let packages = packages.transpose()?;

    let main_class = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::ModuleMainClass(index) => {
                Some(resolve_class(pool, *index, &origin).map(dotted))
            }
            _ => None,
        });
    let main_class = main_class.transpose()?;

    Ok(JdkModuleDescriptor {
        name,
        version,
        flags: module.module_flags,
        requires,
        exports,
        opens,
        uses,
        provides,
        packages,
        main_class,
        origin,
    })
}

fn resolve_module(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
    origin: &ClassOrigin,
) -> Result<String, JdkModuleCatalogError> {
    pool.module_name(index)
        .map(str::to_owned)
        .map_err(|source| invalid_reference(origin, source))
}

fn resolve_package(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
    origin: &ClassOrigin,
) -> Result<String, JdkModuleCatalogError> {
    pool.package_name(index)
        .map(str::to_owned)
        .map_err(|source| invalid_reference(origin, source))
}

fn resolve_class(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
    origin: &ClassOrigin,
) -> Result<String, JdkModuleCatalogError> {
    pool.class_name(index)
        .map(str::to_owned)
        .map_err(|source| invalid_reference(origin, source))
}

fn resolve_utf8(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
    origin: &ClassOrigin,
) -> Result<String, JdkModuleCatalogError> {
    pool.utf8(index)
        .map(str::to_owned)
        .map_err(|source| invalid_reference(origin, source))
}

fn invalid_reference(origin: &ClassOrigin, source: PoolRefError) -> JdkModuleCatalogError {
    JdkModuleCatalogError::InvalidReference {
        origin: origin.clone(),
        source,
    }
}

fn dotted(internal_name: String) -> String {
    internal_name.replace('/', ".")
}

fn jmod_path(origin: &ClassOrigin) -> Option<&Path> {
    match origin {
        ClassOrigin::Jmod(path) => Some(path),
        _ => None,
    }
}
