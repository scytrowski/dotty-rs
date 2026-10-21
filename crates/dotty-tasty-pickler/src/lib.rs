//! Source semantic validation and type projection for Scala 3.9.0 TASTy.
//!
//! This increment validates a public, ordinary top-level class in one explicit
//! package, its default constructor and Int-result methods with literals,
//! formal references, same-class parameterless references, or bounded
//! same-class calls with one plain Int clause. Methods may have no value clause, one
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
//!
//! [`ClassStatePicklePlan`] is a separate plan-only contract for source-ordered
//! private primitive fields and bounded primary-constructor inputs. It does not
//! extend the current serializer's emitted forms.

mod artifact_name;
mod error;
mod expression;
mod output;
mod plan;
mod projection;
mod skeleton;

/// Hard ceiling for native recursive expression traversals.
pub(crate) const MAX_EXPRESSION_DEPTH: usize = 256;

pub(crate) const fn bounded_expression_depth(configured: usize) -> usize {
    if configured < MAX_EXPRESSION_DEPTH {
        configured
    } else {
        MAX_EXPRESSION_DEPTH
    }
}

pub use error::{
    Builtin, LimitKind, PickleError, ReferenceError, SourceRole, TypeRole, UnsupportedFeature,
};
pub use plan::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, DefaultConstructorPlan,
    LocalValueBinding, MethodPicklePlan, PicklePlan, PrimaryConstructorInputPlan, SourceMetadata,
    SourceTypedBindings, ValidatedClassStatePlan, ValidatedPicklePlan, ValidationLimits,
};
pub use projection::{EncodingContext, ProjectedFragment, ProjectionLimits};
pub use skeleton::{DeclarationSkeleton, OmittedMethodBody, SkeletonDeclaration};

pub use output::{PickleLimits, PickledTasty};
