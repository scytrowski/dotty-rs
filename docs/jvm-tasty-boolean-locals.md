# Bounded Boolean locals and value flow in JVM and TASTy

Target Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main: `dd6c1d35a709f1c3f2c46b5f3bf578b976596830`.

## Scope and increments

1. Freeze this scope and pinned Scala reference with independent Java/Scala clients.
2. Authenticate Boolean binding types on JVM branches already proven to return.
3. Validate and emit immutable Boolean locals and bounded Boolean values in TASTy.
4. Compose mutable flags, assignments, conditions, loops and existing local returns.
5. Require original Rust and Scala from-TASTy execution, negative-plan/limit coverage,
   deterministic output, current documentation and mandatory CI inventories.

Ordinary initialized method-local `val` and `var Boolean` use exact source-typer
bindings, explicit or inferred canonical types, and the existing primitive value
subset: literals, formals, visible locals/case bindings, comparisons, negation,
short-circuit operators, ascriptions, If, Block and bounded Int/Boolean Match.
Locals can feed existing primitive method results, conditions, selectors and guards.
Assignments require a visible mutable Boolean local and a Boolean RHS. Boolean
value blocks preserve source evaluation order, branch visibility and short-circuit
effects. Existing final irrefutable match cases remain required.

JVM lowering retains semantic Boolean kinds and exact Z descriptors despite the
shared Integer verifier category. A recursively proven return may leave a Block
with a TermRef-typed unreachable tail; authenticate its exact planned Boolean
parameter/local (including singleton case bindings) rather than inventing Nothing.
The pickler also validates unreachable source/typed expressions.

General Boolean-returning calls, new fields/constructor/getter shapes, closures,
local methods, non-local returns, try/finally and new numeric operations are
excluded. Returns inside initializers, assignment operands, conditions and guards
remain excluded from the paired subset. General nested same-name ordinary-local
shadowing remains outside scope; sibling scopes and parameter shadowing preserve
canonical symbol identity without redesigning visibility.

## Evidence and acceptance

The fixture's no-argument `run.py` mode creates **Scala-produced reference**
evidence at the pinned revision. Candidate mode must consume actual Rust class
and complete TASTy bytes, run independent Java/Scala clients against original
artifacts and separately rebuilt `-from-tasty` classes, and preserve original
bytes. This establishes semantic/runtime compatibility, not byte equality.

Use `SCALA39_ROOT`, `JAVA_HOME` (JDK 21+, also put its bin on PATH) and
`COURSIER_MAVEN_ROOT` as documented by the shared offline pickler oracle.

```sh
python3 tests/fixtures/jvm_tasty_boolean_locals/run.py
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 cargo test --test jvm_tasty_boolean_locals --locked -- --nocapture --test-threads=1
```

Both producers must validate exact source/typed edges, symbol ownership/origin,
local scopes, declaration order, mutability, canonical type and ABI agreement.
Reject forged bindings, escaped locals, Boolean/Int confusion, malformed joins,
cycles and exhausted budgets read-only, without publishing partial artifacts.
Reuse existing stores, local proofs, wire nodes and dependency boundaries.
