//! Shared semantic foundation for the Dotty frontend and backend.
//!
//! `dotty-core` represents symbols, types, and syntax/typed trees as one
//! model shared by the source parser, the classfile loader, the TASTy
//! unpickler, and the namer/typer, instead of each of them inventing its own.
//! It knows nothing about the concrete lexer implementation, JVM classfiles,
//! TASTy's wire format, or type inference — see `docs/dotty-core-design.md`
//! for the full design and the boundary with those crates.
//!
//! This crate is under active construction; the identity, name,
//! source-position, type, symbol/scope, and AST phase layers exist so far.
//! The parser and typer that will consume this crate do not exist yet.

pub mod ast;
pub mod definitions;
pub mod diagnostics;
pub mod ids;
pub mod names;
pub mod packages;
pub mod resolution;
pub mod source;
pub mod store;
pub mod symbols;
pub mod token;
pub mod types;

/// Shared compiler foundation: source text, diagnostics, parser-facing token
/// contracts, identity, names, the type and symbol/scope model, and
/// phase-indexed syntax/typed trees.
pub mod core {
    pub use super::ast;
    pub use super::ast::*;
    pub use super::definitions;
    pub use super::definitions::*;
    pub use super::diagnostics;
    pub use super::diagnostics::*;
    pub use super::ids;
    pub use super::ids::*;
    pub use super::names;
    pub use super::names::*;
    pub use super::packages;
    pub use super::packages::*;
    pub use super::resolution;
    pub use super::resolution::*;
    pub use super::source;
    pub use super::source::*;
    pub use super::store;
    pub use super::store::*;
    pub use super::symbols;
    pub use super::symbols::*;
    pub use super::token;
    pub use super::token::*;
    pub use super::types;
    pub use super::types::*;
}

pub use ast::{AstArena, AstPhase, Modifiers, Tree, TreeKind, Typed, TypedAstBuilder, Untyped};
pub use definitions::Definitions;
pub use diagnostics::{Diagnostic, DiagnosticSeverity};
pub use ids::{
    AnnotationId, ClassfileOriginId, CompletionId, NameId, ScopeId, SourceId, SymbolId,
    TastyOriginId, TreeId, TypeId,
};
pub use names::{Name, NameInterner, Namespace, TermName, TypeName};
pub use packages::{EnteredPackage, Packages};
pub use resolution::{
    MemberRequest, MemberSelector, MemberSpace, NoResolver, ResolutionError, SymbolResolver,
};
pub use source::{
    LineIndex, SourceSpan, SourceText, SourceTextError, Span, SpanError, TextRange, TextRangeError,
    is_line_break_char,
};
pub use store::{SemanticStore, StoreCheckpoint};
pub use symbols::{
    OriginTable, Scope, ScopeArena, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
    SymbolOrigin, SymbolTable, Visibility,
};
pub use token::{
    HardKeyword, Punctuation, ScannerEvent, Token, TokenKind, TokenSource, TokenValue,
};
pub use types::{
    Annotation, AnnotationArena, AnnotationArgument, AnnotationArguments, AnnotationValue,
    ClassInfo, Constant, ErrorType, MatchType, MethodKind, MethodParam, MethodType, PolyType,
    ReservedTypeId, Type, TypeArena, TypeLambda, TypeParam, TypeRebindError, Variance,
    rebind_type_lambda,
};
