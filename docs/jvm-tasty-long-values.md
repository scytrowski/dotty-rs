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
constructor parameters, Calls with Long arguments or results, and Float/Double parity.
Existing independent JVM support for those field/call shapes is unchanged.

## Oracle boundary

Scala 3.9 TastyPrinter reads signed numeric payloads with readNat. Large Long
constants fail its 31-bit bound; small negative numbers can print incorrectly.
The fixture therefore uses semantic decompilation, not -Yprint-tasty. The pinned
compiler and target remain unchanged. Scala-generated fixtures are reference
only: interoperability must consume original Rust artifacts, execute them via
Java and Scala, and execute Scala's from-tasty output without replacing originals.


## Implemented validation and execution

Long signatures and references use the original canonical Definitions.long,
method/formal/local symbols and source-to-typed bindings. The pickler compares
Long literals as i64 values, including mismatches that have identical low 32
bits. A malformed literal singleton type, foreign same-named formal, wrong
local type tree, wrong Return target/type, forged Nothing join or cross-case
local read fails read-only validation. Inferred local type syntax is preserved
through the typer's actual TypeTree binding. Returns inside value positions
remain rejected; no intermediate trees are synthesized by either producer.

Normal Long-valued Int matches now join with a two-slot JVM stack value and a
single Long verification entry. The selector temporary follows complete Long
parameter/local slots. All-returning matches contain no placeholder or trailing
epilogue. Proven returning branches retain Long constant/type/formal/local
tail types only after validating the actual branch and planned identities.
Tests pin exact match code, PCs, stack/local maxima and full-frame entries, and
verify that two Long formals keep their exact order in (IJZJ)J signatures.

The required jvm_tasty_long_values target runs in the JDK job and strict TASTy
shard 3. DOTTY_CLASSFILE_REQUIRE_JDK=1 requires original Rust class execution
with Java 21 verification. DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 requires pinned
semantic decompilation, independent Java and Scala consumers of original Rust
output, and Java execution after from-tasty. Original class/TASTy hashes remain
unchanged, and fresh canonical sessions produce byte-identical artifacts.

The source fixture also observes selector/guard evaluation exactly once and
checks signed extrema and values beyond the Int range. Scala-generated oracle
bytes are kept separate from Rust-produced artifacts. No Scala printer patch
or compatibility-version change is used.
