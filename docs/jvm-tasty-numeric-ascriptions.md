# Bounded numeric expression ascriptions in JVM and TASTy

Reference Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0 and class-file major 65. Base main:
`6917201a9ceb4c65ebc316f3d3fb47dea5a58d92`.

## Scope and increments

1. Freeze this contract and the pinned Scala reference with independent clients.
2. Lower canonical Long/Float/Double ascriptions in JVM, retaining exact kinds.
3. Authenticate paired source/typed ascriptions and preserve TASTy TYPED nodes.
4. Compose supported locals, control flow, matches and local return values;
   validate malformed plans, resource limits and deterministic artifacts.
5. Require genuine Rust/original and from-TASTy Java/Scala consumers, complete
   repository checks and current documentation.

Explicit `(value: Long)`, `(value: Float)` and `(value: Double)` use the existing
canonical primitive type, TypedExpr and reified TypeTree. Unqualified primitive
syntax and supported same-type operands are admitted: exact literals, parameters,
ordinary locals, nested wrappers, blocks, If/While, Int-selector matches with the
existing fallback, and exhaustive Boolean matches. Existing guards and case
bindings retain their identities and scopes. Operands run exactly once.

Local declarations may be inferred or explicit, immutable or mutable. Assignment
RHS values and `return (value: T)` compose with the existing value subset. Normal
and all-returning branch/case bodies may return annotated values. Returns inside
an ascribed operand or initializer remain outside this paired producer slice.
Numeric conversions/widening, boxing, new arithmetic/comparisons, new calls,
fields or constructors, numeric selectors, partial matches/MatchError and discarded
numeric expression statements are excluded. Qualified, aliased, generic, reference
and singleton ascription syntax are not added. Existing Int/Boolean/Unit support
is preserved.

## Producer and validation contract

JVM emits the operand's exact primitive value without a conversion instruction.
Long/Double occupy two physical slots but remain distinct verifier kinds; Float
occupies one slot and must not be treated as Int. Validate the wrapper, TypeTree
and actual operand kinds, including same-width mismatches.

TASTy retains every original TYPED and explicit primitive type tree. The fixture
has 83 explicit wrappers; Scala additionally inserts widening wrappers on some
local reads. Semantic/runtime compatibility does not require copying these
synthetic Scala nodes into the canonical Rust trees. Numeric
validation must dispatch before inserting the wrapper into the visited set, then
reuse the existing authenticated source/typed ascription validator. Merely
extending its target-name list is insufficient because numeric values have a
separate dispatcher. No new AST/codec model, synthetic source node or dependency
is required.

Both producers borrow canonical session stores and exact typed roots. Preserve
source/typed mappings, spans, symbol ownership, visibility, type identity and
resource bounds. Rejection is read-only and must not return partial artifacts.
The historical Float/Double ascription exclusions in the Long/floating test
slices become supported cases intentionally; other exclusions remain.

## Evidence and acceptance

The fixture contains Scala-produced reference evidence, kept separate from Rust
acceptance. Its independent Java and Scala clients each perform 2888 checks on
original classes and separately rebuilt from-TASTy classes with JVM verification.
Values cover Long extrema/beyond Int, signed zeros, subnormals, greatest finite
values, infinities and NaN classification. Exact non-NaN floating bits are checked;
arbitrary JVM execution does not promise preservation of NaN payload bits.
Semantic decompilation avoids the pinned raw printer's signed-payload bug.

Actual Rust-produced pairs must pass the same independent consumers before and
after Scala `-from-tasty`, with original hashes unchanged. Structural Rust round
trips and Scala-generated references alone do not establish interoperability.
Tests belong to the existing mandatory `jvm_tasty_expressions` target (strict
TASTy shard 3 and JDK gate), with a separate direct source-to-JVM fixture test.

```sh
SCALA39_ROOT=/path/to/pinned/scala3 JAVA_HOME=/path/to/jdk21 \
COURSIER_MAVEN_ROOT=/path/to/cached/maven \
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
cargo test --test jvm_tasty_expressions --locked -- --nocapture --test-threads=1
```

Put the JDK bin directory on PATH. The oracle runner checks the pinned revision,
cached compiler artifacts, fixture/client hashes and unchanged original outputs.

## Implemented verification

The existing expression target now has 20 tests, including 10 numeric-ascription
regressions. The complete serial target with mandatory Scala/JDK consumers
measured 12.50 seconds locally; strict TASTy inventory remains at 38 targets and
keeps its existing assignments. Its shard-3 estimate includes the measured increase.

JVM unit tests pin the constant/load-return instructions and reject inconsistent
type trees and equal-width operand mismatches. The direct source fixture passes
all 2888 Java checks independently of TASTy production. Paired tests preserve all
83 original TYPED nodes, exact primitive syntax, mixed formal order, descriptors,
physical slots, stack maxima and full numeric match frames. They exercise normal,
mixed-returning and all-returning branches/cases, guards and nested matches.

Read-only rejection covers forged wrapper/type-tree/operand types, foreign
same-named formals, incorrect source syntax, spans and source/typed bindings,
bit mismatches and cycles. The method-only path also checks deterministic output
and explicit validation item/depth and encoding-depth limits. Unsupported value
positions, widening, partial matches and numeric calls publish no pair.

The independently compiled Java and Scala consumers each pass 2888 checks against
original Rust artifacts and Scala's rebuilt from-TASTy classes. Original hashes
remain unchanged. The Scala-generated reference is kept separately and is not
used as a substitute for Rust artifact acceptance.

The full local workspace check passed 8633 tests (57 existing ignored), plus 44
all-feature doctests. Formatting, frontend boundaries, Clippy/rustdoc with warnings
denied, the standalone compiler client, strict inventory/timing checks, workflow
lint and diff checks also passed.
