use crate::binary_name::BinaryName;
use crate::error::ClassLoadCategory;
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

/// Which decoder a [`ClassResource`]'s bytes should go through:
/// `dotty_classfile::class_file::ClassFile::decode` for [`Self::Class`],
/// `dotty_tasty::file::TastyFile::parse_scala_3_9` for [`Self::Tasty`].
///
/// A classpath entry that finds both a `.tasty` and a `.class` file for
/// the same name reports [`Self::Tasty`] (see `docs/classloader.md` §9
/// and this crate's module doc comment: `.tasty` is preferred).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassFormat {
    Class,
    Tasty,
}

/// A decoded-but-not-yet-parsed classpath entry: the raw bytes of a class
/// file, which decoder they need, and where they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassResource {
    bytes: Vec<u8>,
    format: ClassFormat,
    origin: ClassOrigin,
}

impl ClassResource {
    pub fn new(bytes: Vec<u8>, format: ClassFormat, origin: ClassOrigin) -> Self {
        Self {
            bytes,
            format,
            origin,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn format(&self) -> ClassFormat {
        self.format
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
    InvalidBinaryName(BinaryName),
}

impl ClassPathError {
    pub fn new(source: io::Error) -> Self {
        Self {
            source: ClassPathErrorSource::Io(source),
        }
    }

    /// `name` is not safe to turn into a filesystem or archive path (see
    /// [`BinaryName::is_path_safe`]) — reported by a [`ClassPathEntry`]
    /// instead of resolving it, since `name` may come from untrusted
    /// class-file bytes elsewhere on the classpath, not just a
    /// compiler-driven lookup.
    pub fn invalid_binary_name(name: BinaryName) -> Self {
        Self {
            source: ClassPathErrorSource::InvalidBinaryName(name),
        }
    }

    /// Stable identifier for the retained classpath failure kind.
    pub fn code(&self) -> &'static str {
        match &self.source {
            ClassPathErrorSource::Io(error) if error.kind() == io::ErrorKind::InvalidData => {
                "classpath.invalid_data"
            }
            ClassPathErrorSource::Io(_) => "classpath.io",
            ClassPathErrorSource::Zip(error) => error.code(),
            ClassPathErrorSource::InvalidBinaryName(_) => "classpath.invalid_binary_name",
        }
    }

    /// Category derived from the retained classpath cause.
    pub fn category(&self) -> ClassLoadCategory {
        match &self.source {
            ClassPathErrorSource::Io(error) if error.kind() == io::ErrorKind::InvalidData => {
                ClassLoadCategory::Conditional
            }
            ClassPathErrorSource::Io(_) => ClassLoadCategory::IoOrEnvironment,
            ClassPathErrorSource::Zip(error) => error.category(),
            ClassPathErrorSource::InvalidBinaryName(_) => ClassLoadCategory::MalformedArtifact,
        }
    }
}

impl fmt::Display for ClassPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            ClassPathErrorSource::Io(error) => write!(formatter, "classpath I/O error: {error}"),
            ClassPathErrorSource::Zip(error) => write!(formatter, "classpath I/O error: {error}"),
            ClassPathErrorSource::InvalidBinaryName(name) => {
                write!(formatter, "class name is not safe to use as a path: {name}")
            }
        }
    }
}

impl std::error::Error for ClassPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.source {
            ClassPathErrorSource::Io(error) => Some(error),
            ClassPathErrorSource::Zip(error) => Some(error),
            ClassPathErrorSource::InvalidBinaryName(_) => None,
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
    /// error. `Err` covers actual I/O failures, and (every filesystem/
    /// archive-backed implementor must check this) `name` failing
    /// [`BinaryName::is_path_safe`] — `name` can come from untrusted
    /// class-file bytes elsewhere on the classpath, not just a
    /// compiler-driven lookup, so it must never be resolved unchecked.
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError>;

    /// Whether this entry contains at least one direct class or TASTy
    /// resource in `package`. `false` means this entry provides no positive
    /// evidence for the package; it is deliberately not a negative cache
    /// result (another entry may contain it, or an implementation may not
    /// support package inspection).
    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        let _ = package;
        Ok(false)
    }

    /// Whether this entry has positive classpath evidence for `package` or
    /// any descendant package. `true` only establishes that at least one
    /// usable `.class` or `.tasty` resource exists at or below this logical
    /// package path; it does not imply a direct class, module, or
    /// package-object term exists in `package`. `false` means this entry has
    /// no such evidence and is not a negative result for other entries or
    /// source declarations.
    ///
    /// The default preserves compatibility for custom entries by treating
    /// [`Self::contains_package`] as partial positive evidence. Such entries
    /// may not discover descendant-only packages. Filesystem and archive
    /// implementations must reject unsafe package segments and propagate
    /// actual I/O or archive errors.
    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        if !safe_package_segments(package) {
            return Ok(false);
        }
        self.contains_package(package)
    }
}

/// A [`ClassPathEntry`] backed by a single filesystem directory, mapping
/// `BinaryName` to `<root>/<internal name>.tasty`, preferred, or
/// `<root>/<internal name>.class` as a fallback (`docs/classloader.md`
/// §9: `.tasty` is preferred over `.class` when both exist for the same
/// name).
pub struct DirectoryClassPath {
    root: PathBuf,
}

