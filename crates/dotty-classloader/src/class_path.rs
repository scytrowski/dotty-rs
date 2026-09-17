use crate::binary_name::BinaryName;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Where a [`ClassResource`]'s bytes came from, kept for diagnostics and
/// duplicate-class detection.
///
/// `#[non_exhaustive]`: further classpath sources (see
/// `docs/classloader.md` §3) may still add variants without breaking
/// existing matches on this type.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClassOrigin {
    /// The resource was read from a file under this directory root.
    Directory(PathBuf),
    /// The resource was read from an entry inside this JAR file.
    Jar(PathBuf),
    /// The resource was read from a `classes/` entry inside this JMOD
    /// file.
    Jmod(PathBuf),
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
/// I/O failures (a permission error, a truncated read, a malformed JAR,
/// etc.).
#[derive(Debug)]
pub struct ClassPathError {
    source: ClassPathErrorSource,
}

#[derive(Debug)]
enum ClassPathErrorSource {
    Io(io::Error),
    Zip(crate::zip_archive::ZipError),
}

impl ClassPathError {
    pub fn new(source: io::Error) -> Self {
        Self {
            source: ClassPathErrorSource::Io(source),
        }
    }
}

impl fmt::Display for ClassPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            ClassPathErrorSource::Io(error) => write!(formatter, "classpath I/O error: {error}"),
            ClassPathErrorSource::Zip(error) => write!(formatter, "classpath I/O error: {error}"),
        }
    }
}

impl std::error::Error for ClassPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.source {
            ClassPathErrorSource::Io(error) => Some(error),
            ClassPathErrorSource::Zip(error) => Some(error),
        }
    }
}

impl From<io::Error> for ClassPathError {
    fn from(source: io::Error) -> Self {
        Self::new(source)
    }
}

impl From<crate::zip_archive::ZipError> for ClassPathError {
    fn from(source: crate::zip_archive::ZipError) -> Self {
        Self {
            source: ClassPathErrorSource::Zip(source),
        }
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

/// A [`ClassPathEntry`] backed by a single filesystem directory, mapping
/// `BinaryName` to `<root>/<internal name>.class`.
pub struct DirectoryClassPath {
    root: PathBuf,
}

impl DirectoryClassPath {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn class_file_path(&self, name: &BinaryName) -> PathBuf {
        self.root.join(format!("{}.class", name.as_internal()))
    }
}

impl ClassPathEntry for DirectoryClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let path = self.class_file_path(name);

        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(ClassResource::new(
                bytes,
                ClassOrigin::Directory(self.root.clone()),
            ))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ClassPathError::from(error)),
        }
    }
}

/// An ordered, first-match-wins composite of [`ClassPathEntry`]s: entries
/// are searched in order, and the first one that returns `Ok(Some(_))`
/// wins. An entry returning `Ok(None)` falls through to the next entry; an
/// entry returning `Err` stops the search immediately and propagates that
/// error.
pub struct CompositeClassPath {
    entries: Vec<Box<dyn ClassPathEntry>>,
}

impl CompositeClassPath {
    pub fn new(entries: Vec<Box<dyn ClassPathEntry>>) -> Self {
        Self { entries }
    }
}

impl ClassPathEntry for CompositeClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        for entry in &self.entries {
            if let Some(resource) = entry.find_class(name)? {
                return Ok(Some(resource));
            }
        }

        Ok(None)
    }
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

    struct FixedEntry(Result<Option<ClassResource>, ()>);

    impl ClassPathEntry for FixedEntry {
        fn find_class(&self, _name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            match &self.0 {
                Ok(resource) => Ok(resource.clone()),
                Err(()) => Err(ClassPathError::from(io::Error::other("boom"))),
            }
        }
    }

    fn found(marker: u8) -> Result<Option<ClassResource>, ()> {
        Ok(Some(ClassResource::new(
            vec![marker],
            ClassOrigin::Directory(PathBuf::from("/irrelevant")),
        )))
    }

    #[test]
    fn composite_returns_the_first_entry_that_finds_the_class() {
        let composite = CompositeClassPath::new(vec![
            Box::new(FixedEntry(found(1))),
            Box::new(FixedEntry(found(2))),
        ]);

        let resource = composite
            .find_class(&BinaryName::from_internal("Anything"))
            .unwrap()
            .unwrap();

        assert_eq!(resource.bytes(), &[1]);
    }

    #[test]
    fn composite_falls_through_to_the_next_entry_on_a_miss() {
        let composite = CompositeClassPath::new(vec![
            Box::new(FixedEntry(Ok(None))),
            Box::new(FixedEntry(found(2))),
        ]);

        let resource = composite
            .find_class(&BinaryName::from_internal("Anything"))
            .unwrap()
            .unwrap();

        assert_eq!(resource.bytes(), &[2]);
    }

    #[test]
    fn composite_returns_none_when_no_entry_has_the_class() {
        let composite = CompositeClassPath::new(vec![Box::new(FixedEntry(Ok(None)))]);

        let resource = composite
            .find_class(&BinaryName::from_internal("Anything"))
            .unwrap();

        assert!(resource.is_none());
    }

    #[test]
    fn composite_propagates_an_entrys_error_without_trying_later_entries() {
        let composite = CompositeClassPath::new(vec![
            Box::new(FixedEntry(Err(()))),
            Box::new(FixedEntry(found(2))),
        ]);

        let error = composite
            .find_class(&BinaryName::from_internal("Anything"))
            .unwrap_err();

        assert_eq!(error.to_string(), "classpath I/O error: boom");
    }
}
