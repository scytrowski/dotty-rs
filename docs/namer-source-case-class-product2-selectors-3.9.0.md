# Generated two-field case-class selector identities (Scala 3.9.0)

For the frozen top-level monomorphic source shape
`case class Flagged(value: Int, enabled: Boolean)`, Namer publishes two
synthetic public Method identities on the `Flagged` class. `_1` derives from
the original `value` constructor `ValDef`; `_2` derives from the original
`enabled` `ValDef`. The existing `(owner, source, tree)` derived-symbol keys
keep the selectors distinct from each other, the canonical class-owned Field
symbols, and the constructor/companion parameter copies. No `_3` is published.

The existing one-field `Box(value: Int)` selector remains. Selector identities
are published even when an explicit companion `unapply` replaces the generated
extractor. A class member already named for a selector prevents a duplicate
synthetic entry for that name. This increment publishes identities only: it
does not complete selector signatures, add Product ancestry, or change
extractor typing. The compact identity summary is in
`namer-source-case-class-product2-selectors-3.9.0.tsv`.
