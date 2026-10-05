use dotty_core::ast::{CaseDef, DefDef, Match, ModuleDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind, Untyped};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn case_guard_accepts_a_leading_infix_operator_on_the_next_line() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/match-case-guard-infix-pattern.scala"
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
        panic!("expected a package root");
    };
    let [object] = package.stats.as_slice() else {
        panic!("expected one object definition");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ModuleDef { template, .. })) =
        &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected an object template");
    };
    let [method, following] = body.as_slice() else {
        panic!("expected the match method and following template member");
    };
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = &result.ast.get(*method).kind else {
        panic!("expected the method to have a body");
    };
    let TreeKind::Match(Match { cases, .. }) = &result.ast.get(*rhs).kind else {
        panic!("expected the method body to be a match");
    };
    let [case, _] = cases.as_slice() else {
        panic!("expected the guarded case and fallback case");
    };
    let TreeKind::CaseDef(CaseDef {
        guard: Some(guard), ..
    }) = &result.ast.get(*case).kind
    else {
        panic!("expected the first case to keep its guard");
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(operator)) = &result.ast.get(*guard).kind
    else {
        panic!("expected the guard to include its leading infix operator");
    };
    assert_eq!(names.resolve(operator.op.text()), "||");
    assert!(matches!(
        result.ast.get(*following).kind,
        TreeKind::DefDef(_)
    ));
}

#[test]
fn case_guard_infix_continuations_accept_prefix_operands() {
    for (source, prefix) in [
        (
            include_str!(
                "../../scala-parser-oracle/fixtures/compilation/match-case-guard-infix-prefix-minus.scala"
            ),
            "-",
        ),
        (
            include_str!(
                "../../scala-parser-oracle/fixtures/compilation/match-case-guard-infix-prefix-not.scala"
            ),
            "!",
        ),
    ] {
        let (ast, names, guard) = first_case_guard(source);
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(or)) = &ast.get(guard).kind else {
            panic!("expected the case guard to be an infix expression");
        };
        assert_eq!(names.resolve(or.op.text()), "||");

        if prefix == "-" {
            let TreeKind::PhaseSpecific(UntypedNode::InfixOp(comparison)) = &ast.get(or.right).kind
            else {
                panic!("expected the negative literal comparison to remain an infix tree");
            };
            let TreeKind::PhaseSpecific(UntypedNode::Number(number)) =
                &ast.get(comparison.left).kind
            else {
                panic!("expected Scala's negative-number literal shape");
            };
            assert_eq!(names.resolve(number.text), "-1");
        } else {
            let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix_op)) = &ast.get(or.right).kind
            else {
                panic!("expected the right operand to begin with `{prefix}`");
            };
            assert_eq!(names.resolve(prefix_op.op.text()), prefix);
        }
    }
}

fn first_case_guard(
    source: &str,
) -> (
    dotty_core::ast::AstArena<Untyped>,
    NameInterner,
    dotty_core::TreeId<Untyped>,
) {
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
        panic!("expected a package root");
    };
    let [object] = package.stats.as_slice() else {
        panic!("expected one object definition");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ModuleDef { template, .. })) =
        &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected an object template");
    };
    let [method, following] = body.as_slice() else {
        panic!("expected the match method and following template member");
    };
    assert!(matches!(
        result.ast.get(*following).kind,
        TreeKind::ValDef(_)
    ));
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = &result.ast.get(*method).kind else {
        panic!("expected the method to have a body");
    };
    let TreeKind::Match(Match { cases, .. }) = &result.ast.get(*rhs).kind else {
        panic!("expected the method body to be a match");
    };
    let [case, _] = cases.as_slice() else {
        panic!("expected the guarded case and fallback case");
    };
    let TreeKind::CaseDef(CaseDef {
        guard: Some(guard), ..
    }) = &result.ast.get(*case).kind
    else {
        panic!("expected the first case to keep its guard");
    };
    let guard = *guard;
    (result.ast, names, guard)
}
