//! The semantic type model: [`Type`], its arena, and supporting payloads.

mod annotation;
mod arena;
mod binder;
mod class_info;
mod constant;
mod method;
mod rebind;
mod ty;

pub use annotation::{
    Annotation, AnnotationArena, AnnotationArgument, AnnotationArguments, AnnotationValue,
};
pub use arena::TypeArena;
pub use binder::ReservedTypeId;
pub use class_info::ClassInfo;
pub use constant::Constant;
pub use method::{MethodKind, MethodParam, MethodType, PolyType, TypeLambda, TypeParam, Variance};
pub use rebind::{TypeRebindError, rebind_type_lambda};
pub use ty::{ErrorType, MatchType, Type};
