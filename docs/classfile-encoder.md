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

## Supported typed generation subset

The executable fixture covers the current straight-line generation subset:

| Generated class | Typed operations exercised |
| --- | --- |
| `Calculator` | `iload`, `iadd`, `ireturn` |
| `Box` | `aload`, `iload`, `invokespecial`, `putfield`, `getfield`, `return`, `ireturn` |
| `Caller` | integer constants, `invokestatic`, `new`, `dup`, `iload`, `invokespecial`, `areturn`, `ireturn` |

Every class is built with `ClassFileBuilder` and `CodeBuilder`; the fixture
does not embed complete class bytes, constant-pool indices, or caller-provided
stack/local limits. It decodes each generated file, checks that the decoder
consumes all bytes, validates constant-pool references, and checks method
descriptors, flags, opcodes, and inferred `max_stack`/`max_locals`.

`Box` exercises a generated constructor that calls `java/lang/Object.<init>`,
stores its argument in an instance field, and reads it back. `Caller.answer`
uses `invokestatic` to invoke the generated `Calculator.add` method. Its
`makeBox(I)LBox;` factory exercises builder-generated allocation with
`new`/`dup`/`invokespecial` and returns the new object.

## JVM execution gate

`tests/classfile_jvm_encoder.rs` asks `javap -v` to parse all three generated
classes, compiles a small Java caller, and executes it with
`java -Xverify:all`. The Java assertions check that `Calculator.add(20, 22)`,
`new Box(42).get()`, `Caller.answer()`, and `Caller.makeBox(42).get()` each
return `42`. The last scenario executes the generated allocation and
constructor-call sequence rather than relying on javac to emit it.

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
unsupported tool and skips its JVM execution portion locally, after running
the generated-class structure and reference checks. CI's dedicated job
provisions Temurin JDK 21 and sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`, making all
three runtime scenarios mandatory. The JDK corpus itself is checked in, so
the structural round-trip suite does not depend on an installed JDK.

The existing `assemble(&[Instruction], ...)`, analyzer, `CodeBuilder`, and
class builder remain limited to straight-line methods. The separate symbolic
`CodeSequence` API supports labels and `goto`, `goto_w`, `ifeq`, `ifne`, and
`if_icmp{eq,ne,lt,ge,gt,le}` branches. `assemble_labeled` returns complete
bytes plus deterministic instruction and label byte offsets. It uses checked
branch displacement and code-length arithmetic, and never widens a short
branch automatically. Control-flow analysis, `StackMapTable` generation,
exception edges, and general verification of arbitrary input bytecode remain
out of scope. This does not add Scala compilation or a JVM backend.
