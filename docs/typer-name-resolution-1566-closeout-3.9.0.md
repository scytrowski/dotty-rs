# #1566 closeout audit, Scala 3.9.0

This records the final source snapshot used to close #1566 after accepting
that its fully typed real-method target was not achieved. It is a focused
report for the frozen `package-term` cohort, not a full-corpus claim.

## Pinned inputs

- Repository source: `1b71f494fb6db882a42f31378a24ae6551986be0`
- Scala source: `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: 3.9.0
- JDK: 21.0.8
- Cohort: the existing 60-file, 369-method `package-term` cohort
- Frozen primary cohort: five `path#tree` roots in
  [`typer-name-resolution-1566-primary-3.9.0.tsv`](typer-name-resolution-1566-primary-3.9.0.tsv)
  compared from baseline `e06c4ee` through post-`+=` intermediate `75eab259` to final
  source `1b71f49`.

## Outcome

Both serial runs attempted all 369 methods. Parser failures: 0; namer
failures: 0; fully typed methods: 0. Every frozen row had an observed current
method outcome. The normalized reports had SHA-256
`704595c365110e84b8d849d4495862da32c1b550b095b473b8825f8e5a9ae8c3`; the
per-method TSVs had SHA-256
`8a1072ae4df4e6597d355d1b9bbecf85324355d0ce74a08212826b8cfa5c4853`.
Each pair matched byte-for-byte.

The largest first-blocker groups were:

| First blocker | Methods |
| --- | ---: |
| `ImportQualifierNotFound` | 144 |
| `SymbolResolution` | 91 |
| `TypeNameNotFound` | 30 |
| `WrongTypeNameKind` | 20 |
| `ObjectModuleClassUnavailable` | 18 |
| `LocalValueConformanceUnsupported` | 13 |
| `UnstableSelectionPrefix` | 11 |
| `MemberLookup` | 11 |
| Other blockers | 31 |

The complete compact method snapshot is in
[`typer-name-resolution-1566-closeout-3.9.0.tsv`](typer-name-resolution-1566-closeout-3.9.0.tsv).
In the five-method primary cohort, `ScalaVersion.scala#tree=234` advances
from `ApplicationCalleeNotMethod` to `UnsupportedExpression::InterpolatedString`:
the `Failure.apply` call is reached, but the method still does not type
completely. `InlineInfoAttribute.scala#tree=212` also records intermediate
expression progress through its bounded array, equality, bitwise, and
no-`else` behavior; its enclosing method remains blocked at a later
`MemberNotFound`. The other three selected roots retain their prior blocker.
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
  --verify-determinism target/issue-1566-closeout-package-term.md
```

This closeout is deliberately not marked as a successful full-method typing
milestone. It records the unmet result and leaves future typer work to a new,
narrowly scoped objective.
