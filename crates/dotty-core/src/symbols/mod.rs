//! Symbols, scopes, and the tables that own them.

mod completion;
mod flags;
mod kind;
mod origin;
mod scope;
mod symbol;
mod table;

pub use completion::SymbolInfo;
pub use flags::SymbolFlags;
pub use kind::SymbolKind;
pub use origin::SymbolOrigin;
pub use scope::{Scope, ScopeArena};
pub use symbol::{Symbol, SymbolLinks};
pub use table::SymbolTable;
