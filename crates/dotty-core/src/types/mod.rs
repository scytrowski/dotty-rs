//! The semantic type model: [`Type`], its arena, and supporting payloads.

mod annotation;
mod arena;
mod binder;
mod class_info;
mod constant;
mod method;
mod rebind;
mod structural;
mod ty;

pub use annotation::{
    Annotation, AnnotationArena, AnnotationArgument, AnnotationArguments, AnnotationValue,
};
pub use arena::TypeArena;
pub use binder::ReservedTypeId;
pub use class_info::ClassInfo;
pub use constant::Constant;
pub use method::{MethodKind, MethodParam, MethodType, PolyType, TypeLambda, TypeParam, Variance};
pub use rebind::{
    MethodParamSpec, TypeParamSpec, TypeRebindError, close_over_this, method_type_from_symbols,
    poly_type_from_symbols, rebind_type_lambda, type_lambda_from_symbols,
};
pub use structural::{
    MAX_STRUCTURAL_DEPTH, StructuralLookupError, StructuralMemberLookup, lookup_structural_member,
};
pub use ty::{ErrorType, MatchType, TermRefTarget, Type, TypeRefTarget};
