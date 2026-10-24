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

The exact method identities and next blockers are in
[`typer-name-resolution-progress-3.9.0.tsv`](typer-name-resolution-progress-3.9.0.tsv).
For `CommandLineParser.scala#tree=397` (`tokenize`), the next blocker is
`LocalValueConformanceUnsupported` at the inferred local `accum`. The other
remaining `CommandLineParser.scala#tree=498` (`expandArg`) blocker is still
`TermNameNotFound` at `paths`.

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
