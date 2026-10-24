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
that need to provide bytes and explicit `max_stack`/`max_locals`. For
straight-line methods and the supported integer/reference/Float/Double branched subset,
`add_static_method`, `add_instance_method`, and `add_code_method` accept a
closure over `CodeBuilder`:

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
Straight-line methods can use Float and Double constants, local loads/stores,
and typed returns alongside the existing integer/reference operations. The
`fconst_bits` and `dconst_bits` entry points preserve raw IEEE-754 bits and use
immediate instructions only for exact positive zero/one/two (Float) or
zero/one (Double); all other patterns are resolved through the constant pool.
Float constants use `ldc`/`ldc_w` and tag 4, while Double constants use
`ldc2_w` and tag 6. The linear analyzer tracks Float as category one and Double
as category two, including Double local head/tail overlap and exact return
types. These methods do not receive an automatic
`StackMapTable`. Branched methods use symbolic labels, CFG analysis, final
assembled byte offsets, and `full_frame` entries. Automatic branched emission
requires class-file major version 65 or newer. It accepts only the analyzer's
integer and bounded category-two Long states, plus null, initialized-object,
and current-receiver states; it is not a general JVM verifier and rejects
unsupported category-two operations, incompatible-stack, uninitialized-local,
and unreachable-block flows. Reference branches accept initialized objects and
`null`, but reject an uninitialized allocation. The
identity branches compare object identity and do not infer a source-level
equality operation. Unreachable bytecode is rejected because automatic frame generation
does not infer verifier states for dead code.

```rust
use dotty_classfile::classfile::{ClassFileBuilder, ClassFileVersion};

let mut class = ClassFileBuilder::new(
    "example/Maximum",
    ClassFileVersion { major: 65, minor: 0 },
)?;
class.add_static_method("max", "(II)I", |code| {
    let left = code.new_label()?;
    code.iload(0)?;
    code.iload(1)?;
    code.if_icmpge(left.clone())?;
    code.iload(1)?;
    code.ireturn()?;
    code.mark(left)?;
    code.iload(0)?;
    code.ireturn()?;
    Ok(())
})?;
let bytes = class.encode()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`CodeBuilder` exposes `new_label`, `mark`, `goto`/`goto_w`, `ifeq`, `ifne`,
all `if_icmp*` branches, `if_acmpeq`, `if_acmpne`, `ifnull`, and `ifnonnull`.
It validates label ownership and definitions
before committing the method. `ClassFileBuilder` stages symbolic references,
the `Code` and `StackMapTable` names, and frame payloads before finalizing the
constant pool. A branched method gets exactly one nested `StackMapTable`
attribute; its code bytes and payload are owned until a temporary borrowed
class-file view is encoded. A failed closure or encode returns no class bytes.

Structural equivalence does not promise byte-identical output for every
arbitrary class file. A modeled attribute stores its semantic fields rather
than its original `attribute_name_index`; encoding therefore uses the first
matching UTF-8 name entry. Unknown `Attribute::Other` payloads retain their
original name index and bytes. The Rust `String` model also cannot represent
unpaired UTF-16 surrogate code units from modified UTF-8 input. Bytecode and
unmodeled nested attributes remain opaque; this crate does not verify general
bytecode or synthesize stack maps.

## Supported typed generation subset

The executable fixture covers the current straight-line and branched generation
subsets:

| Generated class | Typed operations exercised |
| --- | --- |
| `Calculator` | `iload`, `iadd`, `ireturn` |
| `IntegerArithmetic` | `isub`, `imul`, `idiv`, `irem`; signed operands, truncation toward zero, minimum-int division overflow, and division/remainder by zero |
| `Calculator.max` | `if_icmpge`, both branches, and equality; executed for `(20,22)`, `(22,20)`, and `(22,22)` |
| `ReferenceBranches` | `if_acmpeq`, `if_acmpne`, `ifnull`, and `ifnonnull`; identity and null predicates verified and executed with JDK 21+ |
| `Calculator.sumTo` | loop back-edge, `goto`, integer locals, and merge frames; executed for `0`, `1`, and `10` |
| `Calculator.nested` | nested integer conditionals, four branch targets, and a shared join of initialized integer locals |
| `Box` | `aload`, `iload`, `invokespecial`, `putfield`, `getfield`, `return`, `ireturn` |
| `Caller` | integer constants, `invokestatic`, `new`, `dup`, `iload`, `invokespecial`, `areturn`, `ireturn` |
| `FloatingPointValues` | bit-preserving Float/Double constants, `ldc`/`ldc2_w`, parameter and local loads/stores, `freturn`/`dreturn`; verified and executed with JDK 21 |
| `FloatDoubleCalls` | exact `(FD)` and `(IFDJ)` calls, constructor forwarding, Float/Double branch joins and Double loop frames; covered by source integration and the mandatory JDK gate |
| `FloatingPointCategory2.scala` source batch | `NumericBox`, `NumericConstants`, and `Runner` together; all three class files are decoded at major version 65, checked for exact descriptors/fields/raw literal bits/slots/frames, compared across independent sessions, then executed through `javac` and `java -Xverify:all` in the hosted JDK 21 gate |

These integer instructions operate directly on JVM 32-bit values. Subtraction
and multiplication wrap modulo 2^32; division truncates toward zero; remainder
has the dividend's sign. `idiv` and `irem` throw `ArithmeticException` for a
zero divisor, and `Integer.MIN_VALUE / -1` evaluates to `Integer.MIN_VALUE`
with remainder zero. The classfile builder does not evaluate or pre-check
these arithmetic operations.

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

`tests/classfile_jvm_encoder.rs` asks `javap -v` to parse generated
classes, compiles a small Java caller, and executes it with
`java -Xverify:all`. The Java assertions check `Calculator.add(20, 22)`, all
three `Calculator.max` branch outcomes, `Calculator.sumTo` for zero, one, and
multiple iterations, four paths through `Calculator.nested`,
`new Box(42).get()`, `Caller.answer()`, and `Caller.makeBox(42).get()`. The test
decodes each `StackMapTable` payload, checks full-frame counts and increasing
PCs, and verifies that frame and branch targets are instruction boundaries.
It also checks that `javap -v` reports the branch targets and frame counts.
Rebuilding the same calculator twice must produce identical class bytes,
including its code and frames.

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
provisions Temurin JDK 21 and sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`; missing
tools or verifier failures fail that job. The JDK corpus itself is checked in,
so the structural round-trip suite does not depend on an installed JDK.

The existing `assemble(&[Instruction], ...)` and linear analyzer remain
straight-line APIs. Their typed instruction set includes bounded Float/Double
constants, locals and returns; arithmetic, conversions and comparisons remain
unsupported. Class-aware CFG analysis also checks exact Float/Double field,
method, and approved constructor transfers, including category widths and
Float/Double stack-map entries. The separate symbolic `CodeSequence` API supports labels
and `goto`, `goto_w`, `ifeq`, `ifne`, `if_icmp{eq,ne,lt,ge,gt,le}`,
`if_acmpeq`, `if_acmpne`, `ifnull`, and `ifnonnull` branches.
`assemble_labeled` returns complete bytes plus deterministic instruction and
label byte offsets. It uses checked branch displacement and code-length
arithmetic, and never widens a short branch automatically. The public typed
`CodeBuilder` connects these pieces for the supported integer, Float/Double,
and initialized-reference subset and emits `StackMapTable`; it rejects
unreachable blocks and unsupported verifier states. Follow-up work can add
switch instructions, exception edges, and broader object initialization states. Frame compression
and general JVM verification remain out of scope. This does not add Scala
compilation or a JVM backend.
