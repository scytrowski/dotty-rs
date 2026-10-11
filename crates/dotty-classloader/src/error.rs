use crate::binary_name::BinaryName;
use crate::class_path::ClassPathError;
use crate::tasty_symbol::TastyDecodeError;
use dotty_classfile::attribute::AttributeError;
use dotty_classfile::class_file::{ClassFileError, ClassFileVersion};
use dotty_classfile::constant_pool::PoolRefError;
use dotty_classfile::descriptor::ResolveDescriptorError;
use dotty_classfile::signature::SignatureError;
use std::fmt;
use std::rc::Rc;

/// Broad machine-readable class-loading failure category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassLoadCategory {
    Missing,
    MalformedArtifact,
    UnsupportedFeatureOrVersion,
    ResolutionOrDependency,
    IoOrEnvironment,
    InternalInvariant,
    Conditional,
}

/// Why loading a class failed.
///
/// `Clone` so a `ClassEntry::Failed` cache entry can be returned
/// repeatedly without re-running the failed load. Non-`Clone` payloads
/// (`ClassPathError`, and this type itself for [`Self::DependencyFailure`])
/// are wrapped in `Rc` to keep the whole enum cheaply cloneable.
#[derive(Debug, Clone)]
pub enum ClassLoadError {
    /// No classpath entry contains this class.
    NotFound(BinaryName),
    /// A classpath entry failed with a real I/O error while looking up
    /// this class.
    Io(BinaryName, Rc<ClassPathError>),
    /// The bytes found for this class did not decode as a valid class
    /// file.
    InvalidClassFile(BinaryName, ClassFileError),
    /// The class file decoded structurally, but its own declared
    /// `major_version`/`minor_version` falls outside the range this
    /// decoder can safely give a semantic reading — either newer than
    /// this project's JDK-26 compatibility ceiling, older than the JVMS
    /// has ever defined, or a preview-features class this decoder never
    /// has preview features enabled for. Real compatibility corpora cover
    /// JDK 21 through 26 (see
    /// [`ClassFileVersion::is_compatible`] and
    /// `docs/classfile-format-jdk25.md` §2.1).
    UnsupportedClassVersion(BinaryName, ClassFileVersion),
    /// The class file decoded structurally, but one of its own
    /// `this_class`/`super_class`/interface constant-pool indices, a
    /// field/method's `name_index`, or a `Signature` attribute's own
    /// index, does not resolve to a usable entry (JVMS §4.4.1).
    MalformedReference(BinaryName, PoolRefError),
    /// The class file decoded structurally, but a field or method's
    /// descriptor did not resolve or parse as a valid type (JVMS
    /// §4.3.2/§4.3.3).
    MalformedDescriptor(BinaryName, ResolveDescriptorError),
    /// The class file decoded structurally, but a class, field, or
    /// method's `Signature` attribute did not parse as a valid generic
    /// signature (JVMS §4.7.9.1).
    MalformedSignature(BinaryName, SignatureError),
    /// The class file's own `this_class` name did not match the name it
    /// was requested under (JVMS §5.3.5).
    NameMismatch {
        requested: BinaryName,
        actual: BinaryName,
    },
    /// The bytes found for this class did not decode as a valid
    /// `.tasty` file, or no `TypeDef` in it matched the requested name —
    /// including a `.tasty` supertype reference that could not be
    /// resolved to a name (see [`TastyDecodeError::UnresolvedSupertype`]
    /// and `tasty_symbol::resolve_parent_name`'s doc comment).
    InvalidTastyFile(BinaryName, TastyDecodeError),
    /// A class is (in)directly its own superclass or interface — a hard
    /// JVMS §5.3.5 error, not a legitimate mutual reference.
    CircularInheritance(BinaryName),
    /// The superclass/interface chain exceeded the loader's recursion bound.
    InheritanceDepthExceeded(BinaryName),
    /// Loading `owner` failed because resolving `dependency` (its
    /// superclass, an interface, or — since `docs/classloader.md` §9's
    /// Milestone 6 — a member's declared type) failed.
    DependencyFailure {
        owner: BinaryName,
        dependency: BinaryName,
        source: Rc<ClassLoadError>,
    },
    /// A class, field, or method's `Signature` attribute referenced a
    /// `TypeVariable` (JVMS §4.7.9.1) by a name that no enclosing
    /// `SymbolKind::TypeParameter` symbol was entered under — either a
    /// genuinely malformed class file, or a signature referencing an
    /// enclosing (owner) class's own type parameter, which is not yet
    /// resolved (only the immediately declaring class's type parameters are
    /// in scope so far).
    UnresolvedTypeVariable(BinaryName, String),
}

