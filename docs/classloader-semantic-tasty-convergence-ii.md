# Semantic TASTy convergence II: bounded single-class audit

Status: completed for the two frozen inputs; the final CI gate remains.

## Frozen inputs and environment

This audit is pinned to fresh `origin/main` commit
`53b55d2126934da5f2d2ca0b4a0bea9e67f15bc6`, Rust `1.98.1` (the repository
toolchain), and Temurin JDK `21.0.13+9` at
`/home/vdrake6/.sdkman/candidates/java/current`. Scala fixture semantics are
pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. The two preselected inputs, exact
binary names, source declarations/visibility, TASTy versions, and SHA-256
digests are frozen in [the two-row manifest](classloader-semantic-tasty-convergence-ii-inputs.tsv).
The manifest was committed before any semantic measurements.

The semantic tests use one ordered classpath, matching
`semantic_tasty_parity.rs`: `crates/dotty-classloader/tests/fixtures/tasty_sample`,
`crates/dotty-classloader/tests/fixtures/tasty_fields`,
`crates/dotty-classloader/tests/fixtures/tasty_visibility`,
`crates/dotty-tasty/tests/fixtures/scala3-library`,
`crates/dotty-tasty/tests/fixtures/path_dependent`, then
`$JAVA_HOME/jmods` (JDK 21). Fresh audit runs set `TMPDIR` and
`java.io.tmpdir` to a private directory under the isolated issue worktree;
the host `/tmp` is full and is not used.

Historical comparison artifacts are treated as immutable evidence. Their
pre-measurement hashes are:

| Artifact | SHA-256 |
| --- | --- |
| `docs/classloader-semantic-tasty-convergence/cohort.tsv` | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| `docs/classloader-semantic-tasty-convergence/comparison.tsv` | `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e` |
| `docs/classloader-semantic-tasty-convergence/comparison-1948.tsv` | `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0` |

The earlier 14-unit / 126-row denominator and all its outputs remain out of
scope for regeneration. The two selected positive fixtures are unchanged; a
single existing cohort fixture is used only for the bounded constructor probe.
Outcomes are normalized by semantic role and qualified names, never by numeric
`SymbolId` or `TypeId`; within each store, canonical symbol references,
owner/visibility/flags, parent and signatures, constructor kind/owner/result,
and origin are checked explicitly. The semantic constructor parent is compared
to `Definitions::object_class`, its result to the exact entered class
`SymbolId`, and the legacy constructor result to its canonical
`Definitions::unit` `TypeId`. Serialized constructor result and effective
constructor result remain separate observations.

## Audit procedure

The regression in `crates/dotty-classloader/tests/semantic_tasty_parity.rs`
discovers the class `TYPEDEF` and primary constructor `DEFDEF` structurally in
each original TASTy tree, records addresses as fixture evidence only, and
explicitly completes both nodes through the existing resolver-backed
`TastySession`. Each run uses a new `SemanticStore`, exactly one
`Definitions::bootstrap`, and the frozen classpath. It repeats this for two
independent sessions per class and compares normalized receipts byte for byte.
It separately invokes legacy `ClassLoader::load_class` in fresh stores and
checks repeatability. The compact per-fixture receipt is in [the result TSV]
(classloader-semantic-tasty-convergence-ii-results.tsv).

Both unpickler runs complete the class and constructor. For both fixtures the
original constructor `DEFDEF@21` has one empty clause and serializes its result
as `scala.Unit`; effective semantic completion gives a plain zero-argument
constructor whose result is its owning class. The legacy loader reports
`()Plain:Unit`. This is an observed constructor-signature semantic delta, not a
load failure. `PlainPrivate` is represented as
`PrivateWithin(me.cytrowski.tastyfixtures.visibility)`, matching its Scala
`private` source declaration under Scala 3.9.0 semantics.

The legacy loader loads both classes from the frozen visibility fixture
directory, assigns public visibility to both constructors, and completes
`java.lang.Object` as the sole parent. The unpickler produces the same class
identity role, class visibility, constructor owner/kind/visibility, and parent.
The result does not claim exact signature parity: the constructor result
representation differs as described above.

Negative controls feed each original class through resolver-backed semantic
completion without the JDK modules (leaving the external `java.lang.Object`
dependency unavailable), present the same original bytes under a mismatched
requested package, and truncate each original TASTy by one byte. Missing
Object produces a typed `ResolverFailure` / `UnresolvedPackage`; completion
rolls back the entered class to `Missing`, and the unpickler type count is
unchanged. The requested-owner control compares the entered class path against
the mismatched requested owner and returns a test-local typed
`RequestedOwnerMismatch`; no class completion or partial info is published.
Separate legacy controls still report typed
`NotFound` / `DependencyFailure` and unchanged store checkpoints. The TASTy
parser rejects truncated input. No partial class is accepted and no `Any` or
`Object` fallback is inferred. Existing malformed-input and missing-dependency
regressions remain intact.

## Result

**2/2 frozen classes are eligible for explicit class and primary-constructor
completion** through one narrow shape: a top-level class with one empty
primary-constructor clause, no type parameters or declared members, and a
resolvable `java.lang.Object` parent, with either public visibility or private
visibility qualified to the containing package. This is semantic completion
eligibility, not exact legacy signature parity: both rows retain the reported
`Unit` versus owning-class constructor result delta.

The historical #1927 `<missing>` observation does not establish a constructor
failure because that comparison did not explicitly complete pending
constructors. The bounded neighboring probe explicitly completed the existing
`fields/Body(x: Int)` TASTy with the same resolver/bootstrap strategy:

| Probe | Explicit completion result |
| --- | --- |
| `fields/Body(x: Int)` primary constructor | Supported; one `Int` parameter, owning class result |

Therefore the first unsupported neighboring constructor shape remains
unmeasured. Establishing it requires a separate frozen input with a constructor
dependency or clause shape beyond this successful one-`Int` probe and an
explicit completion attempt; the historical `<missing>` row is not used as a
boundary. The two selected positive targets and the 14-unit/126-row denominator
remain unchanged. No production gate or reusable loader contract was changed.

Validation ran with the pinned toolchain and isolated temporary/build
directories (`TMPDIR` under the issue worktree, JDK `java.io.tmpdir` under the
same worktree):

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
cargo test -p dotty-classloader --test semantic_tasty_parity --locked
cargo test -p dotty-tasty-unpickler --locked
cargo test -p dotty-classloader --test real_jdk_class_path --locked -- --ignored
git diff --check
```

All commands passed; the explicit real-JDK gate passed its three ignored JDK
21 tests. Final hashes confirm all three historical artifacts above remain
unchanged.
