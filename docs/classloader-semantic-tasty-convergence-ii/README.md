# Semantic TASTy member completion audit (issue #1995)

This supplemental audit explicitly enters and completes each frozen `DEFDEF`
at its address in that unit's original TASTy index. Every method runs with a
new `SemanticStore`, exactly one `Definitions::bootstrap`, the real fixture
classpath, and Temurin OpenJDK `21.0.13+9-LTS` (`java.base.jmod` SHA-256
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`). The integration test repeats
the whole four-row audit twice and compares its normalized TSV byte-for-byte.
The fixture paths, owner names, method names, and addresses are fixed in the
test; a missing or duplicate method is a test failure, never a replacement
candidate.

The four TASTy inputs are format 28.9 and their byte hashes are recorded in
[`member-completion.tsv`](member-completion.tsv). The Scala 3.9.0 reference
compiler source fixtures in this repository are pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`. Option's already-reviewed source
contract is `getOrElse[B >: A](default: => B): B`; its output retains the
method-local bound, by-name clause, and `B` result. `InfoWrapped.m` retains
its method-local `F` application and returns canonical `F[Int]` semantics.

## Outcomes

| Member | Explicit completion | Normalized finding |
| --- | --- | --- |
| `scala.Option.getOrElse` | `COMPLETE` at `DEFDEF@666` | Canonical and current legacy member signatures are equal, including `B >: Option.A` and the by-name parameter. This agrees with the reviewed #1930 source/typed contract. |
| `InfoWrapped.m` | `COMPLETE` at `DEFDEF@115` | Canonical and legacy projections are semantically equal. The normalized result is `ByName(InfoWrapped.F[Int])`, preserving the wrapper carried by this compiled TASTy; the source member is declared `m: F[Int]`. For the legacy comparison only, the test copies the original `InfoWrapped.tasty` and `InfoBase.tasty` bytes into a temporary package-relative classpath layout because the fixture generator stores them flat. No TASTy bytes are rewritten. |
| `Animal.sound` | `UNRESOLVED_EXTERNAL` at `DEFDEF@22` | First typed error is `UnresolvedMember` for type member `String` at TASTy address 27. The configured TASTy/classpath and JDK 21 resolver lacks the `scala/Predef.String` alias path required by this member. This is a missing resolver prerequisite, so the method remains incomplete. |
| `Dog.sound` | `UNRESOLVED_EXTERNAL` at `DEFDEF@36` | Independent original unit and owner; its first typed error is `UnresolvedMember` for `String` at address 41, with the same measured resolver miss. The failure is recorded separately and was not inferred from Animal. |

The legacy outcomes are obtained by loading the owner through the existing
`ClassLoader` member projection in a separate fresh store. `MATCH` means the
legacy member symbol exists with a complete normalized signature. The
historical class-root reports, their frozen 14-unit / 126-field denominator,
and their hashes are unchanged: `cohort.tsv` SHA-256
`bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95`,
`comparison.tsv` SHA-256
`34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e`, and
`comparison-1948.tsv` SHA-256
`a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0`. The
Option row here reflects the current direct legacy projection; the historical
class-root-only report remains a separate immutable artifact.

The first unsupported dependency for Animal and Dog is specifically the
`scala/Predef.String` alias lookup. The evidence does not establish whether
the resolver should add alias loading or whether the canonical semantic model
needs another type representation; it only establishes the first required
missing lookup and its owning boundary (`dotty-classloader` resolver).

## Negative controls

The focused test checks that a one-byte-truncated original `InfoWrapped.tasty`
is rejected before it can mutate the bootstrapped store; a foreign owner
request for the same method name produces an explicit `ForeignOwner` value;
and a dangling method address produces the exact typed
`MissingEnteredSymbol { address: u32::MAX }`. The dangling-address control
leaves the real method pending and that same method completes successfully on
retry. `InfoWrapped.m` completes successfully after the malformed-input
control; the foreign-owner check leaves its committed semantic state intact.

## Ranked follow-up

1. **Bounded external alias resolution for `scala/Predef.String`** — owner:
   `dotty-classloader` resolver. Reproduce with either frozen
   `Animal.sound@22` (`UnresolvedMember` at 27) or `Dog.sound@36` (at 41) using
   the recorded JDK 21 and classpath. Determine the exact alias/dependency
   chain from Scala 3.9.0 inputs before changing resolver behavior. This audit
   does not implement that change or claim broader Scala standard-library
   support.
