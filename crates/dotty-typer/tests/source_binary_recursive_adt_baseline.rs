use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, MethodKind, Packages, SemanticStore,
    SourceDefinition, SourceId, SourceSemanticIndex, SourceText, SymbolFlags, SymbolInfo,
    SymbolKind, SymbolOrigin, TermName, TermRefTarget, Type, TypeName, TypeRefTarget,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;

const SOURCE: &str = include_str!("fixtures/source-binary-recursive-adt-3.9.0/Expr.scala");

#[test]
fn binary_recursive_adt_roots_are_attempted_in_two_fresh_resolver_sessions() {
    let first = analyze_roots(SourceId::from_index(20611), "session-a");
    let second = analyze_roots(SourceId::from_index(20612), "session-b");
    assert_eq!(first.normalized, second.normalized);
    assert_eq!(
        first.roots.len(),
        2,
        "only the original source roots count per session"
    );
    assert!(first.roots.iter().any(|row| row.contains("\teval\t")));
    assert!(first.roots.iter().any(|row| row.contains("\tselectLeft\t")));
    let receipt = first
        .roots
        .iter()
        .chain(second.roots.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        format!("{}\n", receipt.join("\n")),
        include_str!("fixtures/source-binary-recursive-adt-3.9.0/rust-after-product2-two-root.tsv")
            .split_once('\n')
            .expect("post-Product2 root receipt header")
            .1
    );
    for row in receipt {
        println!("ISSUE2072_ROOT_AFTER_PRODUCT2\t{row}");
    }
}

#[test]
fn binary_recursive_adt_generated_identities_are_discoverable_without_completion() {
    let actual = characterize_class(SourceId::from_index(20613));
    assert_eq!(
        format!("{actual}\n"),
        include_str!("fixtures/source-binary-recursive-adt-3.9.0/rust-after-namer-identities.tsv")
            .split_once('\n')
            .expect("post-Namer identity evidence header")
            .1
    );
    println!("ISSUE2066_NAMER_IDENTITIES\t{actual}");
}

#[test]
fn select_left_completes_first_with_authentic_two_field_provenance_in_fresh_sessions() {
    let first = select_left_receipt(SourceId::from_index(20761));
    let second = select_left_receipt(SourceId::from_index(20762));
    assert_eq!(
        first, second,
        "fresh source/resolver sessions normalize equally"
    );
    let after =
        format!("after\tsession-a\tselectLeft\t{first}\nafter\tsession-b\tselectLeft\t{second}\n");
    assert!(
        include_str!(
            "fixtures/source-binary-recursive-adt-3.9.0/rust-select-left-before-after.tsv"
        )
        .ends_with(&after),
        "the checked receipt records both fresh after sessions: {after}"
    );
    println!("ISSUE2076_SELECT_LEFT_PROOF\t{first}");
}

