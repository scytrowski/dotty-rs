# Typer Stdlib Acceptance V baseline (Scala 3.9.0)

## Outcome

Measured the unchanged original `DefDef` roots on current integrated `main`
`cd8d72f09c6e9342c0a761c8887bb2c48a52add8`. `Tuple.toArray` remains blocked
in ordinary typing at its argument conformance check. `Either.swap` advances
past the historical `MemberLookup` blocker and now reaches the first
`Left` extractor pattern, where it stops at `ExtractorUnapplyNotFound`. The
separate explicit-owner-preparation proof still types the original
`Tuple.toArray` method; it is not ordinary-audit acceptance.

The complete unchanged main cohort remains **18/25 typed** and the analysis
cohort **16/25 typed**. Both normalized complete-`DefDef` runs are byte
identical. No root in the preceding Acceptance IV integration success sets
regressed, and neither cohort membership nor the 25-root denominator changed.
The full per-root result reports are in [`main-audit.tsv`](typer-stdlib-acceptance-v/main-audit.tsv)
and [`analysis-audit.tsv`](typer-stdlib-acceptance-v/analysis-audit.tsv);
[`successful-roots.tsv`](typer-stdlib-acceptance-v/successful-roots.tsv)
freezes every typed identity, result type, and typed-tree ID.

## Frozen inputs and provenance

- dotty-rs tested revision: `cd8d72f09c6e9342c0a761c8887bb2c48a52add8`.
- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`
  (Scala 3.9.0).
- Original source SHA-256: `Tuple.scala`
  `4d33bdb23f8446d313d531defd9957bb82672bd04211bca54d4bf08309c14d8e`;
  `Either.scala`
  `7991d19a22ce0eb3d63b41ac61e7feabdeb97ed2f95555073cb3757224be8caa`.
- Unchanged main manifest SHA-256:
  `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244`;
  analysis manifest SHA-256:
  `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`.
- JDK: Eclipse Temurin 21.0.13+9, feature 21. `release` SHA-256
  `f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898`;
  `bin/java` SHA-256
  `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`;
  `jmods/java.base.jmod` SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- The eight Scala 3.9.0 audit jars matched the pinned artifact hashes in the
  Acceptance IV baseline: `compiler-interface-1.12.0.jar`
  `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3`;
  `scala-asm-9.9.0-scala-1.jar`
  `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375`;
  `scala-library-3.9.0.jar`
  `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`;
  `scala3-compiler_3-3.9.0.jar`
  `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`;
  `scala3-interfaces-3.9.0.jar`
  `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db`;
  `scala3-library_3-3.9.0.jar`
  `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`;
  `tasty-core_3-3.9.0.jar`
  `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f`;
  `util-interface-1.11.5.jar`
  `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af`.

## Ordinary audit reproduction and results

The existing `tools/typer-classpath-corpus-audit/run` harness was run with
`DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1`, `--method-kind ordinary`, and
`--verify-determinism` for each unchanged manifest. This exercises the full
source `DefDef` through the real lexer, parser, namer, and
`SourceTyper::type_method_definition`; it does not count RHS-only probes.

| Cohort | Selected / attempted | Typed | Blocked | First / second normalized SHA-256 |
| --- | ---: | ---: | ---: | --- |
| main | 25 / 25 | 18 | 7 | `304f67ba1454383bd4dbc3a7e5c9d4d30e6c6e4cf15a41295d05eed2335a6bf5` |
| analysis | 25 / 25 | 16 | 9 | `671b3b9e1037d0416b228ab634b2cde4515beecab0926ae945488eb95eb9ed01` |

The two compared reports for each cohort had identical hashes. Current typed
identity/result sets exactly equal the prior Acceptance IV integration
snapshot's sets: main 18/25 and analysis 16/25; zero previously typed identities
regressed, and all 34 successful result shapes match. All remaining attempted
rows are explicitly `blocked`; there are no excluded or unattempted entries.
The two complete reports contain each root's status, first error, source tree,
and typed-tree ID. The harness emits an empty final `typed_tree` field for
blocked roots; the checked-in report copies use `-` for that empty field so the
TSV has no trailing whitespace. This representation change does not alter
outcomes and both copies pass the existing cohort verifier. Their SHA-256
values are `d6f2c0772afe4b0ef8da8d6f6c19961b4c5975e3a8db00840f38cc8c6850fc30`
for main and
`fe61dc2d328bd9d65c6d6c020fc22dff31f60890387ff3839c115b7eaa2c93d3` for
analysis; the table reports the unmodified byte-identical harness output
hashes.

Reproduction from the tested dotty-rs revision, with environment variables
`SCALA39_ROOT`, `JAVA_HOME`, `SCALA39_JDK_RELEASE=21`,
`COURSIER_MAVEN_ROOT`, and `TMPDIR`, `TMP`, `TEMP`,
`DOTTY_AUDIT_TMP_ROOT` pointing to managed scratch:

```sh
export DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism docs/typer-stdlib-acceptance-v/main-audit.tsv
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism docs/typer-stdlib-acceptance-v/analysis-audit.tsv
```

## Target root observations

[`roots.tsv`](typer-stdlib-acceptance-v/roots.tsv) records the exact source
root and failure evidence. The Rust failure observer for `Either.swap` reads
the live semantic store inside the transaction failure callback, before
rollback. Reported arena and symbol IDs are local to these captured sessions;
source paths, tree IDs, and semantic symbol paths are the stable identities.
The Tuple observation uses the existing Acceptance IV observer and the same
full-method pipeline.

- `Tuple.toArray`, original source tree 23, span `[282, 350)`, owner
  `scala.Tuple`, result `Array[Object]`, original RHS
  `runtime.Tuples.toArray(this)`. Ordinary typing fails first at tree 22,
  argument 0: `ApplicationArgumentConformanceUnsupported`; the nested error is
  `UnsupportedType`. Its actual operand is canonical `scala.Tuple.this`; the
  formal is an `ErrorType` from the loaded TASTy member signature. The selected
  member is `scala.runtime.Tuples$.toArray` from
  `scala-library-3.9.0.jar`; the failed attempt leaves `Tuple` class info
  `Missing`. The detailed unresolved formal message records the conflicting
  canonical identity for `scala/Tuple`. Separately,
  `original_tuple_to_array_types_after_explicit_canonical_owner_preparation`
  succeeds on the original complete method and preserves its mapping. That
  prepared result does not change the ordinary-audit outcome.
- `Either.swap`, original source tree 84, span `[8241, 8341)`, owner
  `scala.util.Either`, result `Either[B, A]`, original RHS
  `this match { case Left(a) => Right(a); case Right(b) => Left(b) }`. Ordinary
  typing fails first at source pattern tree 69, span `[8288, 8295)`, with
  `ExtractorUnapplyNotFound` for the first `Left(a)` pattern. This error carries
  no nested cause. The original AST has an `Apply(Ident(Left), Ident(a))`;
  observed source `Left` and `Right` case-class symbols link to synthetic
  companion objects, but the typer index returns no generated `unapply`
  method for either class at this failure point. The rejected extractor is the
  synthetic `.scala.util.Left` object; `Either` has complete class info while
  `Left` and `Right` remain `Missing`.
  The following original `Right(b)` pattern is separately recorded as
  `Apply(Ident(Right), Ident(b))`; it is observed in the original AST but is
  never reached by the failed first-pattern attempt.

## Pinned Scala 3.9.0 compiler oracle

The two unchanged complete source files were typechecked by
`dotty.tools.dotc.Main` from `scala3-compiler_3-3.9.0.jar` with the verified
Scala 3.9.0 jars and `-Ystop-after:typer -Xprint:typer`. The compiler reports
version `3.9.0`. Its printed tree confirms the original `Tuple.toArray`
signature and body:

```scala
inline def toArray: Array[Object] =
  runtime.Tuples.toArray(this): Array[Object]
