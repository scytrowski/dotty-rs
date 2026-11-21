//! Statement recovery through the concrete scanner and compilation-unit entry point.
use dotty::core::{NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty::lexer::ContextualScanner;
use dotty::parser::{ParseResult, parse_compilation_unit};

fn parse(text: &str) -> ParseResult {
    parse_compilation_unit(
        SourceText::new(text).unwrap(),
        SourceId::from_index(8),
        ContextualScanner::new(text).unwrap(),
        &mut NameInterner::new(),
    )
}

fn assert_reports(text: &str, parsed: &ParseResult, expected: &[(&str, &str)]) {
    assert_eq!(
        parsed.diagnostics.len(),
        expected.len(),
        "{:?}",
        parsed.diagnostics
    );
    for (diagnostic, &(code, token)) in parsed.diagnostics.iter().zip(expected) {
        let start = text.find(token).unwrap() as u32;
        assert_eq!(diagnostic.issue().code(), code);
        assert_eq!(diagnostic.source(), SourceId::from_index(8));
        assert_eq!(
            diagnostic.span(),
            TextRange::new(start, start + token.len() as u32).unwrap()
        );
    }
}

#[test]
fn top_level_typed_var_recovers_at_newline_and_keeps_a_real_separator_error() {
    let text = "var broken: Int 10\nval ok = 20 30\nval next = 40";
    let parsed = parse(text);
    assert_reports(
        text,
        &parsed,
        &[
            ("parser.declaration.expected_value_equals", "10"),
            ("parser.layout.expected_top_level_separator", "30"),
        ],
    );
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected a package root")
    };
    assert_eq!(package.stats.len(), 3);
    for (&tree, expected) in
        package
            .stats
            .iter()
            .zip(["var broken: Int", "val ok = 20", "val next = 40"])
    {
        let node = parsed.ast.get(tree);
        assert!(matches!(node.kind, TreeKind::ValDef(_)));
        let range = node.position.unwrap().span().range();
        assert_eq!(
            &text[range.start() as usize..range.end() as usize],
            expected
        );
    }
}

#[test]
fn block_recovery_reports_each_broken_field_and_preserves_its_final_expression() {
    let text = "def f = { val a: Int 11; var b: Int 22; 33 }";
    let parsed = parse(text);
    assert_reports(
        text,
        &parsed,
        &[
            ("parser.declaration.expected_value_equals", "11"),
            ("parser.declaration.expected_value_equals", "22"),
        ],
    );
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected a package root")
    };
    let TreeKind::DefDef(method) = &parsed.ast.get(package.stats[0]).kind else {
        panic!("expected f")
    };
    let TreeKind::Block(block) = &parsed.ast.get(method.rhs.unwrap()).kind else {
        panic!("expected a block")
    };
    assert_eq!(block.stats.len(), 2);
    assert!(block.stats.iter().all(
        |&tree| matches!(&parsed.ast.get(tree).kind, TreeKind::ValDef(field) if field.rhs.is_none())
    ));
    let range = parsed.ast.get(block.expr).position.unwrap().span().range();
    assert_eq!(&text[range.start() as usize..range.end() as usize], "33");
}

#[test]
fn abstract_members_and_repaired_initializers_remain_clean() {
    let text = "trait Fields:\n  val absent: Int\n  var count: Int\n  val ready: Int = 1";
    assert_reports(text, &parse(text), &[]);
}

#[test]
fn an_indented_field_recovers_before_the_next_member_and_outdent() {
    let text = "class A:\n  val hidden: Int 11\n  def next = 22\nclass B";
    let parsed = parse(text);
    assert_reports(
        text,
        &parsed,
        &[("parser.declaration.expected_value_equals", "11")],
    );
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected a package root")
    };
    let [a, b] = package.stats[..] else {
        panic!("expected both classes")
    };
    let TreeKind::TypeDef(class) = &parsed.ast.get(a).kind else {
        panic!("expected class A")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class.rhs).kind else {
        panic!("expected a template")
    };
    let [field, next] = template.body[..] else {
        panic!("expected both members")
    };
    assert!(matches!(&parsed.ast.get(field).kind, TreeKind::ValDef(field) if field.rhs.is_none()));
    assert!(matches!(&parsed.ast.get(next).kind, TreeKind::DefDef(method) if method.rhs.is_some()));
    let range = parsed.ast.get(b).position.unwrap().span().range();
    assert_eq!(
        &text[range.start() as usize..range.end() as usize],
        "class B"
    );
}

#[test]
fn adjacent_named_declarations_report_the_missing_separator_and_keep_both_fields() {
    let text = "trait Fields { val abstractField: Int final val ready = 42 }";
    let parsed = parse(text);
    assert_reports(
        text,
        &parsed,
        &[("parser.template.expected_member_separator", "final")],
    );
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package")
    };
    let TreeKind::TypeDef(class) = &parsed.ast.get(package.stats[0]).kind else {
        panic!("expected Fields")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class.rhs).kind else {
        panic!("expected template")
    };
    let [first, second] = template.body[..] else {
        panic!("expected both declarations")
    };
    assert!(matches!(&parsed.ast.get(first).kind, TreeKind::ValDef(field) if field.rhs.is_none()));
    assert!(matches!(&parsed.ast.get(second).kind, TreeKind::ValDef(field) if field.rhs.is_some()));
}
