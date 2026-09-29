//! Companion-link survey over the pinned Scala 3.9.0 library and compiler
//! TASTy corpora, including reversed unit-entry order.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::Definitions;
use dotty_core::ids::SymbolId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastySession, TastyUnpickler};

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

#[derive(Debug, Default)]
struct Audit {
    class_or_trait_identities: usize,
    object_identities: usize,
    class_or_trait_with_companion: usize,
    traits_with_companion: usize,
    object_with_companion: usize,
    nested_pairs: usize,
    pairs_linked: BTreeSet<String>,
    one_sided_identities: usize,
    ambiguous_candidates: usize,
    conflicting_links: usize,
    module_class_links: usize,
    failed_units: usize,
}

fn symbol_path(store: &SemanticStore, symbol: SymbolId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(symbol);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            parts.push("<owner-cycle>".to_owned());
            break;
        }
        let symbol = store.symbols.get(id);
        let namespace = match symbol.name.namespace() {
            Namespace::Term => "term",
            Namespace::Type => "type",
        };
        parts.push(format!(
            "{namespace}:{}",
            store.names.resolve(symbol.name.text())
        ));
        current = symbol.owner;
    }
    parts.reverse();
    parts.join("/")
}

fn run_corpus(root: &Path, corpus: &str, reverse: bool) -> (Audit, BTreeSet<String>) {
    let mut paths = tasty_files(&root.join(corpus));
    if reverse {
        paths.reverse();
    }
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut session = TastySession::new();
    let mut symbols = Vec::new();
    let mut failed_units = 0;

    for path in paths {
        let bytes = fs::read(&path).unwrap();
        let file = match TastyFile::parse_compatible_with(&bytes, 28, 9, 0) {
            Ok(file) => file,
            Err(_) => {
                failed_units += 1;
                continue;
            }
        };
        let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
        match unpickler.enter_symbols() {
            Ok(index) => symbols.extend(index.entered_symbols()),
            Err(error) => {
                if failed_units == 0 {
                    println!(
                        "first {corpus} entry failure at {}: {error:?}",
                        path.display()
                    );
                }
                failed_units += 1;
            }
        }
        let (_, next_session) = unpickler.into_session_parts();
        session = next_session;
    }

    let mut audit = Audit {
        failed_units,
        ..Audit::default()
    };
    symbols.sort_unstable_by_key(|symbol| symbol.index());
    symbols.dedup();
    for &id in &symbols {
        let symbol = store.symbols.get(id);
        if symbol.kind == SymbolKind::ModuleClass {
            audit.module_class_links += usize::from(symbol.links.companion.is_some());
            continue;
        }
        let is_class = matches!(symbol.kind, SymbolKind::Class | SymbolKind::Trait);
        let is_object = symbol.kind == SymbolKind::Object;
        if !is_class && !is_object {
            continue;
        }
        if is_class {
            audit.class_or_trait_identities += 1;
        } else {
            audit.object_identities += 1;
        }

        let Some(owner) = symbol.owner else {
            audit.one_sided_identities += 1;
            continue;
        };
        let Some(scope) = session.scope_of(owner) else {
            audit.one_sided_identities += 1;
            continue;
        };
        let other_name = Name::new(
            symbol.name.text(),
            if is_class {
                Namespace::Term
            } else {
                Namespace::Type
            },
        );
        let candidates: Vec<_> = store
            .scopes
            .get(scope)
            .lookup_all(&other_name)
            .iter()
            .copied()
            .filter(|candidate| {
                let kind = store.symbols.get(*candidate).kind;
                if is_class {
                    kind == SymbolKind::Object
                } else {
                    matches!(kind, SymbolKind::Class | SymbolKind::Trait)
                }
            })
            .collect();
        match candidates.as_slice() {
            [] => audit.one_sided_identities += 1,
            [other] => {
                let reciprocal = symbol.links.companion == Some(*other)
                    && store.symbols.get(*other).links.companion == Some(id);
                if reciprocal {
                    if is_class {
                        audit.class_or_trait_with_companion += 1;
                        if symbol.kind == SymbolKind::Trait {
                            audit.traits_with_companion += 1;
                        }
                        if store.symbols.get(owner).kind != SymbolKind::Package {
                            audit.nested_pairs += 1;
                        }
                        audit.pairs_linked.insert(format!(
                            "{} <-> {}",
                            symbol_path(&store, id),
                            symbol_path(&store, *other)
                        ));
                    } else {
                        audit.object_with_companion += 1;
                    }
                } else if symbol.links.companion.is_some()
                    || store.symbols.get(*other).links.companion.is_some()
                {
                    audit.conflicting_links += 1;
                } else {
                    audit.one_sided_identities += 1;
                }
            }
            _ => audit.ambiguous_candidates += 1,
        }
    }
    (audit, session_pairs(&store, &symbols))
}

