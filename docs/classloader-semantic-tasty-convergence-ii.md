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
scope for regeneration. This issue makes no fixture substitutions. Outcomes
are normalized by semantic role and qualified names, never by numeric
`SymbolId` or `TypeId`; within each store, canonical symbol references,
owner/visibility/flags, parent and signatures, constructor kind/owner/result,
and origin are checked explicitly. Serialized constructor result and effective
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

Negative controls request each original class without the JDK module path
(leaving `java.lang.Object` unavailable), request each class under a wrong
package, and truncate each original TASTy by one byte. The loader errors are
typed `NotFound` / `DependencyFailure`, no wrong-owner request aliases the
frozen input, and store checkpoints are unchanged after failed loads. The
TASTy parser rejects truncated input. No partial class is accepted and no
`Any` or `Object` fallback is inferred. Existing malformed-input and
missing-dependency regressions remain intact.

## Result

**2/2 frozen classes are eligible for explicit class and primary-constructor
completion** through one narrow shape: a top-level class with one empty
primary-constructor clause, no type parameters or declared members, and a
resolvable `java.lang.Object` parent, with either public visibility or private
visibility qualified to the containing package. This is semantic completion
eligibility, not exact legacy signature parity: both rows retain the reported
`Unit` versus owning-class constructor result delta.

The first evidenced neighboring unsupported shape remains a non-empty
constructor clause: the preserved #1927 comparison records a parameterized
constructor as `<missing>` on the unpickler side. That historical observation
defines the rejection boundary for this contract; this issue did not substitute
that fixture or run a broader cohort. The 14-unit/126-row denominator remains
unchanged. No production gate or reusable loader contract was changed.

Focused validation ran with the pinned toolchain and isolated temporary/build
directories:

```text
cargo test -p dotty-classloader --test semantic_tasty_parity --locked
cargo fmt --all -- --check
```

Repository-wide checks and the JDK 21 gate are recorded in the PR validation
summary. Final hashes confirm all three historical artifacts above remain
unchanged.