fn select_left_receipt(source: SourceId) -> String {
    let (ast, _root, index, mut store, definitions, packages) = setup(source);
    let mut method = None;
    let mut classes = std::collections::BTreeMap::new();
    for (tree, node) in ast.iter() {
        match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "selectLeft" =>
            {
                method = Some((tree, definition.clone(), node.position.unwrap()));
            }
            TreeKind::TypeDef(definition) => {
                let name = store.names.resolve(definition.name.as_name().text());
                if matches!(name, "Expr" | "Num" | "Add") {
                    classes.insert(
                        name.to_owned(),
                        (tree, index.symbol_at(source, tree).unwrap()),
                    );
                }
            }
            _ => {}
        }
    }
    let (method_tree, source_method, method_position) = method.expect("original selectLeft root");
    let expr = classes["Expr"].1;
    let num = classes["Num"].1;
    let add = classes["Add"].1;
    assert_eq!(store.symbols.get(expr).kind, SymbolKind::Trait);
    assert!(store.symbols.get(expr).flags.contains(SymbolFlags::SEALED));
    for (name, symbol) in [("Num", num), ("Add", add)] {
        let (class_tree, _) = classes[name];
        let TreeKind::TypeDef(definition) = &ast.get(class_tree).kind else {
            unreachable!()
        };
        let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
            panic!("{name} must keep its source template")
        };
        assert_eq!(template.parents.len(), 1);
        let TreeKind::Ident(parent) = &ast.get(template.parents[0]).kind else {
            panic!("{name} must retain its source parent tree")
        };
        assert_eq!(store.names.resolve(parent.name.text()), "Expr");
        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Class);
        assert!(
            matches!(store.symbols.get(symbol).origin, SymbolOrigin::Source(id) if id == source)
        );
    }

    let method_symbol = index.symbol_at(source, method_tree).unwrap();
    let parameter_tree = source_method.value_param_clauses[0][0];
    let parameter_symbol = index.symbol_at(source, parameter_tree).unwrap();
    assert_ne!(method_symbol, parameter_symbol);
    assert_eq!(
        store.symbols.get(parameter_symbol).owner,
        Some(method_symbol)
    );
    assert!(
        matches!(store.symbols.get(method_symbol).origin, SymbolOrigin::Source(id) if id == source)
    );
    assert!(
        matches!(store.symbols.get(parameter_symbol).origin, SymbolOrigin::Source(id) if id == source)
    );
    let method_owner = store.symbols.get(method_symbol).owner.unwrap();
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(method_owner).name.text()),
        "Evaluator$"
    );

    let mut typer =
        dotty_typer::SourceTyper::new(&ast, source, &index, &mut store, definitions, &packages)
            .with_resolver(Box::new(resolver(definitions)));
    let typed_root = typer
        .type_method_definition(method_tree)
        .expect("selectLeft must type before eval in a fresh resolver session");
    assert_eq!(
        typer.source_typed_index().get(source, method_tree),
        Some(typed_root)
    );
    let typed_method = typer.typed_ast().get(typed_root);
    let TreeKind::DefDef(typed_definition) = &typed_method.kind else {
        panic!("whole source-owned DefDef is published")
    };
    assert_eq!(typed_definition.value_param_clauses[0].len(), 1);
    let source_parameter = source_method.value_param_clauses[0][0];
    let typed_parameter_tree = typed_definition.value_param_clauses[0][0];
    assert_eq!(
        typer.source_typed_index().get(source, source_parameter),
        Some(typed_parameter_tree),
        "the actual DefDef formal is its source-mapped formal"
    );
    assert_eq!(
        typer.source_typed_index().get(source, source_method.tpt),
        Some(typed_definition.tpt),
        "the actual DefDef result tree is its source-mapped result tree"
    );
    assert_eq!(
        typed_definition.rhs,
        Some(
            typer
                .source_typed_index()
                .get(source, source_method.rhs.unwrap())
                .unwrap()
        )
    );
    let SymbolInfo::Complete(method_info) = *typer.store().symbols.info(method_symbol) else {
        panic!("source method signature is complete")
    };
    let Type::Method(method_type) = typer.store().types.get(method_info) else {
        panic!("selectLeft has a method signature")
    };
    assert_eq!(method_type.kind, MethodKind::Plain);
    assert_eq!(method_type.params.len(), 1);
    assert_type_ref(typer.store(), method_type.params[0].ty, expr);
    assert_type_ref(typer.store(), method_type.result, expr);
    assert_term_ref(typer.store(), typed_method.ty, method_symbol);
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_definition.tpt).ty,
        expr,
    );
    let TreeKind::ValDef(typed_parameter) = &typer.typed_ast().get(typed_parameter_tree).kind
    else {
        panic!("source formal remains in the typed DefDef")
    };
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_parameter.tpt).ty,
        expr,
    );
    let source_rhs = source_method.rhs.unwrap();
    let TreeKind::Match(source_match) = &ast.get(source_rhs).kind else {
        panic!("original selectLeft RHS is a Match")
    };
    let typed_match_tree = typer.source_typed_index().get(source, source_rhs).unwrap();
    let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_match_tree).kind else {
        panic!("entire RHS Match is typed")
    };
    assert_eq!(typed_definition.rhs, Some(typed_match_tree));
    assert_eq!(source_match.cases.len(), 2);
    assert_eq!(typed_match.cases.len(), 2);
    assert_eq!(
        typed_match.selector,
        typer
            .source_typed_index()
            .get(source, source_match.selector)
            .unwrap()
    );
    let TreeKind::Ident(selector) = &typer.typed_ast().get(typed_match.selector).kind else {
        panic!("Match selector remains the original expr reference")
    };
    assert_eq!(typer.store().names.resolve(selector.name.text()), "expr");
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_match.selector).ty,
        parameter_symbol,
    );
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_match_tree).ty,
        expr,
    );

    let mut normalized_cases = Vec::new();
    for (case_index, (source_case_tree, typed_case_tree)) in source_match
        .cases
        .iter()
        .zip(&typed_match.cases)
        .enumerate()
    {
        assert_eq!(
            typer.source_typed_index().get(source, *source_case_tree),
            Some(*typed_case_tree),
            "source CaseDef order and mapping are retained"
        );
        let TreeKind::CaseDef(source_case) = &ast.get(*source_case_tree).kind else {
            panic!("source case #{case_index} remains a CaseDef")
        };
        let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(*typed_case_tree).kind else {
            panic!("typed case #{case_index} remains a CaseDef")
        };
        assert_eq!(
            typer.source_typed_index().get(source, source_case.pattern),
            Some(typed_case.pattern),
            "the actual CaseDef pattern is its source-mapped pattern"
        );
        assert_eq!(
            typer.source_typed_index().get(source, source_case.body),
            Some(typed_case.body),
            "the actual CaseDef body is its source-mapped body"
        );
        if let Some(source_guard) = source_case.guard {
            assert_eq!(
                typer.source_typed_index().get(source, source_guard),
                typed_case.guard
            );
        } else {
            assert!(typed_case.guard.is_none());
        }
        let TreeKind::Apply(source_extractor) = &ast.get(source_case.pattern).kind else {
            panic!("source case pattern is an extractor application")
        };
        let TreeKind::Ident(source_constructor) = &ast.get(source_extractor.function).kind else {
            panic!("extractor source is a constructor name")
        };
        let constructor_name = typer.store().names.resolve(source_constructor.name.text());
        let (expected_class, expected_unapply, pattern_arity) = match constructor_name {
            "Num" => (
                num,
                index
                    .generated_case_class_method(num, GeneratedCaseClassMethodRole::Unapply)
                    .unwrap(),
                1,
            ),
            "Add" => (
                add,
                index
                    .generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply)
                    .unwrap(),
                2,
            ),
            other => panic!("unexpected source case constructor {other}"),
        };
        let typed_pattern_tree = typer
            .source_typed_index()
            .get(source, source_case.pattern)
            .unwrap();
        assert_eq!(typed_case.pattern, typed_pattern_tree);
        let TreeKind::UnApply(typed_unapply) = &typer.typed_ast().get(typed_pattern_tree).kind
        else {
            panic!("{constructor_name} pattern uses a typed UnApply")
        };
        assert_eq!(typed_unapply.patterns.len(), pattern_arity);
        assert_type_ref(
            typer.store(),
            typer.typed_ast().get(typed_pattern_tree).ty,
            expected_class,
        );
        assert_term_ref(
            typer.store(),
            typer.typed_ast().get(typed_unapply.function).ty,
            expected_unapply,
        );
        let TreeKind::Select(typed_extractor) = &typer.typed_ast().get(typed_unapply.function).kind
        else {
            panic!("actual UnApply owns the selected companion extractor")
        };
        assert_eq!(
            typer
                .source_typed_index()
                .get(source, source_extractor.function),
            Some(typed_extractor.qualifier),
            "the actual UnApply qualifier is its source-mapped qualifier"
        );
        assert_eq!(source_extractor.args.len(), typed_unapply.patterns.len());
        for (source_argument, typed_pattern) in
            source_extractor.args.iter().zip(&typed_unapply.patterns)
        {
            assert_eq!(
                typer.source_typed_index().get(source, *source_argument),
                Some(*typed_pattern),
                "each UnApply child is the typed mapping of its source argument"
            );
        }
        let owner = typer.store().symbols.get(expected_unapply).owner.unwrap();
        assert_eq!(
            typer.store().symbols.get(owner).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            index
                .generated_case_class_method_identity(expected_unapply)
                .unwrap()
                .class,
            expected_class
        );
        let SymbolInfo::Complete(unapply_info) = *typer.store().symbols.info(expected_unapply)
        else {
            panic!("generated {constructor_name}.unapply signature is complete")
        };
        let Type::Method(unapply_type) = typer.store().types.get(unapply_info) else {
            panic!("generated {constructor_name}.unapply has a method signature")
        };
        assert_eq!(unapply_type.params.len(), 1);
        assert_type_ref(typer.store(), unapply_type.params[0].ty, expected_class);
        assert_type_ref(typer.store(), unapply_type.result, expected_class);
        assert_value_type_ref(
            typer.store(),
            typer.typed_ast().get(*typed_case_tree).ty,
            expr,
        );
        assert_value_type_ref(
            typer.store(),
            typer.typed_ast().get(typed_case.body).ty,
            expr,
        );

        match constructor_name {
            "Num" => {
                assert_eq!(source_extractor.args.len(), 1);
                let source_wildcard = source_extractor.args[0];
                let TreeKind::Ident(wildcard) = &ast.get(source_wildcard).kind else {
                    panic!("Num's nested pattern is the source wildcard")
                };
                assert_eq!(typer.store().names.resolve(wildcard.name.text()), "_");
                assert!(
                    typer
                        .pattern_binding_symbol_at(source, source_wildcard)
                        .is_none()
                );
                assert!(matches!(
                    typer.typed_ast().get(typed_unapply.patterns[0]).kind,
                    TreeKind::Ident(_)
                ));
                let TreeKind::Block(body) = &typer.typed_ast().get(typed_case.body).kind else {
                    panic!("Num result keeps its source body block")
                };
                let TreeKind::Block(source_body) = &ast.get(source_case.body).kind else {
                    panic!("Num result keeps its source body block")
                };
                assert_eq!(
                    typer.source_typed_index().get(source, source_body.expr),
                    Some(body.expr),
                    "the returned body points at its source-mapped expression"
                );
                assert_term_ref(
                    typer.store(),
                    typer.typed_ast().get(body.expr).ty,
                    parameter_symbol,
                );
                normalized_cases.push("Num(_)=>expr".to_owned());
            }
            "Add" => {
                assert_eq!(source_extractor.args.len(), 2);
                let source_binder = source_extractor.args[0];
                let source_wildcard = source_extractor.args[1];
                let TreeKind::Ident(binder_name) = &ast.get(source_binder).kind else {
                    panic!("Add's first field is one source binder")
                };
                let TreeKind::Ident(wildcard_name) = &ast.get(source_wildcard).kind else {
                    panic!("Add's second field is a source wildcard")
                };
                assert_eq!(typer.store().names.resolve(binder_name.name.text()), "left");
                assert_eq!(typer.store().names.resolve(wildcard_name.name.text()), "_");
                let binder = typer
                    .pattern_binding_symbol_at(source, source_binder)
                    .expect("exactly one case-local left binder is published");
                assert_eq!(
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(binder).name.text()),
                    "left"
                );
                assert_eq!(typer.store().symbols.get(binder).owner, Some(method_symbol));
                let SymbolInfo::Complete(binder_info) = *typer.store().symbols.info(binder) else {
                    panic!("case-local binder has its canonical completed type")
                };
                assert_type_ref(typer.store(), binder_info, expr);
                assert!(
                    typer
                        .pattern_binding_symbol_at(source, source_wildcard)
                        .is_none()
                );
                assert!(matches!(
                    typer.typed_ast().get(typed_unapply.patterns[0]).kind,
                    TreeKind::Bind(_)
                ));
                assert!(matches!(
                    typer.typed_ast().get(typed_unapply.patterns[1]).kind,
                    TreeKind::Ident(_)
                ));
                let TreeKind::Block(body) = &typer.typed_ast().get(typed_case.body).kind else {
                    panic!("Add result keeps its source body block")
                };
                let TreeKind::Block(source_body) = &ast.get(source_case.body).kind else {
                    panic!("Add result keeps its source body block")
                };
                assert_eq!(
                    typer.source_typed_index().get(source, source_body.expr),
                    Some(body.expr),
                    "the returned body points at its source-mapped expression"
                );
                assert_term_ref(typer.store(), typer.typed_ast().get(body.expr).ty, binder);
                let add_tree = classes["Add"].0;
                let TreeKind::TypeDef(add_definition) = &ast.get(add_tree).kind else {
                    unreachable!()
                };
                let TreeKind::Template(add_template) = &ast.get(add_definition.rhs).kind else {
                    unreachable!()
                };
                let TreeKind::DefDef(constructor) = &ast.get(add_template.constructor).kind else {
                    panic!("Add's original constructor is retained")
                };
                let fields = &constructor.value_param_clauses[0];
                assert_eq!(fields.len(), 2);
                let class_scope = index.scope_of(add).unwrap();
                let selectors = fields
                    .iter()
                    .enumerate()
                    .map(|(position, field_tree)| {
                        let field = index.symbol_at(source, *field_tree).unwrap();
                        let selector = index.derived_symbol_at(add, source, *field_tree).unwrap();
                        assert_eq!(typer.store().symbols.get(field).owner, Some(add));
                        assert_eq!(typer.store().symbols.get(selector).owner, Some(add));
                        assert_ne!(field, selector);
                        assert_eq!(
                            index.definition_of(field),
                            Some(SourceDefinition::Canonical {
                                source,
                                tree: *field_tree
                            })
                        );
                        assert_eq!(
                            index.definition_of(selector),
                            Some(SourceDefinition::Derived {
                                source,
                                tree: *field_tree
                            })
                        );
                        let selector_name = typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(selector).name.text());
                        assert_eq!(selector_name, if position == 0 { "_1" } else { "_2" });
                        let SymbolInfo::Complete(selector_info) =
                            *typer.store().symbols.info(selector)
                        else {
                            panic!("Product selector {selector_name} is complete")
                        };
                        let Type::Method(selector_type) = typer.store().types.get(selector_info)
                        else {
                            panic!("Product selector {selector_name} is a method")
                        };
                        assert!(selector_type.params.is_empty());
                        assert_type_ref(typer.store(), selector_type.result, expr);
                        assert!(
                            typer
                                .store()
                                .scopes
                                .get(class_scope)
                                .entered_symbols()
                                .any(|entered| entered == selector)
                        );
                        selector
                    })
                    .collect::<Vec<_>>();
                assert_eq!(selectors.len(), 2);
                assert_ne!(selectors[0], selectors[1]);
                normalized_cases.push("Add(left,_2)=>left".to_owned());
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(normalized_cases, ["Num(_)=>expr", "Add(left,_2)=>left"]);

    let range = method_position.span().range();
    let mut mapped = 0usize;
    for (source_tree, source_node) in ast.iter().filter(|(_, node)| {
        node.position.is_some_and(|span| {
            let node_range = span.span().range();
            node_range.start() >= range.start()
                && node_range.end() <= range.end()
                && node_range.start() < node_range.end()
        })
    }) {
        let typed_tree = typer
            .source_typed_index()
            .get(source, source_tree)
            .unwrap_or_else(|| panic!("source tree {source_tree:?} lacks its typed mapping"));
        assert_eq!(
            typer.typed_ast().get(typed_tree).position,
            source_node.position,
            "source span is preserved for {source_tree:?}"
        );
        mapped += 1;
    }
    assert!(
        mapped >= 19,
        "all original nested source trees have mappings: {mapped}"
    );
    format!(
        "root=selectLeft@{}:{};signature=(Expr):Expr;selector=expr:Expr;cases={};Add.unapply=companion-owned(Add):Add;selectors=_1(left):Expr,_2(right):Expr;binder=left@Add._1:case-local;wildcard=Add._2:unbound;mapped_source_trees={mapped}",
        range.start(),
        range.end(),
        normalized_cases.join(",")
    )
}

