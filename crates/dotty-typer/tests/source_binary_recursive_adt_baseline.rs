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
const NESTED_SOURCE: &str = include_str!("fixtures/source-nested-recursive-adt-3.9.0/Expr.scala");
// This receipt includes arena IDs: canonical scala.Boolean TASTy lowering now
// reuses Definitions.boolean instead of allocating an equivalent TypeRef, so
// later diagnostic TypeIds are two slots earlier while rejection behavior is
// unchanged.
const STRUCTURAL_CONTROL_RECEIPTS: &str =
    include_str!("fixtures/source-binary-recursive-adt-3.9.0/rust-structural-controls.tsv");

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
fn nested_recursive_adt_original_roots_are_attempted_in_two_fresh_sessions() {
    let source_a = SourceId::from_index(21321);
    let source_b = SourceId::from_index(21322);
    assert_ne!(
        source_a, source_b,
        "fresh sessions use disjoint source identities"
    );
    let first = nested_roots(source_a, "session-a");
    let second = nested_roots(source_b, "session-b");
    assert_eq!(first.normalized, second.normalized);
    assert_eq!(
        first.semantic_receipt, second.semantic_receipt,
        "fresh sessions have normalized peelLeft semantic parity"
    );
    assert_eq!(
        first.semantic_receipt,
        include_str!("fixtures/source-nested-recursive-adt-3.9.0/rust-peelleft-complete.tsv")
            .split_once('\n')
            .expect("normalized semantic receipt header")
            .1,
        "the original peelLeft semantics are pinned without arena allocation IDs"
    );
    println!("ISSUE2133_PEELLEFT\t{}", first.semantic_receipt.trim_end());
    let receipt = first
        .roots
        .iter()
        .chain(second.roots.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        format!("{}\n", receipt.join("\n")),
        include_str!("fixtures/source-nested-recursive-adt-3.9.0/rust-two-root-baseline.tsv")
            .split_once('\n')
            .expect("nested two-root receipt header")
            .1
    );
    for row in receipt {
        println!("ISSUE2132_NESTED_ROOT\t{row}");
    }
}

#[test]
fn nested_peelleft_controls_keep_bounded_rejections_and_positional_identity() {
    let wrong_arity = NESTED_SOURCE.replace(
        "case Add(Add(left, _), _) => left",
        "case Add(Add(left, _, _), _) => left",
    );
    assert_ne!(
        wrong_arity, NESTED_SOURCE,
        "wrong-arity control mutation applies"
    );
    let wrong_arity_error = type_nested_peel_root(&wrong_arity, SourceId::from_index(21332))
        .expect_err("three nested extractor arguments are rejected");
    assert!(
        wrong_arity_error.starts_with("ExtractorProductSelectorCountMismatch"),
        "wrong-arity control reports its typed rejection: {wrong_arity_error}"
    );

    let escaped_binder = NESTED_SOURCE.replacen("case _ => expr", "case _ => left", 1);
    assert_ne!(
        escaped_binder, NESTED_SOURCE,
        "escaped-binder control mutation applies"
    );
    let escaped_error = type_nested_peel_root(&escaped_binder, SourceId::from_index(21333))
        .expect_err("a first-case binder is unavailable in the fallback case");
    assert!(
        escaped_error.starts_with("TermNameNotFound"),
        "escaped-binder control reports an unresolved reference: {escaped_error}"
    );

    let renamed_source =
        include_str!("fixtures/source-nested-recursive-adt-3.9.0/RenamedReorderedFields.scala");
    let mut renamed_session =
        BinarySession::new_with_source(SourceId::from_index(21334), renamed_source);
    renamed_session.with_typer(|ast, index, typer, _definitions| {
        let root = find_method_tree(ast, typer.store(), "peelLeft");
        typer
            .type_method_definition(root)
            .expect("renamed and reordered product fields preserve positional matching");
        let (classes, expr) = nested_class_symbols(ast, index, typer, SourceId::from_index(21334));
        assert_source_product_fields(
            typer,
            ast,
            index,
            SourceId::from_index(21334),
            classes["Add"],
            &["right", "left"],
            expr,
        );
    });

    let foreign_source_id = SourceId::from_index(21335);
    let mut foreign_owner_session =
        BinarySession::new_with_source(foreign_source_id, NESTED_SOURCE);
    let add = foreign_owner_session
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (foreign_owner_session
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "Add")
                .then(|| {
                    foreign_owner_session
                        .index
                        .symbol_at(foreign_source_id, tree)
                        .unwrap()
                })
        })
        .unwrap();
    let num = foreign_owner_session
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (foreign_owner_session
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "Num")
                .then(|| {
                    foreign_owner_session
                        .index
                        .symbol_at(foreign_source_id, tree)
                        .unwrap()
                })
        })
        .unwrap();
    let add_unapply = foreign_owner_session
        .index
        .generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply)
        .unwrap();
    let num_unapply = foreign_owner_session
        .index
        .generated_case_class_method(num, GeneratedCaseClassMethodRole::Unapply)
        .unwrap();
    let fake_owner = foreign_owner_session
        .store
        .symbols
        .get(num_unapply)
        .owner
        .unwrap();
    foreign_owner_session
        .store
        .symbols
        .get_mut(add_unapply)
        .owner = Some(fake_owner);
    foreign_owner_session.with_typer(|ast, _index, typer, _definitions| {
        let root = find_method_tree(ast, typer.store(), "peelLeft");
        let error = typer
            .type_method_definition(root)
            .expect_err("nested Add rejects an unapply with a foreign owner");
        assert!(
            matches!(
                error,
                dotty_typer::TyperError::SymbolSourceKindMismatch { .. }
            ),
            "fake-owner control reports the original identity mismatch: {error:?}"
        );
    });
}

fn type_nested_peel_root(source_text: &str, source: SourceId) -> Result<(), String> {
    let mut session = BinarySession::new_with_source(source, source_text);
    session.with_typer(|ast, _index, typer, _definitions| {
        let root = find_method_tree(ast, typer.store(), "peelLeft");
        typer
            .type_method_definition(root)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    })
}

fn find_method_tree(
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    store: &SemanticStore,
    name: &str,
) -> dotty_core::TreeId<dotty_core::ast::Untyped> {
    ast.iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == name).then_some(tree)
        })
        .unwrap_or_else(|| panic!("source contains method {name}"))
}

fn nested_class_symbols(
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: &SourceSemanticIndex,
    typer: &dotty_typer::SourceTyper<'_>,
    source: SourceId,
) -> (
    std::collections::BTreeMap<String, dotty_core::SymbolId>,
    dotty_core::SymbolId,
) {
    let mut classes = std::collections::BTreeMap::new();
    for (tree, node) in ast.iter() {
        if let TreeKind::TypeDef(definition) = &node.kind {
            let name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text());
            if matches!(name, "Expr" | "Num" | "Add") {
                classes.insert(name.to_owned(), index.symbol_at(source, tree).unwrap());
            }
        }
    }
    let expr = classes["Expr"];
    (classes, expr)
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
    let first_source = SourceId::from_index(20761);
    let second_source = SourceId::from_index(20762);
    assert_ne!(
        first_source, second_source,
        "fresh sessions have disjoint source identities"
    );
    let first = select_left_receipt(first_source);
    let second = select_left_receipt(second_source);
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
    let mut session = BinarySession::new(source);
    session
        .with_typer(|ast, index, typer, _definitions| select_left_proof(source, ast, index, typer))
}

fn select_left_proof(
    source: SourceId,
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: &SourceSemanticIndex,
    typer: &mut dotty_typer::SourceTyper<'_>,
) -> String {
    let mut method = None;
    let mut classes = std::collections::BTreeMap::new();
    for (tree, node) in ast.iter() {
        match &node.kind {
            TreeKind::DefDef(definition)
                if typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == "selectLeft" =>
            {
                method = Some((tree, definition.clone(), node.position.unwrap()));
            }
            TreeKind::TypeDef(definition) => {
                let name = typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text());
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
    assert_eq!(
        method_tree.index(),
        39,
        "the immutable #2061 source tree is preserved"
    );
    let expr = classes["Expr"].1;
    let num = classes["Num"].1;
    let add = classes["Add"].1;
    assert_eq!(typer.store().symbols.get(expr).kind, SymbolKind::Trait);
    assert!(
        typer
            .store()
            .symbols
            .get(expr)
            .flags
            .contains(SymbolFlags::SEALED)
    );
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
        assert_eq!(typer.store().names.resolve(parent.name.text()), "Expr");
        assert_eq!(typer.store().symbols.get(symbol).kind, SymbolKind::Class);
        assert!(
            matches!(typer.store().symbols.get(symbol).origin, SymbolOrigin::Source(id) if id == source)
        );
    }

    let method_symbol = index.symbol_at(source, method_tree).unwrap();
    let parameter_tree = source_method.value_param_clauses[0][0];
    let parameter_symbol = index.symbol_at(source, parameter_tree).unwrap();
    assert_ne!(method_symbol, parameter_symbol);
    assert_eq!(
        typer.store().symbols.get(parameter_symbol).owner,
        Some(method_symbol)
    );
    assert!(
        matches!(typer.store().symbols.get(method_symbol).origin, SymbolOrigin::Source(id) if id == source)
    );
    assert!(
        matches!(typer.store().symbols.get(parameter_symbol).origin, SymbolOrigin::Source(id) if id == source)
    );
    let method_owner = typer.store().symbols.get(method_symbol).owner.unwrap();
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(method_owner).name.text()),
        "Evaluator$"
    );

    assert_product2_parent(typer, index, add, expr);
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
    assert_eq!(
        mapped, 19,
        "every original selectLeft source node is mapped"
    );
    format!(
        "root=selectLeft@{}:{};signature=(Expr):Expr;selector=expr:Expr;cases={};Add.unapply=companion-owned(Add):Add;selectors=_1(left):Expr,_2(right):Expr;binder=left@Add._1:case-local;wildcard=Add._2:unbound;mapped_source_trees={mapped}",
        range.start(),
        range.end(),
        normalized_cases.join(",")
    )
}

