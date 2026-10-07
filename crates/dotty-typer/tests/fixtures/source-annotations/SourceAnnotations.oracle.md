# Scala 3.9.0 source annotation oracle

`SourceAnnotations.scala` compiles with the pinned Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and JDK 21.
`SourceAnnotations.typed-tree.txt` records `-Vprint:typer` output.

The bare `@unchecked` and explicit `@EmptyAnnot()` forms both carry no term
arguments in the typed tree. Positional and named arguments retain their
source order and names. Scala 3.9.0 also accepts a positional argument after a
named argument when it follows the parameter order, as in `@Multi(first = 1,
2)`. The pinned compiler also accepts reordered named arguments before a
positional argument, as in `@Triple(second = 2, first = 1, 3)`, and rewrites
the named arguments into parameter order in its typed tree. The parser's shared untyped AST represents each
annotation as `Apply(Select(New(type), <init>), arguments)`, including a
synthetic empty `Apply` for the bare form.
