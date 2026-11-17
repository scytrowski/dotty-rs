# Semantic TASTy Convergence VIII — Chain hierarchy audit

## Checkpoint 1: authenticated inputs

The integration base is `ab25b31acf4cc1d2307220bbe72d5133334190be`.
Scala source provenance is revision
`777528f19a58e794c9954a42f433373472ec57f8`; the local pinned source checkout
reported that exact `HEAD`. The cached compiler reported `Scala compiler
version 3.9.0`, and the generated files identify `Scala 3.9.0` in their TASTy
tooling header. Generation ran on Temurin OpenJDK `21.0.8+9-LTS`.

The original audit cohort is compiled from
`crates/dotty-classloader/tests/fixtures/semantic_chain_3_9_0/ChainHierarchy.scala`:

```scala
package convergence

trait ChainRoot
class ChainMiddle extends ChainRoot
class ChainLeaf extends ChainMiddle { def answer: Int = 42 }
```

These are three separate compiler-produced units. Their wire version is
TASTy `28.9.0`; their original owner, parent/member addresses and SHA-256
digests are:

| Unit | Original indexed owner | Parent/member addresses | SHA-256 |
| --- | --- | --- | --- |
| `ChainRoot.tasty` | `TYPEDEF@4` | `<init>` `DEFDEF@13` | `4c5ab08bcd61019347716e88c5edb0a0f03fc07bb124ad063318809b389d41b3` |
| `ChainMiddle.tasty` | `TYPEDEF@4` | source edge to `ChainRoot`; first unresolved type member at `23`; `<init>` `DEFDEF@27` | `b737055909e3839893f8e2bcaee0faa10aceac1260da01efa82e3e6864751d00` |
| `ChainLeaf.tasty` | `TYPEDEF@4` | source edge to `ChainMiddle`; parent tree `@9` (tag `136`); `<init>` `DEFDEF@23`; `answer` `DEFDEF@32` | `8cefb2d2e8c74c6c65a9a9dce0df69f320ad7cae08f8b2c4af0679fbbcd72868` |

The source type is a trait followed by two classes. The wire owners are all
top-level `TYPEDEF` nodes; the parent types and member signatures remain
serialized TASTy trees and are not replaced with guessed semantic owners.
`ChainLeaf.answer` has source result type `Int` and is retained as generated
even though the current single-class semantic loader rejects that extra member
shape. The ordinary loader completes its serialized type as `ByName(Int)`. The simpler comparison cohort
below was compiled separately after observing that boundary; it does not
replace or rewrite these three original inputs.

The simpler three-unit cohort is compiled from
`crates/dotty-classloader/tests/fixtures/semantic_chain_simple_3_9_0/ChainHierarchySimple.scala`;
it preserves the hierarchy and removes only `answer`. It uses the same source
revision, compiler, JDK and TASTy wire version.

| Unit | Original indexed owner | Parent/member addresses | SHA-256 |
| --- | --- | --- | --- |
| `ChainRoot.tasty` | `TYPEDEF@4` | `<init>` `DEFDEF@13` | `7cbca928d379b6593ca5c9360316ff5e02960e8a19f81696ff418c1da2cd762f` |
| `ChainMiddle.tasty` | `TYPEDEF@4` | source edge to `ChainRoot`; first unresolved type member at `23`; `<init>` `DEFDEF@27` | `58e5dbb5914bd8b6406591be74152dc22adb5c320b80df632a84f611a1d15bac` |
| `ChainLeaf.tasty` | `TYPEDEF@4` | source edge to `ChainMiddle`; parent tree `@9` (tag `136`); first unresolved type member at `17`; `<init>` `DEFDEF@23` | `cf64ad9429db6a20cfe1d36045a0d80655fd267ed98ed1fd4df7398a72ad3d57` |

The foreign same-spelled control is the independently compiled
`foreignchain.ChainLeaf` in
`crates/dotty-classloader/tests/fixtures/semantic_chain_controls_3_9_0/ForeignChainLeaf.scala`
(`class ChainLeaf`). Its TASTy is outside both three-unit cohorts and has
SHA-256 `a4b475935a33a10ec139cac4b04c7db46fc8a53ed8f8959279fa29dd35ded5ba`.

### Reproduction

The compiler dependencies are the cached Maven artifacts for Scala 3.9.0:
`scala3-compiler_3`, `scala3-interfaces`, `tasty-core_3`, and
`scala3-library_3` version `3.9.0`; `scala-library` version `3.9.0`;
`scala-asm` version `9.9.0-scala-1`; and `compiler-interface` version `1.12.0`.
`COMPILER_CP` contains those seven jars in that order. `LIBRARY_CP` contains
the Scala 3 library and Scala library jars in that order. For each source, run
from its fixture directory:

```sh
java -cp "$COMPILER_CP" dotty.tools.dotc.Main -usejavacp:false \
  -classpath "$LIBRARY_CP" -d "$TMPDIR/chain-compile-out" ChainHierarchy.scala
```

