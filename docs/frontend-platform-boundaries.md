# Frontend and platform boundaries

The Scala source frontend owns target-independent source meaning. JVM is one
platform adapter; its class files, JMODs, JDK layout, descriptors and bytecode
must remain outside frontend production dependencies and APIs. A JS or Native
backend may be added later without changing this rule. This note records the
current workspace graph and guards its production dependency direction; it
does not propose another backend or a semantic-model rewrite.

## Current workspace graph

Edges below are direct Cargo dependencies. “Production/build” includes normal
and build dependencies; “dev” edges are used for tests and tooling only. The
automated check follows production/build edges transitively from the five
frontend crates.

| Crate | Production/build dependencies | Dev dependencies |
| --- | --- | --- |
| `dotty-core` | — | — |
| `dotty-lexer` | `dotty-core`, `unicode-ident` | — |
| `dotty-parser` | `dotty-core` | — |
| `dotty-namer` | `dotty-core` | `dotty-parser`, `dotty-lexer` |
| `dotty-typer` | `dotty-core` | `dotty-classfile`, `dotty-classloader`, `dotty-namer`, `dotty-lexer`, `dotty-parser`, `dotty-tasty`, `dotty-tasty-unpickler` |
| `dotty-tasty` | — | `serde`, `serde_json` |
| `dotty-classfile` | — | — |
| `dotty-tasty-unpickler` | `dotty-tasty`, `dotty-core` | — |
| `dotty-classloader` | `dotty-tasty`, `dotty-classfile`, `dotty-core` | `dotty-tasty-unpickler` |
| `dotty-jvm` | `dotty-core`, `dotty-classfile` | — |
| `dotty` (root facade) | `dotty-tasty`, `dotty-classfile`, `dotty-core`, `dotty-classloader`, `dotty-tasty-unpickler`, `dotty-parser`, `dotty-namer`, `dotty-typer` | `dotty-lexer`, `dotty-jvm` |
| `dotty-parser-smoke-dump` | `dotty-core`, `dotty-lexer`, `dotty-parser` | — |
| `dotty-parser-corpus-report` | `dotty-core`, `dotty-lexer`, `dotty-parser`, `dotty-namer`, `serde`, `serde_json` | — |

The permitted direction is source inputs through `dotty-lexer` and
`dotty-parser` into shared `dotty-core` contracts; `dotty-namer` and
`dotty-typer` consume those shared contracts. Platform and binary adapters
depend inward on `dotty-core`: `dotty-classloader` combines class-file and
TASTy data, while `dotty-jvm` lowers already-typed semantics to
`dotty-classfile`. The reverse direction is prohibited: `dotty-core`, lexer,
parser, namer, and typer production/build closures may not depend on
`dotty-classfile`, `dotty-classloader`, or `dotty-jvm`. Dev dependencies do not
count as production edges; for example, `dotty-typer` uses classloader fixtures
in tests while its only production dependency is `dotty-core`.

`tools/check-frontend-boundaries.py` obtains Cargo's resolved dependency graph
with `cargo metadata --locked --all-features`, then walks only normal/build
transitive edges from `dotty-core`, `dotty-lexer`, `dotty-parser`,
`dotty-namer`, and `dotty-typer`. Enabling all workspace features ensures an
optional production dependency cannot hide behind an inactive feature. Its
built-in self-test accepts a dev-only classloader edge, rejects a transitive
build-only classfile edge, and asks Cargo to resolve a fixture whose optional
classfile dependency is visible only with `--all-features`. CI runs this same
script once; there is no second dependency parser or network service.

## Semantic boundary and target-specific facts

`SymbolResolver` in `dotty-core` is the semantic port for references not
already entered in the session. Its requests use `TypeId`, names, namespaces,
and semantic member selectors; they contain no class-file indices, JMOD paths,
descriptors, or wire tags. `SemanticStore` owns canonical symbols, types,
scopes, origins, and source semantic state. Resolver adapters may materialize
canonical entries in that store and participate in its checkpoint/rollback
contract, while the typer can use `NoResolver` or a caller-supplied resolver.
The source AST and production typer API therefore do not own classpath IO or
binary decoding.

Scala rules such as `Int` arithmetic, subtyping and declaration meaning are
target-independent. Which standard-library declarations can be resolved is
target-dependent (a Scala library on a classpath is different from a JDK
module path), as are runtime representation, ABI names and instruction
selection. Those facts belong in adapters/backends. For example,
`dotty-jvm` consumes a typed tree and `dotty-classfile` builder; source
typing does not return JVM descriptors or bytecode.

## `Object`, `AnyRef`, and deferred risk

`Definitions::bootstrap` in `dotty-core/src/definitions.rs` creates one
synthetic builtin symbol named `Object` and a canonical `object_type`, alongside
distinct `Any` and `Nothing` symbols. It also retains Scala 3.9's canonical
`AnyRef.eq/ne` source identities in a dedicated semantic scope. That scope is
not the ordinary `object_class` declaration scope: the current identity still
represents both Scala's source reference bound and JVM `java.lang.Object`,
while `eq/ne` are compiler semantic intrinsics, not Java Object methods. The
Typer uses these IDs only for its bounded current-unit class subset. Evidence
of the remaining coupling is localized: typer member lookup and subtyping
special-case `definitions.object_class`; source annotation traversal also
recognizes it. The core manifest itself has no dependency, and the JVM backend
separately maps its supported generated-class parent to the JVM name
`java/lang/Object`.

This is a semantic modeling risk, not a dependency leak today. Scala reference
semantics and a target runtime's root object need not be identical in every
future platform/library profile. The dedicated operator scope avoids adding
Scala-only methods to ordinary JVM Object metadata, while the underlying
AnyRef/Object type identity remains shared. Revisit the distinction if a
non-JVM standard-library profile needs a separate reference root; do not infer
runtime methods from the compiler's source identities.

## No-JDK frontend regression

`crates/dotty-typer/tests/typed_expressions.rs` exercises parsing, naming and
typing a small source declaration with `NoResolver`. It supplies no classpath,
JAR, JMOD or JDK fixture. The typer's production manifest depends only on
`dotty-core`; classloader dependencies in that manifest are dev-only and are
excluded from the automated production-edge check.
