use crate::core::{AstArena, SourceId, SourceSpan, TreeKind, Untyped};
use crate::typer::TyperError;

enum ErrorSite {
    Tree,
    Argument(usize),
    LocalInitializer,
}

/// Locates a call rejected with [`TyperError::ApplicationArityMismatch`] or
/// the offending argument of an explicit application rejected with
/// [`TyperError::ApplicationArgumentTypeMismatch`]. For
/// [`TyperError::ExpectedExpressionTypeMismatch`] it locates the expression,
/// and for [`TyperError::LocalValueTypeMismatch`] the initializer of the local value.
///
/// Supply the untyped AST from the parse that produced the error. The
/// reported tree and any selected child must carry positions in the error's source.
/// An unavailable position, unsupported tree shape for the argument lookup,
/// or unsupported error variant yields `None`.
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
        | TyperError::ExpectedExpressionTypeMismatch {
            source, tree_index, ..
        } => (source, tree_index, ErrorSite::Tree),
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
        (ErrorSite::Argument(index), TreeKind::Apply(call)) => *call.args.get(index)?,
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
        assert_eq!(
            typer_error_span_with(&type_arguments, |_| panic!("unexpected AST lookup")),
            None
        );
    }
}
