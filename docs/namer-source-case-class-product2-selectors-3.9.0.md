# Generated two-field case-class selector identities (Scala 3.9.0)

This document records the initial `Flagged` selector increment. Issue #2066
extends publication to the bounded structural two-field case-class shape;
see [`namer-structural-product2-3.9.0.md`](namer-structural-product2-3.9.0.md).
Namer publishes two synthetic public Method identities on the class. `_1` and
`_2` derive from the distinct original constructor parameter `ValDef`s in
source order. The existing `(owner, source, tree)` derived-symbol keys keep
the selectors distinct from each other, the canonical class-owned Field
symbols, and constructor/companion parameter copies. No `_3` is published.

The existing one-field `Box(value: Int)` selector remains. Selector identities
are published even when an explicit companion `unapply` replaces the generated
extractor. A class member already named for a selector prevents a duplicate
synthetic entry for that name. This increment publishes identities only: it
does not complete selector signatures, add Product ancestry, or change
extractor typing. The compact identity summary is in
`namer-source-case-class-product2-selectors-3.9.0.tsv`.

## Generic Product1 identity

For a source case class with exactly two unbounded first-order type parameters
and one ordinary immutable constructor field directly typed as either
parameter, Namer also publishes one class-owned synthetic public `_1` method
identity. Its derived source binding points to the original constructor field
tree, while its `SymbolInfo` remains missing. This includes renamed
isomorphic declarations; eligibility does not depend on class names. Other
generic shapes remain unsupported, and this identity does not provide result
typing, receiver substitution, or Product ancestry.

The pinned Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8` declares
`Left[+A, +B](value: A)` and `Right[+A, +B](value: B)` in
`library/src/scala/util/Either.scala` (lines 555 and 577). Those declarations
are the source evidence for the two supported parameter ordinals; Rust Namer
records the selector identity and exact field provenance without claiming
Scala Typer completion.
