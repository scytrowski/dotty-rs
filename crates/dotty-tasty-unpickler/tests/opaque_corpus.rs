//! Corpus survey for Scala 3.9 opaque aliases.
//!
//! Run with `cargo test -p dotty-tasty-unpickler --release --test opaque_corpus
//! -- --ignored --nocapture`. This measures both corpus entry orders and keeps
//! external lookup gaps separate from malformed/unsupported completion.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_core::{Definitions, Packages, SymbolInfo};
use dotty_tasty::tasty::{
    DefinitionBody, DefinitionTail, LAMBDATPT_TAG, OPAQUE_TAG, RawTree, TYPEBOUNDSTPT_TAG,
    TYPEDEF_TAG, TastyFile,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastySession, TastyUnpickler, UnpickleError};

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Audit {
    aliases: usize,
    generic: usize,
    non_generic: usize,
    explicit_bounds: usize,
    bounded_with_alias: usize,
    bounded_without_alias: usize,
    owner_kinds: BTreeMap<String, usize>,
    completed: usize,
    external_failures: usize,
    external_errors: BTreeMap<String, usize>,
    cycles: usize,
    malformed: usize,
    unsupported: usize,
    unexpected: BTreeMap<String, usize>,
    owner_local_aliases: usize,
    public_bounds: usize,
}

struct Alias {
    address: u32,
    generic: bool,
    explicit_bounds: bool,
    bounded_alias: bool,
    bounded_without_alias: bool,
}

fn tasty_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "tasty")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
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
        let package = *packages
            .enter(store, SymbolOrigin::Synthetic, path)
            .last()
            .unwrap();
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

fn provide_compiler_builtins(store: &mut SemanticStore, packages: &mut Packages) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };

    let scala = *packages
        .enter(store, SymbolOrigin::Synthetic, &["scala"])
        .last()
        .unwrap();
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

fn opaque_aliases(file: &TastyFile<'_>) -> Vec<Alias> {
    let index = file.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(TYPEDEF_TAG)
        .filter_map(|node| {
            let address = u32::try_from(node.offset).ok()?;
            let body = index.get(address)?.decode_definition_body().ok()?;
            let DefinitionBody::TypeDef {
                type_or_template,
                tail,
                ..
            } = body
            else {
                return None;
            };
            if !tail
                .iter()
                .any(|entry| matches!(entry, DefinitionTail::Modifier(OPAQUE_TAG)))
            {
                return None;
            }
            let mut rhs = type_or_template;
            let mut generic = false;
            while let RawTree::LengthNode(lambda) = rhs {
                if lambda.tag != LAMBDATPT_TAG {
                    rhs = RawTree::LengthNode(lambda);
                    break;
                }
                let decoded = lambda.decode_lambda_tpt().ok()?;
                generic |= !decoded.type_params.is_empty();
                rhs = decoded.body;
            }
            let (explicit_bounds, bounded_alias, bounded_without_alias) =
                if let RawTree::LengthNode(bounds) = rhs {
                    if bounds.tag == TYPEBOUNDSTPT_TAG {
                        let decoded = bounds.decode_type_bounds().ok()?;
                        (
                            decoded.high.is_some(),
                            decoded.high.is_some() && decoded.alias.is_some(),
                            decoded.high.is_some() && decoded.alias.is_none(),
                        )
                    } else {
                        (false, false, false)
                    }
                } else {
                    (false, false, false)
                };
            Some(Alias {
                address,
                generic,
                explicit_bounds,
                bounded_alias,
                bounded_without_alias,
            })
        })
        .collect()
}