impl fmt::Display for ClassLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(formatter, "class not found: {name}"),
            Self::Io(name, source) => {
                write!(formatter, "I/O error loading class {name}: {source}")
            }
            Self::InvalidClassFile(name, source) => {
                write!(formatter, "invalid class file for {name}: {source}")
            }
            Self::UnsupportedClassVersion(name, version) => {
                write!(
                    formatter,
                    "unsupported class file version {version} for {name}"
                )
            }
            Self::MalformedReference(name, source) => {
                write!(
                    formatter,
                    "malformed reference in class file for {name}: {source}"
                )
            }
            Self::MalformedDescriptor(name, source) => {
                write!(
                    formatter,
                    "malformed descriptor in class file for {name}: {source}"
                )
            }
            Self::MalformedSignature(name, source) => {
                write!(
                    formatter,
                    "malformed signature in class file for {name}: {source}"
                )
            }
            Self::NameMismatch { requested, actual } => write!(
                formatter,
                "requested class {requested} but its class file declares {actual}"
            ),
            Self::InvalidTastyFile(name, source) => {
                write!(formatter, "invalid .tasty file for {name}: {source}")
            }
            Self::CircularInheritance(name) => {
                write!(formatter, "circular inheritance involving {name}")
            }
            Self::InheritanceDepthExceeded(name) => write!(
                formatter,
                "inheritance chain exceeds the supported depth while loading {name}"
            ),
            Self::DependencyFailure {
                owner,
                dependency,
                source,
            } => write!(
                formatter,
                "class {owner} failed to load because {dependency} failed: {source}"
            ),
            Self::UnresolvedTypeVariable(owner, name) => write!(
                formatter,
                "signature in class file for {owner} references unresolved type variable {name}"
            ),
        }
    }
}

impl ClassLoadError {
    /// Stable identifier for this outer failure variant.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "classloader.not_found",
            Self::Io(_, _) => "classloader.classpath_error",
            Self::InvalidClassFile(_, _) => "classloader.invalid_class_file",
            Self::UnsupportedClassVersion(_, _) => "classloader.unsupported_class_version",
            Self::MalformedReference(_, _) => "classloader.malformed_reference",
            Self::MalformedDescriptor(_, _) => "classloader.malformed_descriptor",
            Self::MalformedSignature(_, _) => "classloader.malformed_signature",
            Self::NameMismatch { .. } => "classloader.name_mismatch",
            Self::InvalidTastyFile(_, _) => "classloader.invalid_tasty_file",
            Self::CircularInheritance(_) => "classloader.circular_inheritance",
            Self::InheritanceDepthExceeded(_) => "classloader.inheritance_depth_exceeded",
            Self::DependencyFailure { .. } => "classloader.dependency_failure",
            Self::UnresolvedTypeVariable(_, _) => "classloader.unresolved_type_variable",
        }
    }

    /// The deepest class-loading error in a dependency chain.
    pub fn root_cause(&self) -> &Self {
        let mut cause = self;
        while let Self::DependencyFailure { source, .. } = cause {
            cause = source;
        }
        cause
    }

    /// Stable identifier for the deepest class-loading cause.
    pub fn root_cause_code(&self) -> &'static str {
        match self.root_cause() {
            Self::Io(_, source) => source.code(),
            Self::InvalidTastyFile(_, source) => source.root_cause_code(),
            cause => cause.code(),
        }
    }

    /// Category derived from the retained typed cause, never `Display` text.
    pub fn category(&self) -> ClassLoadCategory {
        use ClassLoadCategory as C;
        match self {
            Self::DependencyFailure { .. } => C::ResolutionOrDependency,
            error => error.root_cause_category(),
        }
    }

    /// Category of the deepest class-loading cause, after unwrapping any
    /// dependency wrappers.
    pub fn root_cause_category(&self) -> ClassLoadCategory {
        use ClassLoadCategory as C;
        match self.root_cause() {
            Self::NotFound(_) => C::Missing,
            Self::Io(_, source) => source.category(),
            Self::InvalidClassFile(
                _,
                ClassFileError::Attribute(AttributeError::TooDeeplyNestedAnnotation),
            ) => C::UnsupportedFeatureOrVersion,
            Self::InvalidClassFile(_, _) => C::MalformedArtifact,
            Self::UnsupportedClassVersion(_, _) | Self::InheritanceDepthExceeded(_) => {
                C::UnsupportedFeatureOrVersion
            }
            Self::MalformedReference(_, _)
            | Self::MalformedDescriptor(_, _)
            | Self::NameMismatch { .. }
            | Self::CircularInheritance(_) => C::MalformedArtifact,
            Self::MalformedSignature(
                _,
                dotty_classfile::signature::SignatureError::TooDeeplyNested { .. },
            ) => C::UnsupportedFeatureOrVersion,
            Self::MalformedSignature(_, _) => C::MalformedArtifact,
            Self::InvalidTastyFile(_, source) => source.category(),
            Self::DependencyFailure { .. } => C::ResolutionOrDependency,
            Self::UnresolvedTypeVariable(_, _) => C::Conditional,
        }
    }
}

