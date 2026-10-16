use crate::binary_name::BinaryName;
use crate::class_path::{ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource};
use crate::zip_archive::ZipArchive;
use std::collections::HashSet;
use std::io::{self, Read};
use std::path::PathBuf;

/// A JMOD file is a 4-byte magic (`"JM"` followed by a version byte
/// pair, currently `1.0`) immediately followed by an ordinary ZIP
/// archive whose class files live under a `classes/` prefix.
const JMOD_MAGIC: [u8; 4] = [0x4A, 0x4D, 0x01, 0x00];

/// A [`ClassPathEntry`] backed by a single JMOD file, mapping
/// `BinaryName` to the entry named `classes/<internal name>.class`.
///
/// A JMOD is otherwise an ordinary ZIP archive (see `ZipArchive`) once
/// its 4-byte magic is skipped by the file-backed ZIP reader, so this
/// type only adds the magic check and the `classes/` prefix.
pub struct JmodClassPath {
    path: PathBuf,
    archive: ZipArchive,
    package_prefixes: HashSet<String>,
}

impl JmodClassPath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ClassPathError> {
        let path = path.into();
        let mut file = std::fs::File::open(&path)?;
        let mut magic = [0; JMOD_MAGIC.len()];
        if let Err(error) = file.read_exact(&mut magic) {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                return Err(ClassPathError::from(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("not a JMOD file (bad magic): {}", path.display()),
                )));
            }
            return Err(error.into());
        }
        if magic != JMOD_MAGIC {
            return Err(ClassPathError::from(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("not a JMOD file (bad magic): {}", path.display()),
            )));
        }

        let archive = ZipArchive::open_file_handle(file, JMOD_MAGIC.len() as u64)?;
        let package_prefixes = archive.package_prefixes_under("classes/");

        Ok(Self {
            path,
            archive,
            package_prefixes,
        })
    }

    pub(crate) fn origin(&self) -> ClassOrigin {
        ClassOrigin::Jmod(self.path.clone())
    }

    pub(crate) fn read_module_info(&self) -> Result<Option<Vec<u8>>, ClassPathError> {
        self.archive
            .read_entry("classes/module-info.class")
            .map_err(ClassPathError::from)
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

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        if !crate::class_path::safe_package_segments(package) {
            return Ok(false);
        }
        let path = package.join("/");
        let prefix = if path.is_empty() {
            "classes/".to_owned()
        } else {
            format!("classes/{path}/")
        };
        Ok(self.archive.contains_package(&prefix))
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        if !crate::class_path::safe_package_segments(package) {
            return Ok(false);
        }
        Ok(self.package_prefixes.contains(&package.join("/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::ClassFormat;

    fn fixture_path(relative_path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path)
    }

    fn write_temp_jmod(unique_name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("dotty_classloader_jmod_{unique_name}.jmod"));
        let mut bytes = JMOD_MAGIC.to_vec();
        bytes.extend(crate::jar_class_path::tests::minimal_stored_zip(entries));
        std::fs::write(&path, bytes).unwrap();
        path
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
        assert_eq!(resource.format(), ClassFormat::Class);
        assert_eq!(
            resource.origin(),
            &ClassOrigin::Jmod(fixture_path(
                "tests/fixtures/pool_sample_jmod/pool_sample.jmod"
            ))
        );
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

        assert_eq!(error.code(), "classpath.invalid_binary_name");
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
    fn package_prefix_uses_only_class_resources_under_the_jmod_classes_root() {
        let path = write_temp_jmod(
            "nested_package_prefix",
            &[
                ("classes/a/b/c/Leaf.class", b"class bytes"),
                ("classes/a/d/e/Leaf.tasty", b"tasty bytes"),
                ("classes/a/beta/Only.scala", b"source only"),
                ("classes/META-INF/Only.class", b"metadata only"),
                ("bin/a/b/Outside.class", b"non-class section"),
                ("classes/a/../outside/Unsafe.class", b"unsafe path"),
            ],
        );
        let class_path = JmodClassPath::new(&path).unwrap();

        assert!(!class_path.contains_package(&["a"]).unwrap());
        assert!(class_path.contains_package(&["a", "b", "c"]).unwrap());
        assert!(class_path.contains_package_prefix(&[]).unwrap());
        assert!(class_path.contains_package_prefix(&["a"]).unwrap());
        assert!(class_path.contains_package_prefix(&["a", "b"]).unwrap());
        assert!(
            class_path
                .contains_package_prefix(&["a", "b", "c"])
                .unwrap()
        );
        assert!(
            class_path
                .contains_package_prefix(&["a", "d", "e"])
                .unwrap()
        );
        assert!(!class_path.contains_package_prefix(&["a", "beta"]).unwrap());
        assert!(!class_path.contains_package_prefix(&["META-INF"]).unwrap());
        assert!(!class_path.contains_package_prefix(&["bin"]).unwrap());
        assert!(!class_path.contains_package_prefix(&["outside"]).unwrap());

        std::fs::remove_file(&path).unwrap();
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

        assert_eq!(error.code(), "classpath.invalid_data");
        assert_eq!(
            error.category(),
            crate::error::ClassLoadCategory::Conditional
        );
    }

    #[test]
    fn new_rejects_a_file_too_short_to_contain_the_magic() {
        let path = std::env::temp_dir().join("dotty_classloader_jmod_short_fixture.bin");
        std::fs::write(&path, [0x4A, 0x4D]).unwrap();

        let error = JmodClassPath::new(&path)
            .err()
            .expect("too-short file should be an error");
        assert_eq!(error.code(), "classpath.invalid_data");
        assert_eq!(
            error.category(),
            crate::error::ClassLoadCategory::Conditional
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn new_reports_a_typed_error_for_a_truncated_zip_after_valid_jmod_magic() {
        let path = std::env::temp_dir().join("dotty_classloader_jmod_truncated_zip.jmod");
        let mut bytes = JMOD_MAGIC.to_vec();
        bytes.extend_from_slice(b"PK\x03\x04truncated");
        std::fs::write(&path, bytes).unwrap();

        let error = JmodClassPath::new(&path)
            .err()
            .expect("a truncated archive must not open as an empty JMOD");

        assert_eq!(error.code(), "classpath.zip.unsupported_or_invalid_archive");
        assert_eq!(
            error.category(),
            crate::error::ClassLoadCategory::Conditional
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn file_backed_archive_does_not_retain_the_jmod_payload() {
        let path = write_temp_jmod(
            "file_backed_payload",
            &[("classes/pkg/A.class", b"class bytes")],
        );
        let class_path = JmodClassPath::new(&path).unwrap();

        assert_eq!(class_path.archive.retained_archive_payload_bytes(), 0);
        assert_eq!(
            class_path
                .find_class(&BinaryName::from_internal("pkg/A"))
                .unwrap()
                .unwrap()
                .bytes(),
            b"class bytes"
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn repeated_and_concurrent_reads_from_a_jmod_are_stable() {
        let path = write_temp_jmod(
            "concurrent_reads",
            &[("classes/pkg/A.class", b"class bytes")],
        );
        let class_path = JmodClassPath::new(&path).unwrap();
        let name = BinaryName::from_internal("pkg/A");

        std::thread::scope(|scope| {
            let readers = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..10 {
                            let resource = class_path.find_class(&name).unwrap().unwrap();
                            assert_eq!(resource.bytes(), b"class bytes");
                            assert_eq!(resource.format(), ClassFormat::Class);
                            assert_eq!(resource.origin(), &ClassOrigin::Jmod(path.clone()));
                        }
                    })
                })
                .collect::<Vec<_>>();
            for reader in readers {
                reader.join().unwrap();
            }
        });

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn truncating_a_jmod_after_open_returns_a_typed_read_error() {
        let path = write_temp_jmod(
            "truncated_after_open",
            &[("classes/pkg/A.class", b"class bytes")],
        );
        let class_path = JmodClassPath::new(&path).unwrap();
        std::fs::write(&path, JMOD_MAGIC).unwrap();

        let error = class_path
            .find_class(&BinaryName::from_internal("pkg/A"))
            .expect_err("truncating the backing JMOD must not panic or look like a miss");
        assert_eq!(error.code(), "classpath.zip.read_error");

        std::fs::remove_file(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn removing_a_jmod_after_open_keeps_the_open_archive_readable() {
        let path = write_temp_jmod(
            "removed_after_open",
            &[("classes/pkg/A.class", b"class bytes")],
        );
        let class_path = JmodClassPath::new(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("pkg/A"))
            .unwrap()
            .unwrap();
        assert_eq!(resource.bytes(), b"class bytes");
    }
}