#[test]
fn binary_recursive_eval_preserves_two_independent_add_binders_and_calls() {
    let first_source = SourceId::from_index(20781);
    let second_source = SourceId::from_index(20782);
    assert_ne!(
        first_source, second_source,
        "fresh sessions have disjoint source identities"
    );
    let eval_first = binary_eval_receipt(first_source, false);
    let select_left_first = binary_eval_receipt(second_source, true);
    assert_eq!(eval_first, select_left_first);
    assert_eq!(
        format!(
            "session-a\tfresh-session-a\t{eval_first}\nsession-b\tfresh-session-b\t{select_left_first}\n"
        ),
        include_str!(
            "fixtures/source-binary-recursive-adt-3.9.0/rust-eval-two-call-before-after.tsv"
        )
        .split_once('\n')
        .expect("two-call receipt header")
        .1
        .split_once('\n')
        .expect("before row")
        .1
    );
    println!("ISSUE2078_BINARY_EVAL_PROOF\t{eval_first}");
}

fn binary_eval_receipt(source: SourceId, select_left_first: bool) -> String {
    let mut session = BinarySession::new(source);
    session.with_typer(|ast, index, typer, definitions| {
        binary_eval_proof(source, ast, index, typer, definitions, select_left_first)
    })
}

fn binary_eval_proof(
    source: SourceId,
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: &SourceSemanticIndex,
    typer: &mut dotty_typer::SourceTyper<'_>,
    definitions: Definitions,
    select_left_first: bool,
) -> String {
    let mut roots = std::collections::BTreeMap::new();
    let mut classes = std::collections::BTreeMap::new();
    for (tree, node) in ast.iter() {
        match &node.kind {
            TreeKind::DefDef(definition) => {
                let name = typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text());
                if matches!(name, "eval" | "selectLeft") {
                    roots.insert(
                        name.to_owned(),
                        (tree, index.symbol_at(source, tree).unwrap()),
                    );
                }
            }
            TreeKind::TypeDef(definition) => {
                let name = typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text());
                if matches!(name, "Expr" | "Add") {
                    classes.insert(name.to_owned(), index.symbol_at(source, tree).unwrap());
                }
            }
            _ => {}
        }
    }
    let (eval_tree, eval_symbol) = roots["eval"];
    assert_eq!(
        eval_tree.index(),
        64,
        "the immutable #2061 source tree is preserved"
    );
    let (select_tree, _) = roots["selectLeft"];
    let expr = classes["Expr"];
    let add = classes["Add"];
    let int = match typer.store().types.get(definitions.int) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("canonical Int type is a TypeRef: {other:?}"),
    };
    let TreeKind::DefDef(source_method) = &ast.get(eval_tree).kind else {
        unreachable!()
    };
    let parameter_tree = source_method.value_param_clauses[0][0];
    let parameter = index.symbol_at(source, parameter_tree).unwrap();
    let owner = typer.store().symbols.get(eval_symbol).owner.unwrap();
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(owner).name.text()),
        "Evaluator$"
    );
    assert_eq!(
        typer.store().symbols.get(parameter).owner,
        Some(eval_symbol)
    );
    assert_product2_parent(typer, index, add, expr);
    if select_left_first {
        typer
            .type_method_definition(select_tree)
            .expect("the existing selectLeft root remains complete");
    }
    let typed_root = typer
        .type_method_definition(eval_tree)
        .expect("the original binary recursive eval root completes");
    assert_eq!(
        typer
            .type_method_definition(eval_tree)
            .expect("retyping eval is idempotent"),
        typed_root
    );
    let TreeKind::DefDef(typed_method) = &typer.typed_ast().get(typed_root).kind else {
        panic!("the original eval root remains a complete DefDef")
    };
    assert_eq!(
        typer.source_typed_index().get(source, eval_tree),
        Some(typed_root)
    );
    assert_eq!(
        typer.source_typed_index().get(source, parameter_tree),
        Some(typed_method.value_param_clauses[0][0])
    );
    assert_eq!(
        typer.source_typed_index().get(source, source_method.tpt),
        Some(typed_method.tpt)
    );
    let SymbolInfo::Complete(signature_id) = *typer.store().symbols.info(eval_symbol) else {
        panic!("eval's explicit result signature is completed before its RHS")
    };
    let Type::Method(signature) = typer.store().types.get(signature_id) else {
        panic!("eval has a method signature")
    };
    assert_eq!(signature.kind, MethodKind::Plain);
    assert_eq!(signature.params.len(), 1);
    assert_type_ref(typer.store(), signature.params[0].ty, expr);
    assert_type_ref(typer.store(), signature.result, int);

    let source_rhs = source_method.rhs.unwrap();
    let TreeKind::Match(source_match) = &ast.get(source_rhs).kind else {
        panic!("eval's original RHS is a Match")
    };
    let typed_match_tree = typer.source_typed_index().get(source, source_rhs).unwrap();
    assert_eq!(typed_method.rhs, Some(typed_match_tree));
    let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_match_tree).kind else {
        panic!("the complete typed RHS remains a Match")
    };
    assert_eq!(source_match.cases.len(), 2);
    assert_eq!(typed_match.cases.len(), 2);
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_match_tree).ty,
        int,
    );

    let TreeKind::CaseDef(source_num_case) = &ast.get(source_match.cases[0]).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(typed_num_case) = &typer.typed_ast().get(typed_match.cases[0]).kind
    else {
        unreachable!()
    };
    assert_eq!(
        typer
            .source_typed_index()
            .get(source, source_match.cases[0]),
        Some(typed_match.cases[0])
    );
    let TreeKind::Block(typed_num_body) = &typer.typed_ast().get(typed_num_case.body).kind else {
        panic!("Num case keeps its block")
    };
    let TreeKind::Block(source_num_body) = &ast.get(source_num_case.body).kind else {
        panic!("Num source case keeps its block")
    };
    assert_eq!(
        typer.source_typed_index().get(source, source_num_body.expr),
        Some(typed_num_body.expr)
    );
    let TreeKind::Apply(source_num_extractor) = &ast.get(source_num_case.pattern).kind else {
        panic!("Num pattern remains its original extractor")
    };
    let value_binder = typer
        .pattern_binding_symbol_at(source, source_num_extractor.args[0])
        .expect("Num(value) has its source-derived binder");
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_num_body.expr).ty,
        value_binder,
    );
    let SymbolInfo::Complete(value_type) = *typer.store().symbols.info(value_binder) else {
        panic!("Num.value binder is completed")
    };
    assert_type_ref(typer.store(), value_type, int);

    let TreeKind::CaseDef(source_add_case) = &ast.get(source_match.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(typed_add_case) = &typer.typed_ast().get(typed_match.cases[1]).kind
    else {
        unreachable!()
    };
    assert_eq!(
        typer
            .source_typed_index()
            .get(source, source_match.cases[1]),
        Some(typed_match.cases[1])
    );
    let TreeKind::Apply(source_extractor) = &ast.get(source_add_case.pattern).kind else {
        panic!("Add pattern remains the original two-field extractor")
    };
    assert_eq!(source_extractor.args.len(), 2);
    let TreeKind::UnApply(typed_extractor) = &typer.typed_ast().get(typed_add_case.pattern).kind
    else {
        panic!("Add pattern becomes the authentic UnApply")
    };
    assert_eq!(typed_extractor.patterns.len(), 2);
    let unapply = index
        .generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply)
        .expect("canonical source Add.unapply");
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_extractor.function).ty,
        unapply,
    );
    let SourceDefinition::Canonical {
        source: add_source,
        tree: add_tree,
    } = index.definition_of(add).unwrap()
    else {
        panic!("Add has its original source definition")
    };
    assert_eq!(add_source, source);
    let TreeKind::TypeDef(add_definition) = &ast.get(add_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(add_template) = &ast.get(add_definition.rhs).kind else {
        unreachable!()
    };
    let constructor_tree = add_template.constructor;
    let TreeKind::DefDef(constructor) = &ast.get(constructor_tree).kind else {
        unreachable!()
    };
    let fields = &constructor.value_param_clauses[0];
    assert_eq!(fields.len(), 2);
    assert_eq!(
        index.generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply),
        Some(unapply)
    );
    let mut binders = Vec::new();
    let mut selectors = Vec::new();
    for (position, field_tree) in fields.iter().copied().enumerate() {
        let source_binder_tree = source_extractor.args[position];
        let TreeKind::Ident(source_binder) = &ast.get(source_binder_tree).kind else {
            panic!("Add argument {position} is an original binder")
        };
        let expected_name = if position == 0 { "left" } else { "right" };
        assert_eq!(
            typer.store().names.resolve(source_binder.name.text()),
            expected_name
        );
        let binder = typer
            .pattern_binding_symbol_at(source, source_binder_tree)
            .unwrap();
        assert_ne!(binder, parameter);
        assert_eq!(typer.store().symbols.get(binder).owner, Some(eval_symbol));
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(binder).name.text()),
            expected_name
        );
        let SymbolInfo::Complete(binder_type) = *typer.store().symbols.info(binder) else {
            panic!("Add.{expected_name} binder has a completed source type")
        };
        assert_type_ref(typer.store(), binder_type, expr);
        assert_eq!(
            typer.source_typed_index().get(source, source_binder_tree),
            Some(typed_extractor.patterns[position])
        );
        let TreeKind::Bind(typed_binder) = &typer
            .typed_ast()
            .get(typed_extractor.patterns[position])
            .kind
        else {
            panic!("Add field {position} remains a typed binder")
        };
        assert_eq!(
            typer.store().names.resolve(typed_binder.name.text()),
            expected_name
        );
        assert_term_ref(
            typer.store(),
            typer.typed_ast().get(typed_extractor.patterns[position]).ty,
            binder,
        );
        let field = index.symbol_at(source, field_tree).unwrap();
        let TreeKind::ValDef(source_field) = &ast.get(field_tree).kind else {
            panic!("Add field {position} remains its original source parameter")
        };
        assert_eq!(
            typer
                .store()
                .names
                .resolve(source_field.name.as_name().text()),
            expected_name
        );
        let selector = index.derived_symbol_at(add, source, field_tree).unwrap();
        assert_eq!(typer.store().symbols.get(field).owner, Some(add));
        assert_eq!(typer.store().symbols.get(selector).owner, Some(add));
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(selector).name.text()),
            if position == 0 { "_1" } else { "_2" }
        );
        let SymbolInfo::Complete(selector_info) = *typer.store().symbols.info(selector) else {
            panic!("Product selector {position} has a completed signature")
        };
        let Type::Method(selector_type) = typer.store().types.get(selector_info) else {
            panic!("Product selector {position} has a method signature")
        };
        assert!(selector_type.params.is_empty());
        assert_type_ref(typer.store(), selector_type.result, expr);
        selectors.push(selector);
        binders.push(binder);
    }
    assert_ne!(
        binders[0], binders[1],
        "equal Expr types do not alias binder identity"
    );
    assert_ne!(selectors[0], selectors[1]);

    let TreeKind::Block(source_add_body) = &ast.get(source_add_case.body).kind else {
        panic!("Add result retains the source block")
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(source_addition)) =
        &ast.get(source_add_body.expr).kind
    else {
        panic!("Add body is the original left-to-right Int addition")
    };
    assert_eq!(typer.store().names.resolve(source_addition.op.text()), "+");
    let source_calls = [source_addition.left, source_addition.right];
    let mut source_arguments = Vec::new();
    let mut call_receipts = Vec::new();
    for (position, source_call) in source_calls.into_iter().enumerate() {
        let TreeKind::Apply(source_application) = &ast.get(source_call).kind else {
            panic!("recursive expression {position} is an original Apply")
        };
        assert_eq!(source_application.args.len(), 1);
        let source_function = source_application.function;
        let TreeKind::Ident(function_name) = &ast.get(source_function).kind else {
            panic!("recursive eval function is a source Ident")
        };
        assert_eq!(
            typer.store().names.resolve(function_name.name.text()),
            "eval"
        );
        let source_argument = source_application.args[0];
        source_arguments.push(source_argument);
        let TreeKind::Ident(argument_name) = &ast.get(source_argument).kind else {
            panic!("recursive argument is its source binder reference")
        };
        let expected_name = if position == 0 { "left" } else { "right" };
        assert_eq!(
            typer.store().names.resolve(argument_name.name.text()),
            expected_name
        );
        let typed_call = typer.source_typed_index().get(source, source_call).unwrap();
        let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed_call).kind else {
            panic!("recursive expression {position} remains an ordinary Apply")
        };
        assert_eq!(typed_application.args.len(), 1);
        let typed_function = typer
            .source_typed_index()
            .get(source, source_function)
            .unwrap();
        assert_eq!(typed_application.function, typed_function);
        assert_term_ref(
            typer.store(),
            typer.typed_ast().get(typed_function).ty,
            eval_symbol,
        );
        let typed_argument = typer
            .source_typed_index()
            .get(source, source_argument)
            .unwrap();
        assert_eq!(typed_application.args[0], typed_argument);
        assert_term_ref(
            typer.store(),
            typer.typed_ast().get(typed_argument).ty,
            binders[position],
        );
        assert_type_ref(typer.store(), typer.typed_ast().get(typed_call).ty, int);
        call_receipts.push(format!("eval({expected_name}):Int"));
    }
    assert_ne!(source_calls[0], source_calls[1]);
    assert_ne!(source_arguments[0], source_arguments[1]);
    assert_ne!(
        ast.iter()
            .find(|(tree, _)| *tree == source_calls[0])
            .unwrap()
            .1
            .position,
        ast.iter()
            .find(|(tree, _)| *tree == source_calls[1])
            .unwrap()
            .1
            .position
    );
    let typed_addition_tree = typer
        .source_typed_index()
        .get(source, source_add_body.expr)
        .unwrap();
    let TreeKind::Apply(typed_addition) = &typer.typed_ast().get(typed_addition_tree).kind else {
        panic!("typed + is an ordinary selected-method application")
    };
    assert_eq!(typed_addition.args.len(), 1);
    let right_call = typer
        .source_typed_index()
        .get(source, source_calls[1])
        .unwrap();
    assert_eq!(typed_addition.args[0], right_call);
    let TreeKind::Select(typed_plus) = &typer.typed_ast().get(typed_addition.function).kind else {
        panic!("Int + resolves through a selected method")
    };
    assert_eq!(
        typed_plus.qualifier,
        typer
            .source_typed_index()
            .get(source, source_calls[0])
            .unwrap()
    );
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_addition_tree).ty,
        int,
    );
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_match_tree).ty,
        int,
    );

    let range = ast.get(eval_tree).position.unwrap().span().range();
    let mut mapped = 0usize;
    for (tree, node) in ast.iter().filter(|(_, node)| {
        node.position.is_some_and(|position| {
            let span = position.span().range();
            span.start() >= range.start() && span.end() <= range.end() && span.start() < span.end()
        })
    }) {
        let typed = typer
            .source_typed_index()
            .get(source, tree)
            .unwrap_or_else(|| panic!("missing source mapping for {tree:?}"));
        assert_eq!(typer.typed_ast().get(typed).position, node.position);
        mapped += 1;
    }
    assert_eq!(mapped, 25, "every original eval source node is mapped");
    let _ = typer
        .type_method_definition(select_tree)
        .expect("selectLeft remains complete after eval");
    format!(
        "eval@{}:{};signature=(Expr):Int;Add.binders=left@_1:Expr,right@_2:Expr,distinct;calls={};callee=Evaluator.eval:same_source_symbol;addition=Int+Int:Int;match=Int;mappings={mapped}",
        range.start(),
        range.end(),
        call_receipts.join("+")
    )
}

