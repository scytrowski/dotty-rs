use super::*;
use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::{
    GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceDefinition, SourceId, SourceText,
    SymbolKind, Type, TypeRefTarget,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use std::path::PathBuf;

const SOURCE: &str =
    include_str!("../../../tests/fixtures/source-binary-recursive-adt-3.9.0/Expr.scala");

#[test]
fn mutable_reference_product2_corruption_recovers_in_the_same_typer_and_resolver() {
    #[derive(Clone, Copy)]
    enum Fault {
        SwappedScopeSelectors,
        ForeignUnapplyOwner,
        ForeignCompanionLink,
        WrongParent,
        BrokenProductIdentity,
        ForeignSelectorOwner,
    }

    let faults = [
        Fault::SwappedScopeSelectors,
        Fault::ForeignUnapplyOwner,
        Fault::ForeignCompanionLink,
        Fault::WrongParent,
        Fault::BrokenProductIdentity,
        Fault::ForeignSelectorOwner,
    ];
    for (offset, fault) in faults.into_iter().enumerate() {
        let source = SourceId::from_index(20900 + offset as u32);
        let (ast, index, mut store, definitions, packages) = setup(source);
        let mut classes = std::collections::BTreeMap::new();
        let mut method_tree = None;
        for (tree, node) in ast.iter() {
            match &node.kind {
                TreeKind::TypeDef(definition) => {
                    let name = store.names.resolve(definition.name.as_name().text());
                    if matches!(name, "Expr" | "Num" | "Add") {
                        classes.insert(name.to_owned(), index.symbol_at(source, tree).unwrap());
                    }
                }
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "selectLeft" =>
                {
                    method_tree = Some(tree);
                }
                _ => {}
            }
        }
        let method_tree = method_tree.unwrap();
        let add = classes["Add"];
        let num = classes["Num"];
        let TreeKind::TypeDef(add_definition) = &ast
            .get(
                index
                    .definition_of(add)
                    .and_then(|definition| match definition {
                        SourceDefinition::Canonical { tree, .. } => Some(tree),
                        SourceDefinition::Derived { .. } => None,
                    })
                    .unwrap(),
            )
            .kind
        else {
            unreachable!()
        };
        let TreeKind::Template(add_template) = &ast.get(add_definition.rhs).kind else {
            unreachable!()
        };
        let TreeKind::DefDef(constructor) = &ast.get(add_template.constructor).kind else {
            unreachable!()
        };
        let fields = constructor.value_param_clauses[0].clone();
        assert_eq!(fields.len(), 2);
        let selectors = fields
            .iter()
            .map(|field| index.derived_symbol_at(add, source, *field).unwrap())
            .collect::<Vec<_>>();
        let selector_names = selectors
            .iter()
            .map(|selector| store.symbols.get(*selector).name)
            .collect::<Vec<_>>();
        let add_unapply = index
            .generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply)
            .unwrap();
        let num_unapply = index
            .generated_case_class_method(num, GeneratedCaseClassMethodRole::Unapply)
            .unwrap();

        let mut typer = SourceTyper::new(&ast, source, &index, &mut store, definitions, &packages)
            .with_resolver(Box::new(resolver(definitions)));
        let add_info = matches!(fault, Fault::WrongParent | Fault::BrokenProductIdentity)
            .then(|| typer.complete_symbol(add).unwrap());
        let product = add_info.map(|add_info| {
            let Type::ClassInfo(class_info) = typer.store.types.get(add_info) else {
                panic!("Add has its authentic completed ClassInfo")
            };
            class_info
                .parents
                .iter()
                .find_map(|parent| match typer.store.types.get(*parent) {
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } if typer
                        .store
                        .names
                        .resolve(typer.store.symbols.get(*symbol).name.text())
                        == "Product" =>
                    {
                        Some(*symbol)
                    }
                    _ => None,
                })
                .expect("Add resolves the canonical scala.Product parent")
        });
        let num_companion = typer.store.symbols.get(num).links.companion.unwrap();
        let num_companion_class = typer.store.symbols.get(num_unapply).owner.unwrap();
        let add_companion_link = typer.store.symbols.get(add).links.companion;
        let unapply_owner = typer.store.symbols.get(add_unapply).owner;
        let selector_owner = typer.store.symbols.get(selectors[0]).owner;
        let product_kind = product.map(|product| typer.store.symbols.get(product).kind);
        let add_parents = add_info.map(|add_info| match typer.store.types.get(add_info) {
            Type::ClassInfo(class_info) => class_info.parents.clone(),
            _ => unreachable!(),
        });

        match fault {
            Fault::SwappedScopeSelectors => {
                let scope = index.scope_of(add).unwrap();
                typer.store.scopes.get_mut(scope).remove(selectors[0]);
                typer.store.scopes.get_mut(scope).remove(selectors[1]);
                typer
                    .store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[0], selectors[1]);
                typer
                    .store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[1], selectors[0]);
            }
            Fault::ForeignUnapplyOwner => {
                typer.store.symbols.get_mut(add_unapply).owner = Some(num_companion_class);
            }
            Fault::ForeignCompanionLink => {
                typer.store.symbols.get_mut(add).links.companion = Some(num_companion);
            }
            Fault::WrongParent => {
                let num_ref = typer.store.types.alloc(Type::TypeRef {
                    prefix: definitions.no_prefix,
                    target: TypeRefTarget::Symbol(num),
                });
                let Type::ClassInfo(class_info) = typer.store.types.get_mut(add_info.unwrap())
                else {
                    unreachable!()
                };
                assert_eq!(class_info.parents.len(), 3);
                class_info.parents[1] = num_ref;
            }
            Fault::BrokenProductIdentity => {
                typer.store.symbols.get_mut(product.unwrap()).kind = SymbolKind::Class;
            }
            Fault::ForeignSelectorOwner => {
                typer.store.symbols.get_mut(selectors[0]).owner = Some(num);
            }
        }

        let before_store = typer.store.checkpoint();
        let before_typed = typer.typed_arena.checkpoint();
        let source_trees = ast.iter().map(|(tree, _)| tree).collect::<Vec<_>>();
        let before_types = source_trees
            .iter()
            .map(|tree| typer.type_index.type_at(source, *tree))
            .collect::<Vec<_>>();
        let before_typed_mappings = source_trees
            .iter()
            .map(|tree| typer.typed_index.get(source, *tree))
            .collect::<Vec<_>>();
        let before_bindings = source_trees
            .iter()
            .map(|tree| typer.pattern_binding_symbol_at(source, *tree))
            .collect::<Vec<_>>();
        let symbols = source_symbols(&ast, source, &index);
        let before_symbols = symbols
            .iter()
            .map(|id| {
                let symbol = typer.store.symbols.get(*id);
                (
                    symbol.info,
                    symbol.owner,
                    symbol.links,
                    symbol.kind,
                    symbol.origin,
                )
            })
            .collect::<Vec<_>>();
        let mut scopes = symbols
            .iter()
            .filter_map(|symbol| index.scope_of(*symbol))
            .collect::<Vec<_>>();
        scopes.sort_by_key(|scope| scope.index());
        scopes.dedup();
        let before_scopes = scopes
            .iter()
            .map(|scope| {
                let mut members = typer
                    .store
                    .scopes
                    .get(*scope)
                    .entered_symbols()
                    .collect::<Vec<_>>();
                members.sort_by_key(|symbol| symbol.index());
                members
            })
            .collect::<Vec<_>>();

        let error = typer
            .type_method_definition(method_tree)
            .expect_err("corrupted Product2 identity must not publish selectLeft");
        let repeated = typer
            .type_method_definition(method_tree)
            .expect_err("the same corrupted metadata fails stably in this typer");
        assert_eq!(format!("{error:?}"), format!("{repeated:?}"));
        assert_eq!(typer.store.checkpoint(), before_store);
        assert_eq!(typer.typed_arena.checkpoint(), before_typed);
        assert_eq!(
            source_trees
                .iter()
                .map(|tree| typer.type_index.type_at(source, *tree))
                .collect::<Vec<_>>(),
            before_types
        );
        assert_eq!(
            source_trees
                .iter()
                .map(|tree| typer.typed_index.get(source, *tree))
                .collect::<Vec<_>>(),
            before_typed_mappings
        );
        assert_eq!(
            source_trees
                .iter()
                .map(|tree| typer.pattern_binding_symbol_at(source, *tree))
                .collect::<Vec<_>>(),
            before_bindings
        );
        assert_eq!(
            symbols
                .iter()
                .map(|id| {
                    let symbol = typer.store.symbols.get(*id);
                    (
                        symbol.info,
                        symbol.owner,
                        symbol.links,
                        symbol.kind,
                        symbol.origin,
                    )
                })
                .collect::<Vec<_>>(),
            before_symbols
        );
        assert_eq!(
            scopes
                .iter()
                .map(|scope| {
                    let mut members = typer
                        .store
                        .scopes
                        .get(*scope)
                        .entered_symbols()
                        .collect::<Vec<_>>();
                    members.sort_by_key(|symbol| symbol.index());
                    members
                })
                .collect::<Vec<_>>(),
            before_scopes
        );

        match fault {
            Fault::SwappedScopeSelectors => {
                let scope = index.scope_of(add).unwrap();
                typer.store.scopes.get_mut(scope).remove(selectors[0]);
                typer.store.scopes.get_mut(scope).remove(selectors[1]);
                typer
                    .store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[0], selectors[0]);
                typer
                    .store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[1], selectors[1]);
            }
            Fault::ForeignUnapplyOwner => {
                typer.store.symbols.get_mut(add_unapply).owner = unapply_owner
            }
            Fault::ForeignCompanionLink => {
                typer.store.symbols.get_mut(add).links.companion = add_companion_link;
            }
            Fault::WrongParent => {
                let Type::ClassInfo(class_info) = typer.store.types.get_mut(add_info.unwrap())
                else {
                    unreachable!()
                };
                class_info.parents = add_parents.unwrap();
            }
            Fault::BrokenProductIdentity => {
                typer.store.symbols.get_mut(product.unwrap()).kind = product_kind.unwrap();
            }
            Fault::ForeignSelectorOwner => {
                typer.store.symbols.get_mut(selectors[0]).owner = selector_owner;
            }
        }
        assert_eq!(typer.store.checkpoint(), before_store);
        let typed_root = typer
            .type_method_definition(method_tree)
            .expect("restored metadata retries the original source method in this typer");
        assert_eq!(typer.typed_index.get(source, method_tree), Some(typed_root));
    }
}

