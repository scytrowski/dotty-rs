# JVM Boolean operator audit

This audit records the source and typed-tree contract for Boolean operators
against Scala 3.9.0 at revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
It does not register Boolean methods or emit Boolean bytecode. Source-level
observations are reproduced by
[`boolean_operator_audit.rs`](../crates/dotty-typer/tests/boolean_operator_audit.rs).

## Contract matrix

`BooleanId` below means the class symbol reached through the existing
`Definitions.boolean` type. A method ID must be the member symbol selected from
that canonical class's declaration scope after metadata is available; it is
not an ID inferred from the operator spelling. All Boolean operator results
use the existing `Definitions.boolean` `TypeId`.

| Source operator | Scala 3.9 canonical target and signature | Expected typed expression | RHS evaluation | JVM strategy in Scala 3.9 | dotty-rs audit status |
| --- | --- | --- | --- | --- | --- |
| `!a` | `BooleanId.unary_!(): Boolean` | Zero-argument `Select(a, unary_!)`; no `Apply` | One operand | `ZNOT`; condition generation reverses success/failure branches | Parser and namer accept it. Typer reports missing `Boolean` `ClassInfo`. |
| `a && b` | `BooleanId.&&(Boolean): Boolean` | `Apply(Select(a, &&), [b])` | Short-circuit: evaluate `b` only when `a` is true | `ZAND`; branch to skip RHS when the left condition is false | Same missing `Boolean` `ClassInfo`. |
| `a || b` | `BooleanId.||(Boolean): Boolean` | `Apply(Select(a, ||), [b])` | Short-circuit: evaluate `b` only when `a` is false | `ZOR`; branch to skip RHS when the left condition is true | Same missing `Boolean` `ClassInfo`. |
| `a & b` | `BooleanId.&(Boolean): Boolean` | `Apply(Select(a, &), [b])` | Eager; evaluate both operands left to right | `AND`; load both values, then primitive logical operation | Same missing `Boolean` `ClassInfo`. |
| `a | b` | `BooleanId.|(Boolean): Boolean` | `Apply(Select(a, |), [b])` | Eager; evaluate both operands left to right | `OR`; load both values, then primitive logical operation | Same missing `Boolean` `ClassInfo`. |
| `a ^ b` | `BooleanId.^(Boolean): Boolean` | `Apply(Select(a, ^), [b])` | Eager; evaluate both operands left to right | `XOR`; load both values, then primitive logical operation | Same missing `Boolean` `ClassInfo`. |
| `a == b` for Boolean operands | `BooleanId.==(Boolean): Boolean`; Scala `Definitions.Boolean_==` selects the Boolean-parameter overload | `Apply(Select(a, ==), [b])` | Eager; evaluate both operands | Boolean `EQ` primitive | Same missing `Boolean` `ClassInfo`. For operands statically typed `Any`, Scala instead uses `Any_==` and universal equality semantics. |
| `a != b` for Boolean operands | `BooleanId.!=(Boolean): Boolean`; Scala `Definitions.Boolean_!=` selects the Boolean-parameter overload | `Apply(Select(a, !=), [b])` | Eager; evaluate both operands | Boolean `NE` primitive | Same missing `Boolean` `ClassInfo`. For operands statically typed `Any`, Scala instead uses `Any_!=` and universal inequality semantics. |

The binary Boolean methods have a plain one-parameter method signature in the
Scala library. `&&` and `||` still have by-name-like behavior: the library
documents the short circuit and says that by-name declarations cannot be used
because the compiler would not build. The typed call shape alone therefore
does not encode whether the RHS is conditional; the selected canonical method
identity supplies that meaning.

For `==` and `!=`, the receiver's static type matters. Scala `Boolean` declares
Boolean-parameter overloads, while `Any` declares universal equality methods.
Scala's `Definitions.Boolean_==` and `Boolean_!=` specifically find the
Boolean-parameter members. Do not classify every selected `==` or `!=` as a
Boolean primitive: `Any_==`/`Any_!=` preserve universal equality behavior.

## Source and pipeline observations

The audit test parses every operator on Boolean parameters, in local-value
expressions, in `if` conditions, and in `while` conditions. It also covers
literal operands and RHS expressions with division-by-zero effects. The parser
produces no diagnostics and the namer enters the declarations. Typing stops
before a built-in Boolean operator application can be completed:

- `!true`, `true && false`, `a && b`, `a || b`, `a & b`, `a | b`, `a ^ b`,
  Boolean `a == b`, and Boolean `a != b` report
  `TyperError::MemberLookup(ClassInfoUnavailable { symbol: BooleanId,
  state: Missing })`.
