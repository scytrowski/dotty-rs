//! Source declaration signature completion.
//!
//! The typer consumes a parsed and named untyped source tree. It does not
//! parse source, allocate declaration symbols, or load classpath entries.
//! [`SourceTyper::type_expression`] builds typed AST nodes for literals, term
//! identifiers, `this`, unique member selections, plain method applications,
//! explicit positional type applications for one polymorphic callee, and
//! source type ascriptions, assignments to mutable references, ordinary `if`
//! expressions with a bounded branch join, condition-bearing `while`
//! expressions, and local `return` expressions in methods with explicit result
//! types.
//! [`SourceTyper::type_expression_expected`] checks
//! the widened expression type against a semantic expected type while keeping
//! the expression node's own type unchanged.
//! Type arguments are projected in the expression's lexical context and
//! represented by typed `TypeTree` nodes carrying their semantic `TypeId`;
//! each source type-argument root maps to that node in `SourceTypedIndex`,
//! while nested type syntax is retained in its projected semantic type.
//! Unsupported expression forms return typed errors. Semantic type trees and
//! source-to-typed mappings are cached by source tree identity.
//!
//! Qualified type references use `ThisType` for an enclosing class prefix and
//! the canonical package `TypeRef` (with `no_prefix`) for package prefixes.
//! Call [`SourceTyper::with_resolver`] to supply semantic symbols outside the
//! entered source/session state; the default resolver is `NoResolver`.
//!
//! Type-name lookup checks lexical scopes from the innermost context outward.
//! Current-unit package definitions precede explicit imports, which precede
//! wildcard imports and package definitions from other source units. Imports
//! at the same lexical depth have equal precedence: distinct matching symbols
//! are ambiguous, while repeated bindings of the same symbol are accepted.
//! Import contexts are position-aware; a declaration sees only imports
//! preceding it in its source context chain.

mod source_type_index;
mod source_typed_index;
mod typer;
mod types;

pub use source_type_index::SourceTypeIndex;
pub use source_typed_index::{ConflictingTypedTree, SourceTypedIndex};
pub use typer::{
    ExpressionContext, ExpressionScopeId, MAX_MEMBER_LOOKUP_DEPTH, MAX_TYPE_RELATION_DEPTH,
    MAX_TYPE_RELATION_VIEWS, MAX_UNION_RELATION_COMPARISONS, MemberCandidate, MemberLookupError,
    SourceTyper, TypeArgumentBoundSide, TypeRelationError, TyperError,
};
pub use types::{
    MAX_TYPE_NORMALIZATION_DEPTH, SymbolInfoState, TypeNormalizeError, TypeNormalizer,
};
