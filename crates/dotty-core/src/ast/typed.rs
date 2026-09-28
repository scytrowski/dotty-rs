//! Convenience aliases and a builder that cannot construct a typed tree
//! without a real `TypeId`.

use crate::ast::arena::AstArena;
use crate::ast::common::{Apply, ApplyKind, Ident, Literal, Select, This, TypeApply, TypeTree};
use crate::ast::phase::Typed;
use crate::ast::tree::{Tree, TreeKind};
use crate::ids::{TreeId, TypeId};
use crate::names::Name;
use crate::source::SourceSpan;
use crate::types::Constant;

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
        self.ident_with_backquoted(name, false, ty, position)
    }

    /// Allocates a typed identifier while retaining its source backquotes.
    pub fn ident_with_backquoted(
        &mut self,
        name: Name,
        backquoted: bool,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Ident(Ident { name, backquoted }),
            position,
            ty,
        })
    }

    /// Allocates a typed literal with its already-determined semantic type.
    pub fn literal(
        &mut self,
        value: Constant,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Literal(Literal { value }),
            position,
            ty,
        })
    }

    /// Allocates a typed member selection referencing a typed qualifier.
    pub fn select(
        &mut self,
        qualifier: TreeId<Typed>,
        name: Name,
        backquoted: bool,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier,
                name,
                backquoted,
            }),
            position,
            ty,
        })
    }

    /// Allocates a typed `this` reference with an optional source qualifier.
    pub fn this(
        &mut self,
        qual: Option<Name>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::This(This { qual }),
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
        self.apply_with_kind(function, args, ApplyKind::Regular, ty, position)
    }

    /// Allocates a typed application while preserving its source application kind.
    pub fn apply_with_kind(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        kind: ApplyKind,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function,
                args,
                kind,
            }),
            position,
            ty,
        })
    }

    /// Allocates a typed explicit type application with its instantiated
    /// semantic result type.
    pub fn type_apply(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::TypeApply(TypeApply { function, args }),
            position,
            ty,
        })
    }

    /// Reifies a projected source type as a typed type tree. Type trees carry
    /// their meaning in `Tree::ty`; the source typer records the source mapping.
    pub fn type_tree(&mut self, ty: TypeId, position: Option<SourceSpan>) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::TypeTree(TypeTree),
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
        assert_eq!(apply.kind, ApplyKind::Regular);
    }

    #[test]
    fn apply_with_kind_preserves_using_applications() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);
        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );

        let call =
            builder.apply_with_kind(function, Vec::new(), ApplyKind::Using, TypeId::new(3), None);

        let TreeKind::Apply(apply) = &arena.get(call).kind else {
            panic!("expected an Apply node");
        };
        assert_eq!(apply.kind, ApplyKind::Using);
    }

    #[test]
    fn literal_keeps_the_constant_and_explicit_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);
        let ty = TypeId::new(2);

        let literal = builder.literal(Constant::Int(42), ty, None);

        assert_eq!(
            arena.get(literal).kind,
            TreeKind::Literal(Literal {
                value: Constant::Int(42)
            })
        );
        assert_eq!(arena.get(literal).ty, ty);
    }

    #[test]
    fn select_keeps_the_typed_qualifier_and_explicit_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);
        let qualifier = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let selected_name = Name::new(NameId::new(3), Namespace::Term);
        let selected_type = TypeId::new(4);

        let select = builder.select(qualifier, selected_name, true, selected_type, None);

        assert_eq!(
            arena.get(select).kind,
            TreeKind::Select(Select {
                qualifier,
                name: selected_name,
                backquoted: true,
            })
        );
        assert_eq!(arena.get(select).ty, selected_type);
    }

    #[test]
    fn this_keeps_its_optional_qualifier_and_explicit_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);
        let qualifier = Name::new(NameId::new(3), Namespace::Type);
        let ty = TypeId::new(4);

        let this = builder.this(Some(qualifier), ty, None);

        assert_eq!(
            arena.get(this).kind,
            TreeKind::This(This {
                qual: Some(qualifier)
            })
        );
        assert_eq!(arena.get(this).ty, ty);
    }

    #[test]
    fn type_apply_preserves_function_arguments_and_instantiated_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);
        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let argument = builder.type_tree(TypeId::new(3), None);
        let application = builder.type_apply(function, vec![argument], TypeId::new(4), None);

        assert_eq!(arena.get(application).ty, TypeId::new(4));
        let TreeKind::TypeApply(type_apply) = &arena.get(application).kind else {
            panic!("expected TypeApply")
        };
        assert_eq!(type_apply.function, function);
        assert_eq!(type_apply.args, vec![argument]);
    }

    #[test]
    fn type_tree_carries_the_projected_type() {
        let mut arena = TypedAst::new();
        let mut builder = TypedAstBuilder::new(&mut arena);
        let ty = TypeId::new(7);

        let tree = builder.type_tree(ty, None);

        assert_eq!(arena.get(tree).ty, ty);
        assert!(matches!(arena.get(tree).kind, TreeKind::TypeTree(TypeTree)));
    }
}