fn audit(root: &Path, corpus: &str, reverse: bool) -> Audit {
    let mut paths = tasty_files(&root.join(corpus));
    if reverse {
        paths.reverse();
    }
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    definitions.declare_special_aliases(&mut store, &mut packages);
    provide_compiler_builtins(&mut store, &mut packages);
    provide_java_object(&mut store, &mut packages);
    let mut packages = Some(packages);
    let mut session = TastySession::new();
    let mut audit = Audit::default();

    for path in paths {
        let bytes = fs::read(&path).unwrap();
        let file = TastyFile::parse_compatible_with(&bytes, 28, 9, 0)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let aliases = opaque_aliases(&file);
        let mut unpickler = if let Some(packages) = packages.take() {
            TastyUnpickler::with_packages(&file, &mut store, definitions, packages)
        } else {
            TastyUnpickler::with_session(&file, &mut store, definitions, session)
        };
        let index = unpickler
            .enter_symbols()
            .unwrap_or_else(|error| panic!("{}: {error:?}", path.display()))
            .clone();
        let symbols: Vec<_> = aliases
            .iter()
            .filter_map(|alias| index.symbol_at(alias.address).map(|symbol| (alias, symbol)))
            .filter(|(alias, _)| {
                matches!(
                    unpickler.symbol_state_at(alias.address),
                    Some((SymbolKind::TypeAlias, _))
                )
            })
            .collect();
        let mut completed = Vec::new();
        for (alias, symbol) in &symbols {
            audit.aliases += 1;
            audit.generic += usize::from(alias.generic);
            audit.non_generic += usize::from(!alias.generic);
            audit.explicit_bounds += usize::from(alias.explicit_bounds);
            audit.bounded_with_alias += usize::from(alias.bounded_alias);
            audit.bounded_without_alias += usize::from(alias.bounded_without_alias);
            let result = unpickler.complete_symbol(alias.address);
            match result {
                Ok(info) => {
                    audit.completed += 1;
                    completed.push((*symbol, info));
                }
                Err(UnpickleError::UnresolvedPackage { package, .. }) => {
                    audit.external_failures += 1;
                    *audit
                        .external_errors
                        .entry(format!("package:{package}"))
                        .or_default() += 1;
                }
                Err(UnpickleError::UnresolvedMember {
                    name, namespace, ..
                }) => {
                    audit.external_failures += 1;
                    *audit
                        .external_errors
                        .entry(format!("member:{namespace:?}:{name}"))
                        .or_default() += 1;
                }
                Err(UnpickleError::OpaqueAliasCycle { .. }) => audit.cycles += 1,
                Err(
                    UnpickleError::MalformedDefinition { .. }
                    | UnpickleError::MalformedType { .. }
                    | UnpickleError::Ast(_),
                ) => {
                    audit.malformed += 1;
                }
                Err(
                    UnpickleError::UnsupportedType { .. }
                    | UnpickleError::UnsupportedTypeTree { .. },
                ) => {
                    audit.unsupported += 1;
                }
                Err(error) => *audit.unexpected.entry(format!("{error:?}")).or_default() += 1,
            }
        }
        let (_, next_session) = unpickler.into_session_parts();
        session = next_session;
        for (_, symbol) in &symbols {
            if let Some(owner) = store.symbols.get(*symbol).owner {
                *audit
                    .owner_kinds
                    .entry(format!("{:?}", store.symbols.get(owner).kind))
                    .or_default() += 1;
            }
        }
        for (symbol, info) in completed {
            if matches!(
                store.types.get(info),
                dotty_core::Type::Bounds { .. } | dotty_core::Type::TypeLambda(_)
            ) {
                audit.public_bounds += 1;
            } else {
                *audit
                    .unexpected
                    .entry(format!("public info shape: {:?}", store.types.get(info)))
                    .or_default() += 1;
            }
            if let Some(owner) = store.symbols.get(symbol).owner {
                let owner_data = store.symbols.get(owner);
                if let SymbolInfo::Complete(owner_info) = owner_data.info
                    && let dotty_core::Type::ClassInfo(class_info) = store.types.get(owner_info)
                    && let Some(mut self_type) = class_info.self_type
                {
                    let alias_name = store.symbols.get(symbol).name;
                    loop {
                        match store.types.get(self_type) {
                            dotty_core::Type::Refined { parent, name, .. }
                                if *name == alias_name =>
                            {
                                audit.owner_local_aliases += 1;
                                break;
                            }
                            dotty_core::Type::Refined { parent, .. } => self_type = *parent,
                            dotty_core::Type::Recursive { parent } => self_type = *parent,
                            _ => break,
                        }
                    }
                }
            }
        }
    }
    audit
}

#[test]
#[ignore = "walks all pinned Scala TASTy corpus units twice"]
fn measure_opaque_alias_completion_in_both_corpus_orders() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    for corpus in ["scala3-library", "scala3-compiler"] {
        let forward = audit(&root, corpus, false);
        let reverse = audit(&root, corpus, true);
        println!("{corpus} opaque aliases (forward): {forward:#?}");
        println!("{corpus} opaque aliases (reverse): {reverse:#?}");
        // Report both orders because this no-classpath survey intentionally
        // exposes the external symbols available at each point in entry.
        // The raw opaque declaration population itself must be stable.
        assert_eq!(forward.aliases, reverse.aliases);
        assert_eq!(forward.generic, reverse.generic);
        assert_eq!(forward.non_generic, reverse.non_generic);
        assert_eq!(
            forward.completed
                + forward.external_failures
                + forward.cycles
                + forward.malformed
                + forward.unsupported
                + forward.unexpected.values().sum::<usize>(),
            forward.aliases
        );
        assert_eq!(forward.public_bounds, forward.completed);
        assert!(
            forward.unexpected.is_empty(),
            "unexpected opaque errors: {:?}",
            forward.unexpected
        );
    }
}
