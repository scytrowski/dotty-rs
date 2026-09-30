//! Survey of definition-tail modifiers in the pinned Scala 3.9 corpus.
//!
//! Run with `cargo test -p dotty-tasty-unpickler --test
//! definition_tail_corpus -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::store::SemanticStore;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{
    AstAddressIndex, DEFDEF_TAG, DefinitionBody, DefinitionTail, PARAM_TAG, TYPEDEF_TAG,
    TYPEPARAM_TAG, TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

const TARGETS: [(u8, &str); 16] = [
    (24, "ARTIFACT"),
    (16, "INLINEPROXY"),
    (33, "MACRO"),
    (40, "OPEN"),
    (43, "INFIX"),
    (44, "INVISIBLE"),
    (47, "TRACKED"),
    (49, "INTO"),
    (28, "COVARIANT"),
    (29, "CONTRAVARIANT"),
    (26, "FIELDACCESSOR"),
    (27, "CASEACCESSOR"),
    (38, "PARAMSETTER"),
    (41, "PARAMALIAS"),
    (31, "HASDEFAULT"),
    (32, "STABLE"),
];

fn tasty_files(root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
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

fn definition_name_and_tail<'a>(
    ast_index: &AstAddressIndex<'a>,
    address: u32,
    tag: u8,
) -> Option<(u32, Vec<DefinitionTail<'a>>)> {
    let raw = ast_index.get(address)?;
    match tag {
        VALDEF_TAG | TYPEDEF_TAG => match raw.decode_definition_body().ok()? {
            DefinitionBody::ValDef { name, tail, .. }
            | DefinitionBody::TypeDef { name, tail, .. } => Some((name, tail)),
        },
        DEFDEF_TAG => {
            let body = raw.decode_defdef_body().ok()?;
            Some((body.name, body.tail))
        }
        PARAM_TAG | TYPEPARAM_TAG => {
            let parameter = raw.decode_parameter().ok()?;
            Some((parameter.name(), parameter.decode_body().ok()?.tail))
        }
        _ => None,
    }
}

#[derive(Default)]
struct Count {
    total: usize,
    by_corpus: BTreeMap<String, usize>,
    by_definition: BTreeMap<String, usize>,
    by_owner: BTreeMap<String, usize>,
    by_combination: BTreeMap<String, usize>,
    examples: Vec<String>,
}