#[test]
fn binary_recursive_adt_original_roots_share_two_real_sessions() {
    let mut sessions = [
        BinarySession::new(SourceId::from_index(20801)),
        BinarySession::new(SourceId::from_index(20802)),
    ];
    assert_ne!(
        std::ptr::addr_of!(sessions[0].store),
        std::ptr::addr_of!(sessions[1].store),
        "each session owns a distinct semantic store"
    );
    assert_ne!(
        std::ptr::addr_of!(sessions[0].ast),
        std::ptr::addr_of!(sessions[1].ast),
        "each session owns a distinct source AST"
    );
    assert_ne!(
        std::ptr::addr_of!(sessions[0].index),
        std::ptr::addr_of!(sessions[1].index),
        "each session owns a distinct source index"
    );
    let owners = sessions
        .each_ref()
        .map(|session| &*session.ownership_witness as *const u8 as usize);
    assert_ne!(
        owners[0], owners[1],
        "session ownership witnesses are disjoint"
    );

    let mut receipts = Vec::new();
    for (ordinal, session) in sessions.iter_mut().enumerate() {
        let label = if ordinal == 0 {
            "session-a"
        } else {
            "session-b"
        };
        let source = session.source;
        let owner = &*session.ownership_witness as *const u8;
        let store = &session.store as *const SemanticStore;
        let ast = &session.ast as *const dotty_core::ast::AstArena<dotty_core::ast::Untyped>;
        let index = &session.index as *const SourceSemanticIndex;
        println!(
            "ISSUE2080_SESSION\t{label}\towner={owner:p}\tstore={store:p}\tast={ast:p}\tindex={index:p}\tsource={:?}",
            session.source
        );
        let (select_left, eval) = session.with_typer(|ast, index, typer, definitions| {
            let select_left = select_left_proof(source, ast, index, typer);
            let eval = binary_eval_proof(source, ast, index, typer, definitions, false);
            (select_left, eval)
        });
        receipts.push((select_left, eval));
    }
    assert_eq!(
        receipts[0].0, receipts[1].0,
        "selectLeft normalizes across sessions"
    );
    assert_eq!(
        receipts[0].1, receipts[1].1,
        "eval normalizes across sessions"
    );

    for (ordinal, (select_left, eval)) in receipts.into_iter().enumerate() {
        let label = if ordinal == 0 {
            "session-a"
        } else {
            "session-b"
        };
        let select_proof = format!(
            "{select_left};Expr=source-sealed-trait;Num/Add=source-case-class;Add.parents=Expr+scala.Product;Add.fields=left/right:Expr,distinct,class-owned;Add.selectors=_1/_2:Expr,distinct,class-owned;no=_3"
        );
        let eval = eval.replacen("eval@", "root=eval@", 1);
        let eval_proof = format!(
            "{eval};Expr=source-sealed-trait;Num/Add=source-case-class;Add.parents=Expr+scala.Product;Add.fields=left/right:Expr,distinct,class-owned;Add.selectors=_1/_2:Expr,distinct,class-owned;Add.unapply=companion-owned(Add):Add;no=_3"
        );
        println!(
            "ISSUE2080_POSITIVE\t{label}\tselectLeft\tExpr.scala#tree39\t158:271\t{select_proof}"
        );
        println!("ISSUE2080_POSITIVE\t{label}\teval\tExpr.scala#tree64\t275:410\t{eval_proof}");
    }
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
fn nested_peelleft_late_failure_rolls_back_and_original_retries() {
    let source_text = format!(
        "{NESTED_SOURCE}\nobject BrokenEvaluator:\n  def nestedBad(expr: Expr): Int =\n    expr match\n      case Add(Add(left, _), _) => left\n      case _ => 0\n"
    );
    let source = SourceId::from_index(21331);
    let mut session = BinarySession::new_with_source(source, &source_text);
    session.with_typer(|ast, index, typer, _definitions| {
        let method_named = |name: &str| {
            ast.iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    (typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == name)
                        .then_some(tree)
                })
                .unwrap_or_else(|| panic!("source contains method {name}"))
        };
        let broken_method = method_named("nestedBad");
        let peel_method = method_named("peelLeft");
        let TreeKind::DefDef(broken_definition) = &ast.get(broken_method).kind else {
            unreachable!()
        };
        let TreeKind::Match(broken_match) = &ast.get(broken_definition.rhs.unwrap()).kind else {
            panic!("late-failure method has a Match RHS")
        };
        let TreeKind::CaseDef(broken_case) = &ast.get(broken_match.cases[0]).kind else {
            unreachable!()
        };
        let TreeKind::Apply(outer) = &ast.get(broken_case.pattern).kind else {
            unreachable!()
        };
        let nested_pattern = outer.args[0];
        assert!(matches!(ast.get(nested_pattern).kind, TreeKind::Apply(_)));
        let TreeKind::Apply(inner) = &ast.get(nested_pattern).kind else {
            unreachable!()
        };
        let failed_result = match &ast.get(broken_case.body).kind {
            TreeKind::Block(block) => block.expr,
            _ => broken_case.body,
        };
        let TreeKind::Ident(result_name) = &ast.get(failed_result).kind else {
            panic!("the late type mismatch is the nested left result")
        };
        assert_eq!(typer.store().names.resolve(result_name.name.text()), "left");

        let mappings_before = typer.source_typed_index().len();
        let typed_nodes_before = typer.typed_ast().iter().count();
        let store_before = typer.store().checkpoint();
        let first_error = typer
            .type_method_definition(broken_method)
            .expect_err("Expr result cannot satisfy the declared Int result");
        let first_diagnostic = format!("{first_error:?}");
        assert!(
            first_diagnostic.starts_with("ExpectedExpressionTypeMismatch"),
            "nested patterns type before the result mismatch: {first_diagnostic}"
        );
        let diagnostic_tree = parse_tree_index(&first_diagnostic).unwrap();
        assert_eq!(
            diagnostic_tree,
            broken_definition.rhs.unwrap().index(),
            "the late mismatch is reported at the completed nested Match"
        );
        assert_eq!(typer.store().checkpoint(), store_before);
        assert_eq!(typer.source_typed_index().len(), mappings_before);
        assert_eq!(typer.typed_ast().iter().count(), typed_nodes_before);
        for source_tree in [
            broken_method,
            broken_definition.rhs.unwrap(),
            broken_match.cases[0],
            broken_case.pattern,
            nested_pattern,
            inner.args[0],
            failed_result,
        ] {
            assert_eq!(
                typer.source_typed_index().get(source, source_tree),
                None,
                "failed nested root publishes no partial source mapping"
            );
        }

        let retry_error = typer
            .type_method_definition(broken_method)
            .expect_err("the malformed nested root remains invalid on retry");
        assert_eq!(format!("{retry_error:?}"), first_diagnostic);
        assert_eq!(typer.store().checkpoint(), store_before);
        assert_eq!(typer.source_typed_index().len(), mappings_before);
        assert_eq!(typer.typed_ast().iter().count(), typed_nodes_before);

        let typed_peel = typer
            .type_method_definition(peel_method)
            .expect("the original nested peelLeft root retries in the same session");
        assert_eq!(
            typer.source_typed_index().get(source, peel_method),
            Some(typed_peel)
        );
        let TreeKind::DefDef(peel_definition) = &ast.get(peel_method).kind else {
            unreachable!()
        };
        let peel_rhs = peel_definition.rhs.unwrap();
        let TreeKind::Match(peel_match) = &ast.get(peel_rhs).kind else {
            unreachable!()
        };
        let TreeKind::CaseDef(peel_case) = &ast.get(peel_match.cases[0]).kind else {
            unreachable!()
        };
        let TreeKind::Apply(outer) = &ast.get(peel_case.pattern).kind else {
            unreachable!()
        };
        let TreeKind::Apply(inner) = &ast.get(outer.args[0]).kind else {
            unreachable!()
        };
        let binder = index
            .symbol_at(source, inner.args[0])
            .or_else(|| typer.pattern_binding_symbol_at(source, inner.args[0]))
            .expect("same-session retry restores the case-local left binder");
        let returned = match &ast.get(peel_case.body).kind {
            TreeKind::Block(block) => block.expr,
            _ => peel_case.body,
        };
        let typed_return = typer
            .source_typed_index()
            .get(source, returned)
            .expect("same-session retry maps the returned binder reference");
        assert_term_ref(
            typer.store(),
            typer.typed_ast().get(typed_return).ty,
            binder,
        );
        let peel_symbol = index.symbol_at(source, peel_method).unwrap();
        let SymbolInfo::Complete(signature) = *typer.store().symbols.info(peel_symbol) else {
            panic!("successful retry publishes the original peelLeft signature")
        };
        let Type::Method(signature) = typer.store().types.get(signature) else {
            panic!("peelLeft signature remains a method")
        };
        assert_eq!(signature.params.len(), 1);
        let expr = ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::TypeDef(definition) = &node.kind else {
                    return None;
                };
                (typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Expr")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .expect("the original Expr source trait remains available");
        assert_type_ref(typer.store(), signature.params[0].ty, expr);
        assert_type_ref(typer.store(), signature.result, expr);
    });
}

