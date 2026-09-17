use crate::binary_name::BinaryName;
use crate::class_path::{ClassOrigin, ClassPathEntry, ClassPathError, ClassResource};
use crate::zip_archive::ZipArchive;
use std::path::PathBuf;

/// A [`ClassPathEntry`] backed by a single JAR file, mapping
/// `BinaryName` to the entry named `<internal name>.class`.
///
/// The whole JAR is read and its central directory parsed once, in
/// [`JarClassPath::new`]; each [`ClassPathEntry::find_class`] call then
/// only extracts the one entry being looked up.
pub struct JarClassPath {
    path: PathBuf,
    archive: ZipArchive,
}

impl JarClassPath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ClassPathError> {
        let path = path.into();
        let bytes = std::fs::read(&path)?;
        let archive = ZipArchive::open(bytes)?;

        Ok(Self { path, archive })
    }
}

impl ClassPathEntry for JarClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let entry_name = format!("{}.class", name.as_internal());

        match self.archive.read_entry(&entry_name)? {
            Some(bytes) => Ok(Some(ClassResource::new(
                bytes,
                ClassOrigin::Jar(self.path.clone()),
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
    fn finds_a_class_entry_present_in_the_jar() {
        let class_path = JarClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jar/pool_sample_stored.jar",
        ))
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("PoolSample"))
            .unwrap()
            .expect("entry should be found");

        let expected = std::fs::read(fixture_path(
            "../dotty-classfile/tests/fixtures/pool_sample/PoolSample.class",
        ))
        .unwrap();
        assert_eq!(resource.bytes(), expected.as_slice());
        assert!(matches!(resource.origin(), ClassOrigin::Jar(_)));
    }

    #[test]
    fn returns_none_for_a_class_missing_from_the_jar() {
        let class_path = JarClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jar/pool_sample_stored.jar",
        ))
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("DoesNotExist"))
            .unwrap();

        assert!(resource.is_none());
    }

    #[test]
    fn new_reports_an_io_error_for_a_missing_jar_file() {
        let result = JarClassPath::new(fixture_path(
            "tests/fixtures/pool_sample_jar/does_not_exist.jar",
        ));
        let error = result.err().expect("missing file should be an error");

        assert!(error.to_string().starts_with("classpath I/O error:"));
    }
}
