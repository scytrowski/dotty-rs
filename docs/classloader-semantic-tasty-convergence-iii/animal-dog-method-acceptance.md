# Original `Animal.sound` and `Dog.sound` completion (#2030)

This supplemental acceptance report records explicit `TastyUnpickler::complete_symbol` calls for the original Scala 3.9.0 `Animal.sound` and `Dog.sound` definitions. It extends the bounded resolver evidence in the #2029 audit. The historical #1995 four-member comparator and its TSV remain frozen; its `Animal.sound`/`Dog.sound` rows preserve the pre-alias failures as historical evidence and are not rewritten to describe current behavior.

## Frozen inputs and source

The Scala reference revision is `777528f19a58e794c9954a42f433373472ec57f8`. The original `Animal.tasty` SHA-256 is `9f205fe92b667f4f83a6a3d03d6dd45ec8f22f979e5acd73608e75ba1f2af9b3`; the original `Dog.tasty` SHA-256 is `9fb989680c0fbc7811ca1343437f223f4fd60b368d95fc8a0e1fbc49023cd13a`. Their owners are the indexed `Animal` Trait TYPEDEF@4 and `Dog` Class TYPEDEF@4. The method DEFDEF addresses are 22 and 36. The previous unresolved member type addresses were 27 and 41, respectively.

Each method is explicitly completed in its own fresh `SemanticStore`, with one `Definitions::bootstrap` and a resolver-backed `TastySession`. The ordered classpath is `tasty_sample`, Scala library fixtures, path-dependent fixtures, semantic fixtures, and Temurin 21 `jmods`. `scala/Predef.tasty` supplies the source-authentic public Type-namespace `String` alias owned by the canonical `scala.Predef$` ModuleClass. Completing that alias resolves its `java.lang.String` target from `java.base.jmod`. The alias and target class are distinct semantic identities.

## Accepted method shape

Both methods complete as public Term-namespace Methods under their original indexed owners. Their actual signature is `by-name(scala.Predef$.String)`: the method result retains the source alias identity while the alias's completed `AliasingBounds` points to the canonical class owned by `java.lang`. This records both the serialized method shape and the independently resolved target; it does not rewrite the signature to `java.lang.String` or widen it to `Object`.

The test runs each exact original method completion repeatedly in the same unpickler and requires the same method signature `TypeId` each time. It then repeats each method in a new store and compares normalized receipts byte-for-byte. Owner, kind, visibility, method type, alias target role, fixture digest, and JMOD origin are asserted. See [`animal-dog-method-acceptance.tsv`](animal-dog-method-acceptance.tsv).

## Negative and rollback coverage

The focused member-completion suite also retains controls for masking `scala/Predef` (typed failure at address 29), masking `java/lang/String` (typed failure at address 27), forged same-spelled alias owners, wrong member owner/address, malformed TASTy, and late failure after alias target materialization followed by same-session retry. Each failed method completion leaves the method incomplete. The direct Predef target control without JDK stops at its pinned package-resolution error; the JDK-present direct target probe retains its pinned `UnresolvedMember` at address 383. These checks remain separate from the positive two-method receipts. The pinned four-method historical TSV and 14-unit/126-row comparison remain unchanged.