fn setup(
    source: SourceId,
) -> (
    dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    dotty_core::SourceSemanticIndex,
    SemanticStore,
    Definitions,
    Packages,
) {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).unwrap(),
        source,
        ContextualScanner::new(SOURCE).unwrap(),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Expr.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    (parsed.ast, index, store, definitions, packages)
}

fn resolver(definitions: Definitions) -> ClasspathSymbolResolver<CompositeClassPath> {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).unwrap()),
    ]);
    ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new())
}

fn source_symbols(
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    source: SourceId,
    index: &dotty_core::SourceSemanticIndex,
) -> Vec<dotty_core::SymbolId> {
    let trees = ast.iter().map(|(tree, _)| tree).collect::<Vec<_>>();
    let mut symbols = trees
        .iter()
        .filter_map(|tree| index.symbol_at(source, *tree))
        .collect::<Vec<_>>();
    let owners = symbols.clone();
    for owner in &owners {
        symbols.extend(
            trees
                .iter()
                .filter_map(|tree| index.derived_symbol_at(*owner, source, *tree)),
        );
        for role in [
            GeneratedCaseClassMethodRole::Apply,
            GeneratedCaseClassMethodRole::Unapply,
        ] {
            symbols.extend(index.generated_case_class_method(*owner, role));
        }
    }
    symbols.sort_by_key(|symbol| symbol.index());
    symbols.dedup();
    symbols
}
