# Typer first-blocker progress for Scala 3.9.0

Issue #1529 tracks one high-impact first-blocker family at a time. This report
records a focused, reproducible `package-term` cohort; it is not a full-corpus
compatibility claim.

## Frozen diagnosis

- Baseline dotty-rs commit: `dac7f43b0ef76f25a66ea4fd585214991421af06`.
- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0).
- Cohort: 60 files and 369 methods. The baseline had 369 first-blocker rows,
  297 concrete target observations, no missing methods, and no fully typed
  methods.
- Root-error inspection found 27 methods whose first failing source type was
  unresolved (`TypeNameNotFound`). Descendant rows attributable to this family
  accounted for 63 of 369 first-blocker rows. The bounded names included
  `List` (3 roots), `Option` (2), `Array` (4), and `String` (3).
- The selected source-language gap is the low-priority default type imports
  `scala.*` and `java.lang.*`. Pinned Scala 3.9 source defines the nested
  default-import priority with `scala.*` inside `java.lang.*`. Explicit and
  lexical source declarations remain higher priority in this typer's existing
  lookup path. `Predef.*` term/member imports and general standard-library
  member lookup remain outside this increment.
- Representative source: pinned
  `compiler/src/dotty/tools/dotc/config/CommandLineParser.scala`, where
  `tokenize` declares `List[String]` without a source import. The previous
  failure was `TypeNameNotFound(List)` at the exact type-name span.
- The larger blocker counts in the same cohort (`ImportQualifierNotFound`,
  `SymbolResolution`, and `ObjectModuleClassUnavailable`) primarily exercise
  qualified classpath and package/module loading. They remain separate work.

## Scope and evidence

The implementation consults `scala.*` and then `java.lang.*` only after local
type parameters, lexical source declarations, and explicit/wildcard source
imports. It asks the existing `SymbolResolver` for canonical package members;
it does not manufacture class or alias symbols. For a missing direct `scala`
class, the classloader follows a package-object TASTy alias only when its
existing classpath validation proves one target, then returns that target's
canonical symbol. Unknown names remain unresolved.

Pinned Scala oracle controls cover default `List`, `Option`, explicit
`scala.List`, a local `List` declaration that shadows the default, and an
unknown type that must still fail. The focused typer regression checks that the
resolved type points to the exact package-owned symbol returned by the
resolver. The before/after cohort table records whether each frozen method
became typed or advanced to a later blocker; advancing a blocker does not count
as a fully typed method.
