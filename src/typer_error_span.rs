use crate::core::{AstArena, SourceSpan, Untyped};
use crate::typer::TyperError;

/// Returns the source position of a call rejected with
/// [`TyperError::ApplicationArityMismatch`].
///
/// Supply the untyped AST from the parse that produced the error. The tree
/// slot must exist and carry a position in the source identified by the error.
/// Other typer errors currently return `None`.
pub fn typer_error_span(error: &TyperError, ast: &AstArena<Untyped>) -> Option<SourceSpan> {
    let TyperError::ApplicationArityMismatch {
        source, tree_index, ..
    } = error
    else {
        return None;
    };
    let (_, tree) = ast.iter().nth(*tree_index as usize)?;
    tree.position.filter(|span| span.source() == *source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        Apply, ApplyKind, Constant, Literal, SourceId, Span, TextRange, Tree, TreeKind,
    };

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
        let type_arguments = TyperError::ExplicitTypeApplicationArityMismatch {
            source,
            tree_index: call.index(),
            expected: 1,
            actual: 0,
        };
        assert_eq!(typer_error_span(&type_arguments, &ast), None);
    }
}
