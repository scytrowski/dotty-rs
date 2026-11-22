//! Source-side naming infrastructure built on the shared [`dotty_core`] AST
//! and semantic model.
//!
//! This crate deliberately does not depend on the parser implementation. It
//! accepts phase-indexed trees from `dotty-core`, so parser clients can pass
//! their arena without exposing parser-private types to naming.
//!
//! [`name_compilation_unit_with_diagnostics`] separates confirmed source
//! rejections from structural [`NamerError`]s and returns a canonical source
//! semantic index on success. Naming enters declarations but leaves signature
//! and expression completion to the typer. Failures roll back the naming
//! transaction; retain the original AST, source identity, packages, and store
//! together for subsequent phases.

#![deny(missing_docs)]
#![doc = include_str!("guide.md")]

mod diagnostics;
mod naming;

pub use diagnostics::{NamerDiagnostic, NamerIssue, VisibilityAccess};
pub use dotty_core::{SourceContext, SourceContextId, SourceDefinition, SourceSemanticIndex};
pub use naming::{
    NamerError, NamingOutcome, SourceParseStatus, name_compilation_unit,
    name_compilation_unit_with_diagnostics,
};
