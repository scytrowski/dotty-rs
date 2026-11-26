# Bounded Long values in paired JVM/TASTy output

Reference: Scala 3.9.0 at `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0 and class-file major 65. Base main: `e7a8d56506`.

## Increment contract

1. Freeze the Scala semantic oracle and exact bounded producer scope.
2. Add canonical Long method signatures, literal projection and J descriptors.
3. Preserve Long local declarations, assignments, If/While and local Return.
4. Lower Long-valued Int-selector matches and proven abrupt Long joins in JVM.
5. Require original Rust Java/Scala consumers, from-tasty execution, deterministic
   output and failure without partial artifacts in CI.

Methods use explicit monomorphic results and plain clauses. Mixed Int/Boolean/Long
formals retain source order and canonical identities. Long locals may be inferred
or explicitly typed, immutable or mutable. Int-selector matches retain the existing
Int patterns, guards, binders and unconditional fallback. Long is one semantic
value and one verification entry, occupying two physical JVM slots.

This increment excludes Long arithmetic/comparison, Long selectors, numeric
widening, boxing, discarded Long values requiring pop2, Long fields and primary
constructor parameters, Long-producing method calls, and Float/Double parity.
Existing independent JVM support for those field/call shapes is unchanged.

## Oracle boundary

Scala 3.9 TastyPrinter reads signed numeric payloads with readNat. Large Long
constants fail its 31-bit bound; small negative numbers can print incorrectly.
The fixture therefore uses semantic decompilation, not -Yprint-tasty. The pinned
compiler and target remain unchanged. Scala-generated fixtures are reference
only: interoperability must consume original Rust artifacts, execute them via
Java and Scala, and execute Scala's from-tasty output without replacing originals.
