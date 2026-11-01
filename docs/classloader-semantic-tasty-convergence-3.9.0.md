# Semantic TASTy convergence, Scala 3.9.0

## Issue #1929 fixture ledger

| Role | Fixture | SHA-256 | Root | Expected semantic edge |
|---|---|---|---|---|
| Parent | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty` | `e4d58a733e8cc9a516b5d248dd6fb97acb2cb19d763e1e390cfcd1249bd7f5f9` | `TYPEDEF@4`, `InfoBase[A]`, package `me.cytrowski.tastyfixtures.semantic` | No ordinary members |
| Child | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoWrapped.tasty` | `07cca416641dda27a333df99603bf7b119d0bc5660098c5d378a09064e1630fd` | `TYPEDEF@4`, `InfoWrapped[F[_]]`, same package | Applied parent `InfoBase[F[Int]]`; direct member `m` at `DEFDEF@115`, type `F[Int]` |

The pair has one nonbootstrap cross-unit TASTy dependency. JVM `Object` and
`Int` use existing canonical bootstrap/classpath behavior. `InfoBase` has no
ordinary members: this fixture demonstrates the applied parent edge and the
child's directly declared `m`; it does not demonstrate inherited-member
traversal.

The fixture source is pinned by the repository's Scala 3.9.0 semantic fixture
policy. The root and member addresses above are original TASTy addresses; tests
must locate definitions through each unit's own retained semantic index.

## First-failure ledger

| Boundary | Expected behavior | Regression evidence |
|---|---|---|
| One-class `Open` adapter | Remains available as the existing #1928 opt-in regression | Existing `loader.rs` adapter tests |
| Two-unit opt-in integration | Enter both units before completing parent/member references, reusing canonical package and class identity | #1929 loader integration tests |
| Ordinary loader | Keeps legacy default TASTy output and resource priority | `tasty_loading.rs` and legacy parity tests |
| Unsupported or malformed second unit | Return typed error and roll back only unpublished work | #1929 negative controls |

A first failure inside unpickler projection beyond the supported fixture
contract is a prerequisite boundary. Record its original file, address, and
error; do not broaden this loader increment into unrelated unpickler semantics.

## Deferred graph shape

The next graph increment is deferred. This issue covers only the two files and
single `InfoWrapped -> InfoBase` edge above. It makes no claim about general
classpath traversal, arbitrary dependency fan-out, or inherited-member lookup.
