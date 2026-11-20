use crate::core::{AstArena, SourceId, SourceSpan, TreeKind, Untyped};
use crate::typer::TyperError;

enum ErrorSite {
    Tree,
    Argument(usize),
    TypeApplication { argument: Option<usize> },
    IfCondition,
    WhileCondition,
    ReturnValue,
    AssignmentTarget { reported_lhs: Option<u32> },
    LocalInitializer,
    RecordedName(SourceSpan),
}

/// Locates a supported typer error in the untyped AST from the original parse.
///
/// - Call arity errors identify the whole call. Argument type mismatches on
///   explicit applications identify the offending argument.
/// - A type argument count mismatch identifies the explicit type application. A
///   bound violation identifies the type argument. Failed or ambiguous overload
///   selection identifies the whole call.
/// - Expected expression type mismatches identify the checked expression.
///   Local value type mismatches identify the initializer.
/// - An immutable or non-assignable assignment target identifies the left-hand side.
/// - A condition type mismatch points to the condition of the `if` or `while`.
///   A return type mismatch points to the returned expression, or the bare `return`.
/// - Missing, ambiguous and wrong-kind names use the position carried by the
///   error when present, otherwise the reported tree's position. A missing
///   member points to its reported expression (the whole selection for `x.y`).
///
/// A missing source position, a foreign tree, an inconsistent recorded name
/// position, or an unsupported tree shape or error variant yields `None`.
pub fn typer_error_span(error: &TyperError, ast: &AstArena<Untyped>) -> Option<SourceSpan> {
    typer_error_span_with(error, |_| Some(ast))
}

