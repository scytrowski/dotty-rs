# Typer lexical package candidate contract

This document records the source-context candidate model introduced for
qualified import work. The model is observational only: import resolution does
not consume these candidates yet, so #1168 does not change which imports type.

## Candidate order and identity

For an unrooted qualifier, candidates come from package owners actually
represented in the `SourceContext` parent chain. The nearest package context is
`CurrentPackage`; later package contexts are `EnclosingPackage`; the canonical
empty-path package is appended as `Root`. A package identity already present
in the list is emitted once, preserving its nearest priority. A method, class,
or object owner is not used to invent additional lexical package scopes.

A single dotted package clause such as `package a.b` contributes only its
recorded leaf context. Stacked clauses such as `package a; package b`
contribute both recorded package contexts. The candidate keeps its shared
package symbol, registry scope, source context ID, and canonical owner-chain
path. Enumeration is read-only and does not resolve an imported name or
materialize classpath symbols.

For a `_root_`-qualified qualifier, enumeration returns only the canonical
root package. `_root_` is a mode of the lookup, not a package segment. The
root/default package remains the existing empty-path package identity. The
supplemental audit keeps root-package registry presence distinct from whether
the requested first segment exists under root.

Candidate construction validates source-context traversal bounds and cycles,
package-owner cycles and kinds, every owner-chain prefix against `Packages`,
and the package scope owner. Inconsistent data returns a typed typer error.
Package contexts do not require `SourceSemanticIndex::definition_of`: package
symbols are shared and have no authoritative reverse source definition.

## Scala 3.9 reference behavior

The import oracle was run against Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0, and JDK 21. It
confirms:

- the current package-relative route wins when it resolves the qualifier;
- root fallback succeeds when no higher-priority package or term candidate
  resolves the qualifier;
- stacked package clauses expose their actual enclosing package context;
- a dotted package clause does not create a lexical context for each textual
  parent segment;
- an ordinary lexical object binding keeps its normal priority, and a missing
  member on that selected owner does not trigger a root retry;
- `_root_` bypasses relative lookup;
- a missing selected member remains an error after its package is resolved.

The expanded oracle run for this issue reported:

```text
scala=3.9.0 positive_imports=class,object,object_member,rename,_root_,root_package_from_nested_context,current_package_relative,stacked_package_parent_scope,descendant_only_intermediate negative_imports=absent_root,missing_selected_member,source_relative_wins_collision,dotted_package_parent_scope_not_searched,object_package_name_collision,lexical_term_missing_member_no_root_fallback;root_fallback=success root_qualification=bypasses_relative_search;lexical_term_binding=normal_priority
```

## Causal classpath probe

`dotty_core_import_resolves_to_its_package_before_a_later_lookup_blocker` in
`crates/dotty-typer/tests/package_prefix_parity.rs` runs a small Namer-to-
SourceTyper case from `package dotty.tools.dotc` with `import core.Contexts`.
Its traced classpath resolver records the request for
`dotty.tools.dotc.core`; the imported `Contexts` type is then asserted to be a
class whose owner is the exact package `SymbolId` returned for that request.
Typing a reference to a nonexistent member on that imported value reaches the
later `MemberNotFound` diagnostic, which distinguishes the resolved qualifier
and imported class identity from the original first-segment failure.

The classpath fixture supplies a minimal class-file stub at the real package
and class path. The checked-in Scala 3.9.0 `Contexts.tasty` fixture is encoded
as TASTy 28.8.0, while the classloader currently accepts 28.9.0, so it cannot
serve as a loadable fixture here. This probe establishes the package and symbol
identity chain for the dominant `core` prefix shape; it does not claim full
typing coverage of the Scala compiler's `Contexts` implementation. It is a
focused regression test and does not rerun the corpus audit.

## Preserved source evidence

The frozen #1128 profile remains 2,394 weighted observations. Its historical
classifier identifies 1,615 qualified lexical package candidates across the
five named families, based on its own request population and direct-resource
criterion. The #1132 reprofile is a separate, later population and reports
1,618 source-package registry hits across those families. These figures have
different definitions and denominators; neither is substituted for the
other.

[`typer-classpath-import-prefix-profile-1132-3.9.0-lexical-package-candidates.tsv`](typer-classpath-import-prefix-profile-1132-3.9.0-lexical-package-candidates.tsv)
contains stable representative rows for `core`, `ast`, `transform`, `util`,
and `io`, labeled `frozen-1128` or `current-1132`. Each row retains the source
file, import tree, current and enclosing context owners, candidate precedence,
source package-registry status, and direct/descendant classpath evidence at
the requested next segment. Collision, ambiguity, and unsupported markers
report only what the snapshot records; `not_observed` does not assert that no
such case exists outside these rows. The original frozen observation,
reconciliation, target, and classpath reports remain intact.
