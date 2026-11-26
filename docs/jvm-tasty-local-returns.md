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
