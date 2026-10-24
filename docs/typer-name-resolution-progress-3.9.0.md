# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

## Baseline and selected increment

The fresh baseline is `main` at
`e06c4eeea02471f674c6b4c0263eaf69f3bc1f60`. It includes the merged #1571 and
#1574 increments. The pinned inputs are Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and JDK
21.0.8.

This increment handles an imported Java class used as a qualifier for static
members. The Typer preserves the class's canonical symbol and type identity,
allows only members marked `STATIC`, and applies the same rule in ordinary
selection and overloaded method application. The class remains invalid as a
standalone value. It does not synthesize a Java companion or infer staticness
from spelling.

The Scala 3.9 oracle accepts `Paths.get("input.txt")` after importing
`java.nio.file.Paths` and rejects both `Paths` as a value and an instance-method
selection through the class. Typer regression tests cover successful selection
of a static method and rejection of an instance method.

## Frozen-cohort result

The pinned `package-term` cohort covers 60 files and 369 real methods. The
baseline and final run each attempted all 369 methods; both typed zero methods.
One method row advanced, with no first-blocker regressions. The normalized
cohort reports and detailed resolver sidecars matched byte-for-byte across two
final runs. The 3-file focused smoke (31 methods) and the 77-file `object-module`
cohort (576 methods) also completed twice with byte-identical normalized output;
neither audit had parser or namer failures.

| First blocker | Baseline rows | Final rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 2 | 1 |
| `MemberNotFound` | 5 | 6 |
| All other first blockers | 362 | 362 |
| Fully typed methods | 0 | 0 |

The changed row is `compiler/src/dotty/tools/dotc/config/CommandLineParser.scala#tree=467`
(`stripComment`), attempted at `#tree=498` (`expandArg`). The baseline stopped
at `TermNameNotFound(Paths)`. The final audit resolves `Paths.get` to the two
static Java overloads and moves the first blocker to
`MemberNotFound(String.stripPrefix)`. This is verified semantic progress, but
the method is still not typed. The issue's stretch goal of fully typing one
previously blocked real method remains unmet.

The complete per-method before/after reconciliation is in
[`typer-name-resolution-1566-cohort-3.9.0.tsv`](typer-name-resolution-1566-cohort-3.9.0.tsv).
The earlier default Scala package-term increment remains recorded in
[`typer-name-resolution-progress-3.9.0.tsv`](typer-name-resolution-progress-3.9.0.tsv).

## Next bounded blocker

The next failure in `expandArg` is Scala's `String.stripPrefix` extension
method. Resolving this correctly requires the Scala string extension/member
view; this increment does not manufacture it. Other cohort blockers are
unchanged, including 144 `ImportQualifierNotFound`, 97 `SymbolResolution`, 30
`TypeNameNotFound`, and 13 `LocalValueConformanceUnsupported` rows. No full
corpus result is claimed.

## Reproduction

Run the Scala oracle with the pinned artifacts by setting `JAVA_HOME` and
`SCALA39_CLASSPATH`, then:

```sh
python3 tools/typer-classpath-corpus-audit/scala39_import_oracle.py
cargo test -p dotty-typer --test refactor_contract java_class_import_qualifier --locked
```

The deterministic cohort audit used:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-final4-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-final4.md
```
