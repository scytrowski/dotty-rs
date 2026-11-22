//! Scala 3.9.0 lexical frontend.
//!
//! [`RawLexer`] produces the lossless raw stream; [`ContextualScanner`]
//! implements the shared parser-facing token source, including parser feedback
//! for layout. Both construction failures and collected [`LexerDiagnostic`]s
//! must be handled: a recovered token stream does not imply clean source.
//! Positions are UTF-8 byte offsets. This crate performs no source naming,
//! type checking, classpath loading, or JVM emission.

mod cursor;
mod diagnostics;
mod lexer;
mod raw_token;
mod scanner;
mod token_buffer;
mod trivia;
mod xml;

pub use cursor::{Cursor, CursorError};
pub use diagnostics::{
    EscapeIssue, InterpolationIssue, LexerDiagnostic, LexerIssue, LiteralIssue,
    NonIntegerLiteralKind, NumericComponent, NumericIssue, XmlIssue,
};
pub use lexer::{RawLexer, RawLexerError};
pub use raw_token::{HardKeyword, Punctuation, RawItem, RawToken, RawTokenKind};
pub use scanner::ContextualScanner;
pub use trivia::{Trivia, TriviaKind};
