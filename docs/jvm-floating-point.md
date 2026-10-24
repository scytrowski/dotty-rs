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
| Linear bytecode analysis | `ValueKind` recognizes Float and Double and descriptor-derived parameter/local sizes count them. The new regression test checks mixed `(IFDJ)V` instance parameters and exact wrong-return errors. The instruction model has no Float/Double load, store, constant, or return variants, so this is descriptor accounting rather than instruction support. |
| CFG analysis | The CFG entry currently returns `UnsupportedFlowParameter` for Float or Double descriptors. `FlowValue` and `FlowLocalState` do not represent Float/Double values or local heads/tails. The new regression test pins rejection for both kinds, including Double after a preceding Int parameter. |
| JVM source backend | `JvmType` contains Int, Boolean, Long, and planned references, but not Float or Double. The source integration test checks each fixture shape below: it first fails during backend projection with `UnsupportedJvmReferenceType(ExternalClass)` and publishes no class files. |

The source integration test covers these isolated shapes so the current first
failure is observable per shape rather than inferred from a partially emitted
class:

| Source shape | Current first failure |
| --- | --- |
| Float and Double primary-constructor parameters | Backend projection: unsupported external JVM type; no class artifact |
| Float and Double mutable instance fields | Backend projection: unsupported external JVM type; no class artifact |
| Getter-shaped Float result/parameter and Double setter parameter | Backend projection: unsupported external JVM type; no class artifact |
| Complete `NumericBox` fields, constructor, getters, and setter source batch | Backend projection of the class shape; no class artifact |
| Float and Double literal return methods | Backend projection of the method signature; no class artifact |
| Double parameter/local round trip | Backend projection of the method signature; no class artifact |
| Float `if` join | Backend projection of the method signature; no class artifact |
| Double mutable local across an Int-controlled loop | Backend projection of the method signature; no class artifact |

The fixture matrix test does not inspect typed body output after the backend
rejects the containing method. The separate Typer test checks only the six
literal values and canonical widened types listed above. Together these tests
pin current source literal semantics and the first backend failure stage, not
successful Rust compilation of Float/Double body shapes.

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
| Constant-pool entry | `CONSTANT_Float`, tag 4, four raw bits | `CONSTANT_Double`, tag 6, eight raw bits and a reserved next pool slot |
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
| Source backend | Unsupported Float/Double type projection fails before artifact publication. |
| CFG | Float/Double method parameters fail with typed `UnsupportedFlowParameter`; no speculative `FlowValue` is created. |
| Instruction encoding/analysis | F/D opcodes are absent from the typed instruction model. Wrong return kinds remain typed analyzer failures. |
| Class-file pool | A category-one load must not accept a Double entry, nor `ldc2_w` a Float entry. Existing pool validators have separate Float/Double tag checks; future opcode support must keep those checks at the assembler boundary. |
| Verifier shape | Category-one operations on Double and category-two operations on Float must fail. Double tail reads, overwrites, unassigned paths, incompatible joins, and loop backedges require pair-aware tests. |

Planned follow-ups remain ordered: #1533 adds class-file FP opcodes, pool
operand validation, and linear analysis; #1534 adds FP CFG states and
StackMapTable projection after #1533; #1535 adds JVM metadata and slot
projection after this audit and can proceed alongside #1533/#1534; #1536 adds
literal/local/return lowering after #1533–#1535; #1537 adds fields and
constructors after #1536; #1538 adds calls and control flow after #1536 (its
end-to-end fixture also needs #1537); #1539 provides the final verified source
acceptance batch. This issue does not include any of those production changes.
