# Scala Float and Double JVM audit

Issue [#1532](https://github.com/scytrowski/dotty-rs/issues/1532) records the
existing Scala `Float`/`Double` and JVM verifier boundary. This is an audit of
current behavior; it adds no compiler support and does not change Typer
semantics.

The Scala compatibility target is 3.9.0 at
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
That source checkout is available locally and confirms that Float and Double
are primitive backend types, are two-slot locals only for Double, and have
primitive arithmetic/conversion mappings in the JVM backend. The checkout
does not include a built compiler, and the Scala 3.9.0 compiler jars and
Coursier executable are absent from this environment. Therefore this audit
does **not** claim that the issue's source matrix was compiled by the pinned
oracle or that Scala-emitted descriptors/opcodes were observed here. The
fixture below is retained as the exact oracle input for a follow-up run when
those artifacts are available.

## Source compatibility fixture

```scala
class NumericBox(initialF: Float, initialD: Double):
  private var f: Float = initialF
  private var d: Double = initialD
  def getF(): Float = f
  def getD(): Double = d
  def setD(value: Double): Unit = d = value

class Runner:
  def floatValue(): Float = 1.5f
  def doubleValue(): Double = 3.5
  def roundTrip(value: Double): Double =
    val saved: Double = value
    saved
  def choose(flag: Boolean, a: Float, b: Float): Float =
    if flag then a else b
  def repeat(n: Int, first: Double, second: Double): Double =
    var current: Double = first
    var i = 0
    while i < n do
      current = second
      i = i + 1
    current
```

The pinned Scala source references inspected for this contract are
[`Definitions.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala),
[`BTypeLoader.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BTypeLoader.scala),
[`BCodeSkelBuilder.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala),
and
[`ScalaPrimitives.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitives.scala).
They establish source-level primitive identities, backend type mapping,
category-two local sizing, and primitive operation mappings. They are source
inspection evidence; descriptor and emitted-bytecode observations still need
an executable pinned compiler oracle.

## Current dotty-rs evidence

| Layer | Current behavior and evidence |
| --- | --- |
| Lexer/parser | `FloatLiteral` and `DoubleLiteral` are separate token kinds. Expression parsing constructs `Constant::FloatBits` / `Constant::DoubleBits` through the `f32` / `f64` bit-preserving constructors. Existing parser tests cover signed Float/Double literal forms. |
| Core constants | Constants retain raw IEEE-754 bits. `floating_point_constants_keep_their_exact_bits` checks signed zero and distinct NaN payloads. Equality is bitwise; never use floating-point equality to deduplicate or compare serialized payloads. |
| Typer | Literal projection maps FloatBits and DoubleBits to canonical `Definitions.float` and `Definitions.double`. The new `source_float_and_double_literals_keep_exact_bits_and_canonical_types` test checks source values `0.0f`, `1.0f`, `1.5f`, `0.0`, `1.0`, and `3.5`. The existing `LiteralSingletons.scala` fixture has a Scala 3.9.0 oracle and covers `1.0f` and `1.0` singleton types. This does not imply JVM lowering support. |
| Class-file constant pool | Tags 4 (`CONSTANT_Float`) and 6 (`CONSTANT_Double`) preserve raw bits. Double reserves its following constant-pool slot. Builder deduplication uses bits, so positive/negative zero and distinct NaN payloads remain distinct. |
| Descriptors and frames | The descriptor parser recognizes `F` and `D`. Stack-map verification tags are Float=2 and Double=3; one Double entry occupies two JVM slots. Tests cover F/D descriptors, verification tags, entry counts, and slot counts. |
| Linear bytecode analysis | Typed `fconst`/`dconst`, Float `ldc`, Double `ldc2_w`, F/D locals, and F/D returns are modeled and analyzed. Descriptor-derived parameter/local sizes, stack slots, exact kinds, Double heads/tails, overlapping stores, and typed wrong-return errors have focused regression tests. |
| CFG analysis | CFG states now keep Float separate from Integer and Double separate from Long. Tests cover `(ZFF)F` and `(ZDD)D` branch joins, exact parameter slots, Double pair-aware locals and overlaps, Float/Int and Double/Long stack rejection, and an Int-controlled Double local loop backedge. |
| StackMapTable projection | Branch frames emit `Float_variable_info` for Float and one `Double_variable_info` for each Double head/tail pair. Regression checks preserve real Top holes and omit the Double tail; a JDK 21 gate verifies and executes generated branch/loop methods, including mixed Long/Double and Float/Int locals. |
| JVM source backend | Canonical Float/Double types and literals project to `F`/`D` metadata, fields, method results, and parameter slots. #1536 added straight-line expressions and locals; #1537 adds private instance field initialization, reads, and writes. |

The current source backend boundary is:

| Source shape | Current first failure |
| --- | --- |
| Float and Double primary-constructor parameters | Constructor descriptor metadata projects `(FD)V`; parameter slots include the receiver and Double width |
| Private Float/Double mutable instance fields | `F` and `D` fields initialize from exact constructor parameters; reads and mutable writes use checked instance field plans |
| Private Float/Double immutable literal fields | `FloatBits` and `DoubleBits` lower to exact raw constant-pool values before `putfield` |
| Getter-shaped Float/Double methods and setters | Exact `()F`, `()D`, `(F)V`, and `(D)V` methods lower with `getfield`/`putfield`; no synthetic Scala accessors are generated |
| Float/Double method and constructor calls | Planned instance calls and constructors preserve canonical targets, exact `(FD)`/`(IFDJ)` descriptors, source argument order, and category widths |
| Float/Double `if` value | Equal exact Float or Double branch values join with verifier frames carrying tag 2 or tag 3 |
| Double mutable local across an Int-controlled loop | Preheader/backedge joins retain the initialized Double head/tail and emit one Double frame entry |

The field and value matrix is covered by `FloatDoubleFields.scala` and
`FloatDoubleValues.scala`. #1538 adds `FloatDoubleCalls.scala` for virtual and
constructor calls, direct-parent `(FD)V` forwarding, Float/Double `if` joins,
and a Double local across an Int-controlled loop. Fresh frontend sessions must
emit identical bytes, and the JDK 21 gate verifies and executes the fixtures.

## Bounded source lowering in #1536

The source backend lowers Float and Double literals, canonical method
parameters, initialized local values, mutable local assignments, and implicit
or explicit method returns. Literal emission preserves `Constant::FloatBits`
and `Constant::DoubleBits`, including signed zero, through the class-file
constant pool. Float locals use one slot and Double locals use a checked
head/tail pair. Discarded Float block values use category-one `pop`; discarded
Double values fail with a typed error because category-two stack popping is
outside this increment. Unsupported shapes fail before class artifacts are
published. The end-to-end `FloatDoubleValues.scala` regression checks exact
descriptors, opcodes, raw constant-pool bits, Double local widths, deterministic
class bytes across fresh sessions, and Java execution under `-Xverify:all` when
the JDK 21 gate is available. Runtime checks cover signed zero through
parameters/locals/assignments and NaN classification without promising that
the JVM preserves a NaN payload through arbitrary execution.

## Private field lowering in #1537

Primary constructors emit the direct parent call once before source-ordered
private Float/Double field initializers. Constructor-owned Float parameters use
`fload`; Double parameters use `dload` with their exact category-two slots.
Literal initializers preserve their raw bits through `fconst_bits` and
`dconst_bits`. Field getters and setters validate the canonical current-class
field identity and exact descriptor. Setters evaluate the RHS once and spill
the receiver and value to correctly sized locals before `putfield`. The
class-aware CFG verifier accepts exact Float/Double instance field transfers
and rejects category mismatches. The same verifier checks Float/Double method
arguments/results, constructor arguments, and direct-parent constructor
forwarding. Static fields, arithmetic, conversions, and mixed-category branch
joins remain unsupported.

## Calls and control flow in #1538

Planned virtual calls and constructors accept only exact, source-ordered
Float/Double arguments and results from validated metadata. The class-aware
analyzer pops arguments by descriptor in reverse stack order, checks exact
Float versus Double kinds and category widths, validates the initialized
receiver, and pushes the declared result. Constructor prologues can forward
Float/Double parameters or bit-exact literals to an approved direct parent.
`If` joins require matching branch types; Boolean conditions select the exact
Float or Double result, and `While` retains the pre-loop initialized-local set
while merging the Double value through its backedge. No floating-point
arithmetic, comparisons, widening, or conversion is added.

## JVMS 21 contract for follow-up implementation

The class-file and verifier requirements below are the target contract, not
capabilities implied by the wire model. See the [JVMS 21 class-file
chapter](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html),
[JVM structure chapter](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html),
and [instruction set](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html).

| Property | Float | Double |
| --- | --- | --- |
| Field/method descriptor | `F` | `D` |
| Operand/local category | Category one, one slot | Category two, two stack/local slots |
| Constant-pool entry | `CONSTANT_Float`, tag 4, 32 raw bits (four bytes) | `CONSTANT_Double`, tag 6, 64 raw bits (eight bytes) and a reserved next pool slot |
| Stack-map verification entry | `Float_variable_info`, tag 2, one slot | `Double_variable_info`, tag 3, two slots, encoded as one entry |
| Immediate constants | `fconst_0`, `fconst_1`, `fconst_2` only | `dconst_0`, `dconst_1` only |
| General constant loads | `ldc` or `ldc_w` for Float | `ldc2_w` only for Double |
| Local / return family | `fload`, `fstore`, `freturn` | `dload`, `dstore`, `dreturn` |

Immediate forms are valid only for the exact positive constants listed above.
In particular, negative zero must use a constant-pool entry, as must arbitrary
finite values, infinities, and NaNs. Constant-pool and semantic identity must
preserve raw bits so signed zero and distinct NaN payloads do not collapse.
Source parsing/type acceptance of negative-zero spellings is not established
by this audit's pinned compiler run; raw-bit tests use the independent
class-file/core APIs and make no source-language claim.

## Instruction encodings required by #1533

The following table freezes the JVMS 21 encodings for the bounded constant,
local, and return families. Lengths include opcode and operands and are in
bytes. Opcode names and forms link to JVMS 21 §6.5.

| Value / operation | Compact form (opcode, length) | Indexed / wide form (opcode and operands, length) | Required operand validation |
| --- | --- | --- | --- |
| Float constant | `fconst_0` `0x0b`, `fconst_1` `0x0c`, `fconst_2` `0x0d`; each 1 byte | None | These forms represent exactly positive `0.0f`, `1.0f`, `2.0f`. |
| Double constant | `dconst_0` `0x0e`, `dconst_1` `0x0f`; each 1 byte | None | These forms represent exactly positive `0.0`, `1.0`. |
| Float load | [`fload_0`–`fload_3`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.fload_n), opcodes `0x22`–`0x25`; each 1 byte | [`fload`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.fload) `0x17`, `index:u1` (2 bytes); or [`wide fload`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.wide) `0xc4 0x17`, `index:u2` (4 bytes) | Compact index 0–3; narrow index 0–255; wide operand encodes 0–65535. The selected one-slot local must fit below `max_locals`. |
| Double load | [`dload_0`–`dload_3`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.dload_n), opcodes `0x26`–`0x29`; each 1 byte | [`dload`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.dload) `0x18`, `index:u1` (2 bytes); or [`wide dload`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.wide) `0xc4 0x18`, `index:u2` (4 bytes) | Encoded index ranges match Float. The index names the Double head; both `index` and `index + 1` must fit below `max_locals`, and the second slot cannot be loaded as a value. |
| Float store | [`fstore_0`–`fstore_3`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.fstore_n), opcodes `0x43`–`0x46`; each 1 byte | [`fstore`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.fstore) `0x38`, `index:u1` (2 bytes); or [`wide fstore`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.wide) `0xc4 0x38`, `index:u2` (4 bytes) | Compact/narrow/wide index ranges match Float load; storing at a Double tail invalidates the old Double pair. |
| Double store | [`dstore_0`–`dstore_3`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.dstore_n), opcodes `0x47`–`0x4a`; each 1 byte | [`dstore`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.dstore) `0x39`, `index:u1` (2 bytes); or [`wide dstore`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.wide) `0xc4 0x39`, `index:u2` (4 bytes) | Encoded index ranges match Double load. Both destination slots must fit; overlapping category-one or category-two values are invalidated as a whole. |
| Float return | [`freturn`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.freturn) `0xae`; 1 byte | None | Requires a Float result and one Float on the operand stack. |
| Double return | [`dreturn`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.dreturn) `0xaf`; 1 byte | None | Requires a Double result and one Double value (two slots) on the operand stack. |
| Float constant-pool load | None | [`ldc`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.ldc) `0x12`, `index:u1` (2 bytes); [`ldc_w`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.ldc_w) `0x13`, `index:u2` (3 bytes) | Index must be 1–255 for `ldc`, 1–65534 for `ldc_w`, and resolve to tag 4 (`CONSTANT_Float`). Dynamic constants are outside this bounded contract. |
| Double constant-pool load | None | [`ldc2_w`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.ldc2_w) `0x14`, `index:u2` (3 bytes) | Index must be 1–65533 and resolve to tag 6 (`CONSTANT_Double`); the following reserved pool slot must also fit in the pool and is not a valid index. There is no narrow `ldc2` form. Dynamic constants are outside this bounded contract. |

For `wide` local operands, the u2 encoding can name 0–65535, but validity is
also bounded by the u2 `max_locals` field. With `max_locals = 65535`, a
category-one local can start at 65534; a Double head can start no later than
65533 because its tail must occupy slot 65534. The Double tail is not an
independent local. Constant-pool indexes are one-based and the largest
representable valid entry index is 65534; `ldc` can address only indexes
through 255, so index 256 requires `ldc_w` for Float. A Double head can be at
most 65533 because its reserved following slot must also fit below
`constant_pool_count`; a Double uses only `ldc2_w` regardless of its index.

These operands must preserve exact kind: `ldc`/`ldc_w` use a Float entry, and
`ldc2_w` uses a Double entry. A Float entry cannot be loaded by `ldc2_w`, a
Double entry cannot be loaded by `ldc`/`ldc_w`, and the reserved second slot
after Double cannot be used as a constant. Source-derived descriptors must
also stay exact (`F` and `D`); primitive values cannot fall back to references.

The #1533 typed class-file API implements these constant, local and return
families for straight-line methods. Its bit-oriented `fconst_bits` and
`dconst_bits` choose immediate forms only for exact positive values; signed
zero, noncanonical NaNs, infinities and other values are pooled with raw bits
preserved. The assembler checks constant tags and all compact, narrow and wide
local encodings. The linear analyzer checks category widths and exact types.
JDK 21 verification and execution cover generated Float/Double constants,
locals, parameters and returns, including signed zero and NaN payloads. The
#1534 CFG API also verifies branch joins and loop backedges and projects exact
Float/Double verification entries into `StackMapTable`; it does not add
arithmetic, conversions, fields, calls, or Scala source lowering.

Verifier state must preserve Float as category one and Double as category two
through parameters, locals, operand stacks, branches, joins, loops, fields,
and calls. A Double local is represented as one logical value plus a physical
tail slot; loads require a valid head/tail pair, overlapping stores invalidate
the full pair, and joins retain a pair only when both slots agree on all
reachable predecessors. Stack depth and `max_stack` count slots, while
StackMapTable counts verification entries. Cat-one operations, Long/Double
mixups, Float/Int substitutions, malformed local overlap, and incompatible
join shapes must return typed errors.

Method parameter limits count JVM parameter units, including the instance
receiver: Float contributes one unit, Double two. Mixed example descriptor
`(IFDJ)D` accounts for `I=1`, `F=1`, `D=2`, `J=2`, then a Double result. Field
and invocation transfers must check exact descriptor kinds. No fallback to
`Object` is valid for either primitive.

## Negative matrix and issue dependencies

| Boundary | Current/audit rejection to preserve |
| --- | --- |
| Source backend | Mismatched Float/Double arguments, returns, and branch kinds remain typed failures before artifact publication; arithmetic and conversion are unsupported. |
| CFG | Float/Int and Double/Long stack joins fail with typed errors. Local category mismatches and maybe-initialized Double pairs become unusable `Top`; wrong Double tails and loads fail before frame publication. |
| Instruction encoding/analysis | Arithmetic, conversions and comparisons remain unsupported. Wrong return kinds, wrong constant tags, and category mismatches fail with typed errors. |
| Class-file pool | A category-one load rejects Double; `ldc2_w` rejects Float, Long and the reserved second slot. Float bits and Double bits (including NaN payloads) remain exact. |
| Verifier shape | Linear and CFG analyzers reject category-one operations on Double, category-two operations on Float, Double tail reads, and overlapping stores that invalidate the whole Double local. CFG joins and backedges retain only exact Float values or complete Double pairs. |

Follow-ups: #1533 adds class-file FP opcodes, pool operand validation, and
linear analysis; #1534 adds FP CFG states and StackMapTable projection; #1535
adds JVM metadata and slot projection. #1536 can add literal/local/return
lowering after #1533–#1535; #1537 adds fields and constructors after #1536;
#1538 adds calls and control flow after #1536 (its end-to-end fixture also
needs #1537); #1539 provides the final verified source acceptance batch. This
metadata increment does not include Float/Double body code generation.