#[test]
fn late_error_after_add_binder_rolls_back_and_original_select_left_retries() {
    let broken_source = format!(
        "{SOURCE}\nobject BrokenEvaluator:\n  def selectLeftBroken(expr: Expr): Int =\n    expr match\n      case Num(_) => 1\n      case Add(left, _) => left\n"
    );
    let source = SourceId::from_index(20763);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(&broken_source).unwrap(),
        source,
        ContextualScanner::new(&broken_source).unwrap(),
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
    let mut roots = std::collections::BTreeMap::new();
    for (tree, node) in parsed.ast.iter() {
        if let TreeKind::DefDef(definition) = &node.kind {
            let name = store.names.resolve(definition.name.as_name().text());
            if matches!(name, "selectLeft" | "selectLeftBroken") {
                roots.insert(
                    name.to_owned(),
                    (tree, index.symbol_at(source, tree).unwrap()),
                );
            }
        }
    }
    let (valid_tree, valid_symbol) = roots["selectLeft"];
    let (broken_tree, broken_symbol) = roots["selectLeftBroken"];
    let TreeKind::DefDef(broken_definition) = &parsed.ast.get(broken_tree).kind else {
        unreachable!()
    };
    let TreeKind::Match(broken_match) = &parsed.ast.get(broken_definition.rhs.unwrap()).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(add_case) = &parsed.ast.get(broken_match.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(add_pattern) = &parsed.ast.get(add_case.pattern).kind else {
        unreachable!()
    };
    let source_binder = add_pattern.args[0];
    let resolver = resolver(definitions);
    let mut typer = dotty_typer::SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));

    let store_checkpoint = typer.store().checkpoint();
    let typed_checkpoint = typer.typed_ast().checkpoint();
    let semantic_symbols = source_semantic_symbols(&parsed.ast, source, &index);
    let symbols_before = semantic_symbols
        .iter()
        .map(|id| {
            let symbol = typer.store().symbols.get(*id);
            (
                symbol.info,
                symbol.owner,
                symbol.links,
                symbol.kind,
                symbol.origin,
            )
        })
        .collect::<Vec<_>>();
    let mut scope_ids = semantic_symbols
        .iter()
        .filter_map(|symbol| index.scope_of(*symbol))
        .collect::<Vec<_>>();
    scope_ids.sort_by_key(|scope| scope.index());
    scope_ids.dedup();
    let scopes_before = scope_ids
        .iter()
        .map(|id| {
            let mut members = typer
                .store()
                .scopes
                .get(*id)
                .entered_symbols()
                .collect::<Vec<_>>();
            members.sort_by_key(|symbol| symbol.index());
            members
        })
        .collect::<Vec<_>>();
    let source_types_before = parsed
        .ast
        .iter()
        .map(|(tree, _)| typer.source_type_index().type_at(source, tree))
        .collect::<Vec<_>>();
    let source_typed_before = parsed
        .ast
        .iter()
        .map(|(tree, _)| typer.source_typed_index().get(source, tree))
        .collect::<Vec<_>>();
    let error = typer
        .type_method_definition(broken_tree)
        .expect_err("the Add binder is created before its Expr result conflicts with Int");
    assert!(
        matches!(
            error,
            dotty_typer::TyperError::ExpectedExpressionTypeMismatch { .. }
        ),
        "the late result mismatch remains an explicit typer error: {error:?}"
    );
    assert_eq!(typer.store().checkpoint(), store_checkpoint);
    assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
    assert_eq!(
        semantic_symbols
            .iter()
            .map(|id| {
                let symbol = typer.store().symbols.get(*id);
                (
                    symbol.info,
                    symbol.owner,
                    symbol.links,
                    symbol.kind,
                    symbol.origin,
                )
            })
            .collect::<Vec<_>>(),
        symbols_before,
        "existing symbols and derived method info remain unchanged"
    );
    assert_eq!(
        scope_ids
            .iter()
            .map(|id| {
                let mut members = typer
                    .store()
                    .scopes
                    .get(*id)
                    .entered_symbols()
                    .collect::<Vec<_>>();
                members.sort_by_key(|symbol| symbol.index());
                members
            })
            .collect::<Vec<_>>(),
        scopes_before,
        "pre-existing scopes do not retain the failed binder"
    );
    assert_eq!(
        parsed
            .ast
            .iter()
            .map(|(tree, _)| typer.source_type_index().type_at(source, tree))
            .collect::<Vec<_>>(),
        source_types_before,
        "SourceTypeIndex is unchanged"
    );
    assert_eq!(
        parsed
            .ast
            .iter()
            .map(|(tree, _)| typer.source_typed_index().get(source, tree))
            .collect::<Vec<_>>(),
        source_typed_before,
        "SourceTypedIndex is unchanged"
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, broken_tree)
            .is_none()
    );
    assert!(
        typer
            .pattern_binding_symbol_at(source, source_binder)
            .is_none()
    );
    assert_eq!(
        *typer.store().symbols.info(broken_symbol),
        SymbolInfo::Missing
    );

    let typed_valid = typer
        .type_method_definition(valid_tree)
        .expect("original selectLeft retries successfully in the same SourceTyper");
    assert_eq!(
        typer.source_typed_index().get(source, valid_tree),
        Some(typed_valid)
    );
    assert!(matches!(
        typer.typed_ast().get(typed_valid).kind,
        TreeKind::DefDef(_)
    ));
    assert!(matches!(
        typer.store().symbols.get(valid_symbol).info,
        SymbolInfo::Complete(_)
    ));
}

