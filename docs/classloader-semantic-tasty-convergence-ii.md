# Semantic TASTy convergence II: bounded single-class audit

Status: inputs frozen; semantic measurement has not started.

## Frozen inputs and environment

This audit is pinned to fresh `origin/main` commit
`3a854831b9c074261e31e82d325c0350f606b95a`, Rust `1.98.1` (the repository
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
will be normalized by semantic role and qualified names, never by numeric
`SymbolId` or `TypeId`; within each store, canonical symbol references,
owner/visibility/flags, parent and signatures, constructor kind/owner/result,
and origin are checked explicitly. Serialized constructor result and effective
constructor result remain separate observations.

## Audit procedure

After this freeze checkpoint, inspect each original TASTy tree and address
index, then measure each selected class and its discovered primary constructor
in two independent fresh `SemanticStore` / one-`Definitions::bootstrap` runs
through the existing classpath resolver and `TastySession`. Run the legacy
`ClassLoader::load_class` comparison independently. Each attempt records the
exact first typed error and classifies it as semantic unsupported, unresolved
classpath input, or harness/infrastructure failure. No successful root-only
completion counts as acceptance; no partial class is accepted, and no `Any` or
`Object` fallback is inferred.

Negative controls cover wrong requested package/owner, a missing dependency,
and a malformed or unsupported fixture variant. These controls must fail
closed. The two normalized semantic receipts must be byte-identical across
fresh runs before the report makes an eligibility claim.

## Result

Pending measurement. The final section will state the bounded accepted class
shape, the first unsupported neighboring shape, and whether the preselected
2/2 target is feasible. If `PlainPrivate` cannot be accepted with the current
semantics, the result will say `AUDIT TARGET BLOCKED` and name one atomic
prerequisite without implementing it.
