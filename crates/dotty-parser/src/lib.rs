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
mod compilation_unit;
mod context;
mod cursor;
mod definitions;
mod diagnostics;
mod expr;
mod infix;
mod literals;
mod names;
mod parameters;
mod parser;
mod patterns;
mod recovery;
mod spans;
mod statements;
mod type_definitions;
mod type_params;
mod types;

pub use compilation_unit::{ParseResult, parse_compilation_unit};
pub use context::{Location, ParamOwner, ParseContext, ParseKind, ParserFeatures};
pub use cursor::{Cursor, CursorCheckpoint};
pub use diagnostics::{ParseDiagnostic, ParseDiagnosticKind};
pub use infix::{OpInfo, is_assignment_operator, is_right_associative, precedence};
pub use names::KnownNames;
pub use parser::Parser;
pub use patterns::parse_pattern_fragment;
pub use recovery::RecoverySet;
pub use spans::Mark;
