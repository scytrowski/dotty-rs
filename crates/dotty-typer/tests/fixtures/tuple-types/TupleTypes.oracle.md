# Tuple type syntax oracle

These cases pin Scala 3.9.0's interpretation of `(A, B)` as the canonical
`scala.Tuple2[A, B]` type. The positive case compiles. The negative case swaps
the tuple element types and must fail with both mismatches reported.

The compiler sources are pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`; compile with the cached Scala 3.9.0
artifacts and JDK 21:

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/tuple-types-positive \
  crates/dotty-typer/tests/fixtures/tuple-types/TupleTypes.scala

"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/tuple-types-negative \
  crates/dotty-typer/tests/fixtures/tuple-types/TupleTypes.negative.scala
```

The second command is expected to fail. Scala reports that the first actual
element is `Int` where `String` is required, and the second is `String` where
`Int` is required.