#[test]
#[ignore = "walks the pinned Scala 3.9 library and compiler corpora"]
fn report_definition_tail_modifiers_in_scala_corpora() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    let mut report: BTreeMap<&str, Count> = TARGETS
        .iter()
        .map(|(_, name)| (*name, Count::default()))
        .collect();
    let mut entered = 0usize;
    let mut enter_failures = 0usize;
    let mut parse_failures = 0usize;
    let mut first_parse_failure = None;

    for corpus in ["scala3-library", "scala3-compiler"] {
        for path in tasty_files(&root.join(corpus)) {
            let bytes = fs::read(&path).unwrap();
            let file = match TastyFile::parse(&bytes) {
                Ok(file) => file,
                Err(error) => {
                    parse_failures += 1;
                    first_parse_failure
                        .get_or_insert_with(|| format!("{}: {error:?}", path.display()));
                    continue;
                }
            };
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let mut unpickler =
                TastyUnpickler::with_packages(&file, &mut store, definitions, Packages::new());
            let index = match unpickler.enter_symbols() {
                Ok(index) => index.clone(),
                Err(_) => {
                    enter_failures += 1;
                    continue;
                }
            };
            drop(unpickler);
            entered += 1;
            let ast_index = file.ast_address_index().unwrap();
            for node in ast_index.iter() {
                let Some((_name_ref, tail)) =
                    definition_name_and_tail(&ast_index, node.offset as u32, node.tag)
                else {
                    continue;
                };
                let Some(symbol_id) = index.symbol_at(node.offset as u32) else {
                    continue;
                };
                let symbol = store.symbols.get(symbol_id);
                let symbol_kind = format!("{:?}", symbol.kind);
                let owner_kind = symbol
                    .owner
                    .map(|owner| format!("{:?}", store.symbols.get(owner).kind))
                    .unwrap_or_else(|| "None".to_owned());
                let mut modifiers: Vec<&str> = tail
                    .iter()
                    .filter_map(|entry| match entry {
                        DefinitionTail::Modifier(tag) => modifier_name(*tag),
                        _ => None,
                    })
                    .collect();
                modifiers.sort_unstable();
                modifiers.dedup();
                let combination = if modifiers.is_empty() {
                    "(none)".to_owned()
                } else {
                    modifiers.join("+")
                };
                let name = store.names.resolve(symbol.name.text());
                for (tag, target_name) in TARGETS {
                    if !tail.iter().any(
                        |entry| matches!(entry, DefinitionTail::Modifier(actual) if *actual == tag),
                    ) {
                        continue;
                    }
                    let count = report.get_mut(target_name).unwrap();
                    count.total += 1;
                    *count.by_corpus.entry(corpus.to_owned()).or_default() += 1;
                    *count
                        .by_definition
                        .entry(format!("{} ({symbol_kind})", tag_name(node.tag)))
                        .or_default() += 1;
                    *count.by_owner.entry(owner_kind.clone()).or_default() += 1;
                    *count.by_combination.entry(combination.clone()).or_default() += 1;
                    if count.examples.len() < 5 {
                        let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                        count.examples.push(format!("{rel}::{name}"));
                    }
                }
            }
        }
    }

    println!(
        "entered files: {entered}; parse failures: {parse_failures}; enter failures: {enter_failures}"
    );
    println!("first parse failure: {first_parse_failure:?}");
    for (name, count) in report {
        println!("{name}: {}", count.total);
        println!("  corpora: {:?}", count.by_corpus);
        println!("  definitions: {:?}", count.by_definition);
        println!("  owners: {:?}", count.by_owner);
        println!("  combinations: {:?}", count.by_combination);
        println!("  examples: {:?}", count.examples);
    }
}

fn modifier_name(tag: u8) -> Option<&'static str> {
    Some(match tag {
        6 => "PRIVATE",
        8 => "PROTECTED",
        9 => "ABSTRACT",
        10 => "FINAL",
        7 => "SEALED",
        12 => "CASE",
        13 => "IMPLICIT",
        14 => "LAZY",
        15 => "OVERRIDE",
        16 => "INLINEPROXY",
        17 => "INLINE",
        18 => "STATIC",
        19 => "OBJECT",
        20 => "TRAIT",
        21 => "ENUM",
        22 => "LOCAL",
        23 => "SYNTHETIC",
        24 => "ARTIFACT",
        25 => "MUTABLE",
        26 => "FIELDACCESSOR",
        27 => "CASEACCESSOR",
        28 => "COVARIANT",
        29 => "CONTRAVARIANT",
        31 => "HASDEFAULT",
        32 => "STABLE",
        33 => "MACRO",
        34 => "ERASED",
        35 => "OPAQUE",
        36 => "EXTENSION",
        37 => "GIVEN",
        38 => "PARAMSETTER",
        39 => "EXPORTED",
        40 => "OPEN",
        41 => "PARAMALIAS",
        42 => "TRANSPARENT",
        43 => "INFIX",
        44 => "INVISIBLE",
        47 => "TRACKED",
        49 => "INTO",
        _ => return None,
    })
}

fn tag_name(tag: u8) -> &'static str {
    match tag {
        DEFDEF_TAG => "DEFDEF",
        VALDEF_TAG => "VALDEF",
        TYPEDEF_TAG => "TYPEDEF",
        PARAM_TAG => "PARAM",
        TYPEPARAM_TAG => "TYPEPARAM",
        _ => "other",
    }
}
