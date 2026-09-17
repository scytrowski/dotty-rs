use crate::binary_name::BinaryName;
use crate::class_path::{ClassPathEntry, ClassPathError, ClassResource, CompositeClassPath};
use crate::jmod_class_path::JmodClassPath;
use std::path::Path;

/// A [`ClassPathEntry`] backed by a `$JAVA_HOME/jmods`-shaped directory:
/// every `.jmod` file directly inside it is opened as a
/// [`JmodClassPath`], and a lookup is the first of them that has the
/// class (via [`CompositeClassPath`]'s existing first-match-wins
/// semantics — this type only adds directory discovery on top).
///
/// Every `.jmod` file is opened (its ZIP central directory parsed) once,
/// eagerly, in [`JdkClassPath::new`]. Deferring that per file until it
/// might actually be needed is a possible future optimization against a
/// real `jmods/` directory (tens of megabytes across dozens of files),
/// not attempted here.
pub struct JdkClassPath {
    composite: CompositeClassPath,
}

impl JdkClassPath {
    /// `jmods_dir` is the `jmods` directory itself (e.g.
    /// `$JAVA_HOME/jmods`), not `$JAVA_HOME`. Resolving `$JAVA_HOME` is a
    /// driver/CLI-layer concern, not this crate's.
    pub fn new(jmods_dir: impl AsRef<Path>) -> Result<Self, ClassPathError> {
        let mut jmod_paths: Vec<_> = std::fs::read_dir(jmods_dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, std::io::Error>>()?;
        jmod_paths.retain(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jmod")
        });
        jmod_paths.sort();

        let entries = jmod_paths
            .into_iter()
            .map(|path| JmodClassPath::new(path).map(|entry| Box::new(entry) as _))
            .collect::<Result<Vec<Box<dyn ClassPathEntry>>, ClassPathError>>()?;

        Ok(Self {
            composite: CompositeClassPath::new(entries),
        })
    }
}

impl ClassPathEntry for JdkClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        self.composite.find_class(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::ClassOrigin;
    use std::path::PathBuf;

    fn fixture_path(relative_path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path)
    }

    #[test]
    fn finds_classes_across_multiple_jmod_files() {
        let class_path = JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap();

        let pool_sample = class_path
            .find_class(&BinaryName::from_internal("pool/PoolSample"))
            .unwrap()
            .expect("pool/PoolSample should be found");
        assert!(matches!(pool_sample.origin(), ClassOrigin::Jmod(_)));

        let other_sample = class_path
            .find_class(&BinaryName::from_internal("other/OtherSample"))
            .unwrap()
            .expect("other/OtherSample should be found");
        assert!(matches!(other_sample.origin(), ClassOrigin::Jmod(_)));
    }

    #[test]
    fn returns_none_for_a_class_missing_from_every_jmod() {
        let class_path = JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("DoesNotExist"))
            .unwrap();

        assert!(resource.is_none());
    }

    #[test]
    fn new_reports_an_io_error_for_a_missing_directory() {
        let result = JdkClassPath::new(fixture_path("tests/fixtures/does_not_exist_jmods"));
        let error = result.err().expect("missing directory should be an error");

        assert!(error.to_string().starts_with("classpath I/O error:"));
    }
}
