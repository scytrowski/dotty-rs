use dotty_core::ast::TreeKind;
use dotty_core::types::{ClassInfo, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Name, Namespace, Packages, SemanticStore, SourceId, SourceText, Symbol,
    SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/inherited-members-3.9.0/InheritedMembers.scala");
const EXPLICIT_SELECTION: &str =
    include_str!("fixtures/inherited-members-3.9.0/ExplicitSelection.scala");
const ROOTS: [&str; 4] = ["direct", "withBias", "lifted", "mapped"];

#[test]
fn inherited_member_baseline_types_the_four_real_child_method_roots() {
    let first = analyze_candidate_session();
    let second = analyze_candidate_session();
    assert_eq!(first, second, "fresh sessions normalize identically");
    let actual = format!("{}\n", first.join("\n"));
    assert_eq!(
        actual,
        include_str!("fixtures/inherited-members-3.9.0/four-root-baseline.tsv")
            .split_once('\n')
            .expect("baseline header")
            .1
    );
    for row in first {
        eprintln!("{row}");
    }
}

#[test]
fn explicit_member_selection_controls_are_separate_from_the_four_root_denominator() {
    let source = SourceId::from_index(1904);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(EXPLICIT_SELECTION).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(EXPLICIT_SELECTION).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ExplicitSelection.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let child = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "ExplicitChild")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    let parent = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "ExplicitParent")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    let inherited_method = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "add")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    let inherited_field = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "bias")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    assert_eq!(store.symbols.get(inherited_method).owner, Some(parent));
    assert_eq!(store.symbols.get(inherited_field).owner, Some(parent));
    let controls = [
        ("direct", None),
        ("explicitMethod", Some(inherited_method)),
        ("withBias", Some(inherited_field)),
        ("explicitField", Some(inherited_field)),
    ]
    .map(|(name, expected_target)| {
        let (tree, symbol) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == name)
                    .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        (name, tree, symbol, expected_target)
    });
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(child).unwrap();
    let mut outcomes = Vec::new();
    for (name, tree, symbol, expected_target) in controls {
        assert_eq!(typer.store().symbols.get(symbol).owner, Some(child));
        let outcome = typer.type_method_definition(tree);
        let outcome = match outcome {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                if let Some(expected_target) = expected_target {
                    assert!(
                        typer.typed_ast().iter().any(|(_, node)| matches!(
                            typer.store().types.get(node.ty),
                            Type::TermRef {
                                target: TermRefTarget::Symbol(target),
                                ..
                            } if *target == expected_target
                        )),
                        "{name} must select its canonical Parent member"
                    );
                }
                Ok(typed)
            }
            Err(error) => Err(error),
        };
        outcomes.push((name, tree, outcome));
    }
    assert!(matches!(
        outcomes[0].2,
        Err(TyperError::TermNameNotFound { .. })
    ));
    assert!(
        outcomes[1].2.is_ok(),
        "this.add is the diagnostic method control"
    );
    assert!(outcomes[2].2.is_ok(), "implicit inherited field control");
    assert!(
        outcomes[3].2.is_ok(),
        "this.bias is the diagnostic field control"
    );
}

