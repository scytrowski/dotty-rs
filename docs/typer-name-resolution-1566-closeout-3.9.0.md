# #1566 closeout audit, Scala 3.9.0

This records the audited source snapshot used to close #1566 after accepting
that its fully typed real-method target was not achieved. The PR branch includes
later unrelated additions to `main`; the measured source SHA remains explicit
below. This is a focused report for the frozen `package-term` cohort, not a
full-corpus claim.

## Pinned inputs

- Repository source audited: `0921a038f445e37157d3926c33f4c1ec148f1695`
- Scala source: `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: 3.9.0
- JDK: 21.0.8
- Cohort: the existing 60-file, 369-method `package-term` cohort
- Frozen primary cohort: five `path#tree` roots in
  [`typer-name-resolution-1566-primary-3.9.0.tsv`](typer-name-resolution-1566-primary-3.9.0.tsv)
  compared from baseline `e06c4ee` through post-`+=` intermediate `75eab259` to
  audited source `0921a03`.

## Outcome

Both serial runs attempted all 369 methods. Parser failures: 0; namer
failures: 0; fully typed methods: 0. Every frozen row had an observed current
method outcome, and 334 frozen target requests were observed. Both serial runs
matched byte-for-byte: normalized report SHA-256
`8eb1142bdd70a88ace58d460343db02ddfa75a6076a970746c4483b425b58832`; method
TSV SHA-256
`00d2bec6b78fb78ab49139672e4f5b5e87fcea0014398c8c57b2199649466582`.

The largest first-blocker groups were:

| First blocker | Methods |
| --- | ---: |
| `TypeNameNotFound` | 118 |
| `MemberLookup` | 78 |
| `UnstableSelectionPrefix` | 40 |
| `TermNameNotFound` | 24 |
| `ConstructorResultTypeMismatch` | 18 |
| `ObjectModuleClassUnavailable` | 18 |
| `LocalValueConformanceUnsupported` | 13 |
| `OverloadedSelectionDeferred` | 10 |
| Other blockers | 50 |

The complete compact method snapshot is in
[`typer-name-resolution-1566-closeout-3.9.0.tsv`](typer-name-resolution-1566-closeout-3.9.0.tsv).
In the five-method primary cohort, `ScalaVersion.scala#tree=234` advances
from `ApplicationCalleeNotMethod` to `UnsupportedExpression::InterpolatedString`:
the `Failure.apply` call is reached, but the method still does not type
completely. `InlineInfoAttribute.scala#tree=212` also records intermediate
expression progress through its bounded array, equality, bitwise, and
no-`else` behavior; its enclosing method remains blocked at a later
`MemberLookup`. `InlinerHeuristics.scala#tree=426` also stops at
`MemberLookup`; the remaining two selected roots retain their prior blockers.
For older before/after results and blocker transitions, see the linked
increment records in
[`typer-name-resolution-progress-3.9.0.md`](typer-name-resolution-progress-3.9.0.md).
The bounded source semantics have pinned Scala 3.9 positive and negative
oracle fixtures for
[universal equality](../crates/dotty-typer/tests/fixtures/universal-equality/UniversalEquality.oracle.md),
[`Int.&`](../crates/dotty-typer/tests/fixtures/int-bitwise-and/IntBitwiseAnd.oracle.md),
and [conditionals without `else`](../crates/dotty-typer/tests/fixtures/if-without-else/IfWithoutElse.oracle.md).

## Reproduction

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-closeout-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
TMPDIR="$PWD/target/tmp" CARGO_TARGET_DIR="$PWD/target" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-current-main-package-term.md
```

This closeout is deliberately not marked as a successful full-method typing
milestone. It records the unmet result and leaves future typer work to a new,
narrowly scoped objective.
