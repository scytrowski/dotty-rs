# Scala `Int` operator audit for JVM intrinsics

This document records the source-to-typed-tree contract that must be in place
before JVM lowering recognizes Scala `Int` operators. The Scala reference is
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

For this compiler, a future intrinsic is valid only when the selected function
is a typed `Select` whose `Type::TermRef` targets the registered canonical
method `SymbolId`, that symbol is owned by the canonical `Definitions.int`
class symbol, its completed `MethodType` is the audited ordinary one-argument
signature, and the typed receiver, argument, and result have the expected
canonical `TypeId`s. The backend must not infer primitive behavior from the
operator text, the source method name, or an arbitrary owner with a same-named
method. An ordinary source declaration such as `class Custom { def +(x: Int):
Int = x }` keeps its own symbol and remains an ordinary method call.

## Source pipeline characterization

`crates/dotty-typer/tests/int_operator_audit.rs` drives source through the
contextual scanner (lexer), parser, namer, and `SourceTyper`. All nine operator
forms parse without parser diagnostics and their enclosing class and methods
are named. Both parameter expressions and expressions using a mutable local
`var` reach the typer.

The current `Definitions.int` type refers to its one canonical builtin class
symbol, but that symbol has `SymbolInfo::Missing`; it has no completed
`ClassInfo` or declaration scope for nominal lookup. `type_infix_expression`
resolves the left expression and enters ordinary member lookup through
`resolve_infix_application_function`. Lookup fails first, so the public
`TyperError` is
`MemberLookup(ClassInfoUnavailable { symbol: canonical_int_class,
state: Missing })`. The same error occurs for `Boolean` as the receiver. In
particular, `Int + true` fails at receiver lookup before the typer checks the
right operand's type; it is not currently an
`ApplicationArgumentTypeMismatch`. An unknown operator such as `%%` reaches
the same missing-`Int`-class-info blocker. A failed expression transaction
leaves no typed mapping or fabricated `Select`/`Apply` behind.

Once ordinary members are available, existing overload resolution is the
right path: it selects a method, creates `Select` with
`TermRefTarget::Symbol(method_id)`, and applies the typed argument. The passing
user-defined `+` characterization verifies the current successful shape,
method owner and symbol identity, `Int` argument/result signature, and receiver
type. This demonstrates that custom operator methods and builtin methods can
remain distinguishable by identity.

## Operator matrix

| Source | Expected Scala target | Argument | Result | JVM operation or branch family | Current frontend stage and blocker | Next issue |
| --- | --- | --- | --- | --- | --- | --- |
| `x + y` | canonical `scala.Int.+(Int): Int` | `Int` | `Int` | `iadd` | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x - y` | canonical `scala.Int.-(Int): Int` | `Int` | `Int` | `isub` | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x * y` | canonical `scala.Int.*(Int): Int` | `Int` | `Int` | `imul` | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x / y` | canonical `scala.Int./(Int): Int` | `Int` | `Int` | `idiv` | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x % y` | canonical `scala.Int.%(Int): Int` | `Int` | `Int` | `irem` | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x < y` | canonical `scala.Int.<(Int): Boolean` | `Int` | `Boolean` | signed `if_icmplt`, then Boolean `0`/`1` normalization | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x <= y` | canonical `scala.Int.<=(Int): Boolean` | `Int` | `Boolean` | signed `if_icmple`, then Boolean `0`/`1` normalization | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x > y` | canonical `scala.Int.>(Int): Boolean` | `Int` | `Boolean` | signed `if_icmpgt`, then Boolean `0`/`1` normalization | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |
| `x >= y` | canonical `scala.Int.>=(Int): Boolean` | `Int` | `Boolean` | signed `if_icmpge`, then Boolean `0`/`1` normalization | Parser and namer succeed; typer member lookup reports `ClassInfoUnavailable` for canonical `Int` | [#1117](https://github.com/scytrowski/dotty-rs/issues/1117) |

After #1117 supplies canonical member identities and typed applications,
[#1118](https://github.com/scytrowski/dotty-rs/issues/1118) owns the read-only
JVM intrinsic resolver. Opcode emission remains a later step.

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

## Frontend registration recommendation

Prefer verified Scala library metadata when it has already completed the
canonical `Definitions.int` class symbol and exposes the expected unique
operator signatures. If that metadata is absent, the frontend member issue
should install a bounded, session-scoped set of methods into the existing
canonical `Int` class symbol and its declaration scope. Do not create a second
`Int` symbol or make the member resolver synthesize arbitrary members while
looking them up.

The registration must be idempotent. When classpath/TASTy metadata already
provides an operator, validate and reuse that exact symbol if its owner and
signature match. Do not overwrite a completed scope wholesale or add a second
same-signature entry. A wrong-kind, duplicate, or mismatched existing entry is
a deterministic collision error; it must not be silently replaced or treated
as a primitive. If loaded metadata represents `scala.Int` under a different
symbol, reconcile that identity at the loader/definitions boundary before
typing rather than registering members on a competing class.

The registration/accessor should expose the resulting canonical `SymbolId`s
to a later backend-facing contract. The intrinsic resolver can then validate
the symbol owner, one-parameter `Int` `MethodType`, result type, and typed
operand types before returning an intrinsic descriptor. Same-named local or
extension methods do not satisfy this contract.