#[test]
fn corrupted_reference_product2_identities_reject_then_retry_original_select_left() {
    #[derive(Clone, Copy)]
    enum Fault {
        SwappedSourceProvenance,
        SwappedScopeSelectors,
        ForeignUnapplyOwner,
        ForeignCompanionLink,
        WrongParent,
        BrokenProductIdentity,
        ForeignSelectorOwner,
    }
    let faults = [
        Fault::SwappedSourceProvenance,
        Fault::SwappedScopeSelectors,
        Fault::ForeignUnapplyOwner,
        Fault::ForeignCompanionLink,
        Fault::WrongParent,
        Fault::BrokenProductIdentity,
        Fault::ForeignSelectorOwner,
    ];
    for (offset, fault) in faults.into_iter().enumerate() {
        let source = SourceId::from_index(20770 + offset as u32);
        let (ast, _root, index, mut store, definitions, packages) = setup(source);
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

        // Complete Add through the authentic JDK + scala.Product resolver for
        // the parent/Product identity controls, which mutate that real
        // classpath-backed metadata. Other controls start cold so their valid
        // retry exercises the normal completion path in the same store.
        let add_info = if !matches!(fault, Fault::WrongParent | Fault::BrokenProductIdentity) {
            None
        } else {
            Some({
                let mut typer = dotty_typer::SourceTyper::new(
                    &ast,
                    source,
                    &index,
                    &mut store,
                    definitions,
                    &packages,
                )
                .with_resolver(Box::new(resolver(definitions)));
                typer.complete_symbol(add).unwrap()
            })
        };
        let product = add_info.map(|add_info| {
            let Type::ClassInfo(class_info) = store.types.get(add_info) else {
                panic!("Add has its authentic completed ClassInfo")
            };
            class_info
                .parents
                .iter()
                .find_map(|parent| match store.types.get(*parent) {
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } if store.names.resolve(store.symbols.get(*symbol).name.text())
                        == "Product" =>
                    {
                        Some(*symbol)
                    }
                    _ => None,
                })
                .expect("Add completes with the canonical scala.Product parent")
        });
        let num_companion = store.symbols.get(num).links.companion.unwrap();
        let num_companion_class = store.symbols.get(num_unapply).owner.unwrap();
        let add_companion_link = store.symbols.get(add).links.companion;
        let unapply_owner = store.symbols.get(add_unapply).owner;
        let selector_owner = store.symbols.get(selectors[0]).owner;
        let product_kind = product.map(|product| store.symbols.get(product).kind);
        let add_parents = add_info.map(|add_info| match store.types.get(add_info) {
            Type::ClassInfo(class_info) => class_info.parents.clone(),
            _ => unreachable!(),
        });
        let mut swapped_index = None;

        match fault {
            Fault::SwappedSourceProvenance => {
                swapped_index = Some(index_with_swapped_add_selector_provenance(
                    &ast, source, &store, &index, add, &fields,
                ));
            }
            Fault::SwappedScopeSelectors => {
                let scope = index.scope_of(add).unwrap();
                store.scopes.get_mut(scope).remove(selectors[0]);
                store.scopes.get_mut(scope).remove(selectors[1]);
                store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[0], selectors[1]);
                store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[1], selectors[0]);
            }
            Fault::ForeignUnapplyOwner => {
                store.symbols.get_mut(add_unapply).owner = Some(num_companion_class);
            }
            Fault::ForeignCompanionLink => {
                store.symbols.get_mut(add).links.companion = Some(num_companion);
            }
            Fault::WrongParent => {
                let add_info = add_info.unwrap();
                let num_ref = store.types.alloc(Type::TypeRef {
                    prefix: definitions.no_prefix,
                    target: TypeRefTarget::Symbol(num),
                });
                let Type::ClassInfo(class_info) = store.types.get_mut(add_info) else {
                    unreachable!()
                };
                assert_eq!(class_info.parents.len(), 3);
                class_info.parents[1] = num_ref;
            }
            Fault::BrokenProductIdentity => {
                store.symbols.get_mut(product.unwrap()).kind = SymbolKind::Class;
            }
            Fault::ForeignSelectorOwner => {
                store.symbols.get_mut(selectors[0]).owner = Some(num);
            }
        }

        let index_for_attempt = swapped_index.as_ref().unwrap_or(&index);
        let error = {
            let mut typer = dotty_typer::SourceTyper::new(
                &ast,
                source,
                index_for_attempt,
                &mut store,
                definitions,
                &packages,
            )
            .with_resolver(Box::new(resolver(definitions)));
            let before_store = typer.store().checkpoint();
            let before_typed = typer.typed_ast().checkpoint();
            let symbols = source_semantic_symbols(&ast, source, &index);
            let before_symbols = symbols
                .iter()
                .map(|id| {
                    let symbol = typer.store().symbols.get(*id);
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
                        .store()
                        .scopes
                        .get(*scope)
                        .entered_symbols()
                        .collect::<Vec<_>>();
                    members.sort_by_key(|symbol| symbol.index());
                    members
                })
                .collect::<Vec<_>>();
            let error = typer
                .type_method_definition(method_tree.unwrap())
                .expect_err("corrupted reference Product2 identity must not publish selectLeft");
            assert_eq!(
                typer.store().checkpoint(),
                before_store,
                "{offset}: store rollback"
            );
            assert_eq!(
                typer.typed_ast().checkpoint(),
                before_typed,
                "{offset}: typed rollback"
            );
            assert!(
                typer
                    .source_typed_index()
                    .get(source, method_tree.unwrap())
                    .is_none()
            );
            assert_eq!(
                symbols
                    .iter()
                    .map(|id| {
                        let symbol = typer.store().symbols.get(*id);
                        (
                            symbol.info,
                            symbol.owner,
                            symbol.links,
                            symbol.kind,
                            symbol.origin,
                        )
                    })
                    .collect::<Vec<_>>(),
                before_symbols,
                "{offset}: symbol info and identity rollback"
            );
            assert_eq!(
                scopes
                    .iter()
                    .map(|scope| {
                        let mut members = typer
                            .store()
                            .scopes
                            .get(*scope)
                            .entered_symbols()
                            .collect::<Vec<_>>();
                        members.sort_by_key(|symbol| symbol.index());
                        members
                    })
                    .collect::<Vec<_>>(),
                before_scopes,
                "{offset}: preexisting scope membership rollback"
            );
            let repeated = typer
                .type_method_definition(method_tree.unwrap())
                .expect_err("the same corrupt metadata fails stably in one typer");
            assert_eq!(format!("{error:?}"), format!("{repeated:?}"));
            assert_eq!(typer.store().checkpoint(), before_store);
            assert_eq!(typer.typed_ast().checkpoint(), before_typed);
            error
        };
        let rejected_as_expected = match fault {
            Fault::SwappedSourceProvenance
            | Fault::SwappedScopeSelectors
            | Fault::ForeignSelectorOwner => matches!(
                error,
                dotty_typer::TyperError::ExtractorResultMemberNotFound { .. }
            ),
            Fault::ForeignUnapplyOwner => matches!(
                error,
                dotty_typer::TyperError::SymbolSourceKindMismatch { .. }
            ),
            Fault::ForeignCompanionLink => matches!(
                error,
                dotty_typer::TyperError::ExtractorQualifierNotValueLike { .. }
            ),
            Fault::WrongParent | Fault::BrokenProductIdentity => {
                matches!(error, dotty_typer::TyperError::MalformedClassInfo { .. })
            }
        };
        assert!(
            rejected_as_expected,
            "corruption {offset} must reject through its expected diagnostic: {error:?}"
        );
        println!("ISSUE2076_CORRUPTED_PRODUCT2\t{offset}\t{error:?}");

        match fault {
            Fault::SwappedSourceProvenance => {}
            Fault::SwappedScopeSelectors => {
                let scope = index.scope_of(add).unwrap();
                store.scopes.get_mut(scope).remove(selectors[0]);
                store.scopes.get_mut(scope).remove(selectors[1]);
                store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[0], selectors[0]);
                store
                    .scopes
                    .get_mut(scope)
                    .enter(selector_names[1], selectors[1]);
            }
            Fault::ForeignUnapplyOwner => store.symbols.get_mut(add_unapply).owner = unapply_owner,
            Fault::ForeignCompanionLink => {
                store.symbols.get_mut(add).links.companion = add_companion_link
            }
            Fault::WrongParent => {
                let add_info = add_info.unwrap();
                let Type::ClassInfo(class_info) = store.types.get_mut(add_info) else {
                    unreachable!()
                };
                class_info.parents = add_parents.unwrap();
            }
            Fault::BrokenProductIdentity => {
                store.symbols.get_mut(product.unwrap()).kind = product_kind.unwrap()
            }
            Fault::ForeignSelectorOwner => {
                store.symbols.get_mut(selectors[0]).owner = selector_owner
            }
        }
        if !matches!(fault, Fault::WrongParent | Fault::BrokenProductIdentity) {
            let mut typer = dotty_typer::SourceTyper::new(
                &ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            )
            .with_resolver(Box::new(resolver(definitions)));
            typer
                .type_method_definition(method_tree.unwrap())
                .expect("restoring authentic selector/extractor metadata retries selectLeft");
        }
    }
}

