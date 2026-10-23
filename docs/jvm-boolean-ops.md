# JVM Boolean operator support and audit

This document records the source and typed-tree contract and bounded JVM
support for Boolean operators against Scala 3.9.0 at revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
The source typer registers canonical Boolean methods and `dotty-jvm` lowers
those exact symbols to Boolean JVM instructions and control flow. Source-level
observations are reproduced by
[`boolean_operator_audit.rs`](../crates/dotty-typer/tests/boolean_operator_audit.rs).

## End-to-end support

`tests/fixtures/jvm/Logic.scala` is compiled through the real lexer, parser,
namer, typer, JVM plan adapter, and classfile encoder. The source integration
test compiles it twice using fresh stores and output directories and compares
the complete class bytes. It decodes and validates the complete class file,
checks the public instance method descriptors, classfile major version 65,
branch targets, `StackMapTable` frames, and key `javap -c -v` output. The
JDK-required Java harness runs with `java -Xverify:all`.

| Behavior | Supported evidence |
| --- | --- |
| `!`, `&&`, `||`, `&`, `|`, `^`, Boolean `==` and `!=` | Only canonical Boolean member `SymbolId`s are intrinsic; `Any` equality remains unsupported. |
| Short-circuit RHS | `&&` skips when false; `||` skips when true. Division-by-zero cases and a mutable hit counter check the selected and skipped paths. |
| Strict Boolean RHS | `&`, `|`, and `^` evaluate both operands left to right. Hit counters and division-by-zero cases check eager behavior. |
| Branching and verifier state | Symbolic branches and classfile CFG analysis produce the checked `StackMapTable` frames. The JDK gate verifies the emitted class at runtime. |
| User-defined same-spelled operator | Remains an ordinary selected method and is not dispatched as a Boolean intrinsic. |
| Ordinary reference-bearing call inside short-circuit CFG | Rejected with `JvmCallInUnsupportedControlFlow`; no class file is published. |

The tested subset supports Boolean parameters and locals, canonical Boolean
operator results, and composition with the currently implemented expression
control flow. General reference-bearing CFG states, ordinary method calls
inside short-circuit branches, and universal `Any.==`/`Any.!=` are unsupported.

## Contract matrix

`BooleanId` below means the class symbol reached through the existing
`Definitions.boolean` type. A method ID must be the member symbol selected from
that canonical class's declaration scope after metadata is available; it is
not an ID inferred from the operator spelling. All Boolean operator results
use the existing `Definitions.boolean` `TypeId`.

| Source operator | Scala 3.9 canonical target and signature | Expected typed expression | RHS evaluation | JVM strategy in Scala 3.9 | dotty-rs audit status |
| --- | --- | --- | --- | --- | --- |
| `!a` | Parameterless `BooleanId.unary_! : Boolean` (no argument clause) | `Select(a, unary_!)`; no `Apply` | One operand | `ZNOT`; condition generation reverses success/failure branches | Registered by `Definitions`; Typer selects its canonical `SymbolId`. |
| `a && b` | `BooleanId.&&(Boolean): Boolean` | `Apply(Select(a, &&), [b])` | Short-circuit: evaluate `b` only when `a` is true | `ZAND`; branch to skip RHS when the left condition is false | Registered by `Definitions`; Typer selects its canonical `SymbolId`. |
| `a || b` | `BooleanId.||(Boolean): Boolean` | `Apply(Select(a, ||), [b])` | Short-circuit: evaluate `b` only when `a` is false | `ZOR`; branch to skip RHS when the left condition is true | Registered by `Definitions`; Typer selects its canonical `SymbolId`. |
| `a & b` | `BooleanId.&(Boolean): Boolean` | `Apply(Select(a, &), [b])` | Eager; evaluate both operands left to right | `AND`; load both values, then primitive logical operation | Registered by `Definitions`; Typer selects its canonical `SymbolId`. |
| `a | b` | `BooleanId.|(Boolean): Boolean` | `Apply(Select(a, |), [b])` | Eager; evaluate both operands left to right | `OR`; load both values, then primitive logical operation | Registered by `Definitions`; Typer selects its canonical `SymbolId`. |
| `a ^ b` | `BooleanId.^(Boolean): Boolean` | `Apply(Select(a, ^), [b])` | Eager; evaluate both operands left to right | `XOR`; load both values, then primitive logical operation | Registered by `Definitions`; Typer selects its canonical `SymbolId`. |
| `a == b` for Boolean operands | `BooleanId.==(Boolean): Boolean`; Scala `Definitions.Boolean_==` selects the Boolean-parameter overload | `Apply(Select(a, ==), [b])` | Eager; evaluate both operands | Boolean `EQ` primitive | Registered by `Definitions`; Typer selects its Boolean overload. For operands statically typed `Any`, Scala instead uses `Any_==` and universal equality semantics. |
| `a != b` for Boolean operands | `BooleanId.!=(Boolean): Boolean`; Scala `Definitions.Boolean_!=` selects the Boolean-parameter overload | `Apply(Select(a, !=), [b])` | Eager; evaluate both operands | Boolean `NE` primitive | Registered by `Definitions`; Typer selects its Boolean overload. For operands statically typed `Any`, Scala instead uses `Any_!=` and universal inequality semantics. |

