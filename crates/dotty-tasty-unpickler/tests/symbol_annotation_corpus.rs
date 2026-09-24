//! Manual corpus survey for serialized symbol annotations.
//!
//! This target is deliberately feature-gated: it walks the complete Scala
//! 3.9 corpus and is not part of the normal CI test build. Run it with:
//!
//! ```text
//! cargo test -p dotty-tasty-unpickler --features corpus \
//!   --test symbol_annotation_corpus -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::store::SemanticStore;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{
    DEFDEF_TAG, PARAM_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

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

/// Serialized symbol annotations recorded by pass 1, without decoding their
/// payloads.
#[derive(Default)]
struct SymbolAnnotationSurvey {
    total: usize,
    annotated_definitions: usize,
    definitions: usize,
    by_definition_tag: BTreeMap<&'static str, usize>,
    annotated_by_definition_tag: BTreeMap<&'static str, usize>,
    per_definition_distribution: BTreeMap<usize, usize>,
}

fn definition_tag_name(tag: u8) -> &'static str {
    match tag {
        TYPEDEF_TAG => "TYPEDEF",
        VALDEF_TAG => "VALDEF",
        DEFDEF_TAG => "DEFDEF",
        TYPEPARAM_TAG => "TYPEPARAM",
        PARAM_TAG => "PARAM",
        _ => "other",
    }
}

fn survey_symbol_annotations(
    file: &TastyFile<'_>,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    survey: &mut SymbolAnnotationSurvey,
) {
    let index = file.ast_address_index().unwrap();
    for tag in [
        TYPEDEF_TAG,
        VALDEF_TAG,
        DEFDEF_TAG,
        TYPEPARAM_TAG,
        PARAM_TAG,
    ] {
        for node in index.iter_nodes_with_tag(tag) {
            let at = u32::try_from(node.offset).unwrap();
            let Some(annotations) = unpickler.index().annotation_tail_at(at) else {
                continue;
            };
            let name = definition_tag_name(tag);
            survey.definitions += 1;
            survey.total += annotations.len();
            *survey.by_definition_tag.entry(name).or_default() += annotations.len();
            if !annotations.is_empty() {
                survey.annotated_definitions += 1;
                *survey.annotated_by_definition_tag.entry(name).or_default() += 1;
                *survey
                    .per_definition_distribution
                    .entry(annotations.len())
                    .or_default() += 1;
            }
        }
    }
}

#[derive(Default)]
struct SymbolAnnotationCompletionSurvey {
    attempted: usize,
    decoded: usize,
    external: usize,
    unsupported_constructor: usize,
    unsupported_argument: usize,
    invalid_type: usize,
    unsupported_tree: usize,
    malformed: usize,
    other_known: usize,
    unexpected: Vec<String>,
}

fn survey_symbol_annotation_completion(
    file: &TastyFile<'_>,
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    label: &str,
    survey: &mut SymbolAnnotationCompletionSurvey,
) {
    let addresses: Vec<u32> = {
        let index = file.ast_address_index().unwrap();
        [
            TYPEDEF_TAG,
            VALDEF_TAG,
            DEFDEF_TAG,
            TYPEPARAM_TAG,
            PARAM_TAG,
        ]
        .into_iter()
        .flat_map(|tag| {
            index
                .iter_nodes_with_tag(tag)
                .map(|node| u32::try_from(node.offset).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
    };
    for at in addresses {
        let has_annotations = unpickler
            .index()
            .annotation_tail_at(at)
            .is_some_and(|tail| !tail.is_empty());
        if !has_annotations {
            continue;
        }
        survey.attempted += 1;
        match unpickler.complete_symbol_annotations(at) {
            Ok(_) => survey.decoded += 1,
            Err(
                UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. },
            ) => survey.external += 1,
            Err(UnpickleError::UnsupportedAnnotationConstructor { .. }) => {
                survey.unsupported_constructor += 1;
            }
            Err(UnpickleError::UnsupportedAnnotationArgument { .. }) => {
                survey.unsupported_argument += 1;
            }
            Err(
                UnpickleError::InvalidAnnotationType { .. }
                | UnpickleError::InvalidCompactAnnotationType { .. },
            ) => survey.invalid_type += 1,
            Err(UnpickleError::UnsupportedAnnotationTree { .. }) => {
                survey.unsupported_tree += 1;
            }
            Err(UnpickleError::MalformedType { .. }) => survey.malformed += 1,
            Err(UnpickleError::InvalidReferenceTarget { .. }) => survey.other_known += 1,
            Err(other) => survey.unexpected.push(format!("{label} @{at}: {other:?}")),
        }
    }
}

fn provide_compiler_builtins(store: &mut SemanticStore, packages: &mut Packages) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };

    let chain = packages.enter(store, SymbolOrigin::Synthetic, &["scala"]);
    let scala = chain.last().unwrap();
    for class in ["Any", "Nothing", "Null"] {
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(scala.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store.scopes.get_mut(scala.scope).enter(name, symbol);
    }
}

fn provide_java_object(store: &mut SemanticStore, packages: &mut Packages) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };

    for (path, class) in [
        (&["java", "lang"][..], "Object"),
        (&["scala"][..], "AnyRef"),
    ] {
        let chain = packages.enter(store, SymbolOrigin::Synthetic, path);
        let package = chain.last().unwrap();
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(package.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store.scopes.get_mut(package.scope).enter(name, symbol);
    }
}

