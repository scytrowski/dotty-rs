#[test]
fn pinned_oracle_preserves_boolean_patterns_and_binding_shapes() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_matches/oracle.txt");
    for shape in [
        "scala.Boolean",
        "case true",
        "case false",
        "n @ true",
        "(true | true)",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

#[test]
fn baseline_boolean_match_is_rejected_after_frontend_typing() {
    let result = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        "package example; class BooleanMatches { def f(x: Boolean): Int = x match { case true => 1; case _ => 2 } }",
        "BooleanMatches.scala",
        dotty::core::SourceId::from_index(2476),
    );
    assert!(matches!(
        result,
        Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(
            dotty::jvm_tasty_plan::PairedPlanError::Contract(_)
        ))
    ));
}

#[path = "support/guarded_match_plan.rs"]
mod guarded_match_plan;

#[test]
fn jvm_boolean_literals_wildcards_and_primitive_result_flows() {
    for method in [
        "def f(x: Boolean): Int = x match { case true => 1; case _ => 2 }",
        "def f(x: Boolean): Boolean = x match { case false => true; case _ => false }",
        "def f(x: Boolean): Unit = x match { case true => (); case _ => () }",
        "def f(x: Boolean, a: Long, b: Long): Long = x match { case true => a; case _ => b }",
        "def f(x: Boolean, a: Float, b: Float): Float = x match { case false => a; case _ => b }",
        "def f(x: Boolean, a: Double, b: Double): Double = x match { case true => return a; case _ => return b }",
        "def f(x: Boolean): Int = x match { case false => return 1; case _ => 2 }",
        "def f(x: Boolean, y: Boolean): Int = (x && y) match { case true => 1; case _ => 2 }",
    ] {
        guarded_match_plan::with_jvm(
            &format!("package example; class BooleanMatches {{ {method} }}"),
            |unit, defs| {
                dotty::jvm::emit_class(unit, unit.classes[0].class, defs).unwrap();
            },
        );
    }
}

#[test]
fn jvm_rejects_boolean_int_pattern_confusion() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    guarded_match_plan::with_jvm(
        "package example; class BooleanMatches { def f(x: Boolean): Int = x match { case true => 1; case _ => 2 } }",
        |unit, defs| {
            for mode in 0..3 {
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in unit.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                let pattern = ast
                    .iter()
                    .find_map(|(_, n)| {
                        if let TreeKind::CaseDef(c) = n.kind {
                            Some(c.pattern)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let error_node = match mode {
                    0 => {
                        ast.get_mut(pattern).ty = defs.int;
                        "inconsistent Boolean match literal"
                    }
                    1 => {
                        let literal = ast
                            .iter()
                            .find_map(|(id, n)| {
                                matches!(
                                    n.kind,
                                    TreeKind::Literal(dotty::core::ast::Literal {
                                        value: dotty::core::Constant::Int(1)
                                    })
                                )
                                .then_some(id)
                            })
                            .unwrap();
                        ast.get_mut(pattern).kind = ast.get(literal).kind.clone();
                        ast.get_mut(pattern).ty = ast.get(literal).ty;
                        "non-Boolean match pattern type"
                    }
                    _ => {
                        let wildcard = ast
                            .iter()
                            .find_map(|(_, n)| {
                                if let TreeKind::CaseDef(c) = n.kind {
                                    (c.pattern != pattern).then_some(c.pattern)
                                } else {
                                    None
                                }
                            })
                            .unwrap();
                        ast.get_mut(wildcard).ty = defs.int;
                        "non-Boolean match pattern type"
                    }
                };
                let invalid = dotty::jvm::JvmCompilationUnit {
                    store: unit.store,
                    typed_ast: &ast,
                    classes: unit.classes.clone(),
                };
                assert!(
                    matches!(dotty::jvm::emit_class(&invalid, unit.classes[0].class, defs), Err(dotty::jvm::JvmCodegenError::UnsupportedTypedExpression{node,..}) if node == error_node)
                );
            }
        },
    );
}
