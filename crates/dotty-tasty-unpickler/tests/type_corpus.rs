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

/// The compound forms decoded since Milestone 2c1, with the names used in
/// the report. `SUPERtype` does not occur in either corpus.
const COMPOUND_TAGS: [(u8, &str); 5] = [
    (161, "APPLIEDtype"),
    (165, "ANDtype"),
    (167, "ORtype"),
    (158, "SUPERtype"),
    (93, "BYNAMEtype"),
];

/// Forms that stay unsupported; only their instances are counted.
const UNSUPPORTED_TAGS: [(u8, &str); 3] = [
    (153, "ANNOTATEDtype"),
    (170, "TYPELAMBDAtype"),
    (193, "FLEXIBLEtype"),
];

const TYPEBOUNDS_TAG: u8 = 163;

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

/// How the reference nodes of one kind fared.
#[derive(Default)]
struct Outcomes {
    nodes: usize,
    decoded: usize,
    /// Well-formed references whose target lives outside the entered state, so
    /// only an external resolver (a classpath) can supply it.
    needs_external: usize,
    ambiguous: usize,
    signed: usize,
    unsupported_prefix: usize,
    /// References to definitions pass 1 does not enter (locals).
    missing_local: usize,
    /// A child of a compound node is a form with no decoder yet.
    unsupported_child: usize,
    /// Errors that mean a bug or malformed input, never an expected gap.
    unexpected: usize,
}

/// How the `TYPEBOUNDS` nodes are shaped on the wire.
#[derive(Default)]
struct BoundsShapes {
    total: usize,
    /// `low` and `high`.
    two_sided: usize,
    /// One type only: the alias form.
    alias_only: usize,
    /// Any variance marker after the types.
    with_variance: usize,
    /// Not decodable structurally.
    malformed: usize,
}