#[test]
#[ignore = "walks the whole scala3-library and scala3-compiler corpora"]
fn measure_symbol_annotations_over_the_scala3_corpora() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    for (corpus, builtins, _complete, _classes, object, reverse) in [
        ("scala3-library", false, false, true, false, false),
        ("scala3-compiler", false, false, true, false, false),
        ("scala3-library", true, false, true, false, false),
        ("scala3-compiler", true, false, true, false, false),
        ("scala3-library", false, true, true, false, false),
        ("scala3-compiler", false, true, true, false, false),
        ("scala3-library", true, true, true, false, false),
        ("scala3-compiler", true, true, true, false, false),
        ("scala3-library", true, true, false, false, false),
        ("scala3-compiler", true, true, false, false, false),
        ("scala3-library", true, true, true, true, false),
        ("scala3-compiler", true, true, true, true, false),
        ("scala3-library", true, true, false, true, false),
        ("scala3-compiler", true, true, false, true, false),
        ("scala3-library", true, true, true, true, true),
        ("scala3-compiler", true, true, true, true, true),
    ] {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        definitions.declare_special_aliases(&mut store, &mut packages);
        if builtins {
            provide_compiler_builtins(&mut store, &mut packages);
        }
        if object {
            provide_java_object(&mut store, &mut packages);
        }

        let mut annotations = SymbolAnnotationSurvey::default();
        let mut completion = SymbolAnnotationCompletionSurvey::default();
        let mut paths = tasty_files(&root.join(corpus));
        if reverse {
            paths.reverse();
        }
        for path in paths {
            let bytes = fs::read(&path).unwrap();
            let file = TastyFile::parse_compatible_with(&bytes, 28, 9, 0).unwrap();
            let label = path.display().to_string();
            let mut unpickler =
                TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
            unpickler.enter_symbols().unwrap();
            survey_symbol_annotations(&file, &unpickler, &mut annotations);
            survey_symbol_annotation_completion(&file, &mut unpickler, &label, &mut completion);
            let (_, next_packages) = unpickler.into_parts();
            packages = next_packages;
        }

        println!("== {corpus} (builtins={builtins}, object={object}, reverse={reverse})");
        println!(
            "symbol annotations: {} entries over {} entered definitions/parameters, {} annotated",
            annotations.total, annotations.definitions, annotations.annotated_definitions
        );
        println!(
            "  entries by definition tag: {:?}",
            annotations.by_definition_tag
        );
        println!(
            "  annotated definitions by tag: {:?}",
            annotations.annotated_by_definition_tag
        );
        println!(
            "  annotations-per-definition distribution: {:?}",
            annotations.per_definition_distribution
        );
        println!(
            "symbol annotation completion: {} attempted, {} decoded, {} external, {} unsupported constructor, {} unsupported argument, {} invalid type, {} unsupported tree, {} malformed, {} other known, {} unexpected",
            completion.attempted,
            completion.decoded,
            completion.external,
            completion.unsupported_constructor,
            completion.unsupported_argument,
            completion.invalid_type,
            completion.unsupported_tree,
            completion.malformed,
            completion.other_known,
            completion.unexpected.len()
        );
        for error in completion.unexpected.iter().take(10) {
            println!("  {error}");
        }
        assert!(
            completion.unexpected.is_empty(),
            "{:?}",
            completion.unexpected
        );
        assert_eq!(
            completion.attempted,
            completion.decoded
                + completion.external
                + completion.unsupported_constructor
                + completion.unsupported_argument
                + completion.invalid_type
                + completion.unsupported_tree
                + completion.malformed
                + completion.other_known
        );
    }
}
