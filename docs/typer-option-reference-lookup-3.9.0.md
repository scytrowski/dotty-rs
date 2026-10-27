# Option reference equality audit (Scala 3.9.0)

This report records the focused work for #1744. The source oracle is pinned to
Scala revision `777528f19a58e794c9954a42f433373472ec57f8`; the Rust baseline is
`dc71c164aab415dc8a75e698e7ab64fd85891faf`.

## Results

The pinned roots are `Option.isEmpty` (`Option.scala#tree=118`) and
`Option.nonEmpty` (`Option.scala#tree=329`). The frozen ordinary-method cohort
contains 25 roots.

| Scope | Baseline | After | Change |
| --- | ---: | ---: | ---: |
| `isEmpty`, standalone | blocked: `MemberNotFound` for `eq` | typed `Boolean` | unblocked |
| `nonEmpty`, standalone | typed `Boolean` | typed `Boolean` | retained |
| both roots, one source session | 1/2 typed | 2/2 typed | `isEmpty` unblocked |
| frozen 25-root cohort | 10 typed, 15 blocked | 12 typed, 13 blocked | +2 typed |

In the baseline cohort, `nonEmpty` was blocked at its declared `Boolean` type
(`AmbiguousTypeName`) after earlier `Option` roots had populated the shared
session. It typed in isolation. The final cohort types `isDefined`,
`knownSize`, `isEmpty`, and `nonEmpty`. All ten baseline typed roots remain
typed.

The new root allocates typed nodes before later roots in the same session, so
the numeric `typed_tree` IDs for `isDefined` and `knownSize` move from 3 to 9
and from 9 to 15. `nonEmpty` was blocked in the baseline cohort and has ID 18
in the final cohort. These IDs are allocation-specific; the source roots,
result types, and typed statuses are the stable outcomes. The deterministic
audit compares normalized outcomes across two runs; the optional frozen-cohort
gate also compares allocation IDs and therefore reports the expected shifts.

The separate CI focused smoke audit covers 31 methods in three files. Its
checked-in baseline records one related blocker progression in
`StringParsers.rec`: the deferred generic-parent lookup now checks inherited
field names safely and resolves `scala.None`, then reaches a later
`MemberNotFound`. That method remains untyped; the focused baseline records the
new diagnostic and resolver metrics explicitly.

The baseline and final per-root cohort outcomes are recorded in
[`typer-option-reference-lookup-3.9.0.tsv`](typer-option-reference-lookup-3.9.0.tsv).

## What changed

The old identity lookup accepted only a simple source `TypeRef`/`ThisType`
whose completed direct parent was exactly `Object`. `Option.this` is
`Option[A]`, and its parents include generic collection traits. That rejected
the receiver before the registered `AnyRef.eq` symbol could be selected.

The typer now accepts a canonical source class receiver when its source
identity, package prefix, class scope, and applied type-parameter arity agree.
It does not complete the whole class just to establish that a class is a
reference type. Before selecting the registered `eq`/`ne` symbol, it checks
same-named declarations in the current source class and source parents resolved
through their full type-tree identities, including qualified and imported names.
Shared ancestors in a diamond are visited once. This preserves direct and
inherited source shadowing without instantiating unrelated external generic
parents.

For the identity argument, canonical source class values and module objects
with a validated Object/ModuleClass relationship are viewed as `Object` for
argument conformance. Their typed source expressions retain their original
types. When an unqualified inherited-field probe stops at an external generic
parent, a package-level Scala object is considered only after the canonical
`scala` package owner and module identity are validated; the resolver does not
infer `None` from its spelling. Before continuing to an outer lexical scope or
import, the typer checks the raw parent hierarchy for a matching public
inherited field, preserving field shadowing when generic parent adaptation is
deferred.

Resolving canonical builtin types now gives same-package source declarations
their existing precedence, then uses the builtin identity before unrelated
external package-cache candidates. This avoids a cached Scala package object
making `Boolean` ambiguous in later roots of the same session.

## Scala oracle

Using the pinned Scala 3.9.0 compiler:

- `this eq scala.None`, `isDefined`, and `nonEmpty` typecheck with `Boolean`
  results.
- `Any eq Any`, the misspelled `eqq`, and the nonexistent `scala.NotNone` are
  rejected.
- The compiler also accepts `Int eq scala.None` through its extension
  conversion. That separate primitive conversion is outside this increment;
  the typer's existing `Int eq Int` rejection remains covered by the regression
  test.
- The Rust test checks that a generic source class selects the registered
  canonical `eq` symbol. `Definitions` validates its owner, flags, one `Object`
  parameter, and `Boolean` result.

The oracle command used Scala 3.9.0 compiler artifacts from the local Coursier
cache and kept compiler scratch/output under `target/issue-1744/oracle`.

## Reproducing the audit

Run from the repository root with the pinned Scala checkout and cached compiler
artifacts available. Set `SCALA39_ROOT`, `JAVA_HOME`, and
`COURSIER_MAVEN_ROOT` for the local environment. The audit scratch and Cargo
outputs stay under the repository's task-specific `target` directory.

```sh
mkdir -p target/issue-1744/tmp target/issue-1744/audit-tmp target/issue-1744/cargo
env \
  SCALA39_ROOT="$SCALA39_ROOT" \
  JAVA_HOME="$JAVA_HOME" \
  SCALA39_JDK_RELEASE=25 \
  COURSIER_MAVEN_ROOT="$COURSIER_MAVEN_ROOT" \
  CARGO_TARGET_DIR="$PWD/target/issue-1744/cargo" \
  TMPDIR="$PWD/target/issue-1744/tmp" \
  DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1744/audit-tmp" \
  CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
  tools/typer-classpath-corpus-audit/run \
    --method-kind ordinary \
    --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
    --verify-determinism target/issue-1744/final-25.tsv
```

The run completed twice with identical normalized output. It audited the
frozen 25-method cohort only; it did not run the 1,236-source corpus.
