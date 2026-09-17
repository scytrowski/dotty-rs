use crate::binary_name::BinaryName;
use crate::class_path::{ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource};
use crate::manifest;
use crate::zip_archive::ZipArchive;
use std::path::PathBuf;

const MANIFEST_ENTRY_NAME: &str = "META-INF/MANIFEST.MF";

/// The lowest feature-release version multi-release JARs recognize
/// (JEP 238): `META-INF/versions/N/` directories with `N < 9` are not
/// version directories at all.
const MIN_VERSIONED_DIRECTORY: u16 = 9;

/// A [`ClassPathEntry`] backed by a single JAR file, mapping
/// `BinaryName` to the entry named `<internal name>.class` - or, for a
/// multi-release JAR (the JAR File Specification's "Multi-release JAR
/// files" section), to the highest `META-INF/versions/N/<internal
/// name>.class` entry with `N` at or below `release_version`, falling
/// back to the unversioned entry when no such `N` exists.
///
/// `release_version` is the target feature-release version to select
/// `META-INF/versions/` entries for - analogous to a running JVM's own
/// version, but this crate never runs inside one, so the value has to
/// be a plain constructor parameter. Deciding what it should be (e.g.
/// from a compiler `-release` flag) is a driver/CLI-layer concern, the
/// same framing [`crate::jdk_class_path::JdkClassPath`] already uses for
/// resolving `$JAVA_HOME`.
///
/// The whole JAR is read and its central directory parsed once, in
/// [`JarClassPath::new`], which is also where the manifest is read once
/// to determine whether the JAR declares `Multi-Release: true` at all;
/// each [`ClassPathEntry::find_class`] call then only extracts the
/// entries it actually needs to check.
pub struct JarClassPath {
    path: PathBuf,
    archive: ZipArchive,
    release_version: u16,
    multi_release: bool,
}

impl JarClassPath {
    pub fn new(path: impl Into<PathBuf>, release_version: u16) -> Result<Self, ClassPathError> {
        let path = path.into();
        let bytes = std::fs::read(&path)?;
        let archive = ZipArchive::open(bytes)?;
        let multi_release = match archive.read_entry(MANIFEST_ENTRY_NAME)? {
            Some(manifest_bytes) => manifest::declares_multi_release(&manifest_bytes),
            None => false,
        };

        Ok(Self {
            path,
            archive,
            release_version,
            multi_release,
        })
    }

    /// The versioned entry names to probe, highest version first, for
    /// `entry_name`'s multi-release override. Empty when this JAR isn't
    /// multi-release, or when `release_version` is below the lowest
    /// version directory JEP 238 recognizes (`MIN_VERSIONED_DIRECTORY`
    /// then exceeds the upper bound, which `RangeInclusive` already
    /// treats as empty).
    fn versioned_entry_names(&self, entry_name: &str) -> impl Iterator<Item = String> {
        let upper_bound = if self.multi_release {
            self.release_version
        } else {
            0
        };

        (MIN_VERSIONED_DIRECTORY..=upper_bound)
            .rev()
            .map(move |version| format!("META-INF/versions/{version}/{entry_name}"))
    }
}

impl ClassPathEntry for JarClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let entry_name = format!("{}.class", name.as_internal());

        for versioned_entry_name in self.versioned_entry_names(&entry_name) {
            if let Some(bytes) = self.archive.read_entry(&versioned_entry_name)? {
                return Ok(Some(ClassResource::new(
                    bytes,
                    ClassFormat::Class,
                    ClassOrigin::Jar(self.path.clone()),
                )));
            }
        }

        match self.archive.read_entry(&entry_name)? {
            Some(bytes) => Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
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
        let class_path = JarClassPath::new(
            fixture_path("tests/fixtures/pool_sample_jar/pool_sample_stored.jar"),
            21,
        )
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
        let class_path = JarClassPath::new(
            fixture_path("tests/fixtures/pool_sample_jar/pool_sample_stored.jar"),
            21,
        )
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("DoesNotExist"))
            .unwrap();

        assert!(resource.is_none());
    }

    #[test]
    fn new_reports_an_io_error_for_a_missing_jar_file() {
        let result = JarClassPath::new(
            fixture_path("tests/fixtures/pool_sample_jar/does_not_exist.jar"),
            21,
        );
        let error = result.err().expect("missing file should be an error");

        assert!(error.to_string().starts_with("classpath I/O error:"));
    }

    fn multi_release_variant(variant_dir: &str) -> Vec<u8> {
        std::fs::read(fixture_path(&format!(
            "tests/fixtures/multi_release_jar/{variant_dir}/MrSample.class"
        )))
        .unwrap()
    }

    #[test]
    fn resolves_the_base_entry_when_the_target_release_is_below_every_versioned_directory() {
        let class_path = JarClassPath::new(
            fixture_path("tests/fixtures/multi_release_jar/mr_sample.jar"),
            9,
        )
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("MrSample"))
            .unwrap()
            .expect("entry should be found");

        assert_eq!(resource.bytes(), multi_release_variant("base").as_slice());
    }

    #[test]
    fn resolves_the_v11_entry_for_a_target_release_between_11_and_16() {
        for release_version in [11, 16] {
            let class_path = JarClassPath::new(
                fixture_path("tests/fixtures/multi_release_jar/mr_sample.jar"),
                release_version,
            )
            .unwrap();

            let resource = class_path
                .find_class(&BinaryName::from_internal("MrSample"))
                .unwrap()
                .expect("entry should be found");

            assert_eq!(
                resource.bytes(),
                multi_release_variant("v11").as_slice(),
                "release_version {release_version} should select the v11 entry"
            );
        }
    }

    #[test]
    fn resolves_the_v17_entry_for_a_target_release_at_or_above_17() {
        for release_version in [17, 25] {
            let class_path = JarClassPath::new(
                fixture_path("tests/fixtures/multi_release_jar/mr_sample.jar"),
                release_version,
            )
            .unwrap();

            let resource = class_path
                .find_class(&BinaryName::from_internal("MrSample"))
                .unwrap()
                .expect("entry should be found");

            assert_eq!(
                resource.bytes(),
                multi_release_variant("v17").as_slice(),
                "release_version {release_version} should select the v17 entry"
            );
        }
    }

    #[test]
    fn ignores_versioned_entries_when_the_manifest_does_not_declare_multi_release() {
        let class_path = JarClassPath::new(
            fixture_path(
                "tests/fixtures/multi_release_jar/incidental_versions_no_manifest_flag.jar",
            ),
            21,
        )
        .unwrap();

        let resource = class_path
            .find_class(&BinaryName::from_internal("MrSample"))
            .unwrap()
            .expect("entry should be found");

        assert_eq!(resource.bytes(), multi_release_variant("base").as_slice());
    }
}
