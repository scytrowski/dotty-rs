//! Classpath loader unifying TASTy and JVM class file entries.
//!
//! This crate will index classpath entries (directories and archives)
//! containing `.tasty` and `.class` files and resolve symbol lookups against
//! them, preferring `.tasty` over `.class` when a symbol is defined in both.
//! It is the entry point the compiler backend and semantic analysis will use
//! to load classpath dependencies. Implementation is not yet started; this
//! crate is scaffolding built on top of `dotty-tasty` and `dotty-classfile`.

mod binary_name;
mod class_path;
mod error;
mod loader;
mod repository;
mod symbol;
mod zip_reader;

/// Classpath loading APIs.
pub mod classloader {
    pub use crate::binary_name::BinaryName;
    pub use crate::class_path::{
        ClassOrigin, ClassPathEntry, ClassPathError, ClassResource, CompositeClassPath,
        DirectoryClassPath,
    };
    pub use crate::error::ClassLoadError;
    pub use crate::loader::ClassLoader;
    pub use crate::symbol::{ClassRef, ClassSymbol};
}
