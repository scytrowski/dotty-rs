//! Convenience aliases and a builder that cannot construct a typed tree
//! without a real `TypeId`.

use crate::ast::arena::AstArena;
use crate::ast::common::{Apply, ApplyKind, Ident};
use crate::ast::phase::Typed;
use crate::ast::tree::{Tree, TreeKind};
use crate::ids::{TreeId, TypeId};
use crate::names::Name;
use crate::source::SourceSpan;

pub type TypedTree = Tree<Typed>;
pub type TypedTreeId = TreeId<Typed>;
pub type TypedAst = AstArena<Typed>;

/// Builds `Tree<Typed>` nodes. Every constructor takes a `TypeId` argument,
/// so a typed tree cannot be built without one *by construction*, not just
/// by convention.
pub struct TypedAstBuilder<'a> {
    arena: &'a mut AstArena<Typed>,
}

impl<'a> TypedAstBuilder<'a> {
    pub fn new(arena: &'a mut AstArena<Typed>) -> Self {
        Self { arena }
    }

    pub fn ident(&mut self, name: Name, ty: TypeId, position: Option<SourceSpan>) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name,
                backquoted: false,
            }),
            position,
            ty,
        })
    }

    pub fn apply(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function,
                args,
                kind: ApplyKind::Regular,
            }),
            position,
            ty,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NameId;
    use crate::names::Namespace;

    #[test]
    fn ident_produces_a_tree_with_the_given_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);

        let id = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );

        assert_eq!(arena.get(id).ty, TypeId::new(2));
    }

    #[test]
    fn apply_references_its_function_and_args_with_the_given_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);

        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let arg = builder.ident(
            Name::new(NameId::new(3), Namespace::Term),
            TypeId::new(4),
            None,
        );
        let call = builder.apply(function, vec![arg], TypeId::new(5), None);

        assert_eq!(arena.get(call).ty, TypeId::new(5));
        let TreeKind::Apply(apply) = &arena.get(call).kind else {
            panic!("expected an Apply node");
        };
        assert_eq!(apply.function, function);
        assert_eq!(apply.args, vec![arg]);
    }
}
