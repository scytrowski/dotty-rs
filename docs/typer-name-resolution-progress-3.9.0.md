# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

## Baselines and increments

The cumulative comparison starts from `main` at
`e06c4eeea02471f674c6b4c0263eaf69f3bc1f60`, before #1577 was merged. PR #1577
added the Java static-qualifier increment, handling an imported Java class
used as a qualifier for static members. Its historical Scala 3.9 oracle check
accepted `Paths.get("input.txt")` after importing `java.nio.file.Paths`, and rejected
`Paths` as a value and instance-method selection through the class. Those
results describe that earlier increment, not this PR.

The fresh pre-increment baseline for this PR is `main` at
`bfc4759ea7d4eca10767ef5021ffc8386178c4a5`; it includes #1586, #1577, #1571,
and #1574. This PR adds a bounded fallback for `+=` on a mutable local `Int`:
if direct `+=` member lookup misses and no visible extension candidate takes
precedence, it types `value + rhs` and lowers that result to assignment while
preserving the local symbol identity. The pinned inputs are Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and JDK
21.0.8.

## Frozen-cohort result

The pinned `package-term` cohort covers 60 files and 369 real methods. The
fresh pre-increment baseline and latest run each attempted all 369 methods;
both typed zero methods. This PR advances three rows with no first-blocker
regressions. The normalized reports and resolver sidecars for the latest run
matched byte-for-byte across two runs; neither run had parser or namer
failures.

### This PR: `bfc4759` to current

| First blocker | Pre-increment rows | Current rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 1 | 1 |
| `MemberNotFound` | 6 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

| Source method | Pre-increment blocker | Current blocker | Attempt |
| --- | --- | --- | --- |
| `InlineInfoAttribute.scala#tree=212` (`nextByte`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |
| `InlineInfoAttribute.scala#tree=226` (`nextUTF8`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |
| `InlineInfoAttribute.scala#tree=239` (`nextShort`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |

The rewrite types the mutable local integer update in each method. Their next
failure is loading `scala.Array`, whose TASTy parent `java.io.Serializable`
remains unresolved. The methods are still blocked, so the issue's stretch goal
of fully typing one previously blocked real method remains unmet.

### Cumulative: historical `e06c4ee` to current

Four rows advance across all increments captured in the compact TSV: one in
merged #1577 and three in this PR. This table compares the historical baseline
with the latest result; it is not the `bfc4759` pre-increment comparison above.

| First blocker | Historical baseline rows | Current rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 2 | 1 |
| `MemberNotFound` | 5 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

The complete per-method before/after reconciliation is in
[`typer-name-resolution-1566-cohort-3.9.0.tsv`](typer-name-resolution-1566-cohort-3.9.0.tsv).
Its `before_*` columns refer to the historical `e06c4ee` baseline; its
`after_*` columns refer to the current implementation.
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

The Scala oracle command below validates the earlier Java static-qualifier
increment, not this PR. Run it with the pinned artifacts by setting `JAVA_HOME`
and `SCALA39_CLASSPATH`, then:

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
