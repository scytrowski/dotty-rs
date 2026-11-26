# Local method returns in paired JVM/TASTy output

Target Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0 and class-file major 65. Branch base is main
`f374df4f7c389bd1ec0323774e73331c1f17a0fe`.

## Increment contract

1. Freeze the pinned Scala oracle and local-return scope.
2. Support symbolic RETURN target addresses in the structural AST builder.
3. Validate and emit local primitive method returns in semantic TASTy.
4. Compose normal and returning paths in blocks, If, While and Int matches,
   including the typer's authenticated synthetic Unit adaptations.
5. Accept all-returning Nothing-typed matches in JVM lowering and require
   independent Java/Scala consumers and execution after `-from-tasty`.

The bounded producer accepts ordinary monomorphic methods with explicit
Int, Boolean or Unit results. A return targets the exact current method,
never a declaration inferred from its spelling or descriptor. Non-local
returns, lambdas, local methods, reference/category-two results and
try/finally remain outside the paired subset. Return values use the existing
supported primitive expression subset. Nothing alone is not evidence of a
supported terminating operation.

The fixture under `tests/fixtures/jvm_tasty_local_returns` is reference evidence
from pinned Scala, not Rust-produced TASTy. The complete producer gate must
read and execute actual Rust artifacts and preserve their original bytes.


## Implemented producer contract

`tests/jvm_tasty_local_returns.rs` assembles the real frontend and both output
paths in the same canonical session. The public backend and pickler remain
separate borrowed-plan APIs. Methods require explicit primitive results; return
values use the existing primitive leaf validator. Returns inside operands,
conditions, match guards or unsupported nested definitions are rejected.

The semantic validator checks every source/typed edge and span, including
unreachable statements. Branch-local and case-local visibility is restored
after each branch. `If` and `Match` types must equal the semantic primitive
join, with Nothing contributing no value. Bare source `return` in a Unit method
uses the typer's actual Unit constant with the Return span. A then-branch Unit
adaptation is accepted only for an incomplete source else chain and the exact
synthetic Block/stat/Unit shape recorded by the typer. Both are validated
without changing the borrowed session. A separate bounded dispatch scan never
charges expression-validation limits or replaces legacy diagnostics for bodies
without returns; the existing depth-zero and repeated-node regressions stay
unchanged.

The structural builder's `AstField::Reference` writes the first RETURN Nat
field after resolving its DEFDEF label. It supports forward and ancestor
addresses across Nat-width changes without treating the method reference as
a recursive child. Invalid label ownership, field placement and target kind
are rejected.

JVM lowering requires a proven local exit before accepting a Nothing-typed
match. Join labels are created only for real normal exits. Blocks with an
unreachable primitive tail retain the canonical typed tree and still undergo
validation; their returning branch contributes no value or epilogue. Tests
assert exact direct-return opcodes and the full all-returning match code.

## Execution and rejection evidence

The fixture covers direct Int/Boolean/Unit returns, both-returning and mixed
If branches, guards and immutable match binders, loops, mutable state, and
unreachable side effects. Fresh frontend sessions produce identical class
and TASTy bytes. Negative tests cover foreign return targets, missing values,
forged types and Unit adaptations, cyclic trees, normal cases relabeled
Nothing, and returns outside supported positions. Validation errors leave
the canonical semantic store unchanged and publish no artifact.

`DOTTY_CLASSFILE_REQUIRE_JDK=1` makes Java 21 verification and execution of
original Rust class bytes mandatory. `DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1`
makes the pinned Scala reader/consumer gate mandatory: it reads original
Rust TASTy, runs independent Java and Scala clients against original output,
then runs Java against Scala's `-from-tasty` result. The gate hashes original
artifacts before and after these operations. `run.py` uses the frozen Scala
fixture only as an oracle; it never substitutes Scala output for Rust output.
The test target is included in required CI: the JDK job and strict TASTy shard 3.