#[test]
fn corrupted_reference_product2_identities_reject_then_retry_original_select_left() {
    #[derive(Clone, Copy, Debug)]
    enum Fault {
        SwappedSourceProvenance,
        SwappedScopeSelectors,
        AliasedScopeSelectors,
        ForeignUnapplyOwner,
        ForeignCompanionLink,
        WrongParent,
        BrokenProductIdentity,
        ForeignSelectorOwner,
    }
    let faults = [
        Fault::SwappedSourceProvenance,
        Fault::SwappedScopeSelectors,
        Fault::AliasedScopeSelectors,
        Fault::ForeignUnapplyOwner,
        Fault::ForeignCompanionLink,
        Fault::WrongParent,
        Fault::BrokenProductIdentity,
        Fault::ForeignSelectorOwner,
    ];
    let mut receipts = Vec::new();
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
            Fault::AliasedScopeSelectors => {
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
            let before_store_contents = store_content_projection(typer.store());
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
            assert_store_content_projection(
                &before_store_contents,
                &store_content_projection(typer.store()),
                offset,
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
            | Fault::AliasedScopeSelectors
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
        let method_span = ast
            .get(method_tree.unwrap())
            .position
            .unwrap()
            .span()
            .range();
        let receipt = format!(
            "{fault:?}\tselectLeft@{}..{}\t{error:?}\trollback=complete,negative_retry=stable",
            method_span.start(),
            method_span.end()
        );
        println!("ISSUE2079_STRUCTURAL_CONTROL\t{receipt}");
        receipts.push(receipt);

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
            Fault::AliasedScopeSelectors => {
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
    assert_eq!(receipts.len(), 8);
    let header = STRUCTURAL_CONTROL_RECEIPTS
        .lines()
        .next()
        .expect("structural control receipt header");
    assert_eq!(
        format!("{header}\n{}\n", receipts.join("\n")),
        STRUCTURAL_CONTROL_RECEIPTS
    );
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

fn store_content_projection(store: &SemanticStore) -> Vec<(&'static str, String)> {
    vec![
        ("symbols", format!("{:?}", store.symbols)),
        ("types", format!("{:?}", store.types)),
        ("scopes", format!("{:?}", store.scopes)),
        ("annotations", format!("{:?}", store.annotations)),
    ]
}

fn assert_store_content_projection(
    before: &[(&'static str, String)],
    after: &[(&'static str, String)],
    fault: usize,
) {
    let changed = before
        .iter()
        .zip(after)
        .filter_map(|((name, before), (_, after))| (before != after).then_some(*name))
        .collect::<Vec<_>>();
    assert!(
        changed.is_empty(),
        "{fault}: complete semantic-store state rolls back; changed {changed:?}"
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
    semantic_receipt: String,
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
    setup_with_source(source, SOURCE)
}

fn setup_with_source(
    source: SourceId,
    source_text: &str,
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
        SourceText::new(source_text).expect("frozen source is valid"),
        source,
        ContextualScanner::new(source_text).expect("frozen source scans"),
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

struct BinarySession {
    source: SourceId,
    ownership_witness: Box<u8>,
    ast: dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: SourceSemanticIndex,
    store: SemanticStore,
    definitions: Definitions,
    packages: Packages,
}

impl BinarySession {
    fn new(source: SourceId) -> Self {
        Self::new_with_source(source, SOURCE)
    }

    fn new_with_source(source: SourceId, source_text: &str) -> Self {
        let (ast, _root, index, store, definitions, packages) =
            setup_with_source(source, source_text);
        Self {
            source,
            ownership_witness: Box::new(0),
            ast,
            index,
            store,
            definitions,
            packages,
        }
    }

    fn with_typer<R>(
        &mut self,
        f: impl FnOnce(
            &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
            &SourceSemanticIndex,
            &mut dotty_typer::SourceTyper<'_>,
            Definitions,
        ) -> R,
    ) -> R {
        let definitions = self.definitions;
        let resolver = resolver(definitions);
        let mut typer = dotty_typer::SourceTyper::new(
            &self.ast,
            self.source,
            &self.index,
            &mut self.store,
            definitions,
            &self.packages,
        )
        .with_resolver(Box::new(resolver));
        f(&self.ast, &self.index, &mut typer, definitions)
    }
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

fn assert_product2_parent(
    typer: &mut dotty_typer::SourceTyper<'_>,
    index: &SourceSemanticIndex,
    add: dotty_core::SymbolId,
    expr: dotty_core::SymbolId,
) {
    let add_info = typer
        .complete_symbol(add)
        .expect("source Add completes with its canonical Scala Product parent");
    let Type::ClassInfo(class_info) = typer.store().types.get(add_info) else {
        panic!("Add has a completed class info")
    };
    let parents = class_info
        .parents
        .iter()
        .map(|parent| match typer.store().types.get(*parent) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => (
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(*symbol).name.text())
                    .to_owned(),
                *symbol,
            ),
            other => panic!("Add parent is a canonical class symbol: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parents
            .iter()
            .filter(|(parent, _)| parent == "Expr")
            .count(),
        1
    );
    assert_eq!(
        parents
            .iter()
            .filter(|(parent, _)| parent == "Product")
            .count(),
        1
    );
    assert_eq!(
        parents
            .iter()
            .find(|(parent, _)| parent == "Expr")
            .unwrap()
            .1,
        expr
    );
    let product = parents
        .iter()
        .find(|(parent, _)| parent == "Product")
        .unwrap()
        .1;
    assert_eq!(typer.store().symbols.get(product).kind, SymbolKind::Trait);
    let scala_package = typer.store().symbols.get(product).owner.unwrap();
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(scala_package).name.text()),
        "scala"
    );
    assert!(
        !parents
            .iter()
            .any(|(parent, _)| matches!(parent.as_str(), "Option" | "Tuple2"))
    );
    let add_scope = index.scope_of(add).unwrap();
    assert!(
        typer
            .store()
            .scopes
            .get(add_scope)
            .entered_symbols()
            .all(|symbol| {
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(symbol).name.text())
                    != "_3"
            })
    );
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
        semantic_receipt: String::new(),
    }
}

fn nested_roots(source: SourceId, session: &str) -> RootReceipt {
    let mut rust_session = BinarySession::new_with_source(source, NESTED_SOURCE);
    rust_session.with_typer(|ast, index, typer, definitions| {
        let mut roots = Vec::new();
        for (tree, node) in ast.iter() {
            let TreeKind::DefDef(definition) = &node.kind else {
                continue;
            };
            let name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text());
            if !matches!(name, "peelLeft" | "sum") {
                continue;
            }
            let method = index
                .symbol_at(source, tree)
                .expect("source method identity");
            let parameter_tree = definition.value_param_clauses[0][0];
            let parameter = index
                .symbol_at(source, parameter_tree)
                .expect("source parameter identity");
            let owner = typer
                .store()
                .symbols
                .get(method)
                .owner
                .expect("method owner");
            assert_eq!(typer.store().symbols.get(parameter).owner, Some(method));
            assert_ne!(method, parameter);
            assert_eq!(
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(owner).name.text()),
                "Evaluator$"
            );
            assert!(matches!(
                typer.store().symbols.get(method).origin,
                SymbolOrigin::Source(id) if id == source
            ));
            assert!(matches!(
                typer.store().symbols.get(parameter).origin,
                SymbolOrigin::Source(id) if id == source
            ));
            roots.push((name.to_owned(), tree, node.position.unwrap()));
        }
        roots.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            roots.len(),
            2,
            "only the two requested source DefDefs count"
        );

        let mut rows = Vec::new();
        let mut normalized = Vec::new();
        let mut typed_roots = std::collections::BTreeMap::new();
        for (name, tree, position) in roots {
            let range = position.span().range();
            let outcome = match typer.type_method_definition(tree) {
                Ok(typed) => {
                    assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                    assert!(matches!(
                        typer.typed_ast().get(typed).kind,
                        TreeKind::DefDef(_)
                    ));
                    typed_roots.insert(name.clone(), (tree, typed));
                    "complete=whole_DefDef;typed_index=present".to_owned()
                }
                Err(error) => {
                    assert!(typer.source_typed_index().get(source, tree).is_none());
                    let debug = format!("{error:?}");
                    let error_kind = debug.split(" {").next().unwrap_or(&debug);
                    let nested_tree = parse_tree_index(&debug);
                    let nested = nested_tree
                        .and_then(|number| {
                            ast.iter()
                                .find(|(candidate, _)| candidate.index() == number)
                        })
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
                "{session}\t{name}\tExpr.scala#tree{}\t{}:{}\tEvaluator$\t{outcome}",
                tree.index(),
                range.start(),
                range.end(),
            ));
            normalized.push(format!(
                "{name}\tExpr.scala#tree{}\t{}:{}\tEvaluator$\t{outcome}",
                tree.index(),
                range.start(),
                range.end(),
            ));
        }
        let semantic_receipt =
            assert_nested_typed_shapes(source, ast, index, typer, definitions, &typed_roots);
        RootReceipt {
            roots: rows,
            normalized,
            semantic_receipt,
        }
    })
}

fn assert_nested_typed_shapes(
    source: SourceId,
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: &SourceSemanticIndex,
    typer: &mut dotty_typer::SourceTyper<'_>,
    definitions: Definitions,
    roots: &std::collections::BTreeMap<
        String,
        (
            dotty_core::TreeId<dotty_core::ast::Untyped>,
            dotty_core::TreeId<dotty_core::ast::Typed>,
        ),
    >,
) -> String {
    let mut classes = std::collections::BTreeMap::new();
    for (tree, node) in ast.iter() {
        if let TreeKind::TypeDef(definition) = &node.kind {
            let name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text());
            if matches!(name, "Expr" | "Num" | "Add") {
                classes.insert(name.to_owned(), index.symbol_at(source, tree).unwrap());
            }
        }
    }
    let expr = classes["Expr"];
    let num = classes["Num"];
    let add = classes["Add"];
    assert_product2_parent(typer, index, add, expr);
    let int = definitions_int(typer.store(), definitions);
    assert_source_product_fields(typer, ast, index, source, num, &["value"], int);
    assert_source_product_fields(typer, ast, index, source, add, &["left", "right"], expr);

    let peel_left = roots["peelLeft"];
    let TreeKind::DefDef(peel_source) = &ast.get(peel_left.0).kind else {
        unreachable!()
    };
    let TreeKind::Match(peel_match) = &ast.get(peel_source.rhs.unwrap()).kind else {
        panic!("peelLeft's original RHS is a Match")
    };
    assert_eq!(peel_match.cases.len(), 2);
    let TreeKind::CaseDef(peel_case) = &ast.get(peel_match.cases[0]).kind else {
        unreachable!()
    };
    let peel_typed_root = peel_left.1;
    let TreeKind::DefDef(peel_typed_method) = &typer.typed_ast().get(peel_typed_root).kind else {
        unreachable!()
    };
    let peel_typed_method = peel_typed_method.clone();
    let peel_typed_match = typer
        .source_typed_index()
        .get(source, peel_source.rhs.unwrap())
        .unwrap();
    let TreeKind::Match(peel_typed_match_node) = &typer.typed_ast().get(peel_typed_match).kind
    else {
        panic!("peelLeft's complete typed RHS remains a Match")
    };
    let peel_typed_cases = peel_typed_match_node.cases.clone();
    assert_eq!(peel_typed_cases.len(), 2);
    assert_eq!(
        typer
            .source_typed_index()
            .get(source, peel_source.rhs.unwrap()),
        Some(peel_typed_match),
        "the whole source Match maps to its typed replacement"
    );
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(peel_typed_match).ty,
        expr,
    );
    let typed_peel_result = typer
        .source_typed_index()
        .get(source, peel_source.tpt)
        .unwrap();
    assert_eq!(peel_typed_method.tpt, typed_peel_result);
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_peel_result).ty,
        expr,
    );
    let peel_symbol = index.symbol_at(source, peel_left.0).unwrap();
    let SymbolInfo::Complete(peel_signature) = *typer.store().symbols.info(peel_symbol) else {
        panic!("peelLeft's complete signature is published")
    };
    let Type::Method(peel_signature) = typer.store().types.get(peel_signature) else {
        panic!("peelLeft's signature is a method type")
    };
    assert_eq!(peel_signature.params.len(), 1);
    assert_type_ref(typer.store(), peel_signature.params[0].ty, expr);
    assert_type_ref(typer.store(), peel_signature.result, expr);
    let TreeKind::CaseDef(peel_typed_case) = &typer.typed_ast().get(peel_typed_cases[0]).kind
    else {
        unreachable!()
    };
    assert_eq!(
        typer.source_typed_index().get(source, peel_match.cases[0]),
        Some(peel_typed_cases[0]),
        "the nested-pattern CaseDef has a complete typed mapping"
    );
    let peel_parameter = index
        .symbol_at(source, peel_source.value_param_clauses[0][0])
        .unwrap();
    let SymbolInfo::Complete(peel_parameter_type) = *typer.store().symbols.info(peel_parameter)
    else {
        panic!("peelLeft source parameter type is complete")
    };
    assert_eq!(
        typer
            .typed_ast()
            .get(peel_typed_method.value_param_clauses[0][0])
            .ty,
        peel_parameter_type
    );
    assert_pattern_projection(
        source,
        ast,
        index,
        typer,
        peel_case.pattern,
        peel_typed_case.pattern,
        &classes,
    );
    let TreeKind::Apply(peel_outer) = &ast.get(peel_case.pattern).kind else {
        panic!("peelLeft uses an outer Add extractor")
    };
    let TreeKind::Apply(peel_inner) = &ast.get(peel_outer.args[0]).kind else {
        panic!("peelLeft nests Add inside Add")
    };
    let typed_outer = typer
        .source_typed_index()
        .get(source, peel_case.pattern)
        .expect("outer Add pattern has a typed mapping");
    let typed_inner = typer
        .source_typed_index()
        .get(source, peel_outer.args[0])
        .expect("inner Add pattern has a typed mapping");
    assert_ne!(peel_case.pattern, peel_outer.args[0]);
    assert_ne!(
        typed_outer, typed_inner,
        "nested Add occurrences stay distinct"
    );
    for (source_tree, typed_tree) in [
        (peel_case.pattern, typed_outer),
        (peel_outer.args[0], typed_inner),
    ] {
        let source_span = ast.get(source_tree).position.unwrap().span().range();
        let typed_span = typer
            .typed_ast()
            .get(typed_tree)
            .position
            .unwrap()
            .span()
            .range();
        assert_eq!(
            source_span, typed_span,
            "extractor occurrence span is preserved"
        );
    }
    let TreeKind::UnApply(typed_outer_unapply) = &typer.typed_ast().get(typed_outer).kind else {
        panic!("outer Add source pattern maps to UnApply")
    };
    let TreeKind::UnApply(typed_inner_unapply) = &typer.typed_ast().get(typed_inner).kind else {
        panic!("inner Add source pattern maps to UnApply")
    };
    assert_ne!(typed_outer_unapply.function, typed_inner_unapply.function);
    let add_unapply = index
        .generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply)
        .expect("Add has one canonical generated unapply");
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_outer_unapply.function).ty,
        add_unapply,
    );
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_inner_unapply.function).ty,
        add_unapply,
    );
    let source_outer_unapply = typer
        .source_typed_index()
        .get(source, peel_outer.function)
        .expect("outer extractor function has a typed mapping");
    let source_inner_unapply = typer
        .source_typed_index()
        .get(source, peel_inner.function)
        .expect("inner extractor function has a typed mapping");
    for (unapply_function, source_qualifier) in [
        (typed_outer_unapply.function, source_outer_unapply),
        (typed_inner_unapply.function, source_inner_unapply),
    ] {
        let TreeKind::Select(selection) = &typer.typed_ast().get(unapply_function).kind else {
            panic!("typed extractor function is a companion unapply selection")
        };
        assert_eq!(selection.qualifier, source_qualifier);
        assert_eq!(
            typer.store().names.resolve(selection.name.text()),
            "unapply"
        );
    }
    assert_eq!(typed_outer_unapply.patterns.len(), 2);
    assert_eq!(typed_inner_unapply.patterns.len(), 2);
    for (source_apply, typed_apply, typed_unapply) in [
        (peel_case.pattern, typed_outer, typed_outer_unapply),
        (peel_outer.args[0], typed_inner, typed_inner_unapply),
    ] {
        assert_eq!(
            typer.source_typed_index().get(source, source_apply),
            Some(typed_apply)
        );
        let TreeKind::Apply(source_application) = &ast.get(source_apply).kind else {
            unreachable!()
        };
        assert_eq!(source_application.args.len(), 2);
        for (source_argument, typed_argument) in
            source_application.args.iter().zip(&typed_unapply.patterns)
        {
            assert_eq!(
                typer.source_typed_index().get(source, *source_argument),
                Some(*typed_argument),
                "each positional extractor argument preserves its source mapping"
            );
        }
    }
    let TreeKind::Ident(peel_binder) = &ast.get(peel_inner.args[0]).kind else {
        panic!("the nested Add binds left")
    };
    assert_eq!(typer.store().names.resolve(peel_binder.name.text()), "left");
    let peel_binder_symbol = typer
        .pattern_binding_symbol_at(source, peel_inner.args[0])
        .unwrap();
    assert_eq!(
        typer.store().symbols.get(peel_binder_symbol).owner,
        Some(index.symbol_at(source, peel_left.0).unwrap())
    );
    let SymbolInfo::Complete(peel_binder_type) = *typer.store().symbols.info(peel_binder_symbol)
    else {
        panic!("nested peelLeft binder has a complete semantic type")
    };
    assert_type_ref(typer.store(), peel_binder_type, expr);
    let source_return = match &ast.get(peel_case.body).kind {
        TreeKind::Block(block) => block.expr,
        _ => peel_case.body,
    };
    let typed_return = typer
        .source_typed_index()
        .get(source, source_return)
        .expect("peelLeft returns a mapped source expression");
    let TreeKind::Ident(typed_return_name) = &typer.typed_ast().get(typed_return).kind else {
        panic!("the returned value remains the exact bound identifier")
    };
    assert_eq!(
        typer.store().names.resolve(typed_return_name.name.text()),
        "left"
    );
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_return).ty,
        peel_binder_symbol,
    );
    assert!(
        typer
            .pattern_binding_symbol_at(source, peel_outer.args[1])
            .is_none(),
        "outer wildcard does not enter the binder scope"
    );
    assert!(
        typer
            .pattern_binding_symbol_at(source, peel_inner.args[1])
            .is_none(),
        "inner wildcard does not enter the binder scope"
    );
    let TreeKind::CaseDef(fallback_case) = &ast.get(peel_match.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(typed_fallback_case) = &typer.typed_ast().get(peel_typed_cases[1]).kind
    else {
        unreachable!()
    };
    assert_eq!(
        typer.source_typed_index().get(source, peel_match.cases[1]),
        Some(peel_typed_cases[1]),
        "the fallback CaseDef has a complete typed mapping"
    );
    let fallback_pattern = typer
        .source_typed_index()
        .get(source, fallback_case.pattern)
        .expect("fallback pattern has a typed mapping");
    assert_eq!(fallback_pattern, typed_fallback_case.pattern);
    assert!(
        typer
            .pattern_binding_symbol_at(source, fallback_case.pattern)
            .is_none(),
        "fallback wildcard does not inherit the first case binder"
    );
    let fallback_return = match &ast.get(fallback_case.body).kind {
        TreeKind::Block(block) => block.expr,
        _ => fallback_case.body,
    };
    let typed_fallback_return = typer
        .source_typed_index()
        .get(source, fallback_return)
        .expect("fallback expression has a typed mapping");
    let TreeKind::Ident(fallback_return_name) = &ast.get(fallback_return).kind else {
        panic!("peelLeft's fallback returns its source parameter")
    };
    assert_eq!(
        typer
            .store()
            .names
            .resolve(fallback_return_name.name.text()),
        "expr"
    );
    let peel_parameter = index
        .symbol_at(source, peel_source.value_param_clauses[0][0])
        .unwrap();
    assert_term_ref(
        typer.store(),
        typer.typed_ast().get(typed_fallback_return).ty,
        peel_parameter,
    );
    let retyped_root = typer
        .type_method_definition(peel_left.0)
        .expect("retyping peelLeft is idempotent");
    assert_eq!(retyped_root, peel_typed_root);

    let sum_root = roots["sum"];
    let TreeKind::DefDef(sum_source) = &ast.get(sum_root.0).kind else {
        unreachable!()
    };
    let TreeKind::Match(sum_match) = &ast.get(sum_source.rhs.unwrap()).kind else {
        panic!("sum's original RHS is a Match")
    };
    assert_eq!(sum_match.cases.len(), 3);
    let sum_typed_root = sum_root.1;
    let sum_symbol = index.symbol_at(source, sum_root.0).unwrap();
    let sum_typed_match_tree = typer
        .source_typed_index()
        .get(source, sum_source.rhs.unwrap())
        .unwrap();
    let TreeKind::Match(sum_typed_match) = &typer.typed_ast().get(sum_typed_match_tree).kind else {
        panic!("sum's complete typed RHS remains a Match")
    };
    assert_eq!(sum_typed_match.cases.len(), 3);
    let sum_typed_cases = sum_typed_match.cases.clone();
    let mut recursive_calls = Vec::new();
    for (case_index, source_case) in sum_match.cases.iter().copied().enumerate() {
        let TreeKind::CaseDef(source_case_node) = &ast.get(source_case).kind else {
            unreachable!()
        };
        let TreeKind::CaseDef(typed_case_node) =
            &typer.typed_ast().get(sum_typed_cases[case_index]).kind
        else {
            unreachable!()
        };
        assert_pattern_projection(
            source,
            ast,
            index,
            typer,
            source_case_node.pattern,
            typed_case_node.pattern,
            &classes,
        );
        let mut case_binders = std::collections::BTreeMap::new();
        let mut pattern_work = vec![source_case_node.pattern];
        while let Some(pattern_tree) = pattern_work.pop() {
            match &ast.get(pattern_tree).kind {
                TreeKind::Apply(application) => {
                    pattern_work.extend(application.args.iter().copied());
                }
                TreeKind::Ident(ident) => {
                    if let Some(binder) = typer.pattern_binding_symbol_at(source, pattern_tree) {
                        case_binders.insert(
                            typer.store().names.resolve(ident.name.text()).to_owned(),
                            binder,
                        );
                    }
                }
                _ => {}
            }
        }
        let TreeKind::Block(source_body) = &ast.get(source_case_node.body).kind else {
            panic!("sum case body is a source block")
        };
        let source_return = source_body.expr;
        let mut pending = vec![source_return];
        while let Some(tree) = pending.pop() {
            match &ast.get(tree).kind {
                TreeKind::Apply(application) => {
                    let TreeKind::Ident(function) = &ast.get(application.function).kind else {
                        continue;
                    };
                    if typer.store().names.resolve(function.name.text()) != "sum" {
                        continue;
                    }
                    assert_eq!(application.args.len(), 1);
                    let typed_call = typer.source_typed_index().get(source, tree).unwrap();
                    let TreeKind::Apply(typed_application) =
                        &typer.typed_ast().get(typed_call).kind
                    else {
                        panic!("recursive sum call remains an ordinary Apply")
                    };
                    let typed_function = typer
                        .source_typed_index()
                        .get(source, application.function)
                        .unwrap();
                    assert_eq!(typed_application.function, typed_function);
                    assert_term_ref(
                        typer.store(),
                        typer.typed_ast().get(typed_function).ty,
                        sum_symbol,
                    );
                    let source_argument = application.args[0];
                    let typed_argument = typer
                        .source_typed_index()
                        .get(source, source_argument)
                        .unwrap();
                    assert_eq!(typed_application.args, [typed_argument]);
                    let TreeKind::Ident(argument_name) = &ast.get(source_argument).kind else {
                        panic!("recursive argument resolves to a source pattern binder")
                    };
                    let argument_name_text = typer.store().names.resolve(argument_name.name.text());
                    let binder = case_binders
                        .get(argument_name_text)
                        .copied()
                        .expect("recursive argument refers to the original case binder");
                    assert_eq!(
                        typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(binder).name.text()),
                        typer.store().names.resolve(argument_name.name.text())
                    );
                    assert_term_ref(
                        typer.store(),
                        typer.typed_ast().get(typed_argument).ty,
                        binder,
                    );
                    assert_type_ref(typer.store(), typer.typed_ast().get(typed_call).ty, int);
                    let range = ast.get(tree).position.unwrap().span().range();
                    recursive_calls.push((
                        range.start(),
                        typer
                            .store()
                            .names
                            .resolve(argument_name.name.text())
                            .to_owned(),
                    ));
                }
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(infix)) => {
                    pending.extend([infix.left, infix.right]);
                }
                _ => {}
            }
        }
    }
    recursive_calls.sort_by_key(|(start, _)| *start);
    assert_eq!(
        recursive_calls
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["right", "left", "right"],
        "sum retains three distinct source-owned calls with right/left/right binders"
    );
    assert_eq!(recursive_calls.len(), 3);
    let TreeKind::DefDef(sum_typed_method) = &typer.typed_ast().get(sum_typed_root).kind else {
        unreachable!()
    };
    let typed_sum_result = typer
        .source_typed_index()
        .get(source, sum_source.tpt)
        .unwrap();
    assert_eq!(sum_typed_method.tpt, typed_sum_result);
    assert_type_ref(
        typer.store(),
        typer.typed_ast().get(typed_sum_result).ty,
        int,
    );
    let SymbolInfo::Complete(sum_signature) = *typer.store().symbols.info(sum_symbol) else {
        panic!("sum's complete signature is published")
    };
    let Type::Method(sum_signature) = typer.store().types.get(sum_signature) else {
        panic!("sum's signature is a method type")
    };
    assert_eq!(sum_signature.params.len(), 1);
    assert_type_ref(typer.store(), sum_signature.params[0].ty, expr);
    assert_type_ref(typer.store(), sum_signature.result, int);

    let typed_binder = typer
        .source_typed_index()
        .get(source, peel_inner.args[0])
        .expect("nested binder has a typed mapping");
    let source_match = peel_source.rhs.unwrap();
    let source_root_span = ast.get(peel_left.0).position.unwrap().span().range();
    let source_match_span = ast.get(source_match).position.unwrap().span().range();
    let source_case_span = |tree| ast.get(tree).position.unwrap().span().range();
    let span = |range: dotty_core::source::TextRange| format!("{}..{}", range.start(), range.end());
    let outer_span = span(source_case_span(peel_case.pattern));
    let inner_span = span(source_case_span(peel_outer.args[0]));
    let binder_span = span(source_case_span(peel_inner.args[0]));
    let return_span = span(source_case_span(source_return));
    let fallback_case_span = span(source_case_span(peel_match.cases[1]));
    let fallback_pattern_span = span(source_case_span(fallback_case.pattern));
    let fallback_return_span = span(source_case_span(fallback_return));
    let source_tree_receipt = nested_source_tree_receipt(source, ast, roots);
    assert_eq!(
        format!("{source_tree_receipt}\n"),
        include_str!("fixtures/source-nested-recursive-adt-3.9.0/source-tree-spans.tsv")
            .split_once('\n')
            .expect("source tree and span receipt header")
            .1,
        "the immutable source tree IDs and spans remain pinned"
    );
    let semantic_receipt = format!(
        "peelLeft\troot=DefDef@{}..{}\tresult=Expr\tmatch=Match@{}..{}:Expr\tcases=2\tcase0=CaseDef@{}\tcase1=CaseDef@{}\textractors=outer:Add@{}:canonical_Add_companion_unapply(Add);fields=[_1:Expr,_2:Expr]\tinner:Add@{}:same_canonical_unapply;fields=[_1:Expr,_2:Expr]\tbinder=left@{}:case_local_to_peelLeft:Expr\treturn=Ident(left)@{}:same_binder\twildcards=outer._2@{}+inner._2@{}:unbound\tfallback=wildcard@{}:unbound;Ident(expr)@{}:peelLeft_parameter:Expr\tmappings=complete_source_to_typed\tretype=same_typed_root\n",
        source_root_span.start(),
        source_root_span.end(),
        source_match_span.start(),
        source_match_span.end(),
        span(source_case_span(peel_match.cases[0])),
        fallback_case_span,
        outer_span,
        inner_span,
        binder_span,
        return_span,
        span(source_case_span(peel_outer.args[1])),
        span(source_case_span(peel_inner.args[1])),
        fallback_pattern_span,
        fallback_return_span,
    );
    eprintln!(
        "ISSUE2133_PEELLEFT_IDS source={} root={}=>{} match={}=>{} outer={}=>{} inner={}=>{} unapply={} binder={}=>{} symbol={} return={}=>{} fallback={}=>{} parameter={} retype={}",
        source.index(),
        peel_left.0.index(),
        peel_left.1.index(),
        source_match.index(),
        peel_typed_match.index(),
        peel_case.pattern.index(),
        typed_outer.index(),
        peel_outer.args[0].index(),
        typed_inner.index(),
        add_unapply.index(),
        peel_inner.args[0].index(),
        typed_binder.index(),
        peel_binder_symbol.index(),
        source_return.index(),
        typed_return.index(),
        fallback_return.index(),
        typed_fallback_return.index(),
        peel_parameter.index(),
        retyped_root.index(),
    );
    semantic_receipt
}