#[test]
fn renamed_pair_control_keeps_equal_typed_selector_positions_distinct() {
    let source_text = "package adt\nsealed trait Expr\nfinal case class Pair(first: Expr, second: Expr) extends Expr\nobject Reader:\n  def first(pair: Pair): Expr =\n    pair match\n      case Pair(first, _) => first\n";
    let source = SourceId::from_index(20764);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        ContextualScanner::new(source_text).unwrap(),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Pair.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let mut pair = None;
    let mut method = None;
    for (tree, node) in parsed.ast.iter() {
        match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Pair" =>
            {
                pair = Some((tree, index.symbol_at(source, tree).unwrap()));
            }
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "first" =>
            {
                method = Some((tree, index.symbol_at(source, tree).unwrap()));
            }
            _ => {}
        }
    }
    let (pair_tree, pair_symbol) = pair.unwrap();
    let (method_tree, _method_symbol) = method.unwrap();
    let TreeKind::DefDef(source_definition) = &parsed.ast.get(method_tree).kind else {
        unreachable!()
    };
    let TreeKind::Match(source_match) = &parsed.ast.get(source_definition.rhs.unwrap()).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(source_case) = &parsed.ast.get(source_match.cases[0]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(source_pattern) = &parsed.ast.get(source_case.pattern).kind else {
        unreachable!()
    };
    let source_first = source_pattern.args[0];
    let source_second = source_pattern.args[1];
    let TreeKind::Ident(first_name) = &parsed.ast.get(source_first).kind else {
        unreachable!()
    };
    assert_eq!(store.names.resolve(first_name.name.text()), "first");
    let TreeKind::Ident(second_name) = &parsed.ast.get(source_second).kind else {
        unreachable!()
    };
    assert_eq!(store.names.resolve(second_name.name.text()), "_");
    let TreeKind::TypeDef(pair_definition) = &parsed.ast.get(pair_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(pair_template) = &parsed.ast.get(pair_definition.rhs).kind else {
        unreachable!()
    };
    let TreeKind::DefDef(constructor) = &parsed.ast.get(pair_template.constructor).kind else {
        panic!("Pair primary constructor")
    };
    let fields = &constructor.value_param_clauses[0];
    assert_eq!(fields.len(), 2);
    let field_symbols = fields
        .iter()
        .map(|field| index.symbol_at(source, *field).unwrap())
        .collect::<Vec<_>>();
    let selectors = fields
        .iter()
        .map(|field| {
            index
                .derived_symbol_at(pair_symbol, source, *field)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_ne!(field_symbols[0], field_symbols[1]);
    assert_ne!(selectors[0], selectors[1]);
    for (position, (field_tree, field, selector)) in fields
        .iter()
        .zip(&field_symbols)
        .zip(&selectors)
        .enumerate()
        .map(|(position, ((tree, field), selector))| (position, (tree, field, selector)))
    {
        assert_eq!(store.symbols.get(*field).owner, Some(pair_symbol));
        assert_eq!(store.symbols.get(*selector).owner, Some(pair_symbol));
        assert_eq!(
            index.definition_of(*selector),
            Some(SourceDefinition::Derived {
                source,
                tree: *field_tree
            })
        );
        assert_eq!(
            store
                .names
                .resolve(store.symbols.get(*selector).name.text()),
            if position == 0 { "_1" } else { "_2" }
        );
    }
    let mut typer = dotty_typer::SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver(definitions)));
    let typed_method = typer.type_method_definition(method_tree).unwrap();
    let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
        unreachable!()
    };
    let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_definition.rhs.unwrap()).kind
    else {
        panic!("Pair control's Match is typed")
    };
    let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(typed_match.cases[0]).kind else {
        unreachable!()
    };
    assert_eq!(
        typer.source_typed_index().get(source, source_case.pattern),
        Some(typed_case.pattern)
    );
    let TreeKind::UnApply(typed_unapply) = &typer.typed_ast().get(typed_case.pattern).kind else {
        panic!("Pair pattern is typed as UnApply")
    };
    assert_eq!(typed_unapply.patterns.len(), 2);
    assert_eq!(
        typer.source_typed_index().get(source, source_first),
        Some(typed_unapply.patterns[0])
    );
    assert_eq!(
        typer.source_typed_index().get(source, source_second),
        Some(typed_unapply.patterns[1])
    );
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_unapply.function).ty,
        index
            .generated_case_class_method(pair_symbol, GeneratedCaseClassMethodRole::Unapply)
            .unwrap(),
    );
    assert_eq!(
        index.derived_symbol_at(pair_symbol, source, fields[0]),
        Some(selectors[0])
    );
    assert_eq!(
        index.derived_symbol_at(pair_symbol, source, fields[1]),
        Some(selectors[1])
    );
    let binder = typer
        .pattern_binding_symbol_at(source, source_first)
        .expect("first component is bound exactly once");
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(binder).name.text()),
        "first"
    );
    let TreeKind::Bind(typed_first) = &typer.typed_ast().get(typed_unapply.patterns[0]).kind else {
        panic!("first selector is bound")
    };
    assert_eq!(
        typer.store().names.resolve(typed_first.name.text()),
        "first"
    );
    assert!(matches!(
        typer.typed_ast().get(typed_unapply.patterns[1]).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(typer.pattern_binding_symbol_at(source, source_second), None);
    let TreeKind::Block(body) = &typer.typed_ast().get(typed_case.body).kind else {
        unreachable!()
    };
    assert_term_ref(typer.store(), typer.typed_ast().get(body.expr).ty, binder);
}

#[track_caller]
fn assert_type_ref(store: &SemanticStore, ty: dotty_core::TypeId, expected: dotty_core::SymbolId) {
    assert!(
        matches!(
            store.types.try_get(ty),
            Some(Type::TypeRef { target: TypeRefTarget::Symbol(actual), .. }) if *actual == expected
        ),
        "{ty:?} is the canonical TypeRef to {expected:?}: {:?}",
        store.types.try_get(ty)
    );
}

fn assert_term_ref(store: &SemanticStore, ty: dotty_core::TypeId, expected: dotty_core::SymbolId) {
    assert!(
        matches!(
            store.types.try_get(ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. }) if *actual == expected
        ),
        "{ty:?} is the canonical TermRef to {expected:?}: {:?}",
        store.types.try_get(ty)
    );
}

