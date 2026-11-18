use dotty_core::{HardKeyword, NameInterner, SourceId, SourceText, TextRange, TokenKind, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{LayoutIssue, PackageIssue, ParseIssue, ParseResult, parse_compilation_unit};

fn parse(source: &str) -> ParseResult {
    parse_compilation_unit(
        SourceText::new(source).unwrap(),
        SourceId::from_index(0),
        ContextualScanner::new(source).unwrap(),
        &mut NameInterner::new(),
    )
}

#[test]
fn header_requires_a_separator_before_a_definition() {
    let result = parse("package p class C");
    let [diagnostic] = result.diagnostics.as_slice() else {
        panic!("expected a separator error")
    };
    assert_eq!(
        diagnostic.issue(),
        &ParseIssue::Layout(LayoutIssue::ExpectedTopLevelSeparator {
            found: TokenKind::Keyword(HardKeyword::Class)
        })
    );
    assert_eq!(diagnostic.span(), TextRange::new(10, 15).unwrap());
}

#[test]
fn a_late_unbraced_package_leaves_the_next_class_in_the_outer_package() {
    let result = parse("class First\npackage p\nclass Last");
    let [diagnostic] = result.diagnostics.as_slice() else {
        panic!("expected a missing body delimiter")
    };
    assert_eq!(
        diagnostic.issue(),
        &ParseIssue::Package(PackageIssue::ExpectedBodyStart {
            found: TokenKind::Newline
        })
    );
    let TreeKind::PackageDef(root) = &result.ast.get(result.root).kind else {
        panic!("expected root package")
    };
    let [_, nested, last] = root.stats.as_slice() else {
        panic!("expected three sibling definitions")
    };
    assert!(
        matches!(&result.ast.get(*nested).kind, TreeKind::PackageDef(pkg) if pkg.stats.is_empty())
    );
    assert!(matches!(result.ast.get(*last).kind, TreeKind::TypeDef(_)));
}

#[test]
fn a_delimited_body_cannot_start_a_fresh_chain_of_package_headers() {
    let result = parse("package outer { package inner; class Sibling }");
    assert_eq!(result.diagnostics.len(), 1);
    assert!(matches!(
        result.diagnostics[0].issue(),
        ParseIssue::Package(PackageIssue::ExpectedBodyStart { .. })
    ));
    let TreeKind::PackageDef(outer) = &result.ast.get(result.root).kind else {
        panic!("expected outer package")
    };
    assert_eq!(outer.stats.len(), 2);
}
