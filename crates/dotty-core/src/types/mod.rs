//! The semantic type model: [`Type`], its arena, and supporting payloads.

mod annotation;
mod arena;
mod binder;
mod class_info;
mod constant;
mod method;
mod ty;

pub use annotation::{Annotation, AnnotationArena};
pub use arena::TypeArena;
pub use binder::ReservedTypeId;
pub use class_info::ClassInfo;
pub use constant::Constant;
pub use method::{MethodKind, MethodParam, MethodType, PolyType, TypeLambda, TypeParam, Variance};
pub use ty::{ErrorType, MatchType, Type};
