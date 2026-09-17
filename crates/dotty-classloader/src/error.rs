use crate::binary_name::BinaryName;
use crate::class_path::ClassPathError;
use crate::tasty_symbol::TastyDecodeError;
use dotty_classfile::class_file::ClassFileError;
use dotty_classfile::constant_pool::PoolRefError;
use dotty_classfile::descriptor::ResolveDescriptorError;
use dotty_classfile::signature::SignatureError;
use std::fmt;
use std::rc::Rc;

/// Why loading a class failed.
///
/// `Clone` so a [`crate::ClassEntry::Failed`] cache entry can be returned
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
    /// Loading `owner` failed because resolving `dependency` (its
    /// superclass, an interface, or — since `docs/classloader.md` §9's
    /// Milestone 6 — a member's declared type) failed.
    DependencyFailure {
        owner: BinaryName,
        dependency: BinaryName,
        source: Rc<ClassLoadError>,
    },
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
            Self::DependencyFailure {
                owner,
                dependency,
                source,
            } => write!(
                formatter,
                "class {owner} failed to load because {dependency} failed: {source}"
            ),
        }
    }
}

impl std::error::Error for ClassLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound(_) | Self::NameMismatch { .. } | Self::CircularInheritance(_) => None,
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
    fn is_cheaply_cloneable() {
        let error = ClassLoadError::CircularInheritance(BinaryName::from_internal("A"));
        let cloned = error.clone();
        assert_eq!(error.to_string(), cloned.to_string());
    }
}
