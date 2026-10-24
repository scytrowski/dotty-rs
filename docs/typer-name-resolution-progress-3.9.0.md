# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

## Baseline and change

The fresh baseline used `main` at `0f8b4a7aafdeb24dd618b4fecee8abdb193d999d`, the
pinned Scala revision `777528f19a58e794c9954a42f433373472ec57f8`, and JDK 21.0.8.
The frozen `package-term` cohort covered 60 files and 369 methods: 0 methods
typed, 369 first-blocker rows, and 17 `TermNameNotFound` blockers.

The implementation resolves a public value alias declared directly in the
`scala.package` object through `scala/package.tasty`, validates that its target
exists, verifies that the declared type names that same singleton target, then
returns the target's canonical module identity from the classpath resolver.
Nested, inaccessible, mutable, widened, and constructed values are excluded;
it does not infer a term from its spelling. The Scala 3.9 oracle compiles `Nil`
and `None`, checks a local `Nil` shadow, and rejects an unknown default term.

## Frozen-cohort result

The same 60-file, 369-method cohort was run twice after the change; normalized
reports matched byte-for-byte. Fifteen first blockers advanced across two
files. `TermNameNotFound` fell from 17 rows to 2. No cohort method became fully
typed: `methods_typed=0`. This issue's goal of fully typing a previously blocked
real method remains unmet and is recorded as such; the next blocker must be
addressed before claiming that milestone.

The audit records only the first blocker for each method. The after-change
first-blocker distribution is:

| First blocker | Method rows | Files |
| --- | ---: | ---: |
| `ImportQualifierNotFound` | 144 | 11 |
| `SymbolResolution` | 97 | 29 |
| `TypeNameNotFound` | 30 | 12 |
| `WrongTypeNameKind` | 20 | 2 |
| `ObjectModuleClassUnavailable` | 18 | 1 |
| `LocalValueConformanceUnsupported` | 13 | 1 |
| `UnstableSelectionPrefix` | 11 | 3 |
| `MemberLookup` | 9 | 4 |
| `MemberNotFound` | 5 | 3 |
| `ApplicationCalleeNotMethod` | 5 | 2 |
| `PatternTypeMismatch` | 4 | 1 |
| `StringLiteralTypingDeferred` | 4 | 2 |
| `ExplicitTypeApplicationCalleeNotPoly` | 3 | 2 |
| `TermNameNotFound` | 2 | 2 |
| `ApplicationArgumentConformanceUnsupported` | 1 | 1 |
| `IfConditionConformanceUnsupported` | 1 | 1 |
| `InfixPatternDeferred` | 1 | 1 |
| `OverloadedSelectionDeferred` | 1 | 1 |

The change removes an early name-resolution failure but exposes later
unsupported steps. Thirteen method rows in `CommandLineParser.scala` now stop at
`LocalValueConformanceUnsupported` for inferred local `accum`; two rows in
`InlinerHeuristics.scala` stop at `ExplicitTypeApplicationCalleeNotPoly`.
`CommandLineParser.scala#tree=498` (`expandArg`) still cannot resolve `paths`,
and `BTypeLoader.scala#tree=1079` (`dropModule`) is the other remaining
`TermNameNotFound` row. These are next blockers, not a complete trace of every
error later in each method.

The exact method identities and next blockers are in
[`typer-name-resolution-progress-3.9.0.tsv`](typer-name-resolution-progress-3.9.0.tsv).
At attempt root `CommandLineParser.scala#tree=397` (`tokenize`), 13 method rows
now stop at `LocalValueConformanceUnsupported` for inferred local `accum`. One
remaining method is `CommandLineParser.scala#tree=467` (`stripComment`); its
attempt at outer root `tree=498` (`expandArg`) still reports
`TermNameNotFound` for `paths`. The other remaining name blocker belongs to
`BTypeLoader.scala#tree=1019` (`dropModule`) and occurs at attempt root
`tree=1079`.

In the separate three-file focused smoke audit, local method
`StringParsers.scala#tree=123` (`rec`) now resolves `scala.None` at attempt root
`tree=129`. The method advances from `TermNameNotFound` to `MemberNotFound`, so
it also remains untyped.

## Oracle and audit commands

The pinned Scala source controls are `library/src/scala/package.scala:76`
(`Nil`) and `library/src/scala/collection/immutable/List.scala:663` (the target
object). The import oracle fixtures are in
`tools/typer-classpath-corpus-audit/scala39_import_oracle.py`:
`ImportDefaultScalaTerms.scala` and `MissingDefaultTerm.scala`.

Run the Scala oracle and focused regressions with:

```sh
tools/typer-classpath-corpus-audit/run --import-prefix-profile --source-file compiler/src/dotty/tools/dotc/config/CommandLineParser.scala
cargo test -p dotty-classloader scala_default_package_term_resolves_through_its_tasty_alias --locked
cargo test -p dotty-classloader term_alias --locked
cargo test -p dotty-typer --lib default_term_import_resolves_a_scala_package_term --locked
```

The fresh baseline and after-change cohort runs used the same command with
different output and temporary paths:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-baseline-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-baseline.md

PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-after-runs-final" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-after-final.md
```

The fresh baseline command and report are recorded in the issue work log; the
baseline cohort sidecar is `target/issue-1566-baseline-cohort.tsv`. These
generated files live under `target/` and are not source-of-truth fixtures.
