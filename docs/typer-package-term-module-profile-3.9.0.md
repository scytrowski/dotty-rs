# Package-term and Scala object identity profile: Scala 3.9.0

This profile freezes the request and binary evidence from the
[Scala 3.9.0 classpath audit](typer-classpath-corpus-audit-3.9.0.md) for
[#1046](https://github.com/scytrowski/dotty-rs/issues/1046). It is a research
increment. It changes audit instrumentation and documentation only; resolver,
loader, and Typer behavior are unchanged.

The oracle is pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala artifacts 3.9.0, and Temurin
JDK 21.0.8. `tools/typer-classpath-corpus-audit/run --verify-determinism`
runs the normalized corpus audit twice, then compares both the raw report and
the supplemental TSVs byte-for-byte.

## Frozen request observations

The [request profile TSV](typer-classpath-corpus-audit-3.9.0-package-term-profile.tsv)
contains all 2,749 unsupported package-term observations from 115 source files
and their 80 distinct resolver keys. The [first-blocker TSV](typer-classpath-corpus-audit-3.9.0-package-term-first-blockers.tsv)
retains 361 `ImportQualifierNotFound` and eight `TermNameNotFound` observations.
The corpus audit also retains its existing 120 semantic cohort rows.

Every frozen package-term row is observed with a `Package` prefix, term
namespace, `Prefix` member space, `Unique` selector, and the current resolver
outcome `package term member unsupported`. The row retains the package path,
requested name, source file, and enclosing method attempt. The resolved
package prefix and the term member lookup are separate facts: a failed first
package segment is not recorded as one of these 2,749 package-term member
outcomes. Repeated observations remain separate rows; `request_key` identifies
the independent package/name/member-space key.

The first-blocker join preserves each matched resolver request alongside the
method that received the error. All 369 module-related observations are
inherited from an enclosing method attempt. A failed qualifier is recorded for
all 369 rows. `ImportQualifierNotFound` uses the exact qualifier selected by
the import lookup; `TermNameNotFound` uses the exact failed package-term
request. The cause attribution still retains the earlier package probes in
`request_targets`, so the terminal package-term failure is visible next to
any prefix probe that failed while resolving the same qualifier.

The 361 import observations span 36 distinct resolver targets; the eight term
observations span two. The largest import qualifier cohorts are:

| Failed qualifier | Observations |
| --- | ---: |
| `scala.jdk.CollectionConverters` | 161 |
| `dotty.tools.dotc.core.Contexts` | 39 |
| `dotty.tools.dotc.core.Annotations` | 34 |
| `dotty.tools.dotc.ast.tpd` | 25 |
| `dotty.tools.dotc.core.Constants` | 15 |
| `dotty.tools.dotc.ast.Trees` | 15 |
| `dotty.tools.dotc.core.Decorators` | 11 |
| `dotty.tools.backend.jvm.GenBCode` | 10 |

These are blocker observations, not 369 independently typed methods or
promised successes.

## Exact binary evidence and classification

The audit probes `.tasty` and `.class` resources independently for each
requested type-side binary name and its `$` companion name. Every result keeps
the resource format and classpath origin. `ClassPathEntry::find_class` still
prefers TASTy when both formats exist; the separate inventory shows both files
instead of treating that preference as proof of an object.

| Per-key TASTy semantic result | Keys |
| --- | ---: |
| Standalone object | 26 |
| Class with companion object | 20 |
| TASTy has no matching object definition | 28 |
| Unsupported evidence | 6 |
| Malformed or ambiguous TASTy identity | 0 |

The exact resource scan found type-side `.tasty` files for 46 keys and
type-side `.class` files for 51 keys. The sets overlap. All 46 companion
resources are `.class` files; there are no companion `$` `.tasty` files. The
six unsupported rows do not provide TASTy object identity. The 28 no-object
rows have no type-side class or TASTy resource. Five unsupported rows have a
type-side `.class` resource without TASTy. No positive classification is
inferred from a `.class` or `$` filename.

One observed standard-library case is
`scala.concurrent.duration.Duration`: the pinned `scala-library-3.9.0.jar`
contains `Duration.tasty`, `Duration.class`, and `Duration$.class`. Unpickling
`Duration.tasty` yields the term `Object` `scala.concurrent.duration.Duration`,
its distinct `ModuleClass` `scala.concurrent.duration.Duration$`, and the
ordinary class `scala.concurrent.duration.Duration`. This is the
class-with-companion case, not evidence derived from `Duration$.class` alone.

## Scala 3.9 oracle fixtures

The source fixtures in
`tools/typer-classpath-corpus-audit/module-oracle/` are compiled by the pinned
3.9.0 compiler with Temurin 21.0.8. The ignored Rust oracle test unpickles the
resulting TASTy and decodes the class files; run it with
`tools/typer-classpath-corpus-audit/module-oracle/run`.

| Fixture | TASTy identity | JVM evidence |
| --- | --- | --- |
| `object Only` | term `Object issue1046.Only`; type `ModuleClass issue1046.Only$` | `Only$.class` has `MODULE$`; no ordinary `Only` class identity |
| `class Pair` + `object Pair` | type `Class issue1046.Pair`, term `Object issue1046.Pair`, and type `ModuleClass issue1046.Pair$` | `Pair.class` and `Pair$.class`; the module class has `MODULE$` |
| `package object pkg` | package `issue1046.pkg`; object `issue1046.pkg.package`; module class `issue1046.pkg.package$` | `pkg/package.class` and `pkg/package$.class`; module class has `MODULE$` |
| `private[issue1046] object Qualified` | object and module-class identities with `PrivateWithin` visibility | `Qualified$.class` has `MODULE$` |
| Java `class Dollar$` | no TASTy file and no Scala object definition | `Dollar$.class` exists and has no `MODULE$` field |
| `scala.concurrent.duration.Duration` | class, object, and module class decoded from the pinned library TASTy | companion class resource exists in the pinned library JAR |

The package-object fixture demonstrates that its source name `pkg` does not
produce an object named `pkg`: Scala 3.9 records the module value as `package`
inside package `pkg`. The literal-dollar Java fixture demonstrates why a
binary filename ending in `$` is not sufficient evidence.

Focused regression coverage also rejects malformed TASTy and duplicate
module-evidence keys, checks the module profile's frozen counts and stable
sorting, and verifies that a companion `$` class resource with no semantic
object evidence remains unclassified as an object.

## Canonical identity and names

The Scala 3.9 compiler source describes a module as a module value `X` whose
type refers to module class `X$`. Its `moduleClass` and `sourceModule`
navigation are derived from those symbol infos. TASTy likewise contains a
term definition for the object and a `TYPEDEF` for its module class. The
unpickler maps these independently to `SymbolKind::Object` and
`SymbolKind::ModuleClass`; it derives the module class from the owning scope
and the exact `Foo$` type name. `SymbolLinks::companion` remains for a proven
ordinary class/trait and object pair; the module class is not that link's
endpoint.

The bounded next implementation contract is therefore:

- Enter `Foo` as a package-owned term `Object` only when TASTy identifies the
  exact top-level object definition.
- Enter its distinct package-owned type `Foo$` as `ModuleClass` from the
  matching TASTy module-class definition. Keep ordinary class `Foo` as a
  separate type identity when TASTy also defines it.
- Give the module value a stable reference whose semantic type selects the
  module class. Derive object-to-module-class navigation from the owner's
  declaration scope and exact `$` type name.
- Publish a reciprocal `SymbolLinks::companion` between `Class`/`Trait Foo`
  and `Object Foo` only when both are uniquely established in the same owner
  scope. `Foo$` stays a separate type symbol.
- Preserve the owner and TASTy visibility, including qualified private
  visibility. Do not interpret a raw `Foo$.class` resource as the object
  definition.

Scala's package object is a different shape: `package` is an object owned by
the package symbol `pkg`, and its members are exposed through package-object
rules. The Scala 3.9 definitions also distinguish the `_root_` `RootPackage`
from the `scala` package and the required `scala.package` object. Future
resolution must keep those identities separate; this first object slice does
not alias `_root_`, `scala`, or `scala.package`.

## Loader boundary and next implementation slice

`tasty_symbol::decode_identity` currently finds a `TYPEDEF` with the requested
simple name, validates its template, and returns class flags and visibility.
It does not expose whether that TASTy definition is an ordinary class or an
object's module class. A lookup for `Foo$` usually finds only the module
classfile, while the authoritative TASTy object and module-class definitions
are in `Foo.tasty`. A bounded module-identity decoder must inspect those exact
TASTy definition tags and owner relationships before creating either
canonical symbol. A Java class whose binary name contains `$` remains a class
unless its Scala TASTy proves the module identity.

The next issue can reuse #1019's canonical identity shell and transaction
behavior, then #1021's in-place completion for the module-class symbol. It
should keep the module value and module class as separate identities and
complete only when an object member is actually requested. It does not need a
new global completer engine. Keep nested objects, package objects, ambiguous
TASTy ownership, malformed metadata, and general Java static members deferred
until each has an explicit identity contract.

The raw module profile and both TSV artifacts are sorted by package, requested
name, source path, and attempt text. They contain no `SymbolId`, arena
allocation order, or hash-map iteration order. Two pinned corpus runs and the
derived TSVs matched byte-for-byte.
