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
        include_str!("fixtures/source-case-class-match-3.9.0/rust-after-unapply.tsv")
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
    assert!(
        store
            .scopes
            .get(class_scope)
            .lookup(TermName::new(store.names.intern("_1")).as_name())
            .is_none()
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
                let (diagnostic_tree, diagnostic_span) = match &error {
                    TyperError::ExtractorResultMemberNotFound {
                        tree_index,
                        source: error_source,
                        ..
                    } if *error_source == source => parsed
                        .ast
                        .iter()
                        .find_map(|(candidate, node)| {
                            (candidate.index() == *tree_index).then_some((
                                candidate.index(),
                                node.position.expect("diagnostic source span"),
                            ))
                        })
                        .expect("extractor error source tree"),
                    _ => panic!("unexpected first post-signature baseline error: {error:?}"),
                };
                let diagnostic_range = diagnostic_span.span().range();
                format!(
                    "error:{error:?};diag_tree={diagnostic_tree};diag_bytes={}:{};typed_index=absent",
                    diagnostic_range.start(),
                    diagnostic_range.end(),
                )
            }
        };
        let range = position.span().range();
        rows.push(format!(
            "{name}\tpath=Positive.scala#tree{}\tsource={}\tbytes={}:{}\tbox_class={}\tbox_companion={}\tbox_module_class={}\tmoduleclass_link=absent\tapply=present\tunapply={}\tselector_1=absent\t{result}",
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
        .expect("a failed whole-method attempt must leave generated signature retryable");
    assert!(matches!(
        typer.store().types.try_get(signature),
        Some(dotty_core::types::Type::Method(method)) if method.params.len() == 1
    ));
    rows
}
