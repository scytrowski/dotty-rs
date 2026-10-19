# Import-root artifact fixtures for #1336

The frozen cohort is unchanged from #1180: 1,063 observations in 81 files,
31 target keys and 393 distinct blocker origins. All 393 origins are
inherited; none is direct. The row-by-row resource scan is in
[`typer-classpath-import-roots-1336-3.9.0.tsv`](typer-classpath-import-roots-1336-3.9.0.tsv).
The deduplicated candidate list the source declaration ownership table is in
[`typer-classpath-import-roots-1336-target-sources.tsv`](typer-classpath-import-roots-1336-target-sources.tsv),
and the real resolver results are in
[`typer-classpath-import-roots-1336-target-candidates.tsv`](typer-classpath-import-roots-1336-target-candidates.tsv)
and
[`typer-classpath-import-roots-1336-resolver-results.tsv`](typer-classpath-import-roots-1336-resolver-results.tsv).
It finds a class or TASTy resource along one of the recorded lexical candidate
paths for 27 keys (1,055 observations). The exact-name scan has no `.class` or
`.tasty` resource for `collection`, `core`, `num`, and `quotes`; follow-up
probes classify `collection` as a real Scala package path backed by the source
package object, `core` as
an existing source import alias, and `num`/`quotes` as source-local terms. The
historical lexical package list did not include default imports; the Scala
3.9 oracle and pinned JDK 21.0.8 classpath confirm `Character` as
`java.lang.Character`.

## Confirmed compiler object roots

The pinned Scala 3.9.0 compiler oracle successfully compiled direct imports
used as terms for these five real compiler artifact objects:

| object | Scala package | source TASTy fixture | Maven compiler resources |
|---|---|---|---|
| `Types` | `dotty.tools.dotc.core` | `crates/dotty-tasty/tests/fixtures/scala3-compiler/dotty/tools/dotc/core/Types.tasty` | `dotty/tools/dotc/core/Types.tasty`, `Types.class`, `Types$.class` |
| `Contexts` | `dotty.tools.dotc.core` | `crates/dotty-tasty/tests/fixtures/scala3-compiler/dotty/tools/dotc/core/Contexts.tasty` | `dotty/tools/dotc/core/Contexts.tasty`, `Contexts.class`, `Contexts$.class` |
| `Symbols` | `dotty.tools.dotc.core` | `crates/dotty-tasty/tests/fixtures/scala3-compiler/dotty/tools/dotc/core/Symbols.tasty` | `dotty/tools/dotc/core/Symbols.tasty`, `Symbols.class`, `Symbols$.class` |
| `Tokens` | `dotty.tools.dotc.parsing` | `crates/dotty-tasty/tests/fixtures/scala3-compiler/dotty/tools/dotc/parsing/Tokens.tasty` | `dotty/tools/dotc/parsing/Tokens.tasty`, `Tokens.class`, `Tokens$.class` |
| `ProtoTypes` | `dotty.tools.dotc.typer` | `crates/dotty-tasty/tests/fixtures/scala3-compiler/dotty/tools/dotc/typer/ProtoTypes.tasty` | `dotty/tools/dotc/typer/ProtoTypes.tasty`, `ProtoTypes.class`, `ProtoTypes$.class` |

The fixture manifest pins these compiler TASTy files to Scala 3.9.0 source
revision `777528f19a58e794c9954a42f433373472ec57f8`, artifact
`scala3-compiler_3-3.9.0-bin-SNAPSHOT-nonbootstrapped.jar`, and format
28.8.0. The ordered runtime archives used for the oracle are recorded by
SHA-256 in the resource report; their compiler/library TASTy entries parse as
28.9.0. The Scala 3.9.0 oracle accepted each object import as a term. That is
semantic object evidence from the pinned compiler, while the class/TASTy
filenames by themselves remain only resource evidence.

The same oracle accepted unqualified `Character` in a type position through
Scala's default `java.lang` import. JDK 21.0.8's `java.base.jmod` provides
`java/lang/Character.class`; the resolver returns a Type symbol for it and no
package term. `Character` is therefore a built-in class/type candidate, not a
source-only or absent-classpath name.

The existing source fixture corpora also provide a version control pair:
`crates/dotty-tasty/tests/fixtures/scala3-compiler/manifest.toml` pins
compiler fixtures to 28.8.0, and
`crates/dotty-tasty/tests/fixtures/scala3-library/manifest.toml` pins the
library fixtures to 28.9.0. This matches the repository's compatibility rule:
28.8.0 is backward-compatible with the Scala 3.9.0 28.9.0 format, but the
classloader's current strict `parse_scala_3_9` entry point only accepts 28.9.0.
No decoder behavior is changed here.

## Oracle controls and remaining evidence boundary

The updated `scala39_import_oracle.py` includes the five direct object imports
above alongside its existing positive and negative controls. The run passed
with Scala 3.9.0. Existing negative controls keep a missing package root
separate from a missing selected member, and keep class-as-term failure
separate from a successful object term.

The ignored, serial Rust test
`resolver::tests::issue_1336_real_compiler_roots_keep_package_term_and_type_identities_distinct`
was run against the same Scala 3.9.0 artifacts and Temurin 21.0.8 JMODs. It
executes all 118 deduplicated lexical package candidates from the frozen rows
with both `Namespace::Term` and `Namespace::Type` `MemberRequest`s:

| namespace | requests | Object | Package | Class | Trait | `Ok(None)` | resolver error |
|---|---:|---:|---:|---:|---:|---:|---:|
| Term | 118 | 25 | 1 | 0 | 0 | 92 | 0 |
| Type | 118 | 0 | 0 | 3 | 1 | 91 | 23 |

Every candidate package was present. The separate five primary term requests
all succeeded; the resolver profile reports 121 package-term requests total
(118 candidate rows plus these five controls), 30 Object successes, 0 errors
and 25 new object identities. The additional Term result is the `scala.collection`
Package symbol; its package path also exists, but it is not counted as an
Object. The five selected real member probes were
`Types.TypeRef`, `Contexts.FreshContext`, `Symbols.NoSymbol`, `Tokens.maxToken`,
and `ProtoTypes.NoViewsAllowed`: two completed module classes and selected
the requested member; three failed during module completion. The exact error
strings and per-candidate request outcomes are preserved in the resolver TSV.

The 23 Type namespace errors are not evidence of TASTy version incompatibility:
the inputs are valid 28.9.0 resources. Eighteen report that the current
semantic projection found no matching `TypeDef`; three member completion
errors report unresolved compiler supertypes (`TypeUtils`, `SymUtils`, and
`TokensCommon`); the remaining errors identify circular inheritance or a
missing `scala/Serializable` dependency. These are current resolver/unpickler
outcomes, not claims that the pinned compiler artifact is malformed.

The resource and resolver matrices cover all 31 target keys. They still do
not claim an authoritative TASTy `ModuleClass` decode for every resource
present: non-oracle resource hits and `Ok(None)` outcomes remain explicitly
unconfirmed. No object identity is inferred from filenames. The selected
member probes establish a reachable slice for `Contexts` and `ProtoTypes`,
while `Types`, `Symbols`, and `Tokens` expose concrete module completion
blockers for the next implementation issue.
