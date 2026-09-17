use crate::binary_name::BinaryName;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Where a [`ClassResource`]'s bytes came from, kept for diagnostics and
/// duplicate-class detection.
///
/// `#[non_exhaustive]` and only `Directory` today: JAR/JMOD-backed origins
/// are planned (see `docs/classloader.md` §3) and will be added as new
/// variants without breaking existing matches on this type.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClassOrigin {
    /// The resource was read from a file under this directory root.
    Directory(PathBuf),
}

/// A decoded-but-not-yet-parsed classpath entry: the raw bytes of a class
/// file plus where they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassResource {
    bytes: Vec<u8>,
    origin: ClassOrigin,
}

impl ClassResource {
    pub fn new(bytes: Vec<u8>, origin: ClassOrigin) -> Self {
        Self { bytes, origin }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn origin(&self) -> &ClassOrigin {
        &self.origin
    }
}

/// An error reading from a classpath entry.
///
/// A class simply not being present on an entry is not an error: it is
/// `Ok(None)` from [`ClassPathEntry::find_class`]. This type is for actual
/// I/O failures (a permission error, a truncated read, etc.).
#[derive(Debug)]
pub struct ClassPathError {
    source: io::Error,
}

impl ClassPathError {
    pub fn new(source: io::Error) -> Self {
        Self { source }
    }
}

impl fmt::Display for ClassPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "classpath I/O error: {}", self.source)
    }
}

impl std::error::Error for ClassPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<io::Error> for ClassPathError {
    fn from(source: io::Error) -> Self {
        Self::new(source)
    }
}

/// A source of class file bytes, keyed by [`BinaryName`].
///
/// Deliberately an open trait rather than a closed enum of "directory" or
/// "archive" variants (see `docs/classloader.md` §3), so new classpath
/// sources (JARs, JMODs, in-memory sources for tests, ...) can be added
/// without changing this trait or any of its existing implementors.
///
/// `Send + Sync` from the start: classpath sharing across compilation
/// threads is expected later, and adding the bound after the fact would
/// break every implementor.
pub trait ClassPathEntry: Send + Sync {
    /// Looks up `name` on this entry. `Ok(None)` means the entry was
    /// searched successfully and does not contain `name` — that is not an
    /// error. `Err` is reserved for actual I/O failures.
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_path_error_displays_the_source_error() {
        let error = ClassPathError::from(io::Error::new(io::ErrorKind::PermissionDenied, "nope"));
        assert_eq!(error.to_string(), "classpath I/O error: nope");
    }

    #[test]
    fn class_path_error_exposes_the_source_error() {
        use std::error::Error;

        let error = ClassPathError::from(io::Error::new(io::ErrorKind::PermissionDenied, "nope"));
        assert!(error.source().is_some());
    }

    #[test]
    fn class_resource_exposes_its_bytes_and_origin() {
        let origin = ClassOrigin::Directory(PathBuf::from("/classes"));
        let resource = ClassResource::new(vec![0xCA, 0xFE], origin.clone());

        assert_eq!(resource.bytes(), &[0xCA, 0xFE]);
        assert_eq!(resource.origin(), &origin);
    }
}
