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

## Evidence

The focused regression constructs a source-owned class with a source-only
member and a same-qualified classpath class, then loads an external subclass.
It verifies that the source `SymbolId` is reused as the external parent and
that source provenance, `ClassInfo`, declarations, and package membership are
unchanged. It does not establish Scala compiler parity or a whole-method
typing result.

The issue's three real roots were not run in this environment. The pinned
Scala 3.9 checkout and runtime artifacts are not available (`SCALA39_ROOT` is
unset), so current-main before/after root diagnostics, the full 25-method
cohort, and Scala oracle controls remain outstanding. The rows in the adjacent
TSV distinguish the issue-reported hypotheses from reproduced outcomes.

## Reproduction

The test uses the repository's synthetic classfile builder and runs with the
classloader unit suite:

```sh
cargo test -p dotty-classloader --lib \
  classpath_identity_reuses_a_complete_same_package_source_class_without_overwriting_it \
  --locked
```

The local check passed on repository baseline
`21ca7e0d6921166fda810897efa9658956042735` plus this change. Scala source is
pinned to `777528f19a58e794c9954a42f433373472ec57f8` in the audit tooling.
