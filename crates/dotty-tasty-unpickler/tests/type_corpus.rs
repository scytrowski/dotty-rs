//! Smoke coverage of the type pass over real compiler output, and the
//! measurement that scopes the next increment.
//!
//! The small-fixture test runs in CI. The corpus measurement is `#[ignore]`d
//! because it walks every unit of the two Scala 3 corpora; run it with
//! `cargo test -p dotty-tasty-unpickler --release --test type_corpus -- --ignored --nocapture`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::store::SemanticStore;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

/// Every node tag that is a type reference, or a `THIS` prefix.
const REFERENCE_TAGS: [u8; 10] = [61, 62, 63, 64, 65, 90, 114, 115, 116, 117];

fn tasty_files(root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|ext| ext == "tasty") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[derive(Default)]
struct Tally {
    units: usize,
    units_with_a_decoded_type: usize,
    units_fully_decoded: usize,
    nodes: usize,
    decoded: usize,
    unsupported: BTreeMap<u8, usize>,
    unresolved_packages: usize,
    missing_symbols: usize,
    /// Tags of the nodes that missing-symbol references point at.
    missing_targets: BTreeMap<u8, usize>,
    /// Missing targets with no `VALDEF`/`DEFDEF` body, `BLOCK`, `CASEDEF` or
    /// `LAMBDAtpt` ancestor. Those are the two things pass 1 documents as not entered
    /// (locals, including blocks and pattern binders in constructor arguments, and the parameters of
    /// type-lambda aliases); anything else would be a definition pass 1 should
    /// have entered.
    missing_outside_bodies: usize,
    /// Errors that mean a bug or malformed input, never an expected gap.
    unexpected: Vec<String>,
}

/// Enters the unit and decodes every reference node in it.
fn run(
    label: &str,
    bytes: &[u8],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: Packages,
    tally: &mut Tally,
) -> Packages {
    let file = TastyFile::parse_compatible_with(bytes, 28, 9, 0).unwrap();
    let addresses: Vec<u32> = {
        let index = file.ast_address_index().unwrap();
        index
            .iter_nodes()
            .filter(|node| REFERENCE_TAGS.contains(&node.tag))
            .map(|node| u32::try_from(node.offset).unwrap())
            .collect()
    };

    let parents: HashMap<u32, u32> = file
        .ast_address_index()
        .unwrap()
        .iter_tree_edges()
        .map(|edge| {
            (
                u32::try_from(edge.child.offset).unwrap(),
                u32::try_from(edge.parent.offset).unwrap(),
            )
        })
        .collect();
    let tags: HashMap<u32, u8> = file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .map(|node| (u32::try_from(node.offset).unwrap(), node.tag))
        .collect();
    let inside_a_body = |mut at: u32| {
        while let Some(&parent) = parents.get(&at) {
            if matches!(tags.get(&parent), Some(129 | 130 | 140 | 155 | 171)) {
                return true;
            }
            at = parent;
        }
        false
    };

    let mut unpickler = TastyUnpickler::with_packages(&file, store, definitions, packages);
    unpickler.enter_symbols().unwrap();

    tally.units += 1;
    let (mut decoded, mut failed) = (0, 0);
    for at in addresses {
        tally.nodes += 1;
        match unpickler.unpickle_type(at) {
            Ok(first) => {
                decoded += 1;
                // Identity: decoding again never allocates a second type.
                assert_eq!(unpickler.unpickle_type(at), Ok(first));
            }
            Err(error) => {
                failed += 1;
                match error {
                    UnpickleError::UnsupportedType { tag, .. } => {
                        *tally.unsupported.entry(tag).or_default() += 1;
                    }
                    UnpickleError::UnresolvedPackage { .. } => tally.unresolved_packages += 1,
                    UnpickleError::MissingReferencedSymbol { to, .. } => {
                        tally.missing_symbols += 1;
                        tally.missing_outside_bodies += usize::from(!inside_a_body(to));
                        let tag = tags.get(&to).copied();
                        *tally.missing_targets.entry(tag.unwrap_or(0)).or_default() += 1;
                    }
                    other => tally.unexpected.push(format!("{label} @{at}: {other:?}")),
                }
            }
        }
    }
    tally.decoded += decoded;
    tally.units_with_a_decoded_type += usize::from(decoded > 0);
    tally.units_fully_decoded += usize::from(failed == 0);
    unpickler.into_parts().1
}

#[test]
fn the_type_pass_never_fails_unexpectedly_on_the_small_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    let mut tally = Tally::default();
    for path in tasty_files(&root) {
        // The manifest-backed corpora are measured separately.
        if path.components().any(|part| {
            let part = part.as_os_str();
            part == "scala3-library" || part == "scala3-compiler"
        }) {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        let Ok(file) = TastyFile::parse_scala_3_9(&bytes) else {
            continue;
        };
        drop(file);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        run(
            &path.display().to_string(),
            &bytes,
            &mut store,
            definitions,
            Packages::new(),
            &mut tally,
        );
    }

    assert!(tally.units > 20, "found {} units", tally.units);
    assert!(tally.decoded > 0);
    assert_eq!(tally.unexpected, Vec::<String>::new());
}

#[test]
#[ignore = "walks the whole scala3-library and scala3-compiler corpora"]
fn measure_the_type_pass_over_the_scala3_corpora() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    for corpus in ["scala3-library", "scala3-compiler"] {
        // One store and one package registry per corpus, as a classpath
        // would have.
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let mut tally = Tally::default();
        for path in tasty_files(&root.join(corpus)) {
            let bytes = fs::read(&path).unwrap();
            packages = run(
                &path.display().to_string(),
                &bytes,
                &mut store,
                definitions,
                packages,
                &mut tally,
            );
        }

        let mut unsupported: Vec<_> = tally.unsupported.iter().collect();
        unsupported.sort_by(|a, b| b.1.cmp(a.1));
        println!("== {corpus}");
        println!("units: {}", tally.units);
        println!(
            "units with a decoded type: {}",
            tally.units_with_a_decoded_type
        );
        println!(
            "units where every reference node decodes: {}",
            tally.units_fully_decoded
        );
        println!(
            "reference nodes: {}, decoded: {}",
            tally.nodes, tally.decoded
        );
        println!("unresolved packages: {}", tally.unresolved_packages);
        println!(
            "missing symbols (locals, other units): {}",
            tally.missing_symbols
        );
        println!(
            "missing symbols that are neither locals nor type-lambda parameters: {}",
            tally.missing_outside_bodies
        );
        println!("missing symbols by target tag: {:?}", tally.missing_targets);
        println!("unsupported by tag: {unsupported:?}");
        println!("unexpected errors: {}", tally.unexpected.len());
        for error in tally.unexpected.iter().take(10) {
            println!("  {error}");
        }
        assert!(tally.unexpected.is_empty());
        assert_eq!(tally.missing_outside_bodies, 0);
    }
}
