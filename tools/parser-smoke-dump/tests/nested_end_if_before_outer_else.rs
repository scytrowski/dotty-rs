use dotty_core::ast::{Block, DefDef, If, Template, TypeDef};
use dotty_core::{NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn explicit_nested_end_if_keeps_the_enclosing_else_in_a_braced_template() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/nested-end-if-before-outer-else.scala"
    );
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );
    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );

    let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
        panic!("expected package root");
    };
    let [object] = package.stats.as_slice() else {
        panic!("expected one trait");
    };
    let TreeKind::TypeDef(TypeDef { rhs: template, .. }) = &result.ast.get(*object).kind else {
        panic!("expected trait definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected trait body");
    };
    let method = body
        .iter()
        .find(|tree| {
            matches!(
                &result.ast.get(**tree).kind,
                TreeKind::DefDef(definition)
                    if names.resolve(definition.name.as_name().text()) == "messageAndPos"
            )
        })
        .copied()
        .expect("messageAndPos method is retained");
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = result.ast.get(method).kind else {
        panic!("expected method body");
    };
    let TreeKind::Block(Block { stats, .. }) = &result.ast.get(rhs).kind else {
        panic!("expected a method block");
    };

    let outer_start = source.find("if pos.exists &&").unwrap() as u32;
    let outer = stats
        .iter()
        .find(|tree| {
            result
                .ast
                .get(**tree)
                .position
                .is_some_and(|position| position.span().range().start() == outer_start)
        })
        .copied()
        .expect("outer if remains a direct statement in the method body");
    let TreeKind::If(If {
        then_branch,
        else_branch,
        ..
    }) = result.ast.get(outer).kind
    else {
        panic!("expected outer if expression");
    };
    let else_start =
        source.find("else sb.append(msg.message)").unwrap() as u32 + "else ".len() as u32;
    let else_end = source[else_start as usize..].find('\n').unwrap() as u32 + else_start;
    assert!(matches!(
        result.ast.get(else_branch).kind,
        TreeKind::Apply(_)
    ));
    assert_eq!(
        result.ast.get(else_branch).position.unwrap().span().range(),
        TextRange::new(else_start, else_end).unwrap()
    );
    assert_eq!(
        result.ast.get(outer).position.unwrap().span().range(),
        TextRange::new(outer_start, else_end).unwrap()
    );

    let TreeKind::Block(Block {
        stats: then_stats,
        expr: then_expr,
    }) = &result.ast.get(then_branch).kind
    else {
        panic!("expected the outer then branch to remain a block");
    };
    let nested_start = source.find("if inlineStack.nonEmpty").unwrap() as u32;
    let nested = then_stats
        .iter()
        .copied()
        .chain([*then_expr])
        .find(|tree| {
            result
                .ast
                .get(*tree)
                .position
                .is_some_and(|position| position.span().range().start() == nested_start)
        })
        .expect("the nested if remains inside the outer then branch");
    assert!(matches!(result.ast.get(nested).kind, TreeKind::If(_)));
    let end_marker_end = source.find("end if").unwrap() as u32 + "end if".len() as u32;
    assert_eq!(
        result
            .ast
            .get(nested)
            .position
            .unwrap()
            .span()
            .range()
            .end(),
        end_marker_end
    );
}
