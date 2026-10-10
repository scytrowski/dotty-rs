//! Scala 3.9.0 source parser infrastructure.
//!
//! The parser consumes the parser-facing contracts from [`dotty_core`]. It
//! deliberately does not depend on the concrete lexer implementation, so
//! parser tests can use an in-memory [`dotty_core::TokenSource`].
//!
//! The handwritten recursive-descent parser is implemented incrementally.
//! Its public compilation-unit entry point exists, while grammar coverage is
//! intentionally incomplete.

mod case_clauses;
mod class_definitions;
mod compilation_unit;
mod context;
mod cursor;
mod definitions;
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

pub use compilation_unit::{ParseResult, parse_compilation_unit, parse_expression_fragment};
pub use context::{Location, ParamOwner, ParseContext, ParseKind, ParserFeatures};
pub use cursor::{Cursor, CursorCheckpoint};
pub use diagnostics::{
    CaseIssue, ClassDefinitionIssue, ContextualParameterClause, DeclarationIssue,
    ExpressionApplicationTarget, ExpressionIssue, ExtensionIssue, GivenIssue, ImportIssue,
    LayoutExpressionContext, LayoutIssue, ModifierIssue, PackageIssue, ParameterIssue,
    ParameterMutability, ParseDiagnostic, ParseDiagnosticKind, ParseIssue, PatternIssue,
    StatementIssue, StatementSequenceContext, TypeDefinitionIssue, TypeFunctionArrow, TypeIssue,
    TypeParamIssue, ValueDefinitionKind,
};
pub use infix::{OpInfo, is_assignment_operator, is_right_associative, precedence};
pub use names::KnownNames;
pub use parser::Parser;
pub use patterns::parse_pattern_fragment;
pub use recovery::RecoverySet;
pub use spans::Mark;
