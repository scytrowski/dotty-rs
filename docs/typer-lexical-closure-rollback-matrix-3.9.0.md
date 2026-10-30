# Lexical closure rollback matrix for Scala 3.9.0

This matrix records whole-method failure behavior for ordinary `Function1`
closures. The regression suite compares semantic-store contents, allocation
checkpoints, resolver marks, source-to-typed mappings, synthetic lambda method
and parameter mappings before and after each failed attempt. Every negative is
retried on the same `SourceTyper`; the combined capture matrix then types a
valid sibling on that same instance. The combined capture matrix and each
supplementary boundary case are repeated in fresh parser, semantic store and
typer sessions, comparing diagnostic categories and the canonical
`[this, delta]` capture receipt.

A private test-only transaction observer runs after a failed whole-method
operation returns and immediately before rollback. For each late argument and
result error, it inspects the capture index and confirms the recorded entries
are exactly the enclosing `this` and canonical `delta` parameter; the public
whole-method assertions then prove those entries are removed by rollback.

| Case | Scala 3.9.0 classification | Rust result | Rollback/retry evidence |
| --- | --- | --- | --- |
| `Calculator.broken`: `inc(x) + delta + inc(true)` | Invalid: `inc(true)` requires `Int` | `ApplicationArgumentTypeMismatch` after `this` and `delta` uses | Full projection unchanged; same diagnostic on retry; valid sibling succeeds with `[Calculator.this, delta]` |
| `Calculator.wrongResult`: `Int => Boolean` for `inc(x) + delta` | Invalid: result requires `Boolean` | Result-conformance error after both captures are visited | Full projection unchanged; same diagnostic on retry |
| `Calculator.unknown`: `inc(x) + delta + missing` | Invalid: `missing` not found | `TermNameNotFound` | Full projection unchanged; same diagnostic on retry |
| `Calculator.mutable` | Scala-valid | Explicit `FunctionLiteralCaptureUnsupported` for mutable local capture | Full projection unchanged; same diagnostic on retry |
| `Calculator.byName` | Scala-valid | Explicit `FunctionLiteralCaptureUnsupported` for by-name capture | Full projection unchanged; same diagnostic on retry |
| `Outer.wrong` with `Other.this` | Invalid: `Other is not an enclosing class` | `ThisOwnerNotEnclosing` | Full projection unchanged; same diagnostic on retry |
| `Child.inherited` | Scala-valid | `TermNameNotFound` for the inherited member, currently unsupported lookup | Full projection unchanged; same diagnostic on retry |
| `Child.throughSuper` | Scala-valid | Explicit currently unsupported `super` member path | Full projection unchanged; same diagnostic on retry |
| `Calculator.prior`: lambda parameter shadows method parameter | Scala-valid | Successfully typed before negative attempts | Its typed mapping remains identical after each rollback |

The failure suite found no missing transaction journal/checkpoint operation, so
this change does not alter production typer behavior. Existing positive local
value, method parameter, `this`, same-owner member, and shadowing regressions
remain the compatibility controls. Numeric arena IDs are compared only inside
each session; the fresh-session receipt uses error categories and canonical
capture roles.

## Reproduction

Pinned Scala classification and capture snapshots:

```sh
SCALA39_ROOT=/tmp/scala3-3.9.0 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2 \
DOTTY_LEXICAL_CLOSURE_TMP_ROOT=$PWD/target/lexical-closure-contract-oracle \
crates/dotty-typer/tests/fixtures/lexical-closure-contract/run
```

Focused rollback tests:

```sh
cargo test -p dotty-typer lexical_closure_rollback --locked
```

The exact source, Scala classification, Rust error category, checkpoint and
retry outcomes are also recorded in
[`typer-lexical-closure-rollback-matrix-3.9.0.tsv`](typer-lexical-closure-rollback-matrix-3.9.0.tsv).
