use super::*;

#[test]
fn an_unlocated_namer_failure_retains_its_original_file_and_payload() {
    let source = SourceId::from_index(47);
    let error = NamerError::RootIsNotPackage { tree_index: 3 };
    let diagnostic = PreparationDiagnostic::NamerFailure {
        source,
        error: error.clone(),
    };
    assert_eq!(diagnostic.source(), source);
    assert_eq!(diagnostic.location(), None);
    assert_eq!(diagnostic.code(), None);
    assert_eq!(
        diagnostic.render(),
        SourceDiagnostic::error(error.to_string(), None)
    );
    assert!(
        matches!(diagnostic, PreparationDiagnostic::NamerFailure {error: original, ..} if original == error)
    );
}

fn bound(
    text: &str,
    source: SourceId,
    store: &mut SemanticStore,
) -> ParsedSource<SyntaxDiagnostic> {
    crate::parse_source(source, text, Default::default())
        .unwrap()
        .bind(&mut store.names)
}

#[test]
fn syntax_and_naming_failures_are_distinct_and_leave_the_package_uncommitted() {
    let id = SourceId::from_index(17);
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let broken = "package demo\nclass A { private[Missing] val hidden: Int 1 }";
    for (text, site, code) in [
        (
            broken.to_owned(),
            "1",
            "parser.declaration.expected_value_equals",
        ),
        (
            broken.replace("Int 1", "Int = 1"),
            "Missing",
            "namer.visibility.invalid_qualifier",
        ),
    ] {
        let parsed = bound(&text, id, &mut store);
        let before = store.checkpoint();
        let ClassPreparation::Rejected {
            source,
            diagnostics,
        } = parsed.prepare_classes("A.scala", &mut store, &mut packages)
        else {
            panic!("expected a rejection")
        };
        let [d] = diagnostics.as_slice() else {
            panic!("expected one problem")
        };
        assert!(matches!(
            (site, d),
            (
                "1",
                PreparationDiagnostic::Syntax(SyntaxDiagnostic::Parser(_))
            ) | ("Missing", PreparationDiagnostic::Naming(_))
        ));
        assert_eq!((source, d.source(), d.code()), (id, id, Some(code)));
        let range = d.location().unwrap().span().range();
        assert_eq!(&text[range.start() as usize..range.end() as usize], site);
        assert_eq!(store.checkpoint(), before);
        assert!(packages.get(&["demo"]).is_none());
    }
}

#[test]
fn structural_failure_in_a_bound_parse_keeps_its_file_id() {
    let source = SourceId::from_index(9);
    let mut store = SemanticStore::new();
    let mut parsed = bound("class A", source, &mut store);
    let tree = parsed
        .syntax
        .ast
        .iter()
        .find(|(_, node)| matches!(node.kind, crate::core::TreeKind::TypeDef(_)))
        .unwrap()
        .0;
    parsed.syntax.root = tree;
    let ClassPreparation::Rejected {
        source: id,
        diagnostics,
    } = parsed.prepare_classes("A.scala", &mut store, &mut Packages::new())
    else {
        panic!("expected a structural failure")
    };
    assert_eq!(id, source);
    assert!(
        matches!(&diagnostics[..], [PreparationDiagnostic::NamerFailure {source: s, error: NamerError::RootIsNotPackage{tree_index}}] if *s == source && *tree_index == tree.index())
    );
}

#[test]
fn complete_empty_and_partial_files_are_all_prepared() {
    let id = SourceId::from_index(52);
    for (text, names, omitted) in [
        ("", &[][..], false),
        ("trait T", &[][..], true),
        ("class A; trait T; class B", &["A", "B"][..], true),
        ("class A", &["A"][..], false),
    ] {
        let mut store = SemanticStore::new();
        let (file, diagnostics) = bound(text, id, &mut store)
            .prepare_classes("Model.scala", &mut store, &mut Packages::new())
            .into_parts();
        let file = file.expect("a prepared view even for zero classes");
        assert_eq!(file.source(), id);
        assert_eq!(
            file.classes()
                .iter()
                .map(|c| store.names.resolve(store.symbols.get(c.symbol).name.text()))
                .collect::<Vec<_>>(),
            names
        );
        if omitted {
            let [d @ PreparationDiagnostic::Declaration(report)] = diagnostics.as_slice() else {
                panic!("expected just the trait")
            };
            assert!(
                matches!(report.error, DeclarationError::Unsupported {source, ..} if source == id)
            );
            assert_eq!(d.code(), Some("declaration.unsupported"));
            let range = d.location().unwrap().span().range();
            assert_eq!(
                &text[range.start() as usize..range.end() as usize],
                "trait T"
            );
        } else {
            assert!(diagnostics.is_empty());
        }
    }
}
