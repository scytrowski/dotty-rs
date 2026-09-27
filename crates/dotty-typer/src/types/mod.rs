//! Semantic type operations owned by the typer.

mod normalize;

pub use normalize::{
    MAX_TYPE_NORMALIZATION_DEPTH, SymbolInfoState, TypeNormalizeError, TypeNormalizer,
};
