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

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.composite.contains_package(package)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.composite.contains_package_prefix(package)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::ClassFormat;
    use crate::class_path::ClassOrigin;
    use std::fs;
    use std::path::PathBuf;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "dotty-classloader-jdk-{label}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_jmod(directory: &std::path::Path, name: &str, entries: &[(&str, &[u8])]) {
        let mut bytes = [0x4A, 0x4D, 0x01, 0x00].to_vec();
        bytes.extend(crate::jar_class_path::tests::minimal_stored_zip(entries));
        fs::write(directory.join(name), bytes).unwrap();
    }

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
        assert_eq!(pool_sample.format(), ClassFormat::Class);

        let other_sample = class_path
            .find_class(&BinaryName::from_internal("other/OtherSample"))
            .unwrap()
            .expect("other/OtherSample should be found");
        assert!(matches!(other_sample.origin(), ClassOrigin::Jmod(_)));
        assert_eq!(other_sample.format(), ClassFormat::Class);
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
    fn reports_only_packages_with_class_resources() {
        let class_path = JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap();

        assert!(class_path.contains_package(&["pool"]).unwrap());
        assert!(!class_path.contains_package(&["missing"]).unwrap());
        assert!(!class_path.contains_package(&["..", "pool"]).unwrap());
    }

    #[test]
    fn package_prefix_searches_descendants_across_jmods() {
        let class_path = JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap();

        assert!(class_path.contains_package_prefix(&["pool"]).unwrap());
        assert!(class_path.contains_package_prefix(&["other"]).unwrap());
        assert!(!class_path.contains_package_prefix(&["missing"]).unwrap());
        assert!(!class_path.contains_package_prefix(&["..", "pool"]).unwrap());
    }

    #[test]
    fn new_reports_an_io_error_for_a_missing_directory() {
        let result = JdkClassPath::new(fixture_path("tests/fixtures/does_not_exist_jmods"));
        let error = result.err().expect("missing directory should be an error");

        assert!(error.to_string().starts_with("classpath I/O error:"));
    }

    #[test]
    fn sorts_jmods_lexically_and_matches_composite_first_match_wins() {
        let temp = TemporaryDirectory::new("lexical-order");
        let jmods = temp.path().join("jmods");
        fs::create_dir(&jmods).unwrap();
        let alpha = jmods.join("alpha.jmod");
        let zeta = jmods.join("zeta.jmod");
        write_jmod(
            &jmods,
            "zeta.jmod",
            &[
                ("classes/shared/Duplicate.class", b"zeta duplicate"),
                ("classes/zeta/Only.class", b"zeta only"),
            ],
        );
        write_jmod(
            &jmods,
            "alpha.jmod",
            &[
                ("classes/shared/Duplicate.class", b"alpha duplicate"),
                ("classes/alpha/Only.class", b"alpha only"),
            ],
        );

        let class_path = JdkClassPath::new(&jmods).unwrap();
        let duplicate_name = BinaryName::from_internal("shared/Duplicate");
        let duplicate = class_path
            .find_class(&duplicate_name)
            .unwrap()
            .expect("duplicate class must be found");
        let alpha_only = class_path
            .find_class(&BinaryName::from_internal("alpha/Only"))
            .unwrap()
            .expect("class from alpha.jmod must be found");
        let zeta_only = class_path
            .find_class(&BinaryName::from_internal("zeta/Only"))
            .unwrap()
            .expect("class from zeta.jmod must be found");
        let composite = CompositeClassPath::new(vec![
            Box::new(JmodClassPath::new(&alpha).unwrap()),
            Box::new(JmodClassPath::new(&zeta).unwrap()),
        ]);

        assert_eq!(duplicate.bytes(), b"alpha duplicate");
        assert_eq!(duplicate.origin(), &ClassOrigin::Jmod(alpha.clone()));
        assert_eq!(
            duplicate,
            composite.find_class(&duplicate_name).unwrap().unwrap()
        );
        assert_eq!(alpha_only.bytes(), b"alpha only");
        assert_eq!(zeta_only.bytes(), b"zeta only");
        assert_eq!(alpha_only.origin(), &ClassOrigin::Jmod(alpha));
        assert_eq!(zeta_only.origin(), &ClassOrigin::Jmod(zeta));

        for _ in 0..3 {
            let repeated = JdkClassPath::new(&jmods)
                .unwrap()
                .find_class(&duplicate_name)
                .unwrap()
                .unwrap();
            assert_eq!(repeated, duplicate);
        }
    }

    #[test]
    fn a_corrupt_jmod_propagates_an_archive_error_instead_of_a_class_miss() {
        let temp = TemporaryDirectory::new("corrupt-jmod");
        let jmods = temp.path().join("jmods");
        fs::create_dir(&jmods).unwrap();
        fs::write(jmods.join("broken.jmod"), b"JM\x01\x00PK\x03\x04truncated").unwrap();

        let error = JdkClassPath::new(&jmods)
            .err()
            .expect("a corrupt archive must fail eagerly");

        assert_eq!(error.code(), "classpath.zip.unsupported_or_invalid_archive");
        assert_eq!(
            error.category(),
            crate::error::ClassLoadCategory::Conditional
        );
    }

    #[test]
    fn classpath_entries_are_send_sync_and_support_concurrent_reads() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<JmodClassPath>();
        assert_send_sync::<JdkClassPath>();
        assert_send_sync::<CompositeClassPath>();

        let class_path = std::sync::Arc::new(
            JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap(),
        );
        std::thread::scope(|scope| {
            let readers = (0..4)
                .map(|index| {
                    let class_path = std::sync::Arc::clone(&class_path);
                    scope.spawn(move || {
                        let name = if index % 2 == 0 {
                            "pool/PoolSample"
                        } else {
                            "other/OtherSample"
                        };
                        let resource = class_path
                            .find_class(&BinaryName::from_internal(name))
                            .unwrap()
                            .unwrap();
                        assert_eq!(resource.format(), ClassFormat::Class);
                        resource
                    })
                })
                .collect::<Vec<_>>();

            for reader in readers {
                assert!(!reader.join().unwrap().bytes().is_empty());
            }
        });
    }
}
