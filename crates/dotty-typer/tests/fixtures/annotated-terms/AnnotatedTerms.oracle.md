# Scala 3.9.0 annotated term oracle

`AnnotatedTerms.scala` compiles with the pinned Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and JDK 21.
`AnnotatedTerms.typed-tree.txt` records `-Vprint:typer` output.

Expression annotations lower to ordinary typed ascriptions whose type is
annotated. The literal and parameter examples have underlying type `Int`,
while the stable alias retains its declared singleton type `StableTerm.type`.
The nested example retains both annotations in source order.