fn nested_source_tree_receipt(
    _source: SourceId,
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    roots: &std::collections::BTreeMap<
        String,
        (
            dotty_core::TreeId<dotty_core::ast::Untyped>,
            dotty_core::TreeId<dotty_core::ast::Typed>,
        ),
    >,
) -> String {
    let row = |path: &str, tree: dotty_core::TreeId<dotty_core::ast::Untyped>| {
        let node = ast.get(tree);
        let range = node.position.unwrap().span().range();
        let kind = match &node.kind {
            TreeKind::DefDef(_) => "DefDef",
            TreeKind::Match(_) => "Match",
            TreeKind::CaseDef(_) => "CaseDef",
            TreeKind::Apply(_) => "Apply",
            TreeKind::Ident(_) => "Ident",
            _ => "Other",
        };
        format!(
            "{path}\tExpr.scala#tree{}\t{}:{}\t{kind}",
            tree.index(),
            range.start(),
            range.end(),
        )
    };
    let (peel_tree, _) = roots["peelLeft"];
    let TreeKind::DefDef(peel) = &ast.get(peel_tree).kind else {
        unreachable!()
    };
    let peel_rhs = peel.rhs.unwrap();
    let TreeKind::Match(peel_match) = &ast.get(peel_rhs).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(peel_case) = &ast.get(peel_match.cases[0]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(peel_outer) = &ast.get(peel_case.pattern).kind else {
        unreachable!()
    };
    let peel_nested = peel_outer.args[0];
    let TreeKind::Apply(peel_inner) = &ast.get(peel_nested).kind else {
        unreachable!()
    };
    let peel_binder = peel_inner.args[0];

    let (sum_tree, _) = roots["sum"];
    let TreeKind::DefDef(sum) = &ast.get(sum_tree).kind else {
        unreachable!()
    };
    let sum_rhs = sum.rhs.unwrap();
    let TreeKind::Match(sum_match) = &ast.get(sum_rhs).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(sum_nested_case) = &ast.get(sum_match.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(sum_outer) = &ast.get(sum_nested_case.pattern).kind else {
        unreachable!()
    };
    let sum_nested_num = sum_outer.args[0];
    let TreeKind::Apply(sum_num) = &ast.get(sum_nested_num).kind else {
        unreachable!()
    };
    let mut calls = Vec::new();
    for case in &sum_match.cases {
        let TreeKind::CaseDef(case) = &ast.get(*case).kind else {
            unreachable!()
        };
        let TreeKind::Block(body) = &ast.get(case.body).kind else {
            unreachable!()
        };
        let mut pending = vec![body.expr];
        while let Some(tree) = pending.pop() {
            match &ast.get(tree).kind {
                TreeKind::Apply(_) => calls.push(tree),
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(infix)) => {
                    pending.extend([infix.left, infix.right]);
                }
                _ => {}
            }
        }
    }
    calls.sort_by_key(|tree| ast.get(*tree).position.unwrap().span().range().start());

    let mut rows = vec![
        row("peelLeft.root", peel_tree),
        row("peelLeft.match", peel_rhs),
        row("peelLeft.case0.outer_Add", peel_case.pattern),
        row("peelLeft.case0.inner_Add", peel_nested),
        row("peelLeft.case0.left_binder", peel_binder),
        row("sum.root", sum_tree),
        row("sum.match", sum_rhs),
        row("sum.case1.outer_Add", sum_nested_case.pattern),
        row("sum.case1.inner_Num", sum_nested_num),
        row("sum.case1.value_binder", sum_num.args[0]),
    ];
    for (position, call) in calls.into_iter().enumerate() {
        rows.push(row(&format!("sum.recursive_call_{}", position + 1), call));
    }
    rows.join("\n")
}

fn definitions_int(store: &SemanticStore, definitions: Definitions) -> dotty_core::SymbolId {
    match store.types.get(definitions.int) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("canonical Int type is a TypeRef: {other:?}"),
    }
}

