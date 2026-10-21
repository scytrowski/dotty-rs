# Scala 3.9 primitive operators and mutable control flow audit

This audit records the Scala 3.9.0 source, typed-source and TASTy wire
observations for primitive operators, mutable locals, assignment and `while`.
The semantic pickler supports canonical `Int` arithmetic and relational
comparisons, Boolean `!`, `&&`, and `||`, mutable Int locals and assignments,
and `while` expressions. Eager Boolean operators remain audit-only. The source
compiler is pinned to `777528f19a58e794c9954a42f433373472ec57f8`; the emitted
format is TASTy 28.9.0. Exact source and artifact hashes are in
[`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json).

The independently compiled inputs are
[`AlgorithmsReference.scala`](../tests/fixtures/tasty_pickler/AlgorithmsReference.scala) and
[`CustomOperators.scala`](../tests/fixtures/tasty_pickler/CustomOperators.scala).
Their normalized Scala compiler output is checked in beside the `.tasty`
files. `cargo test -p tasty-pickler-oracle --test fixtures` asserts selected
wire edges, and
[`primitive_control_flow_contract_audit.rs`](../crates/dotty-typer/tests/primitive_control_flow_contract_audit.rs)
ties source AST nodes to typed `TreeKind`, `TypeId` and exact `SymbolId`s.

## Operator contract

For source `a op b`, parsing produces an untyped `InfixOp`; typing resolves the
operation against the canonical primitive symbol and produces the corresponding
application/selection tree. The TASTy snapshot represents the operation as
`APPLY(SELECTin(...), ...)`. Its signed name includes parameter and result
types. `SELECTin`'s receiver edge and direct argument references preserve
identity; a rendered operator spelling by itself does not identify its owner.

| Source | Typed selection | Canonical owner and signature | Scala 3.9 TASTy observation |
| --- | --- | --- | --- |
| `a + b`, `a - b`, `a * b`, `a / b`, `a % b` | `APPLY(SELECTin)`; Int result | `scala.Int`; `(scala.Int): scala.Int` | Signed `+`, `-`, `*`, `/`, `%`; Int receiver and direct Int argument |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `APPLY(SELECTin)`; Boolean result | `scala.Int`; `(scala.Int): scala.Boolean` | Signed `<`, `<=`, `>`, `>=`; direct argument |
| `!value` | `SELECT(unary_!)`; Boolean result | `scala.Boolean`; parameterless member with result `scala.Boolean` (no argument clause) | `SELECT` with unsigned `unary_!`; no `APPLY` wrapper |
| `a && b`, `a || b` | `APPLY(SELECTin)`; Boolean result | `scala.Boolean`; `(scala.Boolean): scala.Boolean` | Signed `&&` / `||`; RHS is represented as an application argument |
| `a & b`, `a | b`, `a ^ b` | `APPLY(SELECTin)`; Boolean result | `scala.Boolean`; `(scala.Boolean): scala.Boolean` | Signed `&` / `|` / `^`; RHS is also an application argument |
| user-defined `this + right` | Selection resolves to the class method `SymbolId` | `example.CustomOperators`; `(scala.Int): scala.Int` | Receiver is `THIS`, followed by direct `right` reference. Signed result text alone resembles the primitive `Int.+`; the source typed symbol and receiver establish this is the user method. |

The focused typer test checks all canonical primitive `SymbolId`s, their owner
and signature, source-to-typed mappings and receiver/argument identity. It also
checks the custom `+` resolves to the user declaration, not `scala.Int.+`.
Consequently a future producer must consume typed symbol identity; it must not
choose an intrinsic from the token spelling alone.

## Current pickler coverage

Issue #1356 enables the five binary arithmetic members `Int.+`, `Int.-`,
`Int.*`, `Int./` and `Int.%`. Issue #1357 adds canonical relational members
`Int.<`, `Int.<=`, `Int.>` and `Int.>=`, all with Boolean results. Validation
pairs each source `InfixOp` with its typed `Apply`, verifies the canonical
symbol, owner, method signature, result and receiver types, and checks the
source-to-typed edges for the receiver and argument. Emission uses the ordinary
`APPLY(SELECTin(...), argument)` shape and preserves the selected signed name;
it leaves arithmetic and comparison semantics to Scala/JVM evaluation rather
than computing values during serialization. Regression coverage includes all
four relations, Int boundary values, a comparison over nested arithmetic, and a
comparison used as an `if` condition. The pinned Scala runtime gate exercises
the Rust-produced `Comparisons.tasty`. Boolean operators, assignment and loops
remain unsupported by the pickler.

Source pipeline regression tests inspect signed semantic names and nested wire
structure. When the explicit pinned Scala oracle is enabled, they also compile
the standalone Rust-produced arithmetic and comparison artifacts with
`-from-tasty`, then run overflow, negative division/remainder, division by zero,
signed comparison boundaries, nested arithmetic and comparison-condition
cases.

## Short-circuit behavior

At source and wire level, `&&` and `||` still have an ordinary `APPLY` with an
RHS argument tree. The TASTy tree shape alone does not prove evaluation order.
The pinned Scala 3.9 compiler's source and `-from-tasty` output are both run:
`false && (1 / 0 > 0)` returns false and `true || (1 / 0 > 0)` returns true,
while the analogous `false & ...` and `true | ...` cases throw
`ArithmeticException`. These runtime controls distinguish short-circuit
semantics from eager Boolean operators without claiming a special wire node.

## Local identity, assignment and loop

The fixture's `immutableLocal` has a `saved` local `VALDEF` with no `MUTABLE`
modifier, followed by a direct reference to that exact declaration. In
`sumTo`, `i` and `total` are distinct local `VALDEF`s owned by `sumTo`, each
with the `MUTABLE` modifier. Their uses and assignment targets are direct
references to those declaration nodes. Source typer assertions additionally
check the local binding's typed symbol, owner, scope and mutable flag.

The method body contains the two local definitions and a `WHILE`; its final
expression refers to `total`. The actual loop is:

```text
WHILE
  condition: APPLY(SELECTin signed scala.Int.< (scala.Int): scala.Boolean,
                   direct-ref i, direct-ref n)
  body: BLOCK
    stats: [ASSIGN(direct-ref total,
                   APPLY(SELECTin signed scala.Int.+, total, i))]
    expression: ASSIGN(direct-ref i,
                       APPLY(SELECTin signed scala.Int.+, i, 1))
```

TASTy places the final block expression before its `stats` structurally, while
the block evaluates `stats` first and then its expression. Thus the runtime
updates `total` before the final expression increments `i`. Source typing gives the condition the
canonical Boolean type and the `While` result Unit; assignments also have Unit
type. The snapshot pins `WHILE.condition/body`, `BLOCK.expression/stats`, each
`ASSIGN.left/right`, direct-reference targets and the signed operator names.
The independent reference runtime checks `sumTo(0) == 0`, `sumTo(1) == 0`,
`sumTo(5) == 10`, and `sumTo(-1) == 0`.

## Reproducing the oracle

Use the pinned Scala checkout, JDK and cached dependencies documented in
[`tasty-pickler-oracle.md`](tasty-pickler-oracle.md), then run:

```sh
python3 tools/tasty-pickler-oracle/run.py check
```

This recompiles and hash-checks every fixture, regenerates and compares its
normalized snapshot, decompiles each standalone TASTy file, and compiles it
with `-from-tasty` into a fresh output directory. It executes the Scala source
reference and the fresh `Algorithms.tasty` class. Missing tools, dependencies,
revision mismatches, hash mismatches and compiler failures are errors; the
explicit oracle does not silently skip these checks. Ordinary Rust tests use
committed fixtures and do not require Scala or network access.