#[track_caller]
fn assert_value_type_ref(
    store: &SemanticStore,
    ty: dotty_core::TypeId,
    expected: dotty_core::SymbolId,
) {
    let type_id = match store.types.try_get(ty) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(actual),
            ..
        }) if *actual == expected => return,
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(value),
            ..
        }) => {
            let SymbolInfo::Complete(value_type) = *store.symbols.info(*value) else {
                panic!("value reference {value:?} has no completed type")
            };
            value_type
        }
        other => panic!("{ty:?} is Expr-valued: {other:?}"),
    };
    assert_type_ref(store, type_id, expected);
}

fn source_semantic_symbols(
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
    for owner in owners {
        symbols.extend(
            trees
                .iter()
                .filter_map(|tree| index.derived_symbol_at(owner, source, *tree)),
        );
    }
    let classes = symbols.clone();
    for class in classes {
        for role in [
            GeneratedCaseClassMethodRole::Apply,
            GeneratedCaseClassMethodRole::Unapply,
        ] {
            symbols.extend(index.generated_case_class_method(class, role));
        }
    }
    symbols.sort_by_key(|symbol| symbol.index());
    symbols.dedup();
    symbols
}

fn index_with_swapped_add_selector_provenance(
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    source: SourceId,
    store: &SemanticStore,
    original: &SourceSemanticIndex,
    add: dotty_core::SymbolId,
    fields: &[dotty_core::TreeId<dotty_core::ast::Untyped>],
) -> SourceSemanticIndex {
    assert_eq!(fields.len(), 2);
    let source_symbols = source_semantic_symbols(ast, source, original);
    let mut index = SourceSemanticIndex::new();

    let mut contexts = std::collections::BTreeMap::new();
    for symbol in &source_symbols {
        for context in [
            original.declaration_context_of(*symbol),
            original.field_initializer_context_of(*symbol),
        ]
        .into_iter()
        .flatten()
        {
            let mut current = Some(context);
            while let Some(context_id) = current {
                let context_value = *original.source_context(context_id);
                current = context_value.parent;
                contexts.insert(context_id.index(), context_value);
            }
        }
    }
    let filler = *contexts
        .values()
        .next()
        .expect("source declarations retain at least one lexical context");
    let context_ids = (0..original.source_context_count() as u32)
        .map(|expected_index| {
            let context =
                contexts
                    .get(&expected_index)
                    .copied()
                    .unwrap_or(dotty_core::SourceContext {
                        owner: filler.owner,
                        lexical_scope: filler.lexical_scope,
                        parent: None,
                        import: None,
                    });
            let actual = index.alloc_source_context(context);
            assert_eq!(actual.index(), expected_index);
            (expected_index, actual)
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    for (tree, _) in ast.iter() {
        let Some(symbol) = original.symbol_at(source, tree) else {
            continue;
        };
        if store.symbols.get(symbol).kind == SymbolKind::Package {
            index.record_package_symbol(source, tree, symbol).unwrap();
        } else {
            index.record_symbol(source, tree, symbol).unwrap();
        }
    }

    for owner in &source_symbols {
        for (tree, _) in ast.iter() {
            let Some(mut symbol) = original.derived_symbol_at(*owner, source, tree) else {
                continue;
            };
            if *owner == add && tree == fields[0] {
                symbol = original
                    .derived_symbol_at(add, source, fields[1])
                    .expect("Add._2 original source provenance");
            } else if *owner == add && tree == fields[1] {
                symbol = original
                    .derived_symbol_at(add, source, fields[0])
                    .expect("Add._1 original source provenance");
            }
            index
                .record_derived_symbol(*owner, source, tree, symbol)
                .unwrap();
        }
    }

    for class in &source_symbols {
        for role in [
            GeneratedCaseClassMethodRole::Apply,
            GeneratedCaseClassMethodRole::Unapply,
        ] {
            let Some(method) = original.generated_case_class_method(*class, role) else {
                continue;
            };
            let Some(SourceDefinition::Canonical {
                source: class_source,
                tree,
            }) = original.definition_of(*class)
            else {
                panic!("generated method owner has a canonical source class")
            };
            assert_eq!(class_source, source);
            index
                .record_generated_case_class_method(*class, role, source, tree, method)
                .unwrap();
        }
    }

    for symbol in &source_symbols {
        if let Some(scope) = original.scope_of(*symbol) {
            index.record_scope(*symbol, scope).unwrap();
        }
        if let Some(context) = original.declaration_context_of(*symbol) {
            index
                .record_declaration_context(*symbol, context_ids[&context.index()])
                .unwrap();
        }
        if let Some(context) = original.field_initializer_context_of(*symbol) {
            index
                .record_field_initializer_context(*symbol, context_ids[&context.index()])
                .unwrap();
        }
        if let Some(clauses) = original.extension_prefix_clauses(*symbol) {
            index
                .record_extension_prefix_clauses(*symbol, clauses)
                .unwrap();
        }
    }
    index
}

struct RootReceipt {
    roots: Vec<String>,
    normalized: Vec<String>,
}

fn setup(
    source: SourceId,
) -> (
    dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    dotty_core::TreeId<dotty_core::ast::Untyped>,
    dotty_core::SourceSemanticIndex,
    SemanticStore,
    Definitions,
    Packages,
) {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("frozen source is valid"),
        source,
        ContextualScanner::new(SOURCE).expect("frozen source scans"),
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
    .expect("frozen source names");
    (parsed.ast, parsed.root, index, store, definitions, packages)
}

fn resolver(definitions: Definitions) -> ClasspathSymbolResolver<CompositeClassPath> {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    assert!(fixture.join("scala/Product.tasty").is_file());
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new())
}

fn analyze_roots(source: SourceId, session: &str) -> RootReceipt {
    let (ast, _root, index, mut store, definitions, packages) = setup(source);
    let mut roots = Vec::new();
    for (tree, node) in ast.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let name = store.names.resolve(definition.name.as_name().text());
        if !matches!(name, "selectLeft" | "eval") {
            continue;
        }
        let method = index
            .symbol_at(source, tree)
            .expect("source method identity");
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index
            .symbol_at(source, parameter_tree)
            .expect("source parameter identity");
        let owner = store.symbols.get(method).owner.expect("method owner");
        assert_eq!(store.symbols.get(parameter).owner, Some(method));
        assert_ne!(method, parameter);
        assert_eq!(
            store.names.resolve(store.symbols.get(owner).name.text()),
            "Evaluator$"
        );
        assert!(
            matches!(store.symbols.get(method).origin, SymbolOrigin::Source(id) if id == source)
        );
        assert!(
            matches!(store.symbols.get(parameter).origin, SymbolOrigin::Source(id) if id == source)
        );
        roots.push((
            name.to_owned(),
            tree,
            node.position.unwrap(),
            method,
            parameter,
            owner,
        ));
    }
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(roots.len(), 2);
    let resolver = resolver(definitions);
    let mut typer =
        dotty_typer::SourceTyper::new(&ast, source, &index, &mut store, definitions, &packages)
            .with_resolver(Box::new(resolver));
    let mut rows = Vec::new();
    let mut normalized = Vec::new();
    for (name, tree, position, _method, _parameter, _owner) in roots {
        let range = position.span().range();
        let outcome = match typer.type_method_definition(tree) {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
                format!("complete=whole_DefDef:typed#tree{}", typed.index())
            }
            Err(error) => {
                assert!(typer.source_typed_index().get(source, tree).is_none());
                let debug = format!("{error:?}");
                let error_kind = debug.split(" {").next().unwrap_or(&debug);
                let nested_tree = parse_tree_index(&debug);
                let nested = nested_tree
                    .and_then(|index| ast.iter().find(|(candidate, _)| candidate.index() == index))
                    .map(|(nested_tree, nested_node)| {
                        let nested_range = nested_node.position.unwrap().span().range();
                        format!(
                            "Expr.scala#tree{}@{}:{}",
                            nested_tree.index(),
                            nested_range.start(),
                            nested_range.end()
                        )
                    })
                    .unwrap_or_else(|| "UNREPORTED".to_owned());
                format!("first_error={error_kind}(nested={nested});typed_index=absent")
            }
        };
        rows.push(format!(
            "{session}\t{name}\tpath=Expr.scala#tree{}\tmethod=source-owned\tparameter=owned-by-source-method\tsource_bytes={}:{}\towner=Evaluator$\t{outcome}",
            tree.index(), range.start(), range.end(),
        ));
        normalized.push(format!(
            "{name}\tpath=Expr.scala#tree{}\tsource_bytes={}:{}\towner=Evaluator$\t{outcome}",
            tree.index(),
            range.start(),
            range.end(),
        ));
    }
    RootReceipt {
        roots: rows,
        normalized,
    }
}