fn assert_source_product_fields(
    typer: &mut dotty_typer::SourceTyper<'_>,
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: &SourceSemanticIndex,
    source: SourceId,
    class: dotty_core::SymbolId,
    fields: &[&str],
    field_type: dotty_core::SymbolId,
) {
    let SourceDefinition::Canonical {
        source: owner_source,
        tree,
    } = index.definition_of(class).unwrap()
    else {
        panic!("product class retains its source definition")
    };
    assert_eq!(owner_source, source);
    let TreeKind::TypeDef(definition) = &ast.get(tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
        unreachable!()
    };
    let TreeKind::DefDef(constructor) = &ast.get(template.constructor).kind else {
        unreachable!()
    };
    let source_fields = &constructor.value_param_clauses[0];
    assert_eq!(source_fields.len(), fields.len());
    let mut field_symbols = Vec::new();
    let mut selector_symbols = Vec::new();
    for (position, (field_tree, expected_name)) in source_fields.iter().zip(fields).enumerate() {
        let TreeKind::ValDef(field) = &ast.get(*field_tree).kind else {
            unreachable!()
        };
        assert_eq!(
            typer.store().names.resolve(field.name.as_name().text()),
            *expected_name
        );
        let field_symbol = index.symbol_at(source, *field_tree).unwrap();
        let selector = index.derived_symbol_at(class, source, *field_tree).unwrap();
        field_symbols.push(field_symbol);
        selector_symbols.push(selector);
        assert_eq!(typer.store().symbols.get(field_symbol).owner, Some(class));
        assert_eq!(typer.store().symbols.get(selector).owner, Some(class));
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(selector).name.text()),
            if position == 0 { "_1" } else { "_2" }
        );
        let SymbolInfo::Complete(selector_signature) = *typer.store().symbols.info(selector) else {
            panic!("Product selector signature is complete")
        };
        let Type::Method(selector_signature) = typer.store().types.get(selector_signature) else {
            panic!("Product selector uses a method signature")
        };
        assert!(selector_signature.params.is_empty());
        assert_type_ref(typer.store(), selector_signature.result, field_type);
    }
    for left in 0..field_symbols.len() {
        for right in left + 1..field_symbols.len() {
            assert_ne!(field_symbols[left], field_symbols[right]);
            assert_ne!(selector_symbols[left], selector_symbols[right]);
        }
    }
}

