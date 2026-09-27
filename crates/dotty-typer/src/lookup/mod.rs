//! Semantic lookup operations used by the typer.

mod members;

pub use members::{MAX_MEMBER_LOOKUP_DEPTH, MemberCandidate, MemberLookupError};