fn parse_tree_index(debug: &str) -> Option<u32> {
    let value = debug.split_once("tree_index: ")?.1;
    value
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn characterize_class(source: SourceId) -> String {
    let (ast, _root, index, mut store, _definitions, _packages) = setup(source);
    let mut expr = None;
    let mut add = None;
    let mut add_tree = None;
    let mut add_template = None;
    for (tree, node) in ast.iter() {
        let TreeKind::TypeDef(definition) = &node.kind else {
            continue;
        };
        match store.names.resolve(definition.name.as_name().text()) {
            "Expr" => expr = Some(index.symbol_at(source, tree).expect("Expr identity")),
            "Add" => {
                add = Some(index.symbol_at(source, tree).expect("Add identity"));
                add_tree = Some(tree);
                add_template = Some(definition.rhs);
            }
            _ => {}
        }
    }
    let expr = expr.expect("source Expr trait");
    let add = add.expect("source Add class");
    let expr_info = store.symbols.get(expr);
    let add_info = store.symbols.get(add);
    assert_eq!(expr_info.kind, SymbolKind::Trait);
    assert!(expr_info.flags.contains(SymbolFlags::SEALED));
    assert_eq!(add_info.kind, SymbolKind::Class);
    assert!(add_info.flags.contains(SymbolFlags::CASE));
    assert!(matches!(add_info.origin, SymbolOrigin::Source(id) if id == source));
    let add_tree = add_tree.unwrap();
    let add_template = add_template.unwrap();
    let TreeKind::Template(template) = &ast.get(add_template).kind else {
        panic!("Add has no source template");
    };
    assert_eq!(template.parents.len(), 1);
    let parent_name = match &ast.get(template.parents[0]).kind {
        TreeKind::Ident(ident) => store.names.resolve(ident.name.text()).to_owned(),
        other => panic!("unexpected Add parent: {other:?}"),
    };
    let package = add_info.owner.expect("Add package owner");
    let package_scope = index.scope_of(package).expect("source package scope");
    let parent_symbol = store
        .scopes
        .get(package_scope)
        .lookup(TypeName::new(store.names.intern(&parent_name)).as_name());
    assert_eq!(parent_symbol, Some(expr));
    let class_scope = index.scope_of(add).expect("Add class member scope");
    let left = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("left")).as_name());
    let right = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("right")).as_name());
    let selector1_name = TermName::new(store.names.intern("_1"));
    let selector2_name = TermName::new(store.names.intern("_2"));
    let selector1 = store
        .scopes
        .get(class_scope)
        .lookup(selector1_name.as_name());
    let selector2 = store
        .scopes
        .get(class_scope)
        .lookup(selector2_name.as_name());
    let companion = add_info.links.companion.expect("Add companion");
    let unapply = index.generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply);
    let companion_name = store
        .names
        .resolve(store.symbols.get(companion).name.text())
        .to_owned();
    let class_facts = format!(
        "Expr=sealed_trait;Add=source_case_class;parent={parent_name}:canonical;fields=left:source-member,right:source-member,distinct={};selectors=_1:{},_2:{},unapply={},companion=module({companion_name})",
        left.is_some() && right.is_some() && left != right,
        if selector1.is_some() {
            "published"
        } else {
            "absent"
        },
        if selector2.is_some() {
            "published"
        } else {
            "absent"
        },
        if unapply.is_some() {
            "published"
        } else {
            "absent"
        },
    );
    assert!(left.is_some() && right.is_some() && left != right);
    assert_eq!(store.symbols.get(left.unwrap()).owner, Some(add));
    assert_eq!(store.symbols.get(right.unwrap()).owner, Some(add));
    let (Some(selector1), Some(selector2), Some(unapply)) = (selector1, selector2, unapply) else {
        panic!("canonical Add must expose all bounded generated identities");
    };
    assert_ne!(selector1, selector2);
    assert_eq!(store.symbols.get(selector1).owner, Some(add));
    assert_eq!(store.symbols.get(selector2).owner, Some(add));
    let companion_module = store
        .scopes
        .get(package_scope)
        .lookup(TypeName::new(store.names.intern("Add$")).as_name())
        .expect("Add companion module class");
    assert_eq!(store.symbols.get(unapply).owner, Some(companion_module));
    let input_formals = store
        .scopes
        .get(index.scope_of(unapply).expect("unapply scope"))
        .lookup_all(TermName::new(store.names.intern("x$1")).as_name());
    assert_eq!(input_formals.len(), 1);
    assert_eq!(store.symbols.get(input_formals[0]).owner, Some(unapply));
    assert!(matches!(
        store.symbols.get(unapply).info,
        dotty_core::SymbolInfo::Missing
    ));
    let _ = add_tree;
    format!(
        "{class_facts}\tselectors=distinct-class-owned\tunapply=companion-owned\tformal=x$1\tsignature=uncompleted"
    )
}
