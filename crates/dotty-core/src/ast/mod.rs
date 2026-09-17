//! Phase-indexed syntax and typed trees.
//!
//! Only the phase markers ([`Untyped`], [`Typed`]) exist so far; `Tree`,
//! `TreeKind`, and `AstArena` land in a later implementation step (see
//! `docs/dotty-core-design.md` §7).

mod phase;

pub use phase::{Typed, Untyped};
