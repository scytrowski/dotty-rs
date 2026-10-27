# `Either.left` / `Either.right` nested projection audit (Scala 3.9.0)

**Result: ACCEPTED.** Both targeted methods now produce fully typed method
definitions with the expected projection result types.

## Scope and pinned inputs

- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Scala compiler/library: pinned 3.9.0 artifacts; JDK 21.
- dotty-rs base: `5b84f092d3abae3aed365ccfafa0f46b53e48a9d` (main after #1776).
- Implementation fix: `b775708843afdff3b76c72eadcfbdcb472ab7627` (preserves singleton
  conformance; based on Typer commit `dc1bbeaea0ecd1ebea8be1a67573a39460ee043e`
  and Namer provenance commit `ce52437b0b474d2a856f00f48fc6b418d87071ff`).
- Exact roots: `library/src/scala/util/Either.scala#tree=19` (`left`) and
  `#tree=31` (`right`).
- No hosted full standard-library corpus audit was run.

## First blocker and provenance

On the exact two-method audit at the base SHA, the first blocker was
`ObjectModuleClassUnavailable` for each generated nested projection companion.
After adding canonical provenance for the generated companion `ModuleClass`,
the first failure moved to source-kind validation, then generated `apply`
completion, generic inference from `this`, and finally argument conformance.
The final implementation resolves the `Either` qualifier to its canonical
`ModuleClass`; obtains the nested `CASE Class`, companion `Object` and companion
`ModuleClass` through reciprocal symbol links and `SourceSemanticIndex`
provenance; completes a bounded companion `ClassInfo`; and completes the
generated polymorphic `apply` from the source constructor's parameter symbols.
It uses the existing `ThisType` receiver view for both constraint inference and
argument conformance.

The implementation does not infer identity from names, synthesize companions
for ordinary nested classes, lower constructors, or add `unapply`/`copy`.
Regression coverage includes generated and explicit companions, canonical
ModuleClass provenance, generated `apply` signature shape, and generic `this`
inference. The explicit companion test confirms that an existing `object Node`
and its members are retained while the case class's generated `apply` is
completed separately. Application conformance keeps an original `this.type`
when it already matches the parameter, then falls back to the applied generic
receiver view only when needed.

## Scala 3.9.0 oracle

The checked-in oracle harness under
`tools/typer-classpath-corpus-audit/either-companion-oracle/` compiles the
minimal nested case-class shape and the focused negative controls against the
pinned Scala artifacts. Its typed tree shows `EitherLike.LeftProjection.apply`
and `RightProjection.apply` instantiated with both type parameters and called
with `this`. Dotty emits each nested case class's companion `Object` and
`ModuleClass` and a generated polymorphic `apply`.

The controls establish that same-spelled nested classes retain distinct
owners, a user companion overload can coexist with generated `apply`, and an
ordinary nested class is constructed with `new` rather than a synthesized
companion. Wrong arity/type, inaccessible constructor, and ambiguous overload
examples are rejected by dotc. These results rule out treating every nested
class as a term or fabricating an `apply` based only on its spelling.

## Cohort result

Two deterministic passes of the exact two-method audit and two passes of the
unchanged 25-method audit produced identical normalized reports. The exact
targets are **2/2 fully typed**. The unchanged cohort moved from **12/25 to
14/25 fully typed**; the complete per-method result is checked in at
[`typer-either-nested-projection-3.9.0.tsv`](typer-either-nested-projection-3.9.0.tsv).
The checked-in TSV uses `-` in the final `typed_tree` field when a method has
no typed tree, avoiding ambiguous trailing empty fields. All 12 methods typed
at the base remain typed with the same source-tree IDs and result shapes. The
new roots have report-local typed-tree IDs 6 and 13; the existing `Either.isLeft`
and `Either.isRight` report IDs shifted from 2 and 5 to 16 and 19. These IDs are
not semantic identities.

| Root | Result | First failure / note |
| --- | --- | --- |
| `Either.scala#tree=19` `left` | typed | `Either$.LeftProjection[Either.A, Either.B]`, typed tree 6 |
| `Either.scala#tree=31` `right` | typed | `Either$.RightProjection[Either.A, Either.B]`, typed tree 13 |
| `Either.scala#tree=84` `swap` | blocked | Separate external `scala.util.Left` module identity failure; outside this issue's projection path |

The pinned Scala 3.9.0 oracle expects exactly the observed result shapes. The
frozen 25-method manifest was not changed. No hosted full standard-library
corpus audit was run.

## Disposition

The stretch target in #1747 is met. Existing pinned Scala oracle controls cover
ordinary nested classes, same-named classes with distinct owners, explicit
companion overloads, wrong arity/type, inaccessible constructors, and
ambiguous overloads. Rust regression tests cover the generated and explicit
companion paths used here. #1773 supplies the bounded canonical case-class
companion representation used by this implementation. No constructor lowering,
`unapply`, `copy`, JVM work, broad overload inference, or hosted full-corpus
audit was added.

## Reproduction

From the repository root, use the task-local pinned oracle harness:

```sh
env PATH="$JDK21_HOME/bin:$PATH" JAVA_HOME="$JDK21_HOME" \
  SCALA39_JDK_RELEASE=21 SCALA39_ROOT="$SCALA39_ROOT" \
  COURSIER_MAVEN_ROOT="$COURSIER_MAVEN_ROOT" \
  DOTTY_EITHER_COMPANION_TMP_ROOT="$PWD/target/issue-1747/oracle-script" \
  tools/typer-classpath-corpus-audit/either-companion-oracle/run
```

The script verifies the pinned Scala source revision, JDK release, and SHA-256
manifest for all eight Scala artifacts; it retains compiler output under the
task-specific `target/issue-1747/oracle-script` directory. The audit reports
and run logs used for the two-pass 25-method comparison were retained under
`target/issue-1747/` during this work.

To regenerate the exact two-method result, derive a selection manifest from
the frozen cohort and retain both runs under task-local `target/` directories:

```sh
mkdir -p target/issue-1747/reports target/issue-1747/test-tmp
python3 - <<'PYGEN'
import csv
from pathlib import Path
source = Path("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv")
selected = {
    "library/src/scala/util/Either.scala#tree=19",
    "library/src/scala/util/Either.scala#tree=31",
}
rows = [row for row in csv.DictReader(source.open(encoding="utf-8"), delimiter="\t")
        if row["identity"] in selected]
assert {row["identity"] for row in rows} == selected
with Path("target/issue-1747/reports/either-pair-manifest.tsv").open(
    "w", encoding="utf-8", newline=""
) as output:
    writer = csv.DictWriter(output, fieldnames=rows[0].keys(), delimiter="\t")
    writer.writeheader()
    writer.writerows(rows)
PYGEN
env PATH="/home/vdrake6/.jdks/jdk-21/bin:$PATH" \
  JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
  SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
  SCALA39_JDK_RELEASE=21 \
  COURSIER_MAVEN_ROOT=/home/vdrake6/.codex/worktrees/bdf3/dotty-rs/target/worktrees/issue-1746/target/issue-1746/coursier/https/repo1.maven.org/maven2 \
  TMPDIR="$PWD/target/issue-1747/test-tmp" \
  DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1747/audit-pair-tmp" \
  tools/typer-classpath-corpus-audit/run \
    --method-kind ordinary \
    --method-manifest target/issue-1747/reports/either-pair-manifest.tsv \
    --verify-determinism \
    target/issue-1747/reports/either-pair-reproduced.tsv
```

To regenerate the unchanged 25-method cohort, use the frozen manifest:

```sh
env PATH="/home/vdrake6/.jdks/jdk-21/bin:$PATH" \
  JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
  SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
  SCALA39_JDK_RELEASE=21 \
  COURSIER_MAVEN_ROOT=/home/vdrake6/.codex/worktrees/bdf3/dotty-rs/target/worktrees/issue-1746/target/issue-1746/coursier/https/repo1.maven.org/maven2 \
  TMPDIR="$PWD/target/issue-1747/test-tmp" \
  DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1747/audit-cohort-tmp" \
  tools/typer-classpath-corpus-audit/run \
    --method-kind ordinary \
    --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
    --verify-determinism \
    target/issue-1747/reports/ordinary-reproduced.tsv
```

The runner validates exact `path#tree` identities, forces one Cargo build/test
thread, compares both normalized reports, and retains logs and intermediates
under the task-local directory. Comparison against the base report showed all
12 previously typed roots retained their typed status, source-tree identity,
and result shape. The existing `Either.isLeft` and `Either.isRight` report IDs
shifted from 2 and 5 to 16 and 19 as the two requested methods were added.
Those numbers are report-local IDs, not semantic identity.

## Validation recorded for issue #1747

- `cargo fmt --all -- --check` — passed.
- `python3 tools/check-frontend-boundaries.py` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` — passed.
- `cargo test --workspace --all-targets --locked` — passed with `TMPDIR` pointed at `target/issue-1747/test-tmp` because the shared `/tmp` volume was full.
- Mandatory JDK 21 gate with `DOTTY_CLASSFILE_REQUIRE_JDK=1` — passed for all requested targets.
- Focused Namer regression: `nested_case_class_gets_a_companion_and_source_mapped_apply_parameters` — passed.
- Focused Typer regressions: generated companion identity/ClassInfo/apply signature, explicit companion apply signature, generic `this` inference, and ordinary plus explicit-polymorphic `this.type` arguments — passed.
- Scala 3.9.0 case-companion oracle harness — passed; pinned artifacts and source revision verified.
- The two-method report and both 25-method cohort reports were byte-identical between their repeated deterministic passes.
- Cohort comparison: exactly `Either.left` and `Either.right` newly typed; the 12 prior typed results retain their source IDs and result shapes. `Either.swap` remains blocked by its separate external `scala.util.Left` identity failure.
- `git diff --check` — passed.
