//! Read-only source semantic input for the minimal Scala 3.9.0 TASTy pickler.
//!
//! This increment validates a public, ordinary top-level class in one explicit
//! package, its default constructor and one parameterless Int-literal method.
//! It does not encode trees, return TASTy bytes, load dependencies or type source.
//! See `docs/tasty-pickler-design.md` and `docs/tasty-pickler-oracle.md`.
//!
//! All IDs, indices and arenas must originate in one compilation session. Core
//! IDs are arena-relative, so validation detects out-of-range/stale references
//! and inconsistent relationships, but cannot authenticate a foreign same-slot
//! ID or a dishonest source-to-typed binding provider. Callers must preserve that
//! session invariant. Validation only borrows semantic data and publishes no files.

mod error;
mod plan;
mod projection;

pub use error::{
    Builtin, LimitKind, PickleError, ReferenceError, SourceRole, TypeRole, UnsupportedFeature,
};
pub use plan::{
    DefaultConstructorPlan, MethodPicklePlan, PicklePlan, SourceMetadata, SourceTypedBindings,
    ValidatedPicklePlan, ValidationLimits,
};
pub use projection::{EncodingContext, ProjectedFragment, ProjectionLimits};
