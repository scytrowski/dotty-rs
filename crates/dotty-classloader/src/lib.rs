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
mod crc32;
mod error;
mod field_symbol;
mod inflate;
mod jar_class_path;
mod jdk_class_path;
mod jmod_class_path;
mod loader;
mod repository;
mod symbol;
mod zip_archive;
mod zip_reader;

/// Classpath loading APIs.
pub mod classloader {
    pub use crate::binary_name::BinaryName;
    pub use crate::class_path::{
        ClassOrigin, ClassPathEntry, ClassPathError, ClassResource, CompositeClassPath,
        DirectoryClassPath,
    };
    pub use crate::error::ClassLoadError;
    pub use crate::field_symbol::FieldSymbol;
    pub use crate::jar_class_path::JarClassPath;
    pub use crate::jdk_class_path::JdkClassPath;
    pub use crate::jmod_class_path::JmodClassPath;
    pub use crate::loader::ClassLoader;
    pub use crate::symbol::{ClassRef, ClassSymbol};
}
