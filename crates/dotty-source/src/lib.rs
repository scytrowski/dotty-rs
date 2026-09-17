//! Source text, ranges, and line-index support for the Dotty frontend.

mod line_index;
mod source_text;
mod span;

pub use line_index::LineIndex;
pub use source_text::{SourceText, SourceTextError};
pub use span::{TextRange, TextRangeError};
