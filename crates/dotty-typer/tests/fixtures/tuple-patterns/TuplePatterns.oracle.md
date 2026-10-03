# Tuple pattern oracle

The Scala 3.9.0 fixture pins tuple-pattern lowering, nested tuple patterns,
transparent parentheses, and the empty tuple/unit pattern. In the typed tree,
`(first, second)` is lowered through the canonical `Tuple2.unapply`; nested
components recursively use their own tuple extractors. `(single)` remains a
single variable pattern, while `()` is a Unit literal pattern rather than an
empty tuple extractor.

The normalized compiler output is recorded in
`TuplePatterns.scala39-typed-tree.txt` and is pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`.
