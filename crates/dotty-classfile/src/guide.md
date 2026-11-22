## Construct and inspect a class without a JDK

The symbolic builder computes supported stack and local metadata. Decoding the
result checks binary structure; independent JVM verification and execution
are separate validation steps and require a JDK.

```rust
use dotty_classfile::classfile::{ClassFile, ClassFileBuilder, ClassFileVersion, Reader};

let mut builder = ClassFileBuilder::new(
    "example/Answer", ClassFileVersion { major: 65, minor: 0 },
)?;
builder.add_static_method("value", "()I", |code| {
    code.iconst(42)?;
    code.ireturn()?;
    Ok(())
})?;
let bytes = builder.encode()?;
let class = ClassFile::decode(&mut Reader::new(&bytes))?;
assert_eq!(class.constant_pool.class_name(class.this_class)?, "example/Answer");
assert_eq!(class.methods.len(), 1);
assert!(ClassFile::decode(&mut Reader::new(b"not a class file")).is_err());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Decoded attributes and code can borrow the original byte buffer. Keep that
buffer alive; builders and encoded outputs own their data. Pool indices are
one-based, while bootstrap-method indices are zero-based. The reserved second
slot of a Long or Double is not a usable pool entry.

Raw decoding, reference validation, bytecode analysis, stack-map generation,
and JVM verification establish different guarantees. Unknown attributes can
remain opaque for preservation. An accepted class-file version or descriptor
does not imply that the source frontend or JVM backend can produce every
corresponding construct. Encoding methods return typed errors; low-level
reader methods may have consumed earlier fields before a later read fails.
