# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

## Baseline and selected increment

The prior implementation comparison started from `main` at
`e06c4eeea02471f674c6b4c0263eaf69f3bc1f60`, before #1577 was merged. Its
selected increment handled an imported Java class used as a qualifier for
static members. The fresh post-merge baseline is now `main` at
`bfc4759ea7d4eca10767ef5021ffc8386178c4a5`; it includes #1586, #1577, #1571,
and #1574. The pinned inputs are Scala revision
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
| `MemberNotFound` | 5 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

The changes are `InlineInfoAttribute.scala#tree=212`, `#tree=226`, and `#tree=239`
(`nextByte`, `nextUTF8`, and `nextShort`), all attempted at `#tree=420`. They
previously stopped at `MemberNotFound(+=)` and now type the mutable local
integer update through `+=`'s addition-and-assignment fallback. Each advances
to `SymbolResolution` while loading `scala.Array`, whose TASTy parent
`java.io.Serializable` is currently unresolved. This demonstrates the new
rewrite on three real methods, but none is fully typed. The older Java static
qualifier change and merged #1577 also remain represented in the cumulative
reconciliation. The issue's stretch goal of fully typing one previously
blocked real method remains unmet.

The complete per-method before/after reconciliation is in
[`typer-name-resolution-1566-cohort-3.9.0.tsv`](typer-name-resolution-1566-cohort-3.9.0.tsv).
The earlier default Scala package-term increment remains recorded in
[`typer-name-resolution-progress-3.9.0.tsv`](typer-name-resolution-progress-3.9.0.tsv).

## Next bounded blocker

The next failure in `expandArg` is Scala's `String.stripPrefix` extension
method. The pinned Scala source defines `Predef.augmentString` with the alias
`Predef.String` as its parameter (`library/src/scala/Predef.scala:171`). The
classpath resolver finds the canonical `scala.Predef$.augmentString` method,
but the current TASTy member-type lowering treats its parameter name
`scala/Predef$String` as a JVM class. No such class exists, so the parameter is
represented as `Type::Error` with the loader diagnostic `class not found:
scala/Predef$String`. Replacing that error with `java.lang.String` by spelling
would manufacture a type instead of resolving the alias, so no implicit-view
call is synthesized. Correct support needs alias-aware TASTy member-type
resolution; general implicit search remains outside this bounded increment.
The three newly advanced methods encounter a separate classpath boundary:
decoding `scala.Array` fails on its selected-qualified `java.io.Serializable`
supertype reference. The loader leaves semantic references unresolved when it
cannot prove their owner path; this increment does not add a spelling-based
package guess.

The fresh pre-increment baseline at `bfc4759ea7d4eca10767ef5021ffc8386178c4a5`
and the latest implementation each audited all 60 files and 369 methods twice
with deterministic normalized reports and sidecars. In the latest report,
`CommandLineParser.scala#tree=467` still stops at `MemberNotFound` for
`String.stripPrefix`; the three `InlineInfoAttribute` rows stop at
`SymbolResolution`. The compact TSV records the cumulative before/after rows.
No full-corpus result is claimed.

## Reproduction

Run the Scala oracle with the pinned artifacts by setting `JAVA_HOME` and
`SCALA39_CLASSPATH`, then:

```sh
python3 tools/typer-classpath-corpus-audit/scala39_import_oracle.py
cargo test -p dotty-typer --test refactor_contract java_class_import_qualifier --locked
```

The deterministic pre-increment baseline audit used:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-latest-baseline-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-latest-baseline.md
```

The deterministic latest implementation audit used the same command with
`target/issue-1566-next.md` as the output and
`target/issue-1566-next-runs` as `DOTTY_AUDIT_TMP_ROOT`.
