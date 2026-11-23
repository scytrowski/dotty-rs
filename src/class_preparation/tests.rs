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

#[test]
fn reports_preserve_bound_syntax_order_including_tied_byte_offsets_after_analysis() {
    use crate::lexer::{LexerIssue, NumericComponent, NumericIssue};
    use crate::parser::{DeclarationIssue, ParseIssue};
    let id = SourceId::from_index(7);
    let text = "val x: Int 0x";
    let reports = {
        let mut store = SemanticStore::new();
        let parsed = bound(text, id, &mut store);
        let (file, reports) = parsed
            .prepare_classes("X.scala", &mut store, &mut Packages::new())
            .into_parts();
        assert!(file.is_none());
        reports
    };
    let [
        a @ PreparationDiagnostic::Syntax(SyntaxDiagnostic::Lexer { diagnostic, .. }),
        b @ PreparationDiagnostic::Syntax(SyntaxDiagnostic::Parser(parser)),
    ] = &reports[..]
    else {
        panic!("expected lexer before parser at the same offset")
    };
    assert_eq!(
        diagnostic.issue(),
        &LexerIssue::Numeric(NumericIssue::MissingDigits {
            component: NumericComponent::BasePrefix { base: 16 }
        })
    );
    assert!(matches!(
        parser.issue(),
        ParseIssue::Declaration(DeclarationIssue::ExpectedValueEquals { .. })
    ));
    assert_eq!(a.code(), Some(diagnostic.issue().code()));
    assert_eq!(b.code(), Some("parser.declaration.expected_value_equals"));
    for d in &reports {
        let span = d.location().unwrap();
        assert_eq!((d.source(), span.source()), (id, id));
        let range = span.span().range();
        assert_eq!(&text[range.start() as usize..range.end() as usize], "0x");
        assert_eq!(d.render().code, d.code());
    }
}

#[test]
fn ordered_projection_reports_keep_the_original_trees_and_ranges_after_analysis() {
    let text = "class A; trait Z; trait B";
    let id = SourceId::from_index(19);
    let (reports, trees) = {
        let mut store = SemanticStore::new();
        let parsed = bound(text, id, &mut store);
        let crate::core::TreeKind::PackageDef(package) =
            &parsed.syntax.ast.get(parsed.syntax.root).kind
        else {
            panic!("expected package")
        };
        let trees = [package.stats[1], package.stats[2]];
        let (file, reports) = parsed
            .prepare_classes("Mixed.scala", &mut store, &mut Packages::new())
            .into_parts();
        assert_eq!(file.unwrap().classes().len(), 1);
        (reports, trees)
    };
    assert_eq!(reports.len(), 2);
    for ((d, tree), expected) in reports.iter().zip(trees).zip(["trait Z", "trait B"]) {
        let PreparationDiagnostic::Declaration(report) = d else {
            panic!("expected a typed omission")
        };
        assert_eq!(
            report.error,
            DeclarationError::Unsupported { source: id, tree }
        );
        let span = d.location().unwrap();
        assert_eq!(
            (d.source(), span.source(), d.code()),
            (id, id, Some("declaration.unsupported"))
        );
        let range = span.span().range();
        assert_eq!(
            &text[range.start() as usize..range.end() as usize],
            expected
        );
        assert_eq!(d.render().location, Some(span));
    }
}
