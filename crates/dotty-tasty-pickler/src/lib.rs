//! Source semantic validation and type projection for Scala 3.9.0 TASTy.
//!
//! This increment validates a public, ordinary top-level class in one explicit
//! package, its default constructor and one parameterless Int-literal method.
//! [`EncodingContext`] projects names, types and canonical references into owned
//! wire fragments. [`ValidatedPicklePlan::declaration_skeleton`] assembles a
//! structurally validated, explicitly incomplete declaration document. Concrete
//! method bodies, completed artifacts, loading and source typing remain outside
//! this increment.
//! See `docs/tasty-pickler-design.md` and `docs/tasty-pickler-oracle.md`.
//!
//! All IDs, indices and arenas must originate in one compilation session. Core
//! IDs are arena-relative, so validation detects out-of-range/stale references
//! and inconsistent relationships, but cannot authenticate a foreign same-slot
//! ID or a dishonest source-to-typed binding provider. Callers must preserve that
//! session invariant. Validation only borrows semantic data and publishes no files.

mod artifact_name;
mod error;
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
