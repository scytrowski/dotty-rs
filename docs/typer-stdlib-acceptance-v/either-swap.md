# Acceptance V: original `Either.swap`

## Outcome

The byte-identical Scala 3.9.0 `Either.scala#tree=84` method now completes
through the ordinary `SourceTyper::type_method_definition` path. The expression
`Match` keeps its source-branch union type
`Right[Nothing, A] | Left[B, Nothing]`; that union conforms to the method's
declared `Either[B, A]` result. The typed `Match` is not coerced to the applied
method result. This follows the pinned Scala 3.9.0 typed tree and the explicit
acceptance decision for #2247.

The method retains owner `scala.util.Either`, source result argument order
`[B, A]`, both original pattern cases, generated `Right.apply` and `Left.apply`
calls, and exact source-to-typed span mappings. A test-injected failure after
successful typing rolls semantic publication and the source-to-typed mapping
back; a same-session retry succeeds. The independent run below repeats that
sequence in a fresh frontend, resolver, store, and arena.

## Pinned evidence

- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Original `Either.scala` SHA-256:
  `7991d19a22ce0eb3d63b41ac61e7feabdeb97ed2f95555073cb3757224be8caa`.
- JDK: Eclipse Temurin 21.0.13+9; Scala compiler: official 3.9.0 artifacts.
- Scala's original typed tree declares `Either.this.B, Either.this.A`; its two
  result branches are `Right.apply[Nothing, A](a)` and
  `Left.apply[B, Nothing](b)`.
- Scala and Rust retain both original `Left`/`Right` extractors and their
  `[A, B]` type arguments. Rust verifies the original method and `Match` spans:
  `[8241, 8341)` and `[8266, 8341)`.

The focused Rust test was run twice as separate Cargo test processes. Each
process creates a fresh frontend/resolver/store/arena, injects a late failure,
checks rollback, and retries the full original method. The normalized semantic
receipts were identical:

```text
Match=scala.util.Right[Nothing,A] | scala.util.Left[B,Nothing]
method-result=Either[B,A]
union-conforms-to-method-result=accepted
wrong-generic-order=rejected
incompatible-Boolean-result=rejected
source-root=84@[8241,8341); match=83@[8266,8341)
```

Arena IDs are intentionally excluded from that receipt. The test separately
asserts exact source spans and the canonical owner/type-parameter symbols.

## Frozen cohort checks

The existing main and analysis manifests were not edited. Both unchanged
25-root ordinary audits ran with deterministic double execution and the
complete-member `DefDef` path. The main audit now reports **20/25 typed**, with
`Either.swap` typed as
`scala.util.Either[scala.util.Either.B,scala.util.Either.A]`; the analysis audit
reports **16/25 typed**. All 14 protected main identities from the Acceptance
III reconciliation and all 15 previously typed analysis identities retain
their result shapes. No protected root regressed. The root remains in its
original main cohort position; no denominator or selection changed.

The exact audit artifacts were generated in managed scratch during validation
and are not checked in as replacement baselines. This focused receipt credits
only the complete original `Either.swap` proof; final release-gate decisions
remain with the follow-up issue.

## Reproduction

With the pinned Scala checkout and eight pinned Scala 3.9.0 jars available:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
JAVA_HOME=/path/to/temurin-21
SCALA39_JDK_RELEASE=21
SCALA39_CLASSPATH=/path/to/the/eight/pinned/jars
TMPDIR=/path/to/managed-scratch
TMP="$TMPDIR"
TEMP="$TMPDIR"
export SCALA39_ROOT JAVA_HOME SCALA39_JDK_RELEASE SCALA39_CLASSPATH TMPDIR TMP TEMP

cargo test -p dotty-typer --lib \
  types_acceptance_v_either_swap_with_original_generated_applications \
  --locked -- --ignored --nocapture

DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1 \
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism /path/to/managed-scratch/main.tsv

DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1 \
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism /path/to/managed-scratch/analysis.tsv
```