#[derive(Default)]
struct Tally {
    units: usize,
    units_with_a_decoded_type: usize,
    units_fully_decoded: usize,
    /// References written by definition address (`*direct`, `*symbol`, `*pkg`,
    /// `THIS`, `SHAREDtype`).
    by_address: Outcomes,
    /// Name-based `TYPEREF` and `TERMREF`.
    named_type: Outcomes,
    named_term: Outcomes,
    /// Per compound tag.
    compound: BTreeMap<u8, Outcomes>,
    bounds: BoundsShapes,
    /// Nodes of the forms that stay unsupported.
    unsupported_nodes: BTreeMap<u8, usize>,
    unresolved_members: usize,
    unresolved_packages: usize,
    unsupported: BTreeMap<u8, usize>,
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
    let addresses: Vec<(u32, u8)> = {
        let index = file.ast_address_index().unwrap();
        index
            .iter_nodes()
            .filter(|node| {
                REFERENCE_TAGS.contains(&node.tag)
                    || COMPOUND_TAGS.iter().any(|(tag, _)| *tag == node.tag)
            })
            .map(|node| (u32::try_from(node.offset).unwrap(), node.tag))
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

    {
        let index = file.ast_address_index().unwrap();
        for node in index.iter_nodes() {
            if node.tag == TYPEBOUNDS_TAG {
                tally.bounds.total += 1;
                let raw = index.get(u32::try_from(node.offset).unwrap());
                match raw
                    .ok_or(())
                    .and_then(|raw| raw.decode_type_bounds().map_err(|_| ()))
                {
                    Ok(bounds) => {
                        tally.bounds.two_sided += usize::from(bounds.high.is_some());
                        tally.bounds.alias_only += usize::from(bounds.high.is_none());
                        tally.bounds.with_variance += usize::from(!bounds.variances.is_empty());
                    }
                    Err(_) => tally.bounds.malformed += 1,
                }
            }
            if UNSUPPORTED_TAGS.iter().any(|(tag, _)| *tag == node.tag) {
                *tally.unsupported_nodes.entry(node.tag).or_default() += 1;
            }
        }
    }

    let mut unpickler = TastyUnpickler::with_packages(&file, store, definitions, packages);
    unpickler.enter_symbols().unwrap();

    tally.units += 1;
    let (mut decoded, mut failed) = (0, 0);
    for (at, tag) in addresses {
        let outcomes = match tag {
            117 => &mut tally.named_type,
            115 => &mut tally.named_term,
            tag if COMPOUND_TAGS.iter().any(|(compound, _)| *compound == tag) => {
                tally.compound.entry(tag).or_default()
            }
            _ => &mut tally.by_address,
        };
        outcomes.nodes += 1;
        match unpickler.unpickle_type(at) {
            Ok(first) => {
                decoded += 1;
                outcomes.decoded += 1;
                // Identity: decoding again never allocates a second type.
                assert_eq!(unpickler.unpickle_type(at), Ok(first));
            }
            Err(error) => {
                failed += 1;
                match error {
                    UnpickleError::UnsupportedType { tag, .. } => {
                        // For a reference node this is the node's own form;
                        // for a compound node it is one of its children.
                        outcomes.unsupported_child += 1;
                        *tally.unsupported.entry(tag).or_default() += 1;
                    }
                    UnpickleError::UnresolvedPackage { .. } => {
                        outcomes.needs_external += 1;
                        tally.unresolved_packages += 1;
                    }
                    UnpickleError::UnresolvedMember { .. } => {
                        outcomes.needs_external += 1;
                        tally.unresolved_members += 1;
                    }
                    UnpickleError::AmbiguousMember { .. } => outcomes.ambiguous += 1,
                    UnpickleError::UnsupportedSignedReference { .. } => outcomes.signed += 1,
                    UnpickleError::UnsupportedResolutionPrefix { .. } => {
                        outcomes.unsupported_prefix += 1;
                    }
                    UnpickleError::MissingReferencedSymbol { to, .. } => {
                        outcomes.missing_local += 1;
                        tally.missing_symbols += 1;
                        tally.missing_outside_bodies += usize::from(!inside_a_body(to));
                        let tag = tags.get(&to).copied();
                        *tally.missing_targets.entry(tag.unwrap_or(0)).or_default() += 1;
                    }
                    other => {
                        outcomes.unexpected += 1;
                        tally.unexpected.push(format!("{label} @{at}: {other:?}"));
                    }
                }
            }
        }
    }
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
    assert!(tally.by_address.decoded > 0);
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
        for (label, outcomes) in [
            ("by address", &tally.by_address),
            ("named TYPEREF", &tally.named_type),
            ("named TERMREF", &tally.named_term),
        ] {
            println!(
                "{label}: nodes {}, decoded {}, need external {}, ambiguous {}, signed {}, unsupported prefix {}, unexpected {}",
                outcomes.nodes,
                outcomes.decoded,
                outcomes.needs_external,
                outcomes.ambiguous,
                outcomes.signed,
                outcomes.unsupported_prefix,
                outcomes.unexpected,
            );
        }
        println!("compound types (decoded: the node and every child decode):");
        for (tag, label) in COMPOUND_TAGS {
            let outcomes = tally
                .compound
                .get(&tag)
                .map_or_else(Outcomes::default, |o| Outcomes {
                    nodes: o.nodes,
                    decoded: o.decoded,
                    needs_external: o.needs_external,
                    ambiguous: o.ambiguous,
                    signed: o.signed,
                    unsupported_prefix: o.unsupported_prefix,
                    missing_local: o.missing_local,
                    unsupported_child: o.unsupported_child,
                    unexpected: o.unexpected,
                });
            println!(
                "  {label}: nodes {}, decoded {}; child failures: external {}, ambiguous {}, signed {}, unsupported prefix {}, local {}, unsupported form {}; unexpected {}",
                outcomes.nodes,
                outcomes.decoded,
                outcomes.needs_external,
                outcomes.ambiguous,
                outcomes.signed,
                outcomes.unsupported_prefix,
                outcomes.missing_local,
                outcomes.unsupported_child,
                outcomes.unexpected,
            );
        }
        println!(
            "TYPEBOUNDS: total {}, two-sided {}, alias-only {}, with variance {}, malformed {}",
            tally.bounds.total,
            tally.bounds.two_sided,
            tally.bounds.alias_only,
            tally.bounds.with_variance,
            tally.bounds.malformed,
        );
        for (tag, label) in UNSUPPORTED_TAGS {
            println!(
                "{label}: {}",
                tally.unsupported_nodes.get(&tag).copied().unwrap_or(0)
            );
        }
        println!(
            "unresolved: members {}, packages {}",
            tally.unresolved_members, tally.unresolved_packages
        );
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
