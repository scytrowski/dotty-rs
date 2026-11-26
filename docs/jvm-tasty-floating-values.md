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
output, boxing, discarded floating expression statements, expression ascriptions,
contextual/curried/polymorphic floating methods and non-local returns. Existing
independent JVM field/call support is unchanged. No new frontend or codec model is
needed and production dependency boundaries remain unchanged.

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
