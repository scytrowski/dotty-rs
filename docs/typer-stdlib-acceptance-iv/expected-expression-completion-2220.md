# Expected-expression source completion (#2220)

This report records the caller-ordering correction that lets the ordinary
Typer complete only the source class metadata reachable from an expected
expression's adapted actual and expected types. `TypeRelation`, `is_subtype`,
and `conforms` retain their no-implicit-completion contract.

## Before and after

The issue's two fresh pre-change replays of the unchanged Scala 3.9.0
`library/src/scala/Option.scala#tree=214` stopped at tree 213 with
`ExpectedExpressionConformanceUnsupported` because canonical source
`scala.None$` had Missing `ClassInfo`. Diagnostic sessions that explicitly
completed source `None$`, `Option`, and `Some` then typed the full method.
Those observations isolated caller sequencing; they did not establish an
ordinary source-completion policy.

At dotty-rs base `378a01d0b224b9a8c0bb04b80a5e1495a365c8c4`, the ordinary
expected-expression caller adapted and rebound the actual type and then called
`conforms` without preparation. It now calls the existing bounded,
journaled `complete_relation_type` helper for that actual and the expected
type, using one shared visited set. The existing early return for polymorphic
expectations remains before this step.

## Pinned full-method replay

Inputs:

- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Scala artifacts: Scala 3.9.0 runtime/compiler artifacts, including the
  Scala library, TASTy core, Scala ASM, and compiler interfaces.
- JDK: Temurin 21.0.13+9, feature release 21.
- Test: ignored focused replay
  `captures_acceptance_iv_relation_operands_before_transaction_rollback` in
  `crates/dotty-typer/src/typer/tests/issue_2179_relation_observation.rs`.

The focused test was run twice in separate fresh test processes, each parsing
the original pinned source and typing the complete `Option.map` `DefDef`
without manual `ClassInfo` completion. Both runs passed. They verified the
canonical `scala.Option.map` owner, its one-parameter `B` polymorphic binder,
the canonical `Option`, `Some`, and `None` source symbols, the derived
`None$` module-class identity, and completed metadata for the reachable
`Option`, `Some`, and `None$` class information. The source-to-typed mapping
was identical in both runs:

```text
195:0,196:1,199:2,200:3,203:4,204:5,205:5,206:6,207:7,208:9,209:11,210:12,211:13,212:14,213:15,214:16
```

The test also retains the unrelated `Tuple.scala#tree=23` negative control;
`Tuple.toArray` still stops at its existing
`ApplicationArgumentConformanceUnsupported` error. It is not accepted or
reinterpreted by this caller change.

Reproduce the focused replay from the repository root with the pinned source,
the eight Scala 3.9.0 jars, and a JDK 21 installation configured in the
environment:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f
export JAVA_HOME=/path/to/temurin-21
export SCALA39_JDK_RELEASE=21
export COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache
export PATH="$JAVA_HOME/bin:$PATH"
export SCALA39_CLASSPATH="$(printf '%s:' \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar")"
export SCALA39_CLASSPATH="${SCALA39_CLASSPATH%:}"
export TMPDIR=/path/to/persistent-scratch
export TMP="$TMPDIR" TEMP="$TMPDIR"

cargo test -p dotty-typer --lib \
  captures_acceptance_iv_relation_operands_before_transaction_rollback \
  --locked -- --ignored --nocapture
```

The focused-test marker output SHA-256 was
`705760946f146f4238d52e926f47e80a1f927a231523407101c8abe15f02430a` in both
runs.

## Protected ordinary-method cohorts

The unchanged ordinary-method runner passed its deterministic two-pass gate
for both protected selections:

| Selection | Selected | Typed | Required cohort | Report SHA-256 |
| --- | ---: | ---: | ---: | --- |
| Main method cohort | 25 | 18 | 16 | `304f67ba1454383bd4dbc3a7e5c9d4d30e6c6e4cf15a41295d05eed2335a6bf5` |
| Analysis holdout cohort | 25 | 16 | 15 | `671b3b9e1037d0416b228ab634b2cde4515beecab0926ae945488eb95eb9ed01` |

The main report includes `Option.map` as typed at tree 214. The report hashes
cover the generated TSVs; the runner compared both passes byte-for-byte before
accepting each required cohort.

Reproduce these gates with the same pinned source, artifact classpath, JDK,
and persistent scratch configuration:

```sh
export DOTTY_AUDIT_TMP_ROOT="$TMPDIR/audit"
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv \
  --verify-determinism "$DOTTY_AUDIT_TMP_ROOT/main.tsv"

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv \
  --verify-determinism "$DOTTY_AUDIT_TMP_ROOT/analysis.tsv"
```

These checks validate the existing protected subsets; they are not a full
standard-library acceptance claim and do not regenerate compatibility
baselines.
