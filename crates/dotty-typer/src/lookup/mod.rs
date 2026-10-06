//! Semantic lookup operations used by the typer.

mod members;

pub(in crate::typer) use members::class_symbol_for_type;
pub use members::{MAX_MEMBER_LOOKUP_DEPTH, MemberCandidate, MemberLookupError};
