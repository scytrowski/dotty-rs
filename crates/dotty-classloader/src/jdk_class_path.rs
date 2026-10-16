use crate::binary_name::BinaryName;
use crate::class_path::{ClassPathEntry, ClassPathError, ClassResource};
use crate::jdk_module_catalog::{JdkModuleCatalog, JdkModuleCatalogError};
use crate::jmod_class_path::JmodClassPath;
use std::path::Path;

/// A [`ClassPathEntry`] backed by a `$JAVA_HOME/jmods`-shaped directory:
/// every `.jmod` file directly inside it is opened as a
/// [`JmodClassPath`], and a lookup is the first of them that has the
/// class (preserving [`crate::class_path::CompositeClassPath`]'s existing first-match-wins
/// semantics — this type only adds directory discovery on top).
///
/// Every `.jmod` file is opened (its ZIP central directory parsed) once,
/// eagerly, in [`JdkClassPath::new`]. JMOD payloads remain file-backed and
/// class entries are read on demand; only archive metadata is indexed at
/// construction time.
pub struct JdkClassPath {
    // Keep the opened concrete entries so explicit module metadata requests
    // can use the same file-backed archives without reopening every JMOD.
    jmods: Vec<JmodClassPath>,
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

        let jmods = jmod_paths
            .into_iter()
            .map(JmodClassPath::new)
            .collect::<Result<Vec<_>, ClassPathError>>()?;

        Ok(Self { jmods })
    }

    /// Reads and validates `classes/module-info.class` from each opened JMOD
    /// and returns an owned catalog sorted by descriptor module name. This
    /// work is lazy: constructing a `JdkClassPath` does not decode module
    /// metadata. Each call returns a fresh snapshot and retains no descriptor
    /// bytes or global cache.
    pub fn module_catalog(&self) -> Result<JdkModuleCatalog, JdkModuleCatalogError> {
        JdkModuleCatalog::read(&self.jmods)
    }
}

