# Source and classpath class identity (Scala 3.9.0)

## Scope and status

This increment makes a narrow source-shadowing case explicit in the
classloader: a complete source-owned top-level class or trait already entered
under the canonical package owner can answer a classpath identity request for
that exact package-qualified type. The classpath entry is still decoded and
checked before it can establish this view. The source symbol, its `ClassInfo`,
declaration scope, origin, and source-only members remain untouched. An
external class loaded afterward can refer to the same source symbol in its
parent hierarchy. Hierarchy loading applies this arbitration before parent
completion even when the external child is the first lookup, so it does not
allocate a second class symbol in the package scope.

This is not a name-based merge. Reuse requires one package-scope declaration,
the exact type `Name`, the exact package owner, a matching class-versus-trait
kind, source provenance, and a valid complete `ClassInfo` whose class and
declaration-scope owner are the source symbol itself. Incomplete, duplicate,
malformed, differently owned, or kind-incompatible declarations remain
unresolved or fail with the existing typed error. Object/ModuleClass and
companion-term identities use their existing separate paths. In particular,
source class identity reuse does not authorize publishing a binary object or
module-class companion onto an ordinary source class.

The top-level source-companion change from #1826 now advances the two roots
that previously stopped at module identity collisions. On the combined tree,
`Option.map` reaches `IfBranchJoinUnsupported` and `Either.swap` reaches
`ExtractorUnapplyNotFound`; neither whole method types. `Tuple.toArray` remains
blocked at argument conformance. These root advances come from #1826, not this
classloader change. This PR separately covers identity reuse for complete
source classes and fail-closed behavior for incomplete source identities.

## Evidence

The focused regression constructs a source-owned class with a source-only
member and a same-qualified classpath class, then loads an external subclass
first. It verifies that the source `SymbolId` is reused as the external parent
and that source provenance, `ClassInfo`, declarations, and single package
membership are unchanged. A companion regression preserves ordinary source
class identity reuse for type references while rejecting a same-name binary
object companion without mutating source links or publishing module identities.
It does not establish Scala compiler parity or a whole-method typing result.

After rebasing this PR onto `main` at `8b14506e83019474317e5223c724f667b2de19e5` (including #1826), the exact three roots were audited against pinned Scala revision `777528f19a58e794c9954a42f433373472ec57f8` and JDK 21 (`/home/vdrake6/.jdks/jdk-21`). The deterministic audit reports 0/3 whole methods typed. `Option.map` now reaches `IfBranchJoinUnsupported`, `Either.swap` reaches `ExtractorUnapplyNotFound`, and `Tuple.toArray` remains at `ApplicationArgumentConformanceUnsupported`. The first two advances match #1826's report; this PR makes no claim to have caused them.

The full frozen ordinary cohort passed twice with determinism verification on the combined tree: 14/25 complete, 11 blocked, 0 excluded, 0 not attempted. The three-root audit passed determinism verification, with 0/3 whole methods typed.

The pinned Scala 3.9 import oracle controls passed under JDK 21. A new focused source/classpath oracle also passed. It compiles a binary `Base` and `Child extends Base`, then a source `Base` shadow; Scala accepts the external child as the source parent. For a binary case-class `Product` with a `classpathOnly` companion method, a same-name source case class can be constructed but does not inherit that binary companion method. An unrelated ordinary source `Product` also cannot use that method. Therefore attaching the external `Some`/`Left` module to an incomplete source class based on name, owner, and `CASE` flag would not match Scala 3.9 semantics.

The source audit reports `scala.Some` and `scala.util.Left` as package-owned source type `Class` symbols whose `ClassInfo` remains `Missing`. #1826 now supplies each with a canonical synthetic package-owned companion term and module class, bidirectional class/object links, and a derived `apply` for represented constructor shapes. The module identity collisions are gone, but later Typer blockers remain. The loader continues to reject incomplete or incompatible classpath identities rather than importing binary-only companion members. On the rebased tree, all `dotty-classloader` targets pass, including the source-module collision rollback/retry test, independent classpath snapshot test, and complete source-class parent identity test.

No `scalac` executable is on `PATH`; the pinned Scala controls invoked `dotty.tools.dotc.Main` from the verified Scala 3.9.0 compiler jars. The source/classpath controls confirm that a same-named binary companion's `classpathOnly` member must not flow into a source case class or unrelated source class. The classloader behavior in this PR remains independently useful for complete source-owned parent classes, while incomplete source class and module identities continue to fail closed.

## Reproduction

The source/classpath regression tests use the repository's synthetic classfile and TASTy fixtures:

```sh
cargo test -p dotty-classloader --lib \
  classpath_identity_reuses_a_complete_same_package_source_class_without_overwriting_it \
  --locked
cargo test -p dotty-classloader --lib \
  module_identity_fails_closed_on_an_incomplete_source_case_class_companion_and_can_retry \
  --locked
cargo test -p dotty-classloader --lib \
  module_identity_rejects_a_complete_ordinary_source_companion_class_without_mutation \
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

The focused three-root audit uses the exact rows from the frozen cohort:

```sh
awk -F '\t' 'NR == 1 || $1 == "library/src/scala/Tuple.scala#tree=23" || $1 == "library/src/scala/Option.scala#tree=214" || $1 == "library/src/scala/util/Either.scala#tree=84"' \
  docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  > target/typer-source-classpath-identity-roots-3.9.0.tsv
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest target/typer-source-classpath-identity-roots-3.9.0.tsv \
  --verify-determinism OUTPUT.tsv
```

The frozen ordinary cohort uses the same runner:

```sh
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism OUTPUT.tsv
```

For reproducibility, set `SCALA39_ROOT` to the pinned checkout, `JAVA_HOME` to
the selected JDK, `SCALA39_JDK_RELEASE` to its feature version, and
`COURSIER_MAVEN_ROOT` to the cached Scala 3.9.0 Maven artifacts. The exact
three-root output used a task-local three-row subset of the frozen manifest.