The binary Boolean methods have a plain one-parameter method signature in the
Scala library. `&&` and `||` still have by-name-like behavior: the library
documents the short circuit and says that by-name declarations cannot be used
because the compiler would not build. The typed call shape alone therefore
does not encode whether the RHS is conditional; the selected canonical method
identity supplies that meaning.

The Boolean overloads of `==` and `!=` remain integer-like Boolean
comparisons. Source `AnyRef.eq/ne` on planned references are separate
identity operations: the JVM backend recognizes their canonical selected
`SymbolId`s and emits `if_acmpeq`/`if_acmpne`. It does not rewrite universal
`Any.==`/`Any.!=` or invoke a Java `Object` equality method.

`unary_!` is parameterless: Scala declares `def unary_! : Boolean`, with no
argument clause. It is not `def unary_!(): Boolean`, which the pinned Scala
oracle rejects for prefix syntax with E100 (`method unary_! ... must be called
with () argument`). The source Typer audit confirms that the no-clause form
selects as a `Select` without `Apply`. The distinct empty-clause form is
rejected as `PrefixMethodNeedsArgumentList` by the existing Typer test. A future
Boolean class-info projection must preserve this distinction when it completes
method metadata.

For `==` and `!=`, the receiver's static type matters. Scala `Boolean` declares
Boolean-parameter overloads, while `Any` declares universal equality methods.
Scala's `Definitions.Boolean_==` and `Boolean_!=` specifically find the
Boolean-parameter members. Do not classify every selected `==` or `!=` as a
Boolean primitive: `Any_==`/`Any_!=` preserve universal equality behavior.

## Source and pipeline observations

The audit test parses every operator on Boolean parameters, in local-value
expressions, in `if` conditions, and directly in `while` conditions. Its
untyped AST assertions verify that the loop's `cond` child is the operator
expression itself, rather than an identifier initialized from a separate
operator expression. It also covers literal operands and RHS expressions with
division-by-zero effects. The parser produces no diagnostics and the namer
enters the declarations. Source Typer completes all eight canonical Boolean
operators through normal member lookup and overload resolution:

- Prefix `!` types as a `Select`; each binary operator types as an
  `Apply(Select(...), [rhs])`. The selected references retain the canonical
  operator `SymbolId` from `Definitions::boolean_primitive_operator_symbols`.
- Boolean parameters, literals, local values, `if` conditions, and direct
  `while` conditions all type successfully.
- A source class with the valid parameterless declaration
  `def unary_! : Boolean` types the prefix use as `Select` without `Apply` and
  resolves its own method `SymbolId`. The distinct invalid declaration
  `def unary_!(): Boolean` is covered by the Typer's
  `PrefixMethodNeedsArgumentList` regression test and by the Scala E100 oracle.
- `a == b` and `a != b` with `a` and `b` statically typed `Any` stop at the
  still-missing canonical `Any` class information. This is a separate path
  from Boolean equality.
- `!1` reports `TyperError::MemberNotFound` on the canonical `Int` receiver.
  `a && 42` reports `TyperError::OverloadApplicationNoApplicable` after
  resolving the canonical Boolean member.
- A source class that defines `def &&(rhs: Boolean): Boolean` types its own
  call as `Apply(Select(...), [rhs])`. The typed selection carries that
  user-defined method's `SymbolId`, owner, and ordinary signature. It is not a
  Boolean intrinsic.

