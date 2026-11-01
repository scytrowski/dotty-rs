use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolKind, TermName, TypeName,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-case-class-product2-3.9.0/Positive.scala");
const ROOTS: [&str; 2] = ["select", "classify"];

#[test]
fn two_real_product2_source_roots_have_a_stable_whole_method_baseline_in_fresh_sessions() {
    let (first, first_semantics) = analyze_session();
    let (_second, second_semantics) = analyze_session();
    assert_eq!(
        first_semantics, second_semantics,
        "fresh sessions must preserve canonical source/owner relationships"
    );
    let actual = format!("{}\n", first.join("\n"));
    let expected = include_str!("fixtures/source-case-class-product2-3.9.0/rust-baseline.tsv")
        .split_once('\n')
        .expect("baseline header")
        .1;
    assert_eq!(actual, expected);
    for row in first {
        eprintln!("{row}");
    }
}

fn analyze_session() -> (Vec<String>, Vec<String>) {
    let source = SourceId::from_index(1958);
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
    let mut flagged_class = None;
    for (tree, node) in parsed.ast.iter() {
        if let TreeKind::TypeDef(definition) = &node.kind
            && store.names.resolve(definition.name.as_name().text()) == "Flagged"
        {
            flagged_class = Some(index.symbol_at(source, tree).expect("Flagged symbol"));
        }
        if let TreeKind::DefDef(definition) = &node.kind {
            let name = store.names.resolve(definition.name.as_name().text());
            if ROOTS.contains(&name) {
                let method = index
                    .symbol_at(source, tree)
                    .expect("Matcher method symbol");
                let owner = store.symbols.get(method).owner.expect("method owner");
                assert_eq!(
                    store.names.resolve(store.symbols.get(owner).name.text()),
                    "Matcher$"
                );
                roots.push((
                    name.to_owned(),
                    tree,
                    node.position.expect("source root span"),
                    owner,
                ));
            }
        }
    }
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(roots.len(), 2, "only the requested DefDefs count");
    assert_eq!(
        roots[0].3, roots[1].3,
        "select and classify share one canonical Matcher$ owner in this session"
    );
    let class = flagged_class.expect("Flagged class symbol");
    let package = store
        .symbols
        .get(class)
        .owner
        .expect("Flagged package owner");
    let package_scope = index.scope_of(package).expect("package scope");
    let companion = store
        .scopes
        .get(package_scope)
        .lookup(TermName::new(store.names.intern("Flagged")).as_name())
        .expect("Flagged companion term");
    let module_class = store
        .scopes
        .get(package_scope)
        .lookup(TypeName::new(store.names.intern("Flagged$")).as_name())
        .expect("Flagged module class");
    let module_scope = index
        .scope_of(module_class)
        .expect("Flagged companion scope");
    let apply = store
        .scopes
        .get(module_scope)
        .lookup(TermName::new(store.names.intern("apply")).as_name());
    let unapply = store
        .scopes
        .get(module_scope)
        .lookup(TermName::new(store.names.intern("unapply")).as_name());
    let class_scope = index.scope_of(class).expect("Flagged class scope");
    let selector_1 = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("_1")).as_name());
    let selector_2 = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("_2")).as_name());
    assert!(
        apply.is_some(),
        "case-class apply exists in the source companion"
    );
    assert!(
        unapply.is_none(),
        "the current Namer baseline lacks Flagged.unapply"
    );
    assert!(
        selector_1.is_none(),
        "the current Namer baseline lacks Flagged._1"
    );
    assert!(
        selector_2.is_none(),
        "the current Namer baseline lacks Flagged._2"
    );

    let class_tree = match index
        .definition_of(class)
        .expect("Flagged source definition")
    {
        dotty_core::SourceDefinition::Canonical {
            source: definition_source,
            tree,
        } if definition_source == source => tree,
        definition => panic!("unexpected Flagged source definition: {definition:?}"),
    };
    let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
        panic!("Flagged must be a class")
    };
    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
        panic!("Flagged RHS must be a template")
    };
    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("primary constructor must be a method")
    };
    let parameters = &constructor.value_param_clauses[0];
    assert_eq!(
        parameters.len(),
        2,
        "the source field order is part of the contract"
    );
    for (parameter, expected) in parameters.iter().zip(["value", "enabled"]) {
        let TreeKind::ValDef(definition) = &parsed.ast.get(*parameter).kind else {
            panic!("case-class constructor parameter must be a ValDef");
        };
        assert_eq!(
            store.names.resolve(definition.name.as_name().text()),
            expected
        );
    }
    let field_1 = index
        .symbol_at(source, parameters[0])
        .expect("value field identity");
    let field_2 = index
        .symbol_at(source, parameters[1])
        .expect("enabled field identity");
    assert_eq!(store.symbols.get(field_1).kind, SymbolKind::Field);
    assert_eq!(store.symbols.get(field_2).kind, SymbolKind::Field);
    assert_ne!(field_1, field_2);
    assert_eq!(
        index.derived_symbol_at(class, source, parameters[0]),
        selector_1
    );
    assert_eq!(
        index.derived_symbol_at(class, source, parameters[1]),
        selector_2
    );
    if let (Some(first), Some(second)) = (selector_1, selector_2) {
        assert_ne!(
            first, second,
            "product selectors need independent identities"
        );
    }
    assert_eq!(store.symbols.get(class).links.companion, Some(companion));
    assert_eq!(store.symbols.get(companion).links.companion, Some(class));
    let method_owner_tree = match index
        .definition_of(roots[0].3)
        .expect("Matcher owner source definition")
    {
        dotty_core::SourceDefinition::Canonical {
            source: definition_source,
            tree,
        }
        | dotty_core::SourceDefinition::Derived {
            source: definition_source,
            tree,
        } if definition_source == source => tree,
        definition => panic!("unexpected Matcher owner definition: {definition:?}"),
    };
    let owner_range = parsed
        .ast
        .get(method_owner_tree)
        .position
        .expect("Matcher owner source span")
        .span()
        .range();
    let class_range = parsed
        .ast
        .get(class_tree)
        .position
        .expect("Flagged class source span")
        .span()
        .range();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let mut rows = Vec::new();
    let mut semantic_rows = Vec::new();
    for (name, tree, position, _method_owner) in roots {
        let outcome = typer.type_method_definition(tree);
        let typed_root = typer.source_typed_index().get(source, tree);
        let (result, semantic_result) = match outcome {
            Ok(typed) => {
                assert_eq!(typed_root, Some(typed));
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
                (format!("typed:tree#{}", typed.index()), "typed".to_owned())
            }
            Err(error) => {
                assert!(
                    typed_root.is_none(),
                    "failed roots cannot enter SourceTypedIndex"
                );
                let TyperError::ExtractorUnapplyNotFound {
                    tree_index,
                    source: error_source,
                    extractor,
                } = &error
                else {
                    panic!("unexpected first blocker: {error:?}");
                };
                assert_eq!(*error_source, source);
                assert_eq!(
                    *extractor, companion,
                    "the missing unapply belongs to Flagged$"
                );
                let diag = parsed
                    .ast
                    .iter()
                    .find_map(|(candidate, node)| {
                        (candidate.index() == *tree_index)
                            .then_some(node.position.expect("diagnostic span"))
                    })
                    .expect("extractor error tree span");
                let range = diag.span().range();
                let rendered = format!(
                    "error=ExtractorUnapplyNotFound{{source={},tree={},extractor=Flagged$}};diag=Positive.scala#tree{};diag_bytes={}:{};typed_index=absent",
                    error_source.index(),
                    tree_index,
                    tree_index,
                    range.start(),
                    range.end()
                );
                (
                    rendered,
                    format!(
                        "ExtractorUnapplyNotFound;diag_bytes={}:{};typed_index=absent",
                        range.start(),
                        range.end()
                    ),
                )
            }
        };
        let range = position.span().range();
        let first_parameter = parsed
            .ast
            .get(parameters[0])
            .position
            .unwrap()
            .span()
            .range();
        let second_parameter = parsed
            .ast
            .get(parameters[1])
            .position
            .unwrap()
            .span()
            .range();
        semantic_rows.push(format!(
            "{name}\tsource_bytes={}:{}\towner=Matcher$@{}:{}\tclass=Flagged@{}:{}\tfields=value@{}:{},enabled@{}:{}\t{semantic_result}",
            range.start(), range.end(), owner_range.start(), owner_range.end(),
            class_range.start(), class_range.end(), first_parameter.start(), first_parameter.end(),
            second_parameter.start(), second_parameter.end(),
        ));
        rows.push(format!(
            "{name}\tpath=Positive.scala#tree{}\tsource={}\tbytes={}:{}\tmethod_owner=Positive.scala#tree{}(Matcher$)\tflagged_class=Positive.scala#tree{}\tcompanion=Flagged$.linked-to-Flagged\tmodule_class=Flagged$\tapply=present\tunapply=absent\tselector_1=absent\tselector_2=absent\tfield_1=tree#{}@bytes{}-{}:value\tfield_2=tree#{}@bytes{}-{}:enabled\t{result}",
            tree.index(), position.source().index(), range.start(), range.end(), method_owner_tree.index(), class_tree.index(),
            parameters[0].index(), parsed.ast.get(parameters[0]).position.unwrap().span().range().start(),
            parsed.ast.get(parameters[0]).position.unwrap().span().range().end(),
            parameters[1].index(), parsed.ast.get(parameters[1]).position.unwrap().span().range().start(),
            parsed.ast.get(parameters[1]).position.unwrap().span().range().end(),
        ));
    }
    (rows, semantic_rows)
}
