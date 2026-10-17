//! Source semantic validation and type projection for Scala 3.9.0 TASTy.
//!
//! This increment validates a public, ordinary top-level class in one explicit
//! package, its default constructor and Int-result methods with literals,
//! formal references, same-class parameterless references, or bounded
//! same-class one-Int-argument calls. Methods may have no value clause, one
//! explicit empty clause, or one plain Int-parameter clause.
//! [`EncodingContext`] projects names, types and canonical references into owned
//! wire fragments. [`ValidatedPicklePlan::declaration_skeleton`] assembles a
//! structurally validated, explicitly incomplete declaration document.
//! [`ValidatedPicklePlan::pickle`] includes the actual typed Int RHS in an owned
//! file. Independent Scala interoperability is checked by the pinned oracle.
//! See `docs/tasty-pickler-design.md` and `docs/tasty-pickler-oracle.md`.
//!
//! All IDs, indices and arenas must originate in one compilation session. Core
//! IDs are arena-relative, so validation detects out-of-range/stale references
//! and inconsistent relationships, but cannot authenticate a foreign same-slot
//! ID or a dishonest source-to-typed binding provider. Callers must preserve that
//! session invariant. Validation only borrows semantic data and publishes no files.

mod artifact_name;
mod error;
mod expression;
mod output;
mod plan;
mod projection;
mod skeleton;

pub use error::{
    Builtin, LimitKind, PickleError, ReferenceError, SourceRole, TypeRole, UnsupportedFeature,
};
pub use plan::{
    DefaultConstructorPlan, MethodPicklePlan, PicklePlan, SourceMetadata, SourceTypedBindings,
    ValidatedPicklePlan, ValidationLimits,
};
pub use projection::{EncodingContext, ProjectedFragment, ProjectionLimits};
pub use skeleton::{DeclarationSkeleton, OmittedMethodBody, SkeletonDeclaration};

pub use output::{PickleLimits, PickledTasty};
