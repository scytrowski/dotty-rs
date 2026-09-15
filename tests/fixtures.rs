use std::fs;
use std::path::{Path, PathBuf};

use tasty_rs::Header;

const EXPECTED_FIXTURE_COUNT: usize = 33;
const TASTY_MAGIC: [u8; 4] = [0x5c, 0xa1, 0xab, 0x1f];

fn tasty_fixture_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut directories = vec![root];
    let mut fixtures = Vec::new();

    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()));

        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!(
                    "failed to read an entry in {}: {error}",
                    directory.display()
                )
            });
            let path = entry.path();
            let file_type = entry
                .file_type()
                .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()));

            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "tasty") {
                fixtures.push(path);
            }
        }
    }

    fixtures.sort();
    fixtures
}

#[test]
fn all_tasty_fixtures_are_readable_and_have_the_tasty_magic_header() {
    let fixtures = tasty_fixture_paths();

    assert_eq!(
        fixtures.len(),
        EXPECTED_FIXTURE_COUNT,
        "fixture inventory changed; update EXPECTED_FIXTURE_COUNT deliberately"
    );

    for path in fixtures {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

        assert!(
            bytes.len() >= TASTY_MAGIC.len(),
            "fixture {} is shorter than the TASTy magic header",
            path.display()
        );
        assert_eq!(
            bytes[..TASTY_MAGIC.len()],
            TASTY_MAGIC,
            "fixture {} has an invalid TASTy magic header",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_have_the_scala_3_9_header() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let header = Header::parse(&bytes)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));

        assert_eq!(header.major_version, 28, "fixture {}", path.display());
        assert_eq!(header.minor_version, 9, "fixture {}", path.display());
        assert_eq!(header.experimental_version, 0, "fixture {}", path.display());
        assert_eq!(
            header.tooling_version,
            "Scala 3.9.0",
            "fixture {}",
            path.display()
        );
    }
}
