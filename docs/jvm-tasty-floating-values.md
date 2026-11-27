# Bounded Float/Double values in paired JVM/TASTy output

Reference: Scala 3.9.0 at `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0 and class-file major 65. Base main: `6e5499a8ef`.

## Scope and increments

1. Freeze the pinned Scala semantic oracle, bit-sensitive values and this contract.
2. Project canonical Float/Double method signatures, F/D descriptors and literal bits.
3. Validate locals, assignments, If/While and local Return without guessing identity.
4. Fill JVM Float/Double-valued Int matches and proved returning branch tails.
5. Require independent Java/Scala original and from-tasty consumers, regression
   tests, exact instructions/frames, deterministic output and the CI gates.

Ordinary methods have explicit monomorphic results and at most one plain parameter
clause. Float/Double formals can mix with existing Int/Boolean/Long formals, with
exact source order and canonical identity. Values include FloatBits/DoubleBits
literals, formal/local reads, explicit or inferred locals and mutable assignments.
Existing Int-selector matches retain literal alternatives, guards, case binders
and an unconditional fallback. Float occupies one JVM slot, Double two; Float and
Double verifier entries remain distinct from Int and Long despite equal widths.

This slice excludes floating arithmetic/comparison, numeric conversions/widening,
Float/Double selectors, floating fields/constructor parameters/calls in paired
output, boxing, discarded floating expression statements,
contextual/curried/polymorphic floating methods and non-local returns. Existing
independent JVM field/call support is unchanged. No new frontend or codec model is
needed and production dependency boundaries remain unchanged.

Same-type expression ascriptions are now covered by
[the numeric-ascription contract](jvm-tasty-numeric-ascriptions.md).

## Evidence and acceptance

The fixture oracle is Scala-produced reference evidence. Candidate acceptance must
separately consume original Rust artifacts with independent Java and Scala clients,
then recompile the genuine Rust TASTy with pinned Scala and execute both clients.
Semantic decompilation avoids the pinned raw numeric printer's signed-payload bug.
Compatibility is semantic/runtime, not byte equality with Scala-generated output.

Literal validation and wire encoding compare raw bits, never floating equality:
positive/negative zero, least subnormal and greatest finite values are distinct.
Runtime checks compare exact non-NaN bits including signed zero and infinities;
NaN execution checks classification only. Raw codec tests can separately establish
NaN payload preservation without promising it across arbitrary JVM execution.

Rejection must remain read-only with exact source/typed mappings, symbol ownership,
case visibility, builtin identity and work limits; no partial artifact pair escapes.

## Implementation and reproduction

The existing exact numeric value validator now covers Long/Float/Double; it
compares source values, singleton bits, canonical formal/local identities and
visibility. Raw unsuffixed Double Number nodes retain exact source/typed bindings,
use bounded-name validation before normalization, and check finite f64 bits.
The existing primitive flow proof normalizes these scalar types without accepting
arbitrary Nothing nodes. Numeric statements that would discard Float/Double
values, value-position returns, floating fields/calls and widening remain errors.

JVM matches retain the selector-once, guard-once and case-scope rules. Normal Float
and Double values join with distinct verifier kinds; returning paths contribute no
join value. Unreachable formal/local tail types require exact planned JVM kinds,
including when Double and Long occupy the same number of physical slots.

The regression target checks exact `(IFJDZ)F` and `(IFJDZ)D` signatures, physical local slots,
match instructions, frame PCs and tags; signed zeros, subnormals and extrema;
18 read-only corruptions of bits/types/returns/formal/local identity; explicit
validation/encoding depth limits; unsupported inputs and fresh-session determinism.
A direct source-to-JVM test executes the fixture independently of TASTy emission.
Legacy primitive method syntax also covers Long/Float/Double in the method-only path.

```sh
SCALA39_ROOT=/path/to/pinned/scala3 JAVA_HOME=/path/to/jdk21 \
COURSIER_MAVEN_ROOT=/path/to/cached/maven \
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
cargo test --test jvm_tasty_floating_values --locked -- --nocapture --test-threads=1
```

Put the JDK bin directory on PATH. The JDK workflow and strict TASTy inventory
require this target. The consumer runner checks original artifact hashes remain
unchanged, uses separate output directories, and compiles/executes independent
Java and Scala clients both before and after Scala from-tasty.
