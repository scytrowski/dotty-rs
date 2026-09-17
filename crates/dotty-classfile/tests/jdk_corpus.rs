//! Decodes and validates real `.class` files extracted from a local JDK 25
//! runtime image (`tests/fixtures/jdk_corpus/`, see that directory's
//! `manifest.toml`/`generate.sh`). Corpus membership comes from
//! `classes.txt` rather than being hardcoded here, per
//! `docs/testing-strategy.md`'s baseline-corpus convention. Unlike this
//! crate's other fixtures, individual file paths aren't known at compile
//! time, so this reads fixture bytes via `std::fs::read` instead of
//! `include_bytes!`.

use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use std::path::Path;

const CLASSES_TXT: &str = include_str!("fixtures/jdk_corpus/classes.txt");

fn corpus_classes() -> Vec<&'static str> {
    CLASSES_TXT
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn read_corpus_class(name: &str) -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jdk_corpus");
    let path = root.join(format!("{name}.class"));
    std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read corpus fixture {}: {error}", path.display()))
}

#[test]
fn decodes_and_validates_every_corpus_class() {
    for name in corpus_classes() {
        let bytes = read_corpus_class(name);
        let mut reader = Reader::new(&bytes);
        let class_file = ClassFile::decode(&mut reader)
            .unwrap_or_else(|error| panic!("{name} failed to decode: {error}"));

        assert!(
            reader.is_at_end(),
            "{name} decoded but left unread trailing bytes"
        );
        assert_eq!(
            class_file.validate_references(),
            Ok(()),
            "{name} failed reference validation"
        );
    }
}