fn assert_pattern_projection(
    source: SourceId,
    ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    index: &SourceSemanticIndex,
    typer: &mut dotty_typer::SourceTyper<'_>,
    source_pattern: dotty_core::TreeId<dotty_core::ast::Untyped>,
    typed_pattern: dotty_core::TreeId<dotty_core::ast::Typed>,
    classes: &std::collections::BTreeMap<String, dotty_core::SymbolId>,
) {
    if let TreeKind::Apply(source_extractor) = &ast.get(source_pattern).kind {
        let TreeKind::Ident(extractor_name) = &ast.get(source_extractor.function).kind else {
            panic!("source extractor has a canonical class name")
        };
        let name = typer.store().names.resolve(extractor_name.name.text());
        let class = classes[name];
        let role = GeneratedCaseClassMethodRole::Unapply;
        let unapply = index.generated_case_class_method(class, role).unwrap();
        let typed_patterns = match &typer.typed_ast().get(typed_pattern).kind {
            TreeKind::UnApply(typed_extractor) => {
                assert_term_ref(
                    typer.store(),
                    typer.typed_ast().get(typed_extractor.function).ty,
                    unapply,
                );
                typed_extractor.patterns.clone()
            }
            _ => panic!("source extractor projects to typed UnApply"),
        };
        assert_eq!(source_extractor.args.len(), typed_patterns.len());
        let SymbolInfo::Complete(unapply_type) = *typer.store().symbols.info(unapply) else {
            panic!("generated unapply signature is complete")
        };
        let Type::Method(signature) = typer.store().types.get(unapply_type) else {
            panic!("generated unapply signature is a method")
        };
        assert_eq!(signature.params.len(), 1);
        let unapply_owner = typer.store().symbols.get(unapply).owner.unwrap();
        let class_name = typer
            .store()
            .names
            .resolve(typer.store().symbols.get(class).name.text());
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(unapply_owner).name.text()),
            format!("{class_name}$")
        );
        assert_type_ref(typer.store(), signature.params[0].ty, class);
        assert_type_ref(typer.store(), signature.result, class);
        for (source_child, typed_child) in source_extractor.args.iter().zip(&typed_patterns) {
            if matches!(ast.get(*source_child).kind, TreeKind::Apply(_)) {
                assert_pattern_projection(
                    source,
                    ast,
                    index,
                    typer,
                    *source_child,
                    *typed_child,
                    classes,
                );
            } else if let TreeKind::Ident(source_binder) = &ast.get(*source_child).kind
                && typer.store().names.resolve(source_binder.name.text()) != "_"
            {
                let binder = typer
                    .pattern_binding_symbol_at(source, *source_child)
                    .unwrap();
                let TreeKind::Bind(typed_binder) = &typer.typed_ast().get(*typed_child).kind else {
                    panic!("source variable pattern projects to Bind")
                };
                assert_eq!(
                    typer.store().names.resolve(typed_binder.name.text()),
                    typer.store().names.resolve(source_binder.name.text())
                );
                assert_term_ref(
                    typer.store(),
                    typer.typed_ast().get(*typed_child).ty,
                    binder,
                );
            }
        }
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
