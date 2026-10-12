# JVM class-file encoder

`dotty-classfile` reads and structurally writes the JVM class-file model in
JVMS 4.1–4.7. Its public `ClassFile::encode` entry point preserves the model's
constant-pool slots and duplicates, flags, indices, field and method order,
bytecode, and attribute order. It does not add pool entries or generate JVM
metadata.

## Supported versions and checked limits

The encoder accepts non-preview class-file versions with major versions 45
through 70, matching the crate's historical minimum and JDK 26 compatibility
ceiling. It rejects preview minor versions and versions outside that range with
`EncodeError::UnsupportedVersion`. This is the crate's compatibility policy;
it is not a claim that versions above 70 are invalid according to the JVM.

The existing encoders check wire-width limits before returning bytes: `u2`
counts and lengths, `u4` attribute payload lengths, modified UTF-8 payloads,
and the JVMS `Code` array range of 1 through 65,535 bytes. Recursive annotation
values and nested `Code`/`Record` attributes have explicit depth bounds.
Constant-pool and class/member references are validated without changing
indices or repairing the model.

## Structural round trips

The regression tests decode, encode, and decode all seven checked-in
non-corpus fixtures and every class in the pinned JDK 21–26 corpus. They check
that both decodes consume their full input, validate references on both
models, compare class and member structure, compare constant-pool wire bytes
to preserve every slot and floating-point bit pattern, and verify repeated
encoding is deterministic. Raw unknown attributes, including nested
`StackMapTable` and `LineNumberTable` data, keep their original constant-pool
name index and payload.

## Owned class and typed method builder

`ClassFileBuilder` creates new classes with owned names, descriptors, fields,
and methods. Its raw `MethodBuilder::code` path remains available for callers
that need to provide bytes and explicit `max_stack`/`max_locals`. For ordinary
straight-line methods, `add_static_method`, `add_instance_method`, and
`add_code_method` accept a closure over `CodeBuilder`:

```rust
use dotty_classfile::classfile::{ClassFileBuilder, ClassFileVersion};

let mut class = ClassFileBuilder::new(
    "example/Calculator",
    ClassFileVersion { major: 65, minor: 0 },
)?;
class.add_static_method("add", "(II)I", |code| {
    code.iload(0)?;
    code.iload(1)?;
    code.iadd()?;
    code.ireturn()?;
    Ok(())
})?;
let bytes = class.encode()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Symbolic class, field, and method references are interned into the shared pool
before it is finalized. The builder assembles the typed instructions and uses
the method descriptor and static flag to compute `max_stack` and `max_locals`.
The supported bytecode instruction matrix documents the available operations.
This analysis checks straight-line value categories and resource bounds, but
does not prove initialization states or reference assignability, handle
control flow, or synthesize stack-map frames.

Structural equivalence does not promise byte-identical output for every
arbitrary class file. A modeled attribute stores its semantic fields rather
than its original `attribute_name_index`; encoding therefore uses the first
matching UTF-8 name entry. Unknown `Attribute::Other` payloads retain their
original name index and bytes. The Rust `String` model also cannot represent
unpaired UTF-16 surrogate code units from modified UTF-8 input. Bytecode and
unmodeled nested attributes remain opaque; this crate does not verify general
bytecode or synthesize stack maps.

## JVM execution gate

`tests/classfile_jvm_encoder.rs` builds `Answer.class` using the public
`ClassFileBuilder` and typed `CodeBuilder` API. Its Java 21 static `answer()I`
method is written without opcode bytes or explicit stack/local limits. The
test asks `javap -v` to parse it, compiles a small Java caller, and invokes the
class with `java -Xverify:all`, checking that the caller prints `42`.

Run the structural and ordinary crate tests with:

```sh
cargo test -p dotty-classfile --locked
```

Run the executable gate with `java`, `javac`, and `javap` from JDK 21 or later
on `PATH`:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test -p dotty-classfile --test classfile_jvm_encoder --locked -- --nocapture
```

When a JDK tool is missing or is older than 21, the gate prints the missing or
unsupported tool and skips locally. CI's ordinary Rust job may use an older
JDK; the gate skips there explicitly. CI separately provisions Temurin JDK 21
and sets
`DOTTY_CLASSFILE_REQUIRE_JDK=1`, so a missing tool fails the dedicated job
instead of silently passing. The JDK corpus itself is checked in, so the
structural round-trip suite does not depend on an installed JDK.

The gate verifies one deliberately small executable class. It does not provide
a general bytecode verifier, Scala compilation, or a JVM backend; those remain
separate future work.
