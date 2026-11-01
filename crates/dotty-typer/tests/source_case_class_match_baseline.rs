use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceId, SourceText,
    SymbolInfo, SymbolKind, TermName, TypeName,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-case-class-match-3.9.0/Positive.scala");
const ROOTS: [&str; 2] = ["unwrap", "classify"];

#[test]
fn two_real_source_roots_have_a_stable_whole_method_baseline_in_fresh_sessions() {
    let first = analyze_session();
    let second = analyze_session();
    assert_eq!(first, second, "fresh sessions must normalize identically");
    let historical = include_str!("fixtures/source-case-class-match-3.9.0/rust-baseline.tsv");
    assert!(
        historical.contains("apply=present\tunapply=absent")
            && historical.contains("ExtractorUnapplyNotFound"),
        "the #1933 before-state remains a frozen historical receipt"
    );
    let actual = format!("{}\n", first.join("\n"));
    assert_eq!(
        actual,
        include_str!("fixtures/source-case-class-match-3.9.0/rust-after-product.tsv")
            .split_once('\n')
            .expect("baseline header")
            .1
    );
    for row in first {
        eprintln!("{row}");
    }
}

fn analyze_session() -> Vec<String> {
    let source = SourceId::from_index(1933);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(SOURCE).expect("scan frozen source");
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
        "Positive.scala",
        &mut store,
        &mut packages,
    )
    .expect("name frozen source");

    let mut roots = Vec::new();
    let mut box_class = None;
    for (tree, node) in parsed.ast.iter() {
        if let TreeKind::TypeDef(definition) = &node.kind {
            let name = store.names.resolve(definition.name.as_name().text());
            if name == "Box" {
                let symbol = index.symbol_at(source, tree).expect("Box symbol");
                match store.symbols.get(symbol).kind {
                    SymbolKind::Class => box_class = Some(symbol),
                    kind => panic!("unexpected Box declaration kind: {kind:?}"),
                }
            }
        }
        if let TreeKind::DefDef(definition) = &node.kind {
            let name = store.names.resolve(definition.name.as_name().text());
            if ROOTS.contains(&name) {
                let method = index
                    .symbol_at(source, tree)
                    .expect("Matcher method symbol");
                let method_owner = store.symbols.get(method).owner.expect("method owner");
                assert_eq!(
                    store
                        .names
                        .resolve(store.symbols.get(method_owner).name.text()),
                    "Matcher$",
                    "only methods owned by the source Matcher object count"
                );
                roots.push((
                    name.to_owned(),
                    tree,
                    node.position.expect("source root span"),
                ));
            }
        }
    }
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(roots.len(), 2, "only the two requested DefDefs count");
    let class = box_class.expect("Box class symbol");
    let unapply = index
        .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
        .expect("generated Box.unapply identity");
    let package = store.symbols.get(class).owner.expect("Box package owner");
    let package_scope = index.scope_of(package).expect("package scope");
    let companion = store
        .scopes
        .get(package_scope)
        .lookup(TermName::new(store.names.intern("Box")).as_name())
        .expect("generated Box companion term");
    let module_class = store
        .scopes
        .get(package_scope)
        .lookup(TypeName::new(store.names.intern("Box$")).as_name())
        .expect("generated Box module class");
    assert_eq!(store.symbols.get(class).links.companion, Some(companion));
    assert_eq!(store.symbols.get(companion).links.companion, Some(class));
    assert_eq!(store.symbols.get(module_class).links.companion, None);
    let module_scope = index.scope_of(module_class).expect("Box companion scope");
    assert!(
        store
            .scopes
            .get(module_scope)
            .lookup(TermName::new(store.names.intern("apply")).as_name())
            .is_some()
    );
    assert_eq!(
        store
            .scopes
            .get(module_scope)
            .lookup(TermName::new(store.names.intern("unapply")).as_name()),
        Some(unapply)
    );
    assert_eq!(
        index.generated_case_class_method_identity(unapply),
        Some(dotty_core::GeneratedCaseClassMethod {
            class,
            role: GeneratedCaseClassMethodRole::Unapply,
        })
    );
    let class_scope = index.scope_of(class).expect("Box class scope");
    let selector = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("_1")).as_name())
        .expect("source-backed case class product selector");
    let class_tree = match index.definition_of(class).expect("Box source definition") {
        dotty_core::SourceDefinition::Canonical {
            source: definition_source,
            tree,
        } if definition_source == source => tree,
        definition => panic!("unexpected Box source definition: {definition:?}"),
    };
    let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
        panic!("Box source tree must be a class definition");
    };
    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
        panic!("Box RHS must be a template");
    };
    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("Box primary constructor must be a method");
    };
    let parameter_tree = constructor.value_param_clauses[0][0];
    let canonical_field = index
        .symbol_at(source, parameter_tree)
        .expect("canonical Box.value field");
    assert_eq!(store.symbols.get(canonical_field).kind, SymbolKind::Field);
    assert_ne!(selector, canonical_field, "_1 has its own method identity");
    assert_eq!(
        index.derived_symbol_at(class, source, parameter_tree),
        Some(selector),
        "_1 provenance is tied to the original constructor parameter under Box"
    );

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let mut rows = Vec::new();
    for (name, tree, position) in roots {
        let outcome = typer.type_method_definition(tree);
        let typed_root = typer.source_typed_index().get(source, tree);
        let result = match outcome {
            Ok(typed) => {
                assert_eq!(typed_root, Some(typed));
                let typed_node = typer.typed_ast().get(typed);
                assert!(matches!(typed_node.kind, TreeKind::DefDef(_)));
                format!("typed:tree#{}", typed.index())
            }
            Err(error) => {
                assert!(
                    typed_root.is_none(),
                    "failed roots cannot enter SourceTypedIndex"
                );
                assert_eq!(
                    *typer.store().symbols.info(unapply),
                    SymbolInfo::Missing,
                    "discovery must not invent the generated unapply signature"
                );
                let retry = typer
                    .type_method_definition(tree)
                    .expect_err("failed root retry must remain rejected");
                assert_eq!(
                    format!("{retry:?}"),
                    format!("{error:?}"),
                    "rollback retry must preserve the first error"
                );
                assert!(
                    typer.source_typed_index().get(source, tree).is_none(),
                    "failed retry cannot leave a typed root behind"
                );
                match &error {
                    TyperError::ExtractorResultMemberNotFound {
                        tree_index,
                        source: error_source,
                        ..
                    } if *error_source == source => assert!(
                        parsed
                            .ast
                            .iter()
                            .any(|(candidate, _)| candidate.index() == *tree_index)
                    ),
                    TyperError::TypeNameNotFound {
                        name: missing_name, ..
                    } if typer.store().names.resolve(missing_name.text()) == "Product" => {}
                    _ => panic!("unexpected first post-signature baseline error: {error:?}"),
                }
                match error {
                    TyperError::ExtractorResultMemberNotFound { tree_index, .. } => {
                        let diagnostic_span = parsed
                            .ast
                            .iter()
                            .find_map(|(candidate, node)| {
                                (candidate.index() == tree_index)
                                    .then_some(node.position.expect("diagnostic source span"))
                            })
                            .expect("extractor error source tree");
                        let range = diagnostic_span.span().range();
                        format!(
                            "error:{error:?};diag_tree={tree_index};diag_bytes={}:{};typed_index=absent",
                            range.start(),
                            range.end(),
                        )
                    }
                    TyperError::TypeNameNotFound { .. } => {
                        format!("error:{error:?};product_dependency=unavailable;typed_index=absent")
                    }
                    _ => unreachable!("error variant was validated above"),
                }
            }
        };
        let range = position.span().range();
        rows.push(format!(
            "{name}\tpath=Positive.scala#tree{}\tsource={}\tbytes={}:{}\tbox_class={}\tbox_companion={}\tbox_module_class={}\tmoduleclass_link=absent\tapply=present\tunapply={}\tselector_1=present\t{result}",
            tree.index(),
            position.source().index(),
            range.start(),
            range.end(),
            class.index(),
            companion.index(),
            module_class.index(),
            unapply.index(),
        ));
    }
    let signature = typer
        .complete_symbol(unapply)
        .expect_err("missing scala.Product must leave generated signature retryable");
    assert!(matches!(
        signature,
        TyperError::TypeNameNotFound { name, .. }
            if typer.store().names.resolve(name.text()) == "Product"
    ));
    assert_eq!(*typer.store().symbols.info(unapply), SymbolInfo::Missing);
    rows
}