```

For the unchanged `Either.swap`, the compiler's typed tree expands both source
patterns to the generated polymorphic extractors and confirms their actual
signatures:

```scala
def swap: scala.util.Either[Either.this.B, Either.this.A] =
  this match {
    case scala.util.Left.unapply[A, B](a @ _): Left[A, B] =>
      scala.util.Right.apply[Nothing, A](a)
    case scala.util.Right.unapply[A, B](b @ _): Right[A, B] =>
      scala.util.Left.apply[B, Nothing](b)
  }

def unapply[A, B](x: Left[A, B]): Option[A]
def unapply[A, B](x: Right[A, B]): Option[B]
```

The oracle was run against the pinned official compiler and original complete
source files, not synthesized replacement snippets.

Reproduction, with `CP` set to the colon-separated paths of the eight pinned
jars listed above, `SCALA39_ROOT` at the pinned checkout, and `TMPDIR` at
managed scratch:

```sh
java -cp "$CP" dotty.tools.dotc.Main -version
java -Djava.io.tmpdir="$TMPDIR" -cp "$CP" dotty.tools.dotc.Main \
  -classpath "$CP" -Xprint:typer -Ystop-after:typer \
  "$SCALA39_ROOT/library/src/scala/Tuple.scala"
java -Djava.io.tmpdir="$TMPDIR" -cp "$CP" dotty.tools.dotc.Main \
  -classpath "$CP" -Xprint:typer -Ystop-after:typer \
  "$SCALA39_ROOT/library/src/scala/util/Either.scala"
```