fn session_pairs(store: &SemanticStore, symbols: &[SymbolId]) -> BTreeSet<String> {
    symbols
        .iter()
        .filter_map(|id| {
            let symbol = store.symbols.get(*id);
            if !matches!(symbol.kind, SymbolKind::Class | SymbolKind::Trait)
                || symbol.links.companion.is_none()
            {
                return None;
            }
            Some(format!(
                "{} <-> {}",
                symbol_path(store, *id),
                symbol_path(store, symbol.links.companion?)
            ))
        })
        .collect()
}

#[test]
#[ignore = "walks the pinned Scala 3.9.0 library and compiler TASTy corpora twice"]
fn report_companion_links_and_unit_order_permutations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    let mut unexpected = 0;
    let mut traits_paired = 0;
    let mut nested_pairs = 0;
    for corpus in ["scala3-library", "scala3-compiler"] {
        let (forward, forward_pairs) = run_corpus(&root, corpus, false);
        let (reverse, reverse_pairs) = run_corpus(&root, corpus, true);
        let order_differences = forward_pairs.symmetric_difference(&reverse_pairs).count();
        traits_paired += forward.traits_with_companion;
        nested_pairs += forward.nested_pairs;
        println!(
            "{corpus}: forward(class/trait={}/{}, traits_paired={}, object={}/{}, pairs={}, nested_pairs={}, one_sided={}, ambiguous={}, conflicting={}, moduleclass_links={}, failed_units={}); reverse(class/trait={}/{}, traits_paired={}, object={}/{}, pairs={}, nested_pairs={}, one_sided={}, ambiguous={}, conflicting={}, moduleclass_links={}, failed_units={}); order_dependent_pair_differences={order_differences}",
            forward.class_or_trait_with_companion,
            forward.class_or_trait_identities,
            forward.traits_with_companion,
            forward.object_with_companion,
            forward.object_identities,
            forward.pairs_linked.len(),
            forward.nested_pairs,
            forward.one_sided_identities,
            forward.ambiguous_candidates,
            forward.conflicting_links,
            forward.module_class_links,
            forward.failed_units,
            reverse.class_or_trait_with_companion,
            reverse.class_or_trait_identities,
            reverse.traits_with_companion,
            reverse.object_with_companion,
            reverse.object_identities,
            reverse.pairs_linked.len(),
            reverse.nested_pairs,
            reverse.one_sided_identities,
            reverse.ambiguous_candidates,
            reverse.conflicting_links,
            reverse.module_class_links,
            reverse.failed_units,
        );
        unexpected += forward.ambiguous_candidates
            + forward.conflicting_links
            + forward.module_class_links
            + reverse.ambiguous_candidates
            + reverse.conflicting_links
            + reverse.module_class_links
            + order_differences;
    }
    assert_eq!(
        unexpected, 0,
        "ambiguous, conflicting, module-class, or order-dependent links found"
    );
    assert!(traits_paired > 0, "the corpora exercise trait companions");
    assert!(nested_pairs > 0, "the corpora exercise nested companions");
}
