//! JVM class file decoder and encoder.
//!
//! Reference format: the class file format produced and consumed by JDK 25
//! (class file major version 69), the current LTS release supported by
//! Scala 3.9.0. The decoder is verified against real JDK 23, 24, and 25
//! corpora. See `docs/classfile-format-jdk25.md` for the wire-format
//! reference this type model follows.
//!
//! This is currently a type skeleton only: no decoder, encoder, or bounded
//! reader exists yet.

pub mod access_flags;
pub mod attribute;
pub mod class_file;
pub mod constant_pool;
pub mod descriptor;
pub mod field;
pub mod method;
pub mod reader;
pub mod signature;

/// JVM class file binary and structural APIs.
pub mod classfile {
    pub use super::access_flags::*;
    pub use super::attribute::*;
    pub use super::class_file::*;
    pub use super::constant_pool::*;
    pub use super::descriptor::*;
    pub use super::field::*;
    pub use super::method::*;
    pub use super::reader::*;
    pub use super::signature::*;
}
