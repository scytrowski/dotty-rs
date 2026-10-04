# Wildcard type oracle

`WildcardTypes.scala` is compiled with the pinned Scala 3.9.0 compiler. The
recorded typer tree in `WildcardTypes.scala39-typed-tree.txt` keeps wildcard
arguments as `?` with their lower and upper bounds. In particular, the
unbounded wildcard is shown with canonical `Nothing` and `Any` bounds, while
explicit one-sided and two-sided bounds retain their source meaning. Nested
applied and qualified bound types are included as well.

The fixture is pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The source Typer regression checks
that these argument positions project to `Type::Wildcard { bounds }`, keeping
wildcard arguments distinct from ordinary `Type::Bounds` declarations.