There are no parser or namer failures in these cases. The `Logic.scala` JVM
fixture verifies these semantics at runtime: `false && (1 / 0 > 0)` and
`true || (1 / 0 > 0)` return without evaluating the division, while
`false & (1 / 0 > 0)` and `true | (1 / 0 > 0)` throw `ArithmeticException`.
It also checks the complementary `false ||` and `true &&` paths, eager `^`,
and truth tables for Boolean `==` and `!=`.

The expected typed shape is based on the Scala frontend's selected method
symbols and the backend's handling of primitive `Apply` trees. `!a` has no
argument list in the library signature and is a selection; each binary source
operator is an application of a selected member. Source parentheses and local
references do not change that operator contract.

## Safe bootstrap and intrinsic policy

1. Reuse the existing canonical Boolean class symbol and type from
   `Definitions`; never mint another `scala.Boolean` or a parallel Boolean
   method registry detached from the class declaration scope. Bootstrap
   installs a minimal owned `ClassInfo` only when the canonical symbol has no
   info; an existing valid `ClassInfo` and unrelated overloads are preserved.
2. Before using existing class information, validate that its `ClassInfo`
   identifies the same class symbol and that its declaration scope is owned by
   that symbol. If information is deferred, malformed, or conflicts with a
   canonical signature, report that state without partially registering
   operators or replacing the class definition.
3. Resolve each candidate from the canonical declaration scope and retain its
   `SymbolId`. Validate owner, method kind and signature, parameter
   and result `TypeId`s, and arity. Register `unary_!`, `&&`, `||`, `&`, `|`,
   `^`, and the Boolean overloads of `==`/`!=` by this identity. Keep
   `Any_==`/`Any_!=` separate.
   `STATIC`, `EXTENSION`, and `ERASED` flags are incompatible with these
   ordinary instance operators; metadata flags such as `ABSTRACT` are
   preserved. Registration and the read-only accessor enforce the same rule.
4. A method named `&&` on a user class remains an ordinary call. The Typer
   retains the selected symbol identity and the JVM resolver requires that
   exact canonical identity. A spelling match, Boolean result type, or Boolean
   operands alone do not establish intrinsic identity.
5. Preserve the lowering distinction: `&&`/`||` use conditional RHS control
   flow; `&`/`|`/`^` evaluate both sides left to right; `Any` equality needs
   universal equality handling and is outside this supported subset.

`Definitions::bootstrap` installs canonical Boolean member signatures alongside
the existing bounded `Int` operator registry. `dotty-jvm` resolves the eight
canonical typed operations into `JvmBooleanIntrinsic` values and lowers them
without a method-name fallback. Short-circuit and eager paths preserve their
distinct evaluation behavior. `Logic.scala` provides source-to-runtime evidence
for this contract, with the JDK 21+ CI gate running `javap`, `javac`, and
`java -Xverify:all`.

## Scala 3.9.0 source references

- [`Boolean.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/library/src/scala/Boolean.scala#L22-L119) declares parameterless `unary_!` and the Boolean methods, documents short circuit for `&&`/`||`, and documents eager operands for `&`/`|`/`^`.
- [`Definitions.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala#L297-L318) declares universal `Any_==`/`Any_!=`; [the Boolean definitions](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala#L609-L623) select canonical Boolean methods, including equality overloads by Boolean parameter type.
- [`ScalaPrimitives.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitives.scala#L170-L179) maps Boolean `==`, `!=`, `unary_!`, `||`, `&&`, `|`, `&`, and `^` by their method symbols.
- [`ScalaPrimitivesOps.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala#L37-L43) defines `ZNOT`, `ZOR`, and `ZAND`; its [logical opcode categories](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala#L194-L225) distinguish comparisons, logical operations, and eager bitwise operations.
- [`BCodeBodyBuilder.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala#L1628-L1655) handles `ZNOT`, `ZAND`, and `ZOR` through conditional generation and routes universal equality separately; [the primitive binary emitter](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala#L155-L181) loads both operands before emitting `AND`, `OR`, or `XOR`.

## Reproduction

```sh
cargo test -p dotty-typer --test boolean_operator_audit --locked
```

This audit and the JVM fixture cover distinct stages: the Typer audit checks
canonical source selection, while the root integration test verifies emitted
class files and runtime behavior for the documented subset. CI sets
`DOTTY_CLASSFILE_REQUIRE_JDK=1`, so unavailable JDK tools fail the JVM gate.
