//! Scala 3.9.0 source parser infrastructure.
//!
//! The parser consumes the parser-facing contracts from [`dotty_core`]. It
//! deliberately does not depend on the concrete lexer implementation, so
//! parser tests can use an in-memory [`dotty_core::TokenSource`].
//!
//! [`parse_compilation_unit`] returns an untyped arena, package root, and
//! structured parser diagnostics, including recovered trees for malformed or
//! unsupported syntax. Handle lexer diagnostics at the token-source boundary
//! as well. Parsing does not establish semantic or backend support.
//!
//! [`DetachedParse`] supports worker-local parsing followed by binding names
//! into a shared session interner. Bind files in a stable input order for
//! deterministic name IDs. Grammar coverage remains an incremental subset.

#![deny(missing_docs)]
#![doc = include_str!("guide.md")]

mod call_arguments;
pub use call_arguments::{
    CallArgumentList, ConstructorSyntax, NamedCallArgument, SourceCallArguments,
};
mod case_clauses;
mod class_definitions;
mod compilation_unit;
mod member_names;
pub use member_names::{SourceMemberNames, SourceSelection, SourceTermName};
mod context;
mod cursor;
mod definitions;
mod detached;
mod diagnostics;
mod expr;
mod extensions;
mod givens;
mod imports;
mod infix;
mod literals;
mod modifiers;
mod names;
mod packages;
mod parameters;
mod parser;
mod patterns;
mod recovery;
mod references;
mod spans;
mod statements;
mod templates;
mod type_definitions;
mod type_params;
mod types;

pub use compilation_unit::{
    ParseResult, SourceCallNames, parse_compilation_unit, parse_expression_fragment,
};
pub use context::{Location, ParamOwner, ParseContext, ParseKind, ParserFeatures};
pub use cursor::{Cursor, CursorCheckpoint};
pub use detached::DetachedParse;
#[cfg(test)]
pub use diagnostics::ParseDiagnosticKind;
pub use diagnostics::{
    CaseIssue, ClassDefinitionIssue, ContextualParameterClause, DeclarationIssue,
    ExpressionApplicationTarget, ExpressionIssue, ExtensionIssue, GivenIssue, ImportIssue,
    LayoutExpressionContext, LayoutIssue, ModifierIssue, PackageIssue, ParameterIssue,
    ParameterMutability, ParseDiagnostic, ParseIssue, PatternIssue, StatementIssue,
    StatementSequenceContext, TemplateIssue, TypeDefinitionIssue, TypeFunctionArrow, TypeIssue,
    TypeParamIssue, ValueDefinitionKind,
};
pub use infix::{OpInfo, is_assignment_operator, is_right_associative, precedence};
pub use names::KnownNames;
pub use parser::Parser;
pub use patterns::parse_pattern_fragment;
pub use recovery::RecoverySet;
pub use spans::Mark;
