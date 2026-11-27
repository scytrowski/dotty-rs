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
subset: literals, formals, visible locals/case bindings, existing Int comparisons, negation,
short-circuit operators, ascriptions, If, Block and bounded Int/Boolean Match.
Locals can feed existing primitive method results, conditions, selectors and guards.
Assignments require a visible mutable Boolean local and a Boolean RHS. Boolean
value blocks preserve source evaluation order, branch visibility and short-circuit
effects. The later [exhaustive Boolean-match increment](jvm-tasty-boolean-exhaustive-matches.md)
allows complete unguarded Boolean coverage to replace the final irrefutable case.

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

## Implemented producer behavior

The pickler reuses ordinary local proofs, VALDEF/MUTABLE/ASSIGN wire nodes and
exact TERMREFdirect declaration targets. Its existing read-only primitive flow
validator handles bounded Boolean If/Block values, and Unit blocks can contain
supported primitive locals and flag control flow. Loop emission authenticates
registered local conditions as well as formals, including singleton case locals.
The existing paired contract already compares ordinary Int/Boolean local symbols,
typed declarations and mutability; the increment adds focused mutation regressions.

JVM validation now authenticates Boolean TermRef tails on proven returning paths,
keeping semantic Boolean identity distinct from Int. A constant-selector match
whose cases all return can retain its Boolean singleton join; this exception
requires independently proven exits and does not accept a normal value path as
Nothing. Neither producer changes frontend trees or canonical session stores.

The 33-method fixture covers explicit/inferred locals, literal/If/Match/operator
initializers, assignments and branches, flag loops, side effects on both evaluated
and skipped short-circuit paths (flag toggles detect duplicate RHS execution), guards, primitive results including mixed-width
values, case-local aliases, parameter shadowing, sibling scopes, local returns,
unreachable Boolean tails and singleton loop conditions. Independent clients
execute the original Rust classes and Scala's separately rebuilt from-TASTy
classes with `-Xverify:all -ea`; Scala also compiles clients against both outputs.
Original class/TASTy bytes are hashed and checked unchanged.

The focused target has 12 tests. Lowest-layer source-pickler tests check VALDEF
order, direct references and MUTABLE/ASSIGN targets, missing/forged declaration
proofs, lexical escape and cyclic value blocks. JVM unit coverage distinguishes
Boolean from the identical Integer verifier category and rejects foreign bindings
and prefixes. Paired tests assert exact Z/J/D descriptors, load/store/return
instructions, loop frame offsets/locals/stacks, fresh-session byte determinism,
read-only rejection of malformed plans, validation/encoding limits and explicit
unsupported source shapes.

The final serial Scala/JDK target measured 7.74 seconds locally. Required CI
registers it in the JDK job and strict TASTy shard 4, increasing the independently
checked strict inventory to 37 targets; existing timing evidence is preserved.

Final local workspace checks passed 8,600 tests (57 existing ignored) and 44
all-feature doctests. Formatting, frontend boundary checks, Clippy/rustdoc with
warnings denied, the standalone compiler client and workflow/inventory/timing
checks passed. The initial workspace run hit the host's 1,024-file soft limit in
existing parallel JMOD loader tests; the complete rerun passed after raising the
local process limit to 65,536. No classloader implementation or expectation changed.

All 37 strict TASTy targets passed across four local shards, followed by the
constructor-alias, loader-rejection and dual-reference-reader follow-ups and the
mandatory JDK verifier/execution gate. The strict aggregate passed as well. The
final strengthened short-circuit fixture was consumed again by pinned Scala and
by the mandatory JDK follow-up, including original and from-TASTy execution.
