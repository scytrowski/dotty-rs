//! JVM class-file reading and low-level binary-writing APIs.
//!
//! Reference format: the class file format produced and consumed by JDK 26
//! (class file major version 70), the current compatibility ceiling for
//! Scala 3.9.0. The decoder is verified against real JDK 21, 22, 23, 24, 25,
//! and 26 corpora. See `docs/classfile-format-jdk25.md` for the wire-format
//! reference and compatibility audit this type model follows.
//!
//! [`class_file::ClassFile::encode`] serializes complete class files for
//! supported non-preview major versions 45 through 70. It preserves the
//! constant pool, member flags and indices, raw code bytes, and attribute
//! ordering. [`constant_pool::ConstantPool::encode`] serializes constant-pool entries,
//! and [`attribute::Attribute::encode`] plus [`attribute::encode_attributes`]
//! serialize modeled scalar, list-based, code, record, annotation, JPMS
//! `Module`, `ModulePackages`, and `ModuleMainClass` forms, plus opaque
//! attributes. [`class_file::ClassFile::validate_module_descriptor`] applies
//! explicit JPMS descriptor checks; no module graph is resolved. Bytecode
//! remains byte-for-byte opaque, and recursive
//! annotation values and nested Code/Record attributes are bounded by
//! explicit depth limits.
//! [`constant_pool_builder::ConstantPoolBuilder`] constructs a new basic
//! constant pool with stable first-insertion indices. It currently omits
//! method handles, method types, dynamic/invokedynamic entries, and bootstrap
//! method table construction.
//! [`class_file_builder::ClassFileBuilder`] owns class and member metadata,
//! interns symbolic references during final encoding, and keeps raw method
//! code opaque without claiming JVM verifier correctness.
//! [`writer::Writer`] provides primitive JVM-format writes and checked length
//! prefixes. The crate does not perform semantic JVM verification or generate
//! JVM code from Scala programs.

pub mod access_flags;
pub mod attribute;
pub mod bytecode;
pub mod class_file;
pub mod class_file_builder;
pub mod constant_pool;
pub mod constant_pool_builder;
pub mod descriptor;
pub mod encode_error;
pub mod field;
pub mod method;
pub mod modified_utf8;
pub mod reader;
pub mod signature;
pub mod tasty_link;
pub mod writer;

/// JVM class file binary and structural APIs.
pub mod classfile {
    pub use super::access_flags::*;
    pub use super::attribute::*;
    pub use super::bytecode::*;
    pub use super::class_file::*;
    pub use super::class_file_builder::*;
    pub use super::constant_pool::*;
    pub use super::constant_pool_builder::*;
    pub use super::descriptor::*;
    pub use super::encode_error::*;
    pub use super::field::*;
    pub use super::method::*;
    pub use super::modified_utf8::*;
    pub use super::reader::*;
    pub use super::signature::*;
    pub use super::tasty_link::*;
    pub use super::writer::*;
}