/// Resolves the same source position as [`typer_error_span`] across a batch
/// of source files. For a supported error, `ast_for` is called once with its
/// source ID. Return the untyped AST from the original parse, or `None` if
/// that source is unavailable. Unsupported errors do not trigger a lookup.
pub fn typer_error_span_with<'a>(
    error: &TyperError,
    ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
) -> Option<SourceSpan> {
    let (source, tree_index, site) = match *error {
        TyperError::ApplicationArityMismatch {
            source, tree_index, ..
        }
        | TyperError::OverloadApplicationNoApplicable {
            source, tree_index, ..
        }
        | TyperError::AmbiguousOverloadApplication {
            source, tree_index, ..
        }
        | TyperError::ExpectedExpressionTypeMismatch {
            source, tree_index, ..
        }
        | TyperError::ExtensionProviderDependency {
            source, tree_index, ..
        }
        | TyperError::MemberConversionDependency {
            source, tree_index, ..
        }
        | TyperError::MemberNotFound {
            source, tree_index, ..
        } => (source, tree_index, ErrorSite::Tree),
        TyperError::ExplicitTypeApplicationArityMismatch {
            source, tree_index, ..
        } => (
            source,
            tree_index,
            ErrorSite::TypeApplication { argument: None },
        ),
        TyperError::ExplicitTypeArgumentBoundViolation {
            source,
            tree_index,
            parameter_index,
            ..
        } => (
            source,
            tree_index,
            ErrorSite::TypeApplication {
                argument: Some(parameter_index),
            },
        ),
        TyperError::TermNameNotFound {
            source,
            tree_index,
            position,
            ..
        }
        | TyperError::AmbiguousTermReference {
            source,
            tree_index,
            position,
            ..
        }
        | TyperError::TypeNameNotFound {
            source,
            tree_index,
            position,
            ..
        }
        | TyperError::WrongTypeNameKind {
            source,
            tree_index,
            position,
            ..
        }
        | TyperError::AmbiguousTypeName {
            source,
            tree_index,
            position,
            ..
        } => (
            source,
            tree_index,
            position.map_or(ErrorSite::Tree, ErrorSite::RecordedName),
        ),
        TyperError::AssignmentTargetImmutable {
            source, tree_index, ..
        } => (
            source,
            tree_index,
            ErrorSite::AssignmentTarget { reported_lhs: None },
        ),
        TyperError::AssignmentLhsNotAssignable {
            source,
            tree_index,
            lhs_tree_index,
            ..
        } => (
            source,
            tree_index,
            ErrorSite::AssignmentTarget {
                reported_lhs: Some(lhs_tree_index),
            },
        ),
        TyperError::IfConditionTypeMismatch {
            source, tree_index, ..
        } => (source, tree_index, ErrorSite::IfCondition),
        TyperError::WhileConditionTypeMismatch {
            source, tree_index, ..
        } => (source, tree_index, ErrorSite::WhileCondition),
        TyperError::ReturnExpressionTypeMismatch {
            source, tree_index, ..
        } => (source, tree_index, ErrorSite::ReturnValue),
        TyperError::LocalValueTypeMismatch {
            source, tree_index, ..
        } => (source, tree_index, ErrorSite::LocalInitializer),
        TyperError::ApplicationArgumentTypeMismatch {
            source,
            tree_index,
            argument_index,
            ..
        } => (source, tree_index, ErrorSite::Argument(argument_index)),
        _ => return None,
    };
    let ast = ast_for(source)?;
    let (_, tree) = ast.iter().nth(tree_index as usize)?;
    let position = tree.position.filter(|span| span.source() == source)?;
    let child = match (site, &tree.kind) {
        (ErrorSite::Tree, _) => return Some(position),
        (ErrorSite::RecordedName(name), _) => {
            let tree_range = position.span().range();
            let name_range = name.span().range();
            return (name.source() == source
                && tree_range.start() <= name_range.start()
                && name_range.end() <= tree_range.end())
            .then_some(name);
        }
        (ErrorSite::TypeApplication { argument }, TreeKind::TypeApply(call)) => match argument {
            Some(index) => *call.args.get(index)?,
            None => return Some(position),
        },
        (ErrorSite::Argument(index), TreeKind::Apply(call)) => *call.args.get(index)?,
        (ErrorSite::AssignmentTarget { reported_lhs }, TreeKind::Assign(assign))
            if reported_lhs.is_none_or(|index| index == assign.lhs.index()) =>
        {
            assign.lhs
        }
        (ErrorSite::IfCondition, TreeKind::If(expr)) => expr.cond,
        (ErrorSite::WhileCondition, TreeKind::While(expr)) => expr.cond,
        (ErrorSite::ReturnValue, TreeKind::Return(expr)) => match expr.expr {
            Some(value) => value,
            None => return Some(position),
        },
        (ErrorSite::LocalInitializer, TreeKind::ValDef(def)) => def.rhs?,
        _ => return None,
    };
    ast.try_get(child)?
        .position
        .filter(|span| span.source() == source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        Apply, ApplyKind, Constant, Definitions, Literal, SemanticStore, SourceId, Span, TextRange,
        Tree, TreeKind,
    };

    fn parse_ast(text: &str, source: SourceId, store: &mut SemanticStore) -> AstArena<Untyped> {
        let parsed = crate::parser::DetachedParse::parse(
            crate::core::SourceText::new(text).unwrap(),
            source,
            crate::lexer::ContextualScanner::new(text).unwrap(),
            Default::default(),
        )
        .into_session(&mut store.names);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        parsed.ast
    }

    #[test]
    fn explicit_bound_violation_requires_an_argument_of_the_reported_type_application() {
        let source = SourceId::from_index(1);
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let ast = parse_ast("class C { def f(): Int = g[Int](0) }", source, &mut store);
        let (id, argument) = ast
            .iter()
            .find_map(|(id, tree)| match &tree.kind {
                TreeKind::TypeApply(call) => Some((id, call.args[0])),
                _ => None,
            })
            .unwrap();
        let error = |tree_index, parameter_index| TyperError::ExplicitTypeArgumentBoundViolation {
            source,
            tree_index,
            parameter_index,
            argument: defs.int,
            bound: defs.boolean,
            side: crate::typer::TypeArgumentBoundSide::Upper,
        };
        assert_eq!(
            typer_error_span(&error(id.index(), 0), &ast),
            ast.get(argument).position
        );
        assert_eq!(typer_error_span(&error(id.index(), 1), &ast), None);
        assert_eq!(typer_error_span(&error(argument.index(), 0), &ast), None);
    }

    #[test]
    fn non_assignable_target_must_match_the_reported_left_hand_side() {
        let source = SourceId::from_index(1);
        let ast = parse_ast(
            "class C { def f(n: Int): Unit = n = 2 }",
            source,
            &mut SemanticStore::new(),
        );
        let (id, assign) = ast
            .iter()
            .find_map(|(id, tree)| match &tree.kind {
                TreeKind::Assign(assign) => Some((id, assign)),
                _ => None,
            })
            .unwrap();
        let error = |lhs_tree_index| TyperError::AssignmentLhsNotAssignable {
            source,
            tree_index: id.index(),
            lhs_tree_index,
            expression_kind: "Ident",
        };
        assert_eq!(
            typer_error_span(&error(assign.lhs.index()), &ast),
            ast.get(assign.lhs).position
        );
        assert_eq!(typer_error_span(&error(assign.rhs.index()), &ast), None);
    }

    #[test]
    fn a_return_with_an_unpositioned_value_is_distinct_from_a_bare_return() {
        let source = SourceId::from_index(1);
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut ast = parse_ast(
            "class C { def f(): Int = return false }",
            source,
            &mut store,
        );
        let (tree, value) = ast
            .iter()
            .find_map(|(id, tree)| match &tree.kind {
                TreeKind::Return(expr) => Some((id, expr.expr?)),
                _ => None,
            })
            .unwrap();
        let error = TyperError::ReturnExpressionTypeMismatch {
            source,
            tree_index: tree.index(),
            actual: defs.boolean,
            expected: defs.int,
        };
        assert_eq!(typer_error_span(&error, &ast), ast.get(value).position);
        ast.get_mut(value).position = None;
        assert_eq!(typer_error_span(&error, &ast), None);
    }

    #[test]
    fn recorded_name_position_must_belong_to_the_reported_source_tree() {
        let source = SourceId::from_index(1);
        let other = SourceId::from_index(2);
        let text = "class A { def f() = missing }";
        let mut store = SemanticStore::new();
        let ast = parse_ast(text, source, &mut store);
        let foreign = parse_ast(text, other, &mut store);
        let (tree, name, position) = ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(id) if store.names.resolve(id.name.text()) == "missing" => {
                    Some((tree, id.name, node.position.unwrap()))
                }
                _ => None,
            })
            .unwrap();
        let error = |position| TyperError::TermNameNotFound {
            source,
            tree_index: tree.index(),
            name,
            position,
        };
        let range = position.span().range();
        let recorded = SourceSpan::new(source, Span::new(range, Some(range.start())).unwrap());
        assert_eq!(
            typer_error_span(&error(Some(recorded)), &ast),
            Some(recorded)
        );
        assert_eq!(typer_error_span(&error(None), &ast), Some(position));
        assert_eq!(typer_error_span(&error(Some(recorded)), &foreign), None);
        assert_eq!(
            typer_error_span(&error(Some(SourceSpan::new(other, recorded.span()))), &ast),
            None
        );
        let outside = SourceSpan::new(source, Span::without_point(TextRange::new(0, 1).unwrap()));
        assert_eq!(typer_error_span(&error(Some(outside)), &ast), None);
    }

    #[test]
    fn local_value_mismatch_requires_an_initializer_from_the_same_source() {
        let source = SourceId::from_index(3);
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut ast = parse_ast(
            "class A { def f(): Int = { val n: Int = false; n } }",
            source,
            &mut store,
        );
        let (local, rhs) = ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(def) => Some((tree, def.rhs?)),
                _ => None,
            })
            .unwrap();
        let error = TyperError::LocalValueTypeMismatch {
            source,
            tree_index: local.index(),
            actual: defs.boolean,
            expected: defs.int,
        };
        let position = ast.get(rhs).position.unwrap();
        assert_eq!(typer_error_span(&error, &ast), Some(position));
        ast.get_mut(rhs).position = Some(SourceSpan::new(SourceId::from_index(4), position.span()));
        assert_eq!(typer_error_span(&error, &ast), None);
        let TreeKind::ValDef(def) = &mut ast.get_mut(local).kind else {
            unreachable!()
        };
        def.rhs = None;
        assert_eq!(typer_error_span(&error, &ast), None);
    }

    #[test]
    fn argument_mismatch_requires_a_positioned_argument_in_the_same_source_as_the_call() {
        let source = SourceId::from_index(7);
        let other = SourceId::from_index(8);
        let position = |source, start, end| {
            SourceSpan::new(
                source,
                Span::without_point(TextRange::new(start, end).unwrap()),
            )
        };
        let argument_span = position(source, 2, 6);
        let mut ast = AstArena::<Untyped>::new();
        let argument = ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(true),
            }),
            position: Some(argument_span),
            ty: (),
        });
        let call = ast.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function: argument,
                args: vec![argument],
                kind: ApplyKind::Regular,
            }),
            position: Some(position(source, 0, 7)),
            ty: (),
        });
        let defs = Definitions::bootstrap(&mut SemanticStore::new());
        let mismatch = |tree_index, argument_index| TyperError::ApplicationArgumentTypeMismatch {
            source,
            tree_index,
            argument_index,
            actual: defs.boolean,
            expected: defs.int,
        };
        let error = mismatch(call.index(), 0);
        assert_eq!(typer_error_span(&error, &ast), Some(argument_span));
        assert_eq!(typer_error_span(&mismatch(call.index(), 1), &ast), None);
        assert_eq!(typer_error_span(&mismatch(argument.index(), 0), &ast), None);
        ast.get_mut(call).position = Some(position(other, 0, 7));
        assert_eq!(typer_error_span(&error, &ast), None);
        ast.get_mut(call).position = Some(position(source, 0, 7));
        ast.get_mut(argument).position = Some(position(other, 2, 6));
        assert_eq!(typer_error_span(&error, &ast), None);
        ast.get_mut(argument).position = None;
        assert_eq!(typer_error_span(&error, &ast), None);
    }

    #[test]
    fn arity_error_requires_a_position_at_the_reported_slot_in_its_source() {
        let source = SourceId::from_index(7);
        let position = SourceSpan::new(
            source,
            Span::new(TextRange::new(3, 9).unwrap(), Some(5)).unwrap(),
        );
        let mut ast = AstArena::<Untyped>::new();
        let literal = ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let call = ast.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function: literal,
                args: vec![],
                kind: ApplyKind::Regular,
            }),
            position: Some(position),
            ty: (),
        });
        let error = |source, tree_index| TyperError::ApplicationArityMismatch {
            source,
            tree_index,
            expected: 1,
            actual: 0,
        };
        assert_eq!(
            typer_error_span(&error(source, call.index()), &ast),
            Some(position)
        );
        assert_eq!(
            typer_error_span(&error(SourceId::from_index(8), call.index()), &ast),
            None
        );
        assert_eq!(
            typer_error_span(&error(source, literal.index()), &ast),
            None
        );
        assert_eq!(typer_error_span(&error(source, u32::MAX), &ast), None);
        assert_eq!(
            typer_error_span_with(&error(source, call.index()), |id| {
                assert_eq!(id, source);
                Some(&ast)
            }),
            Some(position)
        );
        assert_eq!(
            typer_error_span_with(&error(source, call.index()), |_| None),
            None
        );
        assert_eq!(
            typer_error_span_with(&error(SourceId::from_index(8), call.index()), |_| Some(
                &ast
            )),
            None
        );
        let type_arguments = TyperError::ExplicitTypeApplicationArityMismatch {
            source,
            tree_index: call.index(),
            expected: 1,
            actual: 0,
        };
        assert_eq!(typer_error_span(&type_arguments, &ast), None);
        let unsupported = TyperError::TreeOutsideArena {
            source,
            tree_index: u32::MAX,
        };
        assert_eq!(
            typer_error_span_with(&unsupported, |_| panic!("unexpected AST lookup")),
            None
        );
    }
}