- `a == b` and `a != b` with `a` and `b` statically typed `Any` stop at the
  analogous missing canonical `Any` class information. This is a separate
  observed path from Boolean equality.
- `!1` reports `TyperError::MemberNotFound` on the canonical `Int` receiver.
  `true && 42` stops earlier at missing Boolean class information; it does not
  currently reach argument applicability checking.
- A source class that defines `def &&(rhs: Boolean): Boolean` types its own
  call as `Apply(Select(...), [rhs])`. The typed selection carries that
  user-defined method's `SymbolId`, owner, and ordinary signature. It is not a
  Boolean intrinsic.

There are no parser or namer failures in these cases. The audit deliberately
does not continue to Codegen because no built-in Boolean typed call is produced
by the current Typer. The RHS effect examples are source-level scenarios, not
runtime claims about dotty-rs. Under Scala 3.9 semantics, `false && (1 / 0 > 0)`
and `true || (1 / 0 > 0)` do not evaluate the division; `false & (1 / 0 > 0)`
and `true | (1 / 0 > 0)` evaluate it and throw `ArithmeticException`.

The expected typed shape is based on the Scala frontend's selected method
symbols and the backend's handling of primitive `Apply` trees. `!a` has no
argument list in the library signature and is a selection; each binary source
operator is an application of a selected member. Source parentheses and local
references do not change that operator contract.

## Safe bootstrap and intrinsic policy

1. Reuse the existing canonical Boolean class symbol and type from
   `Definitions`; never mint another `scala.Boolean` or a parallel Boolean
   method registry detached from the class declaration scope.
2. Let Scala standard-library metadata populate or complete the existing
   canonical class information. Before using it, validate that its `ClassInfo`
   identifies that same class symbol and that its declaration scope is owned
   by that symbol. Preserve unrelated metadata and overloads. If metadata is
   absent, deferred, malformed, or conflicts with a canonical signature,
   report that state instead of fabricating a second Boolean definition.
3. Resolve each candidate from the canonical declaration scope and retain its
   `SymbolId`. Validate owner, method kind and signature, parameter
   and result `TypeId`s, and arity. Register `unary_!`, `&&`, `||`, `&`, `|`,
   `^`, and the Boolean overloads of `==`/`!=` by this identity. Keep
   `Any_==`/`Any_!=` separate.
4. Only lower a selected canonical method that passed those checks. A method
   named `&&` on a user class remains an ordinary call. A spelling match,
   Boolean result type, or Boolean operands alone do not establish intrinsic
   identity.
5. Preserve the distinction in later lowering: `&&`/`||` need conditional
   RHS control flow; `&`/`|`/`^` need left-to-right evaluation of both sides;
   `Any` equality needs universal equality handling.

The current dotty-rs member lookup reaches `ClassInfoUnavailable` for the
synthetic canonical Boolean symbol; `Definitions::bootstrap` only installs a
bounded operator `ClassInfo` for `Int`. This issue does not change that
bootstrap behavior. Adding metadata-backed Boolean registration and bytecode
lowering belongs to the follow-up implementation increments.

## Scala 3.9.0 source references

- [`Boolean.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/library/src/scala/Boolean.scala#L22-L119) declares the Boolean methods, documents short circuit for `&&`/`||`, and documents eager operands for `&`/`|`/`^`.
- [`Definitions.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala#L297-L318) declares universal `Any_==`/`Any_!=`; [the Boolean definitions](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala#L609-L623) select canonical Boolean methods, including equality overloads by Boolean parameter type.
- [`ScalaPrimitives.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitives.scala#L170-L179) maps Boolean `==`, `!=`, `unary_!`, `||`, `&&`, `|`, `&`, and `^` by their method symbols.
- [`ScalaPrimitivesOps.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala#L37-L43) defines `ZNOT`, `ZOR`, and `ZAND`; its [logical opcode categories](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala#L194-L225) distinguish comparisons, logical operations, and eager bitwise operations.
- [`BCodeBodyBuilder.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala#L1628-L1655) handles `ZNOT`, `ZAND`, and `ZOR` through conditional generation and routes universal equality separately; [the primitive binary emitter](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala#L155-L181) loads both operands before emitting `AND`, `OR`, or `XOR`.

## Reproduction

```sh
cargo test -p dotty-typer --test boolean_operator_audit --locked
```

This audit adds no Boolean intrinsic implementation, no skipped/failing CI
case, and no runtime expectation for code that dotty-rs does not yet emit.
