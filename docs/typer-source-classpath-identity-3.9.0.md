# Source and classpath class identity (Scala 3.9.0)

## Scope and status

This increment makes a narrow source-shadowing case explicit in the
classloader: a complete source-owned top-level class or trait already entered
under the canonical package owner can answer a classpath identity request for
that exact package-qualified type. The classpath entry is still decoded and
checked before it can establish this view. The source symbol, its `ClassInfo`,
declaration scope, origin, and source-only members remain untouched. An
external class loaded afterward can refer to the same source symbol in its
parent hierarchy.

This is not a name-based merge. Reuse requires one package-scope declaration,
the exact type `Name`, the exact package owner, a matching class-versus-trait
kind, source provenance, and a valid complete `ClassInfo` whose class and
declaration-scope owner are the source symbol itself. Incomplete, duplicate,
malformed, differently owned, or kind-incompatible declarations remain
unresolved or fail with the existing typed error. Object/ModuleClass and
companion-term identities use their existing separate paths.

This change does **not** resolve the three pinned standard-library roots. The
exact root audit still reports both `Option.map` and `Either.swap` blocked by
external module identity collisions. `Tuple.toArray` reaches argument
conformance, but that same diagnostic was already present in the immediate
pre-change checkout; therefore this PR does not claim a real-root advance.
The scoped source-owned class reuse is covered synthetically, but it is not
sufficient evidence for the issue's real-root acceptance criterion.

## Evidence

The focused regression constructs a source-owned class with a source-only
member and a same-qualified classpath class, then loads an external subclass.
It verifies that the source `SymbolId` is reused as the external parent and
that source provenance, `ClassInfo`, declarations, and package membership are
unchanged. It does not establish Scala compiler parity or a whole-method
typing result.

The exact three roots were audited against the pinned Scala checkout at
`777528f19a58e794c9954a42f433373472ec57f8`, using Scala compiler 3.9.0 and the
JDK 21 (`/home/vdrake6/.jdks/jdk-21`). The immediate pre-change checkout was `21ca7e0d6921166fda810897efa9658956042735` (the PR base `2738ddf91cfba88a92649aec0f36a27988c7f4f8` adds disjoint JVM-only PR #1805). The result TSV records both method outcomes. The before/after root outcomes were identical: 0/3 fully typed, `Option.map` and `Either.swap` at module identity collisions, and `Tuple.toArray` at argument conformance.

The full frozen ordinary cohort ran twice with determinism verification under JDK 21 and reported 14/25 complete, 11 blocked, 0 excluded, and 0 not attempted. This preserves the baseline count of 14/25, but the report is not a claim that any blocked method was completed. The three-root focused run also passed twice with identical normalized output.

The pinned Scala 3.9 import oracle controls passed under JDK 21. A new focused source/classpath oracle also passed. It compiles a binary `Base` and `Child extends Base`, then a source `Base` shadow; Scala accepts the external child as the source parent. For a binary case-class `Product` with a `classpathOnly` companion method, a same-name source case class can be constructed but does not inherit that binary companion method. An unrelated ordinary source `Product` also cannot use that method. Therefore attaching the external `Some`/`Left` module to an incomplete source class based on name, owner, and `CASE` flag would not match Scala 3.9 semantics.

The real root audit's source-side trace records `scala.Some` and `scala.util.Left` as unique package-owned type-namespace `Class` symbols from `SourceId(0)`, with `SymbolInfo::Missing`, case flags, and no companion link. `scala.Tuple` is a source `Trait` with missing info and an existing source companion link. The pinned `scala-library-3.9.0.jar` contains both `.tasty` and `.class` entries for these names; the loader's TASTy-preferred lookup selects that jar's TASTy resource. The root errors are nested `IdentityConflict`s for an occupied canonical package name. The new synthetic collision test verifies rollback leaves the source declaration intact and removes partial module shells, then succeeds on retry after the conflict is removed. A separate test verifies two different classpath snapshot origins remain local to independent stores. These tests do not expose post-failure state from the full typer audit itself.

No `scalac` executable is on `PATH`; both oracles invoked `dotty.tools.dotc.Main` from the verified Scala 3.9.0 compiler jars. The evidence indicates the unresolved prerequisite is source Namer support for a canonical synthetic companion for top-level case classes (and its `apply` signature from source constructor parameters). That should be handled in the coordinated Namer workstream. The classloader should keep failing closed until it can consume a source-owned companion identity; it must not borrow the binary companion, which the Scala oracle rejects.

## Reproduction

The source/classpath regression tests use the repository's synthetic classfile and TASTy fixtures:

```sh
cargo test -p dotty-classloader --lib \
  classpath_identity_reuses_a_complete_same_package_source_class_without_overwriting_it \
  --locked
cargo test -p dotty-classloader --lib \
  module_identity_fails_closed_on_an_incomplete_source_case_class_and_can_retry \
  --locked
cargo test -p dotty-classloader --lib \
  module_identity_keeps_classpath_snapshot_origins_in_independent_stores \
  --locked
```

The Scala 3.9 source/classpath control uses the pinned compiler jars:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  tools/typer-classpath-corpus-audit/scala39_source_classpath_identity_oracle.py
```

The ordinary method cohort and three-root audit use the pinned-checkout harness:

```sh
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism OUTPUT.tsv
```

For reproducibility, set `SCALA39_ROOT` to the pinned checkout, `JAVA_HOME` to
the selected JDK, `SCALA39_JDK_RELEASE` to its feature version, and
`COURSIER_MAVEN_ROOT` to the cached Scala 3.9.0 Maven artifacts. The exact
three-root output used a task-local three-row subset of the frozen manifest.