fn analyze_candidate_session() -> Vec<String> {
    let source = SourceId::from_index(1903);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(SOURCE).expect("scan the frozen source candidate");
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("valid source text"),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "InheritedMembers.scala",
        &mut store,
        &mut packages,
    )
    .expect("the frozen inheritance source should be named");

    let class = |wanted: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::TypeDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == wanted)
                    .then(|| (index.symbol_at(source, tree).unwrap(), tree))
            })
            .unwrap_or_else(|| panic!("missing class {wanted}"))
    };
    let (parent, _) = class("Parent");
    let (child, _) = class("Child");
    assert_eq!(
        symbol_path(&store, parent),
        "Package:/Package:inheritancetyper/Class:Parent"
    );
    assert_eq!(
        symbol_path(&store, child),
        "Package:/Package:inheritancetyper/Class:Child"
    );
    let parent_add = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "add")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("Parent.add symbol");
    let parent_bias = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "bias")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("Parent.bias symbol");
    assert_eq!(store.symbols.get(parent_add).owner, Some(parent));
    assert_eq!(store.symbols.get(parent_bias).owner, Some(parent));
    assert_eq!(store.symbols.get(parent_add).kind, SymbolKind::Method);
    assert_eq!(store.symbols.get(parent_bias).kind, SymbolKind::Field);

    let roots = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let name = store.names.resolve(definition.name.as_name().text());
            ROOTS.contains(&name).then(|| {
                (
                    name.to_owned(),
                    tree,
                    index.symbol_at(source, tree).unwrap(),
                    node.position.unwrap(),
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        roots.len(),
        ROOTS.len(),
        "only the four Child methods count"
    );
    for (name, _, symbol, _) in &roots {
        assert_eq!(
            store.symbols.get(*symbol).owner,
            Some(child),
            "{name} owner"
        );
    }

    enter_scala_function1(&mut store, &mut packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .complete_symbol(child)
        .expect("complete the real source Child hierarchy before typing roots");
    let child_info = match *typer.store().symbols.info(child) {
        SymbolInfo::Complete(info) => info,
        _ => panic!("Child class should be completed"),
    };
    let Type::ClassInfo(ClassInfo { parents, .. }) = typer.store().types.get(child_info) else {
        panic!("Child completion should produce ClassInfo")
    };
    assert_eq!(parents.len(), 1, "the frozen hierarchy has one parent");
    assert!(matches!(
        typer.store().types.get(parents[0]),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == parent
    ));

    let mut receipt = Vec::with_capacity(roots.len());
    for (name, tree, symbol, position) in roots {
        let result = typer.type_method_definition(tree);
        match result {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed).kind else {
                    panic!("successful root must be a typed DefDef")
                };
                let TreeKind::DefDef(source_definition) = &parsed.ast.get(tree).kind else {
                    unreachable!("root discovery selected a DefDef")
                };
                let source_rhs = source_definition.rhs.expect("frozen method body");
                let typed_rhs = typed_definition.rhs.expect("complete typed method body");
                assert_eq!(
                    typer.source_typed_index().get(source, source_rhs),
                    Some(typed_rhs),
                    "complete root must map its original source body"
                );
                assert!(matches!(
                    typer.store().symbols.info(symbol),
                    SymbolInfo::Complete(_)
                ));
                let member_target = if typer.typed_ast().iter().any(|(_, node)| {
                    matches!(
                        typer.store().types.get(node.ty),
                        Type::TermRef {
                            target: dotty_core::types::TermRefTarget::Symbol(target),
                            ..
                        } if *target == parent_add || *target == parent_bias
                    )
                }) {
                    if typer.typed_ast().iter().any(|(_, node)| {
                        matches!(
                            typer.store().types.get(node.ty),
                            Type::TermRef {
                                target: dotty_core::types::TermRefTarget::Symbol(target),
                                ..
                            } if *target == parent_add
                        )
                    }) {
                        "Parent.add"
                    } else {
                        "Parent.bias"
                    }
                } else {
                    "none"
                };
                let result_shape = match typer.store().symbols.info(symbol) {
                    SymbolInfo::Complete(info) => match typer.store().types.get(*info) {
                        Type::Method(method) => type_shape(typer.store(), method.result),
                        other => format!("non-method:{other:?}"),
                    },
                    _ => "signature-unavailable".to_owned(),
                };
                receipt.push(format!(
                    "inheritancetyper.Child\t{name}\tInheritedMembers.scala#{}\t{}..{}\tinheritancetyper.Parent\tinheritancetyper.Parent.add\tinheritancetyper.Parent.bias\t{member_target}\t{}\tfully_typed\t{result_shape}\t-",
                    tree.index(),
                    position.span().range().start(),
                    position.span().range().end(),
                    source_signature(&name),
                ));
            }
            Err(error) => {
                assert!(typer.source_typed_index().get(source, tree).is_none());
                let (first, chain) = nested_error_chain(&error, typer.store());
                receipt.push(format!(
                    "inheritancetyper.Child\t{name}\tInheritedMembers.scala#{}\t{}..{}\tinheritancetyper.Parent\tinheritancetyper.Parent.add\tinheritancetyper.Parent.bias\tunresolved\t{}\tblocked\t-\t{first}:{chain}",
                    tree.index(),
                    position.span().range().start(),
                    position.span().range().end(),
                    source_signature(&name),
                ));
            }
        }
    }
    receipt
}

fn source_signature(name: &str) -> &'static str {
    match name {
        "direct" | "withBias" => "(Int):Int",
        "lifted" | "mapped" => "():Function1[Int,Int]",
        _ => panic!("unexpected frozen Child method {name}"),
    }
}

fn enter_scala_function1(store: &mut SemanticStore, packages: &mut Packages) {
    let scala = packages
        .enter(store, SymbolOrigin::Synthetic, &["scala"])
        .last()
        .expect("canonical scala package")
        .symbol;
    let scope = packages.scope_of(scala).expect("scala package scope");
    let name = Name::new(store.names.intern("Function1"), Namespace::Type);
    let class = store.symbols.alloc(Symbol {
        name,
        owner: Some(scala),
        kind: SymbolKind::Trait,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    store.scopes.get_mut(scope).enter(name, class);
}

fn nested_error_chain(error: &TyperError, store: &SemanticStore) -> (String, String) {
    match error {
        TyperError::TermNameNotFound {
            tree_index,
            name,
            position,
            ..
        } => {
            let name = store.names.resolve(name.text());
            let location = position
                .map(|position| {
                    let range = position.span().range();
                    format!("{}..{}", range.start(), range.end())
                })
                .unwrap_or_else(|| "unknown-span".to_owned());
            (
                "TermNameNotFound".to_owned(),
                format!("TermNameNotFound(name={name},tree={tree_index},span={location})"),
            )
        }
        other => {
            let debug = format!("{other:?}");
            let first = debug.split(" { ").next().unwrap_or(&debug).to_owned();
            (first.clone(), first)
        }
    }
}

fn type_shape(store: &SemanticStore, ty: dotty_core::TypeId) -> String {
    match store.types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => store
            .names
            .resolve(store.symbols.get(*symbol).name.text())
            .to_owned(),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            type_shape(store, *tycon),
            args.iter()
                .map(|arg| type_shape(store, *arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => format!("{other:?}"),
    }
}

fn symbol_path(store: &SemanticStore, symbol: dotty_core::SymbolId) -> String {
    let info = store.symbols.get(symbol);
    let name = store.names.resolve(info.name.text());
    let kind = match info.kind {
        SymbolKind::Package => "Package",
        SymbolKind::Class => "Class",
        SymbolKind::Trait => "Trait",
        other => return format!("{other:?}:{name}"),
    };
    match info.owner {
        Some(owner) => format!("{}/{kind}:{name}", symbol_path(store, owner)),
        None => format!("{kind}:{name}"),
    }
}
