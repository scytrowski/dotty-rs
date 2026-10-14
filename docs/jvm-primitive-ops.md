# Scala `Int` operator contract for JVM intrinsics

This document records the source-to-typed-tree contract used by the JVM
intrinsic resolver. The Scala reference is
the pinned Scala 3.9.0 revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).

## Scala 3.9.0 semantic and backend contract

The expected semantic targets are ordinary members of the one canonical
`scala.Int` class symbol. Arithmetic methods have a single `Int` parameter and
return `Int`; relational methods have a single `Int` parameter and return
`Boolean`. The compiler's `Definitions.scala` obtains several frequently used
symbols with `IntClass.requiredMethod`; `ScalaPrimitives.scala` also registers
`%` and `<` by looking up their members on that same `IntClass`. The latter
file builds a map keyed by method `Symbol`, and `isPrimitive` tests symbol
membership. Its JVM backend consumes the resulting primitive operation codes
and emits the corresponding arithmetic or comparison sequence. Thus a source
spelling such as `+` is not enough to establish an intrinsic.

- [Pinned `Definitions.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala)
- [Pinned `ScalaPrimitives.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitives.scala)
- [Pinned `ScalaPrimitivesOps.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala)

For this compiler, an intrinsic is valid only when the selected function
is a typed `Select` whose `Type::TermRef` targets the registered canonical
method `SymbolId`, that symbol is owned by the canonical `Definitions.int`
class symbol, its completed `MethodType` is the audited ordinary one-argument
signature, and the typed receiver, argument, and result have the expected
canonical `TypeId`s. The backend must not infer primitive behavior from the
operator text, the source method name, or an arbitrary owner with a same-named
method. An ordinary source declaration such as `class Custom { def +(x: Int):
Int = x }` keeps its own symbol and remains an ordinary method call.

## Source pipeline and canonical registration

`crates/dotty-typer/tests/int_operator_audit.rs` drives source through the
contextual scanner (lexer), parser, namer, and `SourceTyper`. All nine operator
forms parse without parser diagnostics and their enclosing class and methods
are named. Both parameter expressions and expressions using a mutable local
`var` reach the typer.

`Definitions::bootstrap` completes the existing canonical `Int` class symbol
with a declaration scope and nine bounded builtin methods. Registration is
idempotent and `Definitions::int_primitive_operator_symbols` exposes their
stable `SymbolId`s from `dotty-core`, so a backend can use the identity
contract without depending on the typer. The classloader maps `scala/Int` to
this same symbol, even if a classpath entry exists; it does not load a second
class identity or replace the canonical scope.

`type_infix_expression` continues through the ordinary member resolver. Each
supported operator becomes a typed `Apply(Select(...))`, with the selected
`TermRefTarget::Symbol` equal to its registered builtin method. `Int + true`
reaches overload resolution and fails because no overload accepts the
argument. `Boolean + 1` remains unsupported because Boolean has no completed
class info, while an unknown `Int` operator such as `%%` reports a missing
member. Failed expression transactions leave no typed mapping behind.

The ordinary member resolver uses existing overload resolution: it selects a
method, creates `Select` with
`TermRefTarget::Symbol(method_id)`, and applies the typed argument. The passing
user-defined `+` characterization verifies the current successful shape,
method owner and symbol identity, `Int` argument/result signature, and receiver
type. This demonstrates that custom operator methods and builtin methods can
remain distinguishable by identity.

## Operator matrix

| Source | Expected Scala target | Argument | Result | JVM operation or branch family | Current frontend stage | Backend status |
| --- | --- | --- | --- | --- | --- | --- |
| `x + y` | canonical `scala.Int.+(Int): Int` | `Int` | `Int` | `iadd` | Typed `Apply(Select(...))` with canonical operator symbol | Lowered in planned JVM methods by [#1120](https://github.com/scytrowski/dotty-rs/issues/1120) |
| `x - y` | canonical `scala.Int.-(Int): Int` | `Int` | `Int` | `isub` | Typed `Apply(Select(...))` with canonical operator symbol | Lowered in planned JVM methods by [#1120](https://github.com/scytrowski/dotty-rs/issues/1120) |
| `x * y` | canonical `scala.Int.*(Int): Int` | `Int` | `Int` | `imul` | Typed `Apply(Select(...))` with canonical operator symbol | Lowered in planned JVM methods by [#1120](https://github.com/scytrowski/dotty-rs/issues/1120) |
| `x / y` | canonical `scala.Int./(Int): Int` | `Int` | `Int` | `idiv` | Typed `Apply(Select(...))` with canonical operator symbol | Lowered in planned JVM methods by [#1120](https://github.com/scytrowski/dotty-rs/issues/1120) |
| `x % y` | canonical `scala.Int.%(Int): Int` | `Int` | `Int` | `irem` | Typed `Apply(Select(...))` with canonical operator symbol | Lowered in planned JVM methods by [#1120](https://github.com/scytrowski/dotty-rs/issues/1120) |
| `x < y` | canonical `scala.Int.<(Int): Boolean` | `Int` | `Boolean` | signed `if_icmplt`, then Boolean `0`/`1` normalization | Typed `Apply(Select(...))` with canonical operator symbol | Recognized; Boolean lowering is unsupported |
| `x <= y` | canonical `scala.Int.<=(Int): Boolean` | `Int` | `Boolean` | signed `if_icmple`, then Boolean `0`/`1` normalization | Typed `Apply(Select(...))` with canonical operator symbol | Recognized; Boolean lowering is unsupported |
| `x > y` | canonical `scala.Int.>(Int): Boolean` | `Int` | `Boolean` | signed `if_icmpgt`, then Boolean `0`/`1` normalization | Typed `Apply(Select(...))` with canonical operator symbol | Recognized; Boolean lowering is unsupported |
| `x >= y` | canonical `scala.Int.>=(Int): Boolean` | `Int` | `Boolean` | signed `if_icmpge`, then Boolean `0`/`1` normalization | Typed `Apply(Select(...))` with canonical operator symbol | Recognized; Boolean lowering is unsupported |

The read-only `dotty-jvm::resolve_int_intrinsic` resolver maps those nine
registered `SymbolId`s to `JvmIntIntrinsic` variants. It accepts only a regular
one-argument `Apply` whose function is a typed `Select`, whose receiver and
argument have canonical `Int` semantic values, and whose result matches the
selected operator's canonical result. Int singleton constants and term
references whose completed symbol type is canonical `Int` are accepted. A
non-registered method returns `Ok(None)` and remains eligible for ordinary
same-class call resolution. Malformed calls that claim a registered builtin
produce `JvmIntIntrinsicError`; no source text is used to classify an
intrinsic.

| `JvmIntIntrinsic` | Registered method | Operand values | Result value | JVM operation family |
| --- | --- | --- | --- | --- |
| `IntAdd` | canonical `scala.Int.+` | Int, Int | Int | `iadd` |
| `IntSubtract` | canonical `scala.Int.-` | Int, Int | Int | `isub` |
| `IntMultiply` | canonical `scala.Int.*` | Int, Int | Int | `imul` |
| `IntDivide` | canonical `scala.Int./` | Int, Int | Int | `idiv` |
| `IntRemainder` | canonical `scala.Int.%` | Int, Int | Int | `irem` |
| `IntLessThan` | canonical `scala.Int.<` | Int, Int | Boolean | signed `if_icmplt`, materialized as `0`/`1` |
| `IntLessOrEqual` | canonical `scala.Int.<=` | Int, Int | Boolean | signed `if_icmple`, materialized as `0`/`1` |
| `IntGreaterThan` | canonical `scala.Int.>` | Int, Int | Boolean | signed `if_icmpgt`, materialized as `0`/`1` |
| `IntGreaterOrEqual` | canonical `scala.Int.>=` | Int, Int | Boolean | signed `if_icmpge`, materialized as `0`/`1` |

Resolution is read-only and returns a descriptor only. It does not select or
emit bytecode instructions, mutate the constant pool, infer types, or load
classes. Planned method-body lowering consumes the arithmetic descriptors and
emits JVM integer instructions; relational descriptors currently produce an
explicit unsupported-intrinsic error.

## JVM semantics and limits

`iadd`, `isub`, and `imul` operate on JVM 32-bit `int` values, so overflow
wraps modulo 2^32. `idiv` and `irem` throw `ArithmeticException` for a zero
divisor. JVM integer division truncates toward zero; the overflow case
`Int.MinValue / -1` yields `Int.MinValue`, and the matching remainder is zero.
The relational branch instructions compare signed 32-bit values. A value-level
Scala `Boolean` is represented as JVM `0` or `1`; branch-based comparison
lowering must materialize exactly those values on both control-flow paths.

The equality operators `==` and `!=`, unary operators, numeric widening and
boxing are not part of this audit.

## Registration and backend boundary

The implementation installs a bounded, session-scoped set of methods into
the existing canonical `Int` class symbol and its declaration scope. Do not
create a second `Int` symbol or make the member resolver synthesize arbitrary
members while looking them up.

The registration is idempotent. If a completed canonical scope already
provides an operator, it validates and reuses that exact symbol when its owner
and signature match. It does not overwrite a completed scope wholesale or add
a second same-signature entry. A wrong-kind, duplicate, or mismatched existing
entry is a deterministic collision error. Classloader lookup aliases
`scala.Int` to the canonical class symbol before classpath metadata can create
a competing identity.

The registration/accessor exposes the resulting canonical `SymbolId`s to the
backend-facing contract. The intrinsic resolver validates
the symbol owner, one-parameter `Int` `MethodType`, result type, and typed
operand types before returning an intrinsic descriptor. Same-named local or
extension methods do not satisfy this contract.
