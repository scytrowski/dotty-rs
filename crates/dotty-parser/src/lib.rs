//! Scala 3.9.0 source parser infrastructure.
//!
//! The parser consumes the parser-facing contracts from [`dotty_core`]. It
//! deliberately does not depend on the concrete lexer implementation, so
//! parser tests can use an in-memory [`dotty_core::TokenSource`].
//!
//! The handwritten recursive-descent parser is introduced incrementally.
//! Until its public entry point is added, this crate only establishes the
//! workspace and dependency boundary.

mod context;
mod cursor;
mod diagnostics;
mod parser;
mod recovery;
mod spans;

pub use context::{Location, ParamOwner, ParseContext, ParseKind};
pub use cursor::Cursor;
pub use diagnostics::{ParseDiagnostic, ParseDiagnosticKind};
pub use parser::Parser;
pub use recovery::RecoverySet;
pub use spans::Mark;
