use crate::binary_name::BinaryName;
use crate::class_path::{ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource};
use crate::zip_archive::ZipArchive;
use std::io;
use std::path::PathBuf;

/// A JMOD file is a 4-byte magic (`"JM"` followed by a version byte
/// pair, currently `1.0`) immediately followed by an ordinary ZIP
/// archive whose class files live under a `classes/` prefix.
const JMOD_MAGIC: [u8; 4] = [0x4A, 0x4D, 0x01, 0x00];

/// A [`ClassPathEntry`] backed by a single JMOD file, mapping
/// `BinaryName` to the entry named `classes/<internal name>.class`.
///
/// A JMOD is otherwise an ordinary ZIP archive (see [`ZipArchive`]) once
/// its 4-byte magic is stripped, so this type only adds the magic check
/// and the `classes/` prefix on top of [`JarClassPath`](crate::jar_class_path::JarClassPath)'s
/// approach.
pub struct JmodClassPath {
    path: PathBuf,
    archive: ZipArchive,
}

impl JmodClassPath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ClassPathError> {
        let path = path.into();
        let mut bytes = std::fs::read(&path)?;

        if bytes.len() < JMOD_MAGIC.len() || bytes[..JMOD_MAGIC.len()] != JMOD_MAGIC {
            return Err(ClassPathError::from(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("not a JMOD file (bad magic): {}", path.display()),
            )));
        }
        bytes.drain(..JMOD_MAGIC.len());

        let archive = ZipArchive::open(bytes)?;

        Ok(Self { path, archive })
    }
}

impl ClassPathEntry for JmodClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if !name.is_path_safe() {
            return Err(ClassPathError::invalid_binary_name(name.clone()));
        }

        let entry_name = format!("classes/{}.class", name.as_internal());

        match self.archive.read_entry(&entry_name)? {
            Some(bytes) => Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Jmod(self.path.clone()),
            ))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(relative_path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path)
    }

    #[test]
    fn finds_a_class_entry_present_in_the_jmod() {
        let class_path = JmodClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jmod/pool_sample.jmod",
        ))
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("pool/PoolSample"))
            .unwrap()
            .expect("entry should be found");

        assert!(!resource.bytes().is_empty());
        assert!(matches!(resource.origin(), ClassOrigin::Jmod(_)));
    }

    #[test]
    fn rejects_a_path_traversal_name_instead_of_probing_the_archive() {
        let class_path = JmodClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jmod/pool_sample.jmod",
        ))
        .unwrap();

        let error = class_path
            .find_class(&BinaryName::from_internal("../../etc/passwd"))
            .expect_err("a path-traversal name must be rejected, not resolved");

        assert!(error.to_string().contains("not safe to use as a path"));
    }

    #[test]
    fn returns_none_for_a_class_missing_from_the_jmod() {
        let class_path = JmodClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jmod/pool_sample.jmod",
        ))
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("DoesNotExist"))
            .unwrap();

        assert!(resource.is_none());
    }

    #[test]
    fn new_reports_an_io_error_for_a_missing_jmod_file() {
        let result = JmodClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jmod/does_not_exist.jmod",
        ));
        let error = result.err().expect("missing file should be an error");

        assert!(error.to_string().starts_with("classpath I/O error:"));
    }

    #[test]
    fn new_rejects_a_file_with_the_wrong_magic() {
        let result = JmodClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jar/pool_sample_stored.jar",
        ));
        let error = result.err().expect("wrong-magic file should be an error");

        assert!(error.to_string().starts_with("classpath I/O error:"));
    }

    #[test]
    fn new_rejects_a_file_too_short_to_contain_the_magic() {
        let path = std::env::temp_dir().join("dotty_classloader_jmod_short_fixture.bin");
        std::fs::write(&path, [0x4A, 0x4D]).unwrap();

        let error = JmodClassPath::new(&path)
            .err()
            .expect("too-short file should be an error");
        assert!(error.to_string().starts_with("classpath I/O error:"));

        std::fs::remove_file(&path).unwrap();
    }
}