impl ClassPathEntry for JdkClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        for jmod in &self.jmods {
            if let Some(resource) = jmod.find_class(name)? {
                return Ok(Some(resource));
            }
        }
        Ok(None)
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        for jmod in &self.jmods {
            if jmod.contains_package(package)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        if !crate::class_path::safe_package_segments(package) {
            return Ok(false);
        }
        for jmod in &self.jmods {
            if jmod.contains_package_prefix(package)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::ClassOrigin;
    use crate::class_path::{ClassFormat, CompositeClassPath};
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

    fn module_fixture(name: &str) -> &'static [u8] {
        match name {
            "fixture.alpha" => include_bytes!(
                "../tests/fixtures/jdk_module_catalog/fixture.alpha-module-info.class"
            ),
            "fixture.beta" => include_bytes!(
                "../tests/fixtures/jdk_module_catalog/fixture.beta-module-info.class"
            ),
            "fixture.minimal" => include_bytes!(
                "../tests/fixtures/jdk_module_catalog/fixture.minimal-module-info.class"
            ),
            _ => panic!("unexpected test fixture name"),
        }
    }

    fn write_module_jmods(directory: &std::path::Path, names: &[&str]) {
        for name in names {
            let descriptor_entry = module_fixture(name);
            let mut entries = vec![("classes/module-info.class", descriptor_entry)];
            if *name == "fixture.alpha" {
                entries.push(("classes/fixture/alpha/api/Service.class", b"class bytes"));
            }
            write_jmod(directory, &format!("{name}.jmod"), &entries);
        }
    }

    fn corrupt_module_name_index(module_info: &[u8]) -> Vec<u8> {
        use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolIndex};
        use dotty_classfile::reader::Reader;

        let mut reader = Reader::new(module_info);
        reader.read_bytes(8).unwrap();
        let pool = ConstantPool::decode(&mut reader).unwrap();
        reader.read_bytes(6).unwrap(); // access_flags, this_class, super_class
        let interface_count = usize::from(reader.read_u16().unwrap());
        reader.read_bytes(interface_count * 2).unwrap();
        assert_eq!(reader.read_u16().unwrap(), 0); // fields_count
        assert_eq!(reader.read_u16().unwrap(), 0); // methods_count
        let attribute_count = reader.read_u16().unwrap();
        let mut corrupted = module_info.to_vec();
        for _ in 0..attribute_count {
            let name_index = ConstantPoolIndex(reader.read_u16().unwrap());
            let length = reader.read_u32().unwrap() as usize;
            let payload_offset = module_info.len() - reader.remaining();
            if pool.utf8(name_index).unwrap() == "Module" {
                corrupted[payload_offset..payload_offset + 2]
                    .copy_from_slice(&u16::MAX.to_be_bytes());
                return corrupted;
            }
            reader.read_bytes(length).unwrap();
        }
        panic!("fixture has no Module attribute");
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
    fn class_lookup_keeps_first_match_and_error_short_circuit_order() {
        let temp = TemporaryDirectory::new("first-match-short-circuit");
        let jmods = temp.path().join("jmods");
        fs::create_dir(&jmods).unwrap();
        let alpha = jmods.join("alpha.jmod");
        let zeta = jmods.join("zeta.jmod");
        write_jmod(
            &jmods,
            "alpha.jmod",
            &[("classes/shared/Duplicate.class", b"alpha")],
        );
        write_jmod(
            &jmods,
            "zeta.jmod",
            &[("classes/shared/Duplicate.class", b"zeta")],
        );

        let class_path = JdkClassPath::new(&jmods).unwrap();
        let zeta_contents = fs::read(&zeta).unwrap();
        fs::write(&zeta, b"changed after indexing").unwrap();
        let duplicate = class_path
            .find_class(&BinaryName::from_internal("shared/Duplicate"))
            .unwrap()
            .unwrap();
        assert_eq!(duplicate.bytes(), b"alpha");

        fs::write(&zeta, zeta_contents).unwrap();
        fs::write(&alpha, b"changed after indexing").unwrap();
        assert!(
            class_path
                .find_class(&BinaryName::from_internal("shared/Duplicate"))
                .is_err()
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

    #[test]
    fn module_catalog_projects_validated_metadata_deterministically_and_keeps_class_lookup() {
        let temp = TemporaryDirectory::new("module-catalog");
        let jmods = temp.path().join("jmods");
        fs::create_dir(&jmods).unwrap();
        write_module_jmods(
            &jmods,
            &["fixture.minimal", "fixture.beta", "fixture.alpha"],
        );

        let class_path = JdkClassPath::new(&jmods).unwrap();
        let service_name = BinaryName::from_internal("fixture/alpha/api/Service");
        let before_catalog = class_path.find_class(&service_name).unwrap().unwrap();
        let catalog = class_path.module_catalog().unwrap();
        let after_catalog = class_path.find_class(&service_name).unwrap().unwrap();

        assert_eq!(
            catalog
                .modules()
                .iter()
                .map(|module| module.name.as_str())
                .collect::<Vec<_>>(),
            ["fixture.alpha", "fixture.beta", "fixture.minimal"]
        );
        assert_eq!(catalog, class_path.module_catalog().unwrap());
        assert_eq!(catalog.get("missing"), None);

        let alpha = catalog.get("fixture.alpha").unwrap();
        assert_eq!(alpha.version.as_deref(), Some("1.2"));
        let beta_requirement = alpha
            .requires
            .iter()
            .find(|require| require.module == "fixture.beta")
            .unwrap();
        assert_ne!(
            beta_requirement.flags & dotty_classfile::attribute::MODULE_ACC_TRANSITIVE,
            0
        );
        assert_eq!(beta_requirement.version, None);
        assert!(
            alpha
                .exports
                .iter()
                .any(|row| row.package == "fixture.alpha.api" && row.targets == ["fixture.beta"])
        );
        assert!(
            alpha
                .opens
                .iter()
                .any(|row| row.package == "fixture.alpha.internal" && row.targets.is_empty())
        );
        assert!(
            alpha
                .uses
                .iter()
                .any(|name| name == "fixture.alpha.api.Service")
        );
        assert!(alpha.provides.iter().any(|row| {
            row.service == "fixture.alpha.api.Service"
                && row.providers == ["fixture.alpha.impl.Provider"]
        }));
        assert!(
            alpha
                .packages
                .as_ref()
                .unwrap()
                .contains(&"fixture.alpha.impl".to_owned())
        );
        assert_eq!(alpha.main_class.as_deref(), Some("fixture.alpha.app.Main"));
        assert_eq!(
            alpha.origin,
            ClassOrigin::Jmod(jmods.join("fixture.alpha.jmod"))
        );

        let minimal = catalog.get("fixture.minimal").unwrap();
        assert_eq!(minimal.packages, None);
        assert_eq!(minimal.main_class, None);
        assert_eq!(minimal.version, None);
        assert_eq!(before_catalog, after_catalog);
        assert_eq!(after_catalog.bytes(), b"class bytes");
        assert_eq!(
            after_catalog.origin(),
            &ClassOrigin::Jmod(jmods.join("fixture.alpha.jmod"))
        );
    }

    #[test]
    fn module_catalog_is_safe_for_concurrent_explicit_reads() {
        let temp = TemporaryDirectory::new("module-catalog-concurrent");
        let jmods = temp.path().join("jmods");
        fs::create_dir(&jmods).unwrap();
        write_module_jmods(&jmods, &["fixture.alpha", "fixture.beta"]);
        let class_path = std::sync::Arc::new(JdkClassPath::new(&jmods).unwrap());

        std::thread::scope(|scope| {
            let readers = (0..4)
                .map(|_| {
                    let class_path = std::sync::Arc::clone(&class_path);
                    scope.spawn(move || {
                        class_path
                            .module_catalog()
                            .unwrap()
                            .modules()
                            .iter()
                            .map(|module| module.name.clone())
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();

            for reader in readers {
                assert_eq!(reader.join().unwrap(), ["fixture.alpha", "fixture.beta"]);
            }
        });
    }

    #[test]
    fn module_catalog_returns_typed_errors_for_missing_malformed_and_invalid_descriptors() {
        use crate::jdk_module_catalog::JdkModuleCatalogError as Error;
        use dotty_classfile::class_file::ModuleDescriptorError;

        let temp = TemporaryDirectory::new("module-catalog-errors");

        let missing_dir = temp.path().join("missing");
        fs::create_dir(&missing_dir).unwrap();
        write_jmod(&missing_dir, "fixture.minimal.jmod", &[]);
        assert!(matches!(
            JdkClassPath::new(&missing_dir).unwrap().module_catalog(),
            Err(Error::MissingDescriptor { .. })
        ));

        let malformed_dir = temp.path().join("malformed");
        fs::create_dir(&malformed_dir).unwrap();
        write_jmod(
            &malformed_dir,
            "fixture.minimal.jmod",
            &[("classes/module-info.class", b"not a class file")],
        );
        assert!(matches!(
            JdkClassPath::new(&malformed_dir).unwrap().module_catalog(),
            Err(Error::InvalidClassFile { .. })
        ));

        let trailing_dir = temp.path().join("trailing-bytes");
        fs::create_dir(&trailing_dir).unwrap();
        let mut trailing = module_fixture("fixture.minimal").to_vec();
        trailing.push(0);
        write_jmod(
            &trailing_dir,
            "fixture.minimal.jmod",
            &[("classes/module-info.class", &trailing)],
        );
        assert!(matches!(
            JdkClassPath::new(&trailing_dir).unwrap().module_catalog(),
            Err(Error::TrailingClassFileBytes { remaining: 1, .. })
        ));

        let invalid_dir = temp.path().join("invalid-reference");
        fs::create_dir(&invalid_dir).unwrap();
        let corrupt = corrupt_module_name_index(module_fixture("fixture.minimal"));
        write_jmod(
            &invalid_dir,
            "fixture.minimal.jmod",
            &[("classes/module-info.class", &corrupt)],
        );
        assert!(matches!(
            JdkClassPath::new(&invalid_dir).unwrap().module_catalog(),
            Err(Error::InvalidDescriptor {
                source: ModuleDescriptorError::ConstantPool(_),
                ..
            })
        ));

        let corrupt_archive_dir = temp.path().join("corrupt-archive");
        fs::create_dir(&corrupt_archive_dir).unwrap();
        write_jmod(
            &corrupt_archive_dir,
            "fixture.minimal.jmod",
            &[(
                "classes/module-info.class",
                module_fixture("fixture.minimal"),
            )],
        );
        let class_path = JdkClassPath::new(&corrupt_archive_dir).unwrap();
        fs::write(
            corrupt_archive_dir.join("fixture.minimal.jmod"),
            b"changed after archive indexing",
        )
        .unwrap();
        assert!(matches!(
            class_path.module_catalog(),
            Err(Error::Archive {
                origin: ClassOrigin::Jmod(_),
                ..
            })
        ));
    }

    #[test]
    fn module_catalog_rejects_duplicate_names_and_filename_mismatches() {
        use crate::jdk_module_catalog::JdkModuleCatalogError as Error;

        let temp = TemporaryDirectory::new("module-catalog-identity");

        let duplicate_dir = temp.path().join("duplicates");
        fs::create_dir(&duplicate_dir).unwrap();
        let alpha = module_fixture("fixture.alpha");
        write_jmod(
            &duplicate_dir,
            "fixture.alpha.jmod",
            &[("classes/module-info.class", alpha)],
        );
        write_jmod(
            &duplicate_dir,
            "shadow.jmod",
            &[("classes/module-info.class", alpha)],
        );
        assert!(matches!(
            JdkClassPath::new(&duplicate_dir)
                .unwrap()
                .module_catalog(),
            Err(Error::DuplicateModuleName { name, .. }) if name == "fixture.alpha"
        ));

        let mismatch_dir = temp.path().join("mismatch");
        fs::create_dir(&mismatch_dir).unwrap();
        write_jmod(
            &mismatch_dir,
            "wrong-name.jmod",
            &[("classes/module-info.class", alpha)],
        );
        assert!(matches!(
            JdkClassPath::new(&mismatch_dir)
                .unwrap()
                .module_catalog(),
            Err(Error::ModuleNameMismatch {
                filename,
                descriptor_name,
                ..
            }) if filename == "wrong-name" && descriptor_name == "fixture.alpha"
        ));
    }
}
