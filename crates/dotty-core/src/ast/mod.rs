//! Phase-indexed syntax and typed trees.

mod arena;
mod common;
mod modifiers;
mod phase;
mod tree;
mod typed;
mod untyped;

pub use arena::AstArena;
pub use common::{
    Alternative, Annotated, AppliedTypeTree, Apply, ApplyKind, Assign, Bind, Block, ByNameTypeTree,
    CaseDef, Closure, DefDef, Export, Ident, If, Import, ImportSelector, Inlined, LambdaTypeTree,
    Literal, Match, MatchTypeTree, NamedArg, New, PackageDef, Quote, QuotePattern, RefinedTypeTree,
    Return, Select, SingletonTypeTree, Splice, SplicePattern, Super, Template, This, Try,
    TypeApply, TypeBoundsTree, TypeDef, TypeTree, TypedExpr, UnApply, ValDef, While,
};
pub use modifiers::{Modifier, Modifiers, VisibilitySyntax};
pub use phase::{AstPhase, Typed, Untyped};
pub use tree::{Tree, TreeKind};
pub use typed::{TypedAst, TypedAstBuilder, TypedTree, TypedTreeId};
pub use untyped::{
    ContextBoundTypeTree, ContextBounds, ErrorNode, ErrorNodeKind, ExtensionMethods, ForDo,
    ForYield, Function, FunctionWithMods, GenAlias, GenCheckMode, GenFrom, InfixOp,
    InterpolatedString, ModuleDef, NumberKind, NumberLiteral, Parens, ParsedTry, PatDef,
    PolyFunction, PostfixOp, PrefixOp, Throw, Tuple, UntypedNode, UntypedTemplateMetadata, UseRef,
};