/// A class-loading failure together with the classpath resource that
/// produced its deepest retained cause, when a resource was selected.
///
/// `load_class` keeps returning [`ClassLoadError`] for compatibility;
/// [`crate::loader::ClassLoader::load_class_detailed`] opts into this
/// additional context. Missing names and failures before a resource is
/// selected have no fabricated origin or format.
#[derive(Debug, Clone)]
pub struct ClassLoadFailure {
    pub error: ClassLoadError,
    pub origin: Option<crate::class_path::ClassOrigin>,
    pub format: Option<crate::class_path::ClassFormat>,
}

impl ClassLoadFailure {
    pub fn code(&self) -> &'static str {
        self.error.code()
    }

    pub fn category(&self) -> ClassLoadCategory {
        self.error.category()
    }

    pub fn root_cause(&self) -> &ClassLoadError {
        self.error.root_cause()
    }

    pub fn root_cause_code(&self) -> &'static str {
        self.error.root_cause_code()
    }

    pub fn root_cause_category(&self) -> ClassLoadCategory {
        self.error.root_cause_category()
    }
}

impl fmt::Display for ClassLoadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for ClassLoadFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl std::error::Error for ClassLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound(_)
            | Self::NameMismatch { .. }
            | Self::CircularInheritance(_)
            | Self::InheritanceDepthExceeded(_)
            | Self::UnresolvedTypeVariable(_, _)
            | Self::UnsupportedClassVersion(_, _) => None,
            Self::Io(_, source) => Some(source.as_ref()),
            Self::InvalidClassFile(_, source) => Some(source),
            Self::MalformedReference(_, source) => Some(source),
            Self::MalformedDescriptor(_, source) => Some(source),
            Self::MalformedSignature(_, source) => Some(source),
            Self::InvalidTastyFile(_, source) => Some(source),
            Self::DependencyFailure { source, .. } => Some(source.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_displays_the_missing_class_name() {
        let error = ClassLoadError::NotFound(BinaryName::from_internal("java/lang/Object"));
        assert_eq!(error.to_string(), "class not found: java/lang/Object");
    }

    #[test]
    fn unsupported_class_version_displays_the_version_and_name() {
        let error = ClassLoadError::UnsupportedClassVersion(
            BinaryName::from_internal("Future"),
            ClassFileVersion {
                major: 71,
                minor: 0,
            },
        );
        assert_eq!(
            error.to_string(),
            "unsupported class file version 71.0 for Future"
        );
    }

    #[test]
    fn unsupported_class_version_has_no_further_source() {
        use std::error::Error;

        let error = ClassLoadError::UnsupportedClassVersion(
            BinaryName::from_internal("Future"),
            ClassFileVersion {
                major: 71,
                minor: 0,
            },
        );
        assert!(error.source().is_none());
    }

    #[test]
    fn name_mismatch_displays_both_names() {
        let error = ClassLoadError::NameMismatch {
            requested: BinaryName::from_internal("Requested"),
            actual: BinaryName::from_internal("Actual"),
        };
        assert_eq!(
            error.to_string(),
            "requested class Requested but its class file declares Actual"
        );
    }

    #[test]
    fn dependency_failure_chains_through_source() {
        let source = Rc::new(ClassLoadError::NotFound(BinaryName::from_internal(
            "java/lang/Object",
        )));
        let error = ClassLoadError::DependencyFailure {
            owner: BinaryName::from_internal("PoolSample"),
            dependency: BinaryName::from_internal("java/lang/Object"),
            source,
        };

        assert_eq!(
            error.to_string(),
            "class PoolSample failed to load because java/lang/Object failed: class not found: java/lang/Object"
        );
    }

    #[test]
    fn nested_dependency_failures_preserve_rc_identity_and_error_sources() {
        use std::error::Error;

        let leaf = ClassLoadError::InvalidClassFile(
            BinaryName::from_internal("missing/Leaf"),
            ClassFileError::Read(dotty_classfile::reader::ReadError::UnexpectedEof {
                offset: 7,
                needed: 2,
                remaining: 0,
            }),
        );
        let inner = Rc::new(ClassLoadError::DependencyFailure {
            owner: BinaryName::from_internal("middle/Owner"),
            dependency: BinaryName::from_internal("missing/Leaf"),
            source: Rc::new(leaf),
        });
        let retained = Rc::clone(&inner);
        let outer = ClassLoadError::DependencyFailure {
            owner: BinaryName::from_internal("root/Owner"),
            dependency: BinaryName::from_internal("middle/Owner"),
            source: inner,
        };

        let first = outer.source().expect("outer error retains its cause");
        assert_eq!(
            first as *const dyn Error as *const (),
            retained.as_ref() as *const ClassLoadError as *const ()
        );
        let second = first.source().expect("nested wrapper retains its cause");
        let leaf = second
            .downcast_ref::<ClassLoadError>()
            .expect("the nested cause remains a typed class load error");
        assert!(
            matches!(leaf, ClassLoadError::InvalidClassFile(name, _) if name.as_internal() == "missing/Leaf")
        );
        assert!(
            second.source().is_some(),
            "class file parse cause is exposed"
        );
    }

    #[test]
    fn dependency_failure_keeps_outer_code_and_category_separate_from_root_cause() {
        let root = ClassLoadError::NotFound(BinaryName::from_internal("missing/Parent"));
        let error = ClassLoadError::DependencyFailure {
            owner: BinaryName::from_internal("Child"),
            dependency: BinaryName::from_internal("missing/Parent"),
            source: Rc::new(root),
        };

        assert_eq!(error.code(), "classloader.dependency_failure");
        assert_eq!(error.category(), ClassLoadCategory::ResolutionOrDependency);
        assert_eq!(error.root_cause_code(), "classloader.not_found");
        assert_eq!(error.root_cause_category(), ClassLoadCategory::Missing);
        assert!(matches!(
            error.root_cause(),
            ClassLoadError::NotFound(name) if name.as_internal() == "missing/Parent"
        ));
    }

    #[test]
    fn nested_depth_limits_are_unsupported_while_bad_grammar_is_malformed() {
        let name = BinaryName::from_internal("Deep");
        let signature = ClassLoadError::MalformedSignature(
            name.clone(),
            SignatureError::TooDeeplyNested { offset: 2064 },
        );
        let invalid_signature = ClassLoadError::MalformedSignature(
            name.clone(),
            SignatureError::UnknownLeadChar {
                offset: 0,
                found: '?',
            },
        );
        let annotation = ClassLoadError::InvalidClassFile(
            name,
            ClassFileError::Attribute(AttributeError::TooDeeplyNestedAnnotation),
        );

        assert_eq!(
            signature.category(),
            ClassLoadCategory::UnsupportedFeatureOrVersion
        );
        assert_eq!(
            invalid_signature.category(),
            ClassLoadCategory::MalformedArtifact
        );
        assert_eq!(
            annotation.category(),
            ClassLoadCategory::UnsupportedFeatureOrVersion
        );
    }

    #[test]
    fn outer_error_codes_are_stable_and_use_the_classloader_namespace() {
        let error = ClassLoadError::UnsupportedClassVersion(
            BinaryName::from_internal("Future"),
            ClassFileVersion {
                major: 71,
                minor: 0,
            },
        );
        assert_eq!(error.code(), "classloader.unsupported_class_version");
        assert_eq!(error.root_cause_code(), error.code());
    }

    #[test]
    fn every_classload_variant_has_an_explicit_stable_code() {
        use dotty_classfile::constant_pool::ConstantPoolIndex;
        use dotty_classfile::reader::ReadError;

        let name = BinaryName::from_internal("A");
        let index = ConstantPoolIndex(1);
        let version = ClassFileVersion {
            major: 70,
            minor: 0,
        };
        let errors = vec![
            ClassLoadError::NotFound(name.clone()),
            ClassLoadError::Io(
                name.clone(),
                Rc::new(ClassPathError::new(std::io::Error::other("io"))),
            ),
            ClassLoadError::InvalidClassFile(
                name.clone(),
                ClassFileError::Read(ReadError::UnexpectedEof {
                    offset: 0,
                    needed: 1,
                    remaining: 0,
                }),
            ),
            ClassLoadError::UnsupportedClassVersion(name.clone(), version),
            ClassLoadError::MalformedReference(name.clone(), PoolRefError::InvalidIndex { index }),
            ClassLoadError::MalformedDescriptor(
                name.clone(),
                ResolveDescriptorError::NotUtf8 { index },
            ),
            ClassLoadError::MalformedSignature(
                name.clone(),
                SignatureError::UnexpectedEnd { offset: 0 },
            ),
            ClassLoadError::NameMismatch {
                requested: name.clone(),
                actual: BinaryName::from_internal("B"),
            },
            ClassLoadError::InvalidTastyFile(name.clone(), TastyDecodeError::MissingTypeDef),
            ClassLoadError::CircularInheritance(name.clone()),
            ClassLoadError::InheritanceDepthExceeded(name.clone()),
            ClassLoadError::DependencyFailure {
                owner: name.clone(),
                dependency: BinaryName::from_internal("B"),
                source: Rc::new(ClassLoadError::NotFound(BinaryName::from_internal("B"))),
            },
            ClassLoadError::UnresolvedTypeVariable(name, "T".to_owned()),
        ];
        let codes: Vec<_> = errors.iter().map(ClassLoadError::code).collect();
        let categories: Vec<_> = errors.iter().map(ClassLoadError::category).collect();

        assert_eq!(
            codes,
            [
                "classloader.not_found",
                "classloader.classpath_error",
                "classloader.invalid_class_file",
                "classloader.unsupported_class_version",
                "classloader.malformed_reference",
                "classloader.malformed_descriptor",
                "classloader.malformed_signature",
                "classloader.name_mismatch",
                "classloader.invalid_tasty_file",
                "classloader.circular_inheritance",
                "classloader.inheritance_depth_exceeded",
                "classloader.dependency_failure",
                "classloader.unresolved_type_variable",
            ]
        );
        assert_eq!(
            categories,
            [
                ClassLoadCategory::Missing,
                ClassLoadCategory::IoOrEnvironment,
                ClassLoadCategory::MalformedArtifact,
                ClassLoadCategory::UnsupportedFeatureOrVersion,
                ClassLoadCategory::MalformedArtifact,
                ClassLoadCategory::MalformedArtifact,
                ClassLoadCategory::MalformedArtifact,
                ClassLoadCategory::MalformedArtifact,
                ClassLoadCategory::Conditional,
                ClassLoadCategory::MalformedArtifact,
                ClassLoadCategory::UnsupportedFeatureOrVersion,
                ClassLoadCategory::ResolutionOrDependency,
                ClassLoadCategory::Conditional,
            ]
        );
    }

    #[test]
    fn typed_classpath_and_tasty_causes_keep_specific_root_classification() {
        use dotty_tasty::{HeaderError, file::TastyFileError};

        let classpath = ClassLoadError::Io(
            BinaryName::from_internal("A"),
            Rc::new(ClassPathError::new(std::io::Error::other("denied"))),
        );
        assert_eq!(classpath.code(), "classloader.classpath_error");
        assert_eq!(classpath.root_cause_code(), "classpath.io");
        assert_eq!(classpath.category(), ClassLoadCategory::IoOrEnvironment);

        let invalid_archive = ClassLoadError::Io(
            BinaryName::from_internal("A"),
            Rc::new(ClassPathError::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad archive magic",
            ))),
        );
        assert_eq!(invalid_archive.root_cause_code(), "classpath.invalid_data");
        assert_eq!(invalid_archive.category(), ClassLoadCategory::Conditional);

        let tasty = ClassLoadError::InvalidTastyFile(
            BinaryName::from_internal("A"),
            TastyDecodeError::Parse(TastyFileError::Header(HeaderError::UnsupportedVersion {
                major: 99,
                minor: 0,
                experimental: 0,
            })),
        );
        assert_eq!(tasty.root_cause_code(), "tasty.unsupported_version");
        assert_eq!(
            tasty.category(),
            ClassLoadCategory::UnsupportedFeatureOrVersion
        );
    }

    #[test]
    fn malformed_reference_displays_the_pool_error() {
        use dotty_classfile::constant_pool::ConstantPoolIndex;

        let error = ClassLoadError::MalformedReference(
            BinaryName::from_internal("PoolSample"),
            PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(7),
            },
        );

        assert_eq!(
            error.to_string(),
            "malformed reference in class file for PoolSample: constant pool index 7 does not resolve to any entry"
        );
    }

    #[test]
    fn malformed_descriptor_displays_the_descriptor_error() {
        use dotty_classfile::constant_pool::ConstantPoolIndex;

        let error = ClassLoadError::MalformedDescriptor(
            BinaryName::from_internal("PoolSample"),
            ResolveDescriptorError::NotUtf8 {
                index: ConstantPoolIndex(9),
            },
        );

        assert_eq!(
            error.to_string(),
            "malformed descriptor in class file for PoolSample: descriptor index 9 does not resolve to a Utf8 entry"
        );
    }

    #[test]
    fn malformed_signature_displays_the_signature_error() {
        let error = ClassLoadError::MalformedSignature(
            BinaryName::from_internal("GenericSample"),
            SignatureError::UnexpectedEnd { offset: 3 },
        );

        assert_eq!(
            error.to_string(),
            "malformed signature in class file for GenericSample: unexpected end of signature at offset 3"
        );
    }

    #[test]
    fn invalid_tasty_file_displays_the_tasty_error() {
        let error = ClassLoadError::InvalidTastyFile(
            BinaryName::from_internal("Dog"),
            TastyDecodeError::MissingTypeDef,
        );

        assert_eq!(
            error.to_string(),
            "invalid .tasty file for Dog: no matching TypeDef found in .tasty file"
        );
    }

    #[test]
    fn unresolved_type_variable_displays_the_owner_and_name() {
        let error = ClassLoadError::UnresolvedTypeVariable(
            BinaryName::from_internal("GenericSample"),
            "U".to_owned(),
        );

        assert_eq!(
            error.to_string(),
            "signature in class file for GenericSample references unresolved type variable U"
        );
    }

    #[test]
    fn is_cheaply_cloneable() {
        let error = ClassLoadError::CircularInheritance(BinaryName::from_internal("A"));
        let cloned = error.clone();
        assert_eq!(error.to_string(), cloned.to_string());
    }
}
