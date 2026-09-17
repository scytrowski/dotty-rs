//! Scala 3.9.0 lexical frontend.

mod cursor;
mod identifier;
mod lexer;
mod raw_token;
mod trivia;

pub use cursor::{Cursor, CursorError};
pub use lexer::{RawLexer, RawLexerError};
pub use raw_token::{HardKeyword, Punctuation, RawItem, RawToken, RawTokenKind};
pub use trivia::{Trivia, TriviaKind};