`TMPDIR` was the registered managed scratch directory. The resulting
`convergence/*.tasty` files were copied without modification into that
cohort's fixture directory. The simple and foreign-control sources were
compiled with the same command and separate scratch output directories.
SHA-256 values were computed over the copied `.tasty` bytes.

## Checkpoint 2: measured boundaries

The non-ignored unit regression
`loader::tests::chain_hierarchy_audit_asserts_measured_boundaries_and_controls`
uses the private semantic loader and selected resolver with the real
`TastyUnpickler` path. The cohort fixture directory is first on the classpath,
the separate foreign-control directory second, and the JDK 21 `jmods` last.
For each requested original and simple unit, `find_class` returned TASTy from
its own fixture directory (`ClassOrigin::Directory`). Failed semantic attempts
do not publish a session origin or class identity.

| Request | First semantic result | Repeat in same store |
| --- | --- | --- |
| Original `ChainRoot` (trait) | `UnsupportedShape("the requested top-level declaration must have a class template")` | Same typed error |
| Original `ChainMiddle` | `Unpickle { address: 4, error: UnresolvedMember { address: 23, name: "ChainRoot", namespace: Type } }` | Same typed error |
| Original `ChainLeaf` with `answer` | `UnsupportedShape("the class template must have no type parameters and one constructor declaration")` | Same typed error |
| Simple `ChainLeaf` without `answer` | `Unpickle { address: 4, error: UnresolvedMember { address: 17, name: "ChainMiddle", namespace: Type } }` | Same typed error |

The `ChainMiddle` request directly measures `ChainMiddle -> ChainRoot`. The
simpler `ChainLeaf` request starts from the leaf alone and fails on its
`ChainMiddle` parent before completing the transitive hierarchy. The original
`answer` member remains in its source-authentic fixture at `DEFDEF@32`; the
single-class semantic loader stops at the additional declaration shape gate.
This is why the separately compiled no-method cohort is used to measure B1/B2;
the original inputs were retained unchanged.

The selected resolver request for `convergence.ChainLeaf` on the simple cohort
returns `ResolutionError::Malformed`, retaining the inner typed
`Unpickle::UnresolvedMember` for `ChainMiddle` at address 17. Repeats and two
newly bootstrapped stores produce identical normalized results for each unit;
normalization compares the error variant, TASTy address, namespace and source
name, excluding arena IDs.

Controls establish that the result is specific to the selected semantic
projection. The ordinary resolver returns a complete `ChainLeaf` `ClassInfo`
with direct parent `ChainMiddle`; that parent's complete `ClassInfo` has parents
`Object` and `ChainRoot`. The ordinary classloader also returns a complete
`ChainLeaf` with parent `ChainMiddle`; its `answer` member is complete and has
canonical result `Int` (serialized as `ByName(Int)`). An independently compiled
`foreignchain.ChainLeaf` completes through the semantic loader, retains its
actual `foreignchain` package owner and `Directory` origin, and is not used as a
name-based replacement for `convergence.ChainLeaf`.

No production behavior was changed. A found symbol counts as complete in the
controls only after checking `SymbolInfo::Complete`, parent relationships and
method type.

## Checkpoint 3: verdicts and validation

| Boundary | Evidence | Verdict |
| --- | --- | --- |
| B1 — direct class-to-class edge | `ChainMiddle` stops at typed `UnresolvedMember@23` for `ChainRoot`; the trait root stops at the semantic loader's class-template shape gate. | `BLOCKED_PREREQUISITE` |
| B2 — transitive completion from leaf only | The simpler `ChainLeaf` stops at typed `UnresolvedMember@17` for `ChainMiddle`. | `BLOCKED_PREREQUISITE` |
| B3 — selected resolver class request | Selected `ChainLeaf` resolution returns `ResolutionError::Malformed` wrapping that exact typed unresolved parent. | `BLOCKED_PREREQUISITE` |

The legacy/unselected route has complete hierarchy information for this cohort;
that does not make the selected semantic class request supported. This evidence
identifies downstream semantic work without making a production change in this
fixtures/tests/docs audit.

Historical convergence fixtures and comparator inputs remain unchanged:

| Comparator input | Expected rows | SHA-256 |
| --- | ---: | --- |
| `docs/classloader-semantic-tasty-convergence/cohort.tsv` | 14 | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| `docs/classloader-semantic-tasty-convergence-ii/member-completion.tsv` | 4 | `5bc658a2c13e35c7751b5ab3a29ed6f0c842b0d0ef1e7bb6468c141a7ad30c61` |

Validation on the final implementation:

- `cargo test -p dotty-classloader --locked`: passed, 576 passed and 5 ignored.
- `cargo test --workspace --all-targets --locked`: passed.
- Strict JDK 21 classfile/JVM gate from `AGENTS.md`: passed using Temurin
  JDK 21.0.13 and `DOTTY_CLASSFILE_REQUIRE_JDK=1`.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`: passed.
- `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`,
  and `git diff --check`: passed.

The audited branch is based on
`ab25b31acf4cc1d2307220bbe72d5133334190be`; its published candidate HEAD is
verified in the associated pull request. No historical comparator input was
regenerated.
