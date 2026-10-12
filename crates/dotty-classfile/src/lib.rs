//! JVM class-file reading and low-level binary-writing APIs.
//!
//! Reference format: the class file format produced and consumed by JDK 26
//! (class file major version 70), the current compatibility ceiling for
//! Scala 3.9.0. The decoder is verified against real JDK 21, 22, 23, 24, 25,
//! and 26 corpora. See `docs/classfile-format-jdk25.md` for the wire-format
//! reference and compatibility audit this type model follows.
//!
//! [`constant_pool::ConstantPool::encode`] serializes constant-pool entries,
//! and [`attribute::Attribute::encode`] plus [`attribute::encode_attributes`]
//! serialize the supported scalar, list-based, and opaque attribute forms.
//! [`writer::Writer`] provides primitive JVM-format writes and checked length
//! prefixes. The crate does not encode complete [`class_file::ClassFile`]
//! values, perform semantic JVM verification, or generate JVM code from Scala
//! programs.

pub mod access_flags;
pub mod attribute;
pub mod class_file;
pub mod constant_pool;
pub mod descriptor;
pub mod encode_error;
pub mod field;
pub mod method;
pub mod modified_utf8;
pub mod reader;
pub mod signature;
pub mod writer;

/// JVM class file binary and structural APIs.
pub mod classfile {
    pub use super::access_flags::*;
    pub use super::attribute::*;
    pub use super::class_file::*;
    pub use super::constant_pool::*;
    pub use super::descriptor::*;
    pub use super::encode_error::*;
    pub use super::field::*;
    pub use super::method::*;
    pub use super::modified_utf8::*;
    pub use super::reader::*;
    pub use super::signature::*;
    pub use super::writer::*;
}
