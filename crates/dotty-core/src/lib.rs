//! Shared semantic foundation for the Dotty frontend and backend.
//!
//! `dotty-core` represents symbols, types, and syntax/typed trees as one
//! model shared by the source parser, the classfile loader, the TASTy
//! unpickler, and the namer/typer, instead of each of them inventing its own.
//! It knows nothing about lexical syntax, JVM classfiles, TASTy's wire
//! format, or type inference — see `docs/dotty-core-design.md` for the full
//! design and the boundary with those crates.
//!
//! This crate is under active construction; the identity, name,
//! source-position, type, symbol/scope, and AST phase layers exist so far.
//! The parser and typer that will consume this crate do not exist yet.

pub mod ast;
pub mod ids;
pub mod names;
pub mod source;
pub mod store;
pub mod symbols;
pub mod types;

pub use ast::{AstArena, AstPhase, Modifiers, Tree, TreeKind, Typed, TypedAstBuilder, Untyped};
pub use ids::{
    AnnotationId, ClassfileOriginId, CompletionId, NameId, ScopeId, SourceId, SymbolId,
    TastyOriginId, TreeId, TypeId,
};
pub use names::{Name, NameInterner, Namespace, TermName, TypeName};
pub use source::{SourceSpan, Span};
pub use store::SemanticStore;
pub use symbols::{
    Scope, ScopeArena, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin,
    SymbolTable,
};
pub use types::{
    Annotation, AnnotationArena, ClassInfo, Constant, ErrorType, MatchType, MethodKind,
    MethodParam, MethodType, PolyType, ReservedTypeId, Type, TypeArena, TypeLambda, TypeParam,
    Variance,
};