impl DirectoryClassPath {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resource_path(&self, name: &BinaryName, extension: &str) -> PathBuf {
        self.root
            .join(format!("{}.{extension}", name.as_internal()))
    }

    fn read(&self, path: &std::path::Path) -> Result<Option<Vec<u8>>, ClassPathError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ClassPathError::from(error)),
        }
    }

    fn contains_package_resource_at_or_below(
        &self,
        package: &[&str],
    ) -> Result<bool, ClassPathError> {
        if !safe_package_segments(package) {
            return Ok(false);
        }

        let root_type = match std::fs::symlink_metadata(&self.root) {
            Ok(metadata) => metadata.file_type(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if root_type.is_symlink() || !root_type.is_dir() {
            return Ok(false);
        }

        let mut package_directory = self.root.clone();
        for segment in package {
            package_directory.push(segment);
            let file_type = match std::fs::symlink_metadata(&package_directory) {
                Ok(metadata) => metadata.file_type(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.into()),
            };
            if file_type.is_symlink() || !file_type.is_dir() {
                return Ok(false);
            }
        }

        // Iterative traversal is limited to the requested package subtree.
        // Symlinks are not followed, preventing both cycles and escapes from
        // the classpath root.
        let mut pending = vec![package_directory];
        while let Some(directory) = pending.pop() {
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                if file_type.is_dir() {
                    pending.push(entry.path());
                } else if file_type.is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "class" || extension == "tasty")
                {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

impl ClassPathEntry for DirectoryClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if !name.is_path_safe() {
            return Err(ClassPathError::invalid_binary_name(name.clone()));
        }

        if let Some(bytes) = self.read(&self.resource_path(name, "tasty"))? {
            return Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Tasty,
                ClassOrigin::Directory(self.root.clone()),
            )));
        }

        match self.read(&self.resource_path(name, "class"))? {
            Some(bytes) => Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(self.root.clone()),
            ))),
            None => Ok(None),
        }
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        if !safe_package_segments(package) {
            return Ok(false);
        }
        let directory = package
            .iter()
            .fold(self.root.clone(), |path, segment| path.join(segment));
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let metadata = match std::fs::metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !metadata.is_file() {
                continue;
            }
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "class" || extension == "tasty")
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.contains_package_resource_at_or_below(package)
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

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        for entry in &self.entries {
            if entry.contains_package(package)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub(crate) fn safe_package_segments(package: &[&str]) -> bool {
    package.iter().all(|segment| {
        !segment.is_empty()
            && *segment != "."
            && *segment != ".."
            && !segment.contains('/')
            && !segment.contains('\\')
            && !segment.contains(':')
            && !segment.contains('\0')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DirectEvidenceClassPath(AtomicUsize);

    impl ClassPathEntry for DirectEvidenceClassPath {
        fn find_class(&self, _name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            Ok(None)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(package == ["pkg"])
        }
    }

    #[test]
    fn class_path_error_displays_the_source_error() {
        let error = ClassPathError::from(io::Error::new(io::ErrorKind::PermissionDenied, "nope"));
        assert_eq!(error.to_string(), "classpath I/O error: nope");
    }

    #[test]
    fn package_prefix_default_uses_direct_evidence_as_a_compatible_fallback() {
        let entry = DirectEvidenceClassPath(AtomicUsize::new(0));

        assert!(entry.contains_package_prefix(&["pkg"]).unwrap());
        assert!(!entry.contains_package_prefix(&["pkg", "nested"]).unwrap());
        assert_eq!(entry.0.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn package_prefix_default_rejects_unsafe_segments_before_delegating() {
        let entry = DirectEvidenceClassPath(AtomicUsize::new(0));

        for package in [
            &["."][..],
            &[".."],
            &["a/b"],
            &["a\\b"],
            &["a:b"],
            &["a\0b"],
            &[""],
        ] {
            assert!(!entry.contains_package_prefix(package).unwrap());
        }
        assert_eq!(entry.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn class_path_error_exposes_the_source_error() {
        use std::error::Error;

        let error = ClassPathError::from(io::Error::new(io::ErrorKind::PermissionDenied, "nope"));
        assert!(error.source().is_some());
    }

    #[test]
    fn invalid_binary_name_displays_the_offending_name() {
        let error =
            ClassPathError::invalid_binary_name(BinaryName::from_internal("../../etc/passwd"));

        assert_eq!(
            error.to_string(),
            "class name is not safe to use as a path: ../../etc/passwd"
        );
    }

    #[test]
    fn invalid_binary_name_has_no_further_source() {
        use std::error::Error;

        let error =
            ClassPathError::invalid_binary_name(BinaryName::from_internal("../../etc/passwd"));
        assert!(error.source().is_none());
    }

    #[test]
    fn class_resource_exposes_its_bytes_format_and_origin() {
        let origin = ClassOrigin::Directory(PathBuf::from("/classes"));
        let resource = ClassResource::new(vec![0xCA, 0xFE], ClassFormat::Class, origin.clone());

        assert_eq!(resource.bytes(), &[0xCA, 0xFE]);
        assert_eq!(resource.format(), ClassFormat::Class);
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
            ClassFormat::Class,
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
