# External object wildcard import oracle

These fixtures use the pinned Scala 3.9.0 compiler at revision
`777528f19a58e794c9954a42f433373472ec57f8`. The positive case demonstrates
that `import scala.jdk.CollectionConverters.*` makes the inherited implicit
conversion available for `java.util.Iterator`. The negative case verifies that
the import does not invent an `asScala` conversion for an unrelated `String`.
This oracle validates source semantics; the Rust regression separately checks
that lookup returns the canonical `IteratorHasAsScala` declaration owned by
`AsScalaExtensions`. Applying implicit conversions remains a distinct Typer
capability.

Compile the positive case and confirm the negative case fails:

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/inherited-object-wildcard-positive \
  crates/dotty-typer/tests/fixtures/inherited-object-wildcard/InheritedObjectWildcard.scala

"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/inherited-object-wildcard-negative \
  crates/dotty-typer/tests/fixtures/inherited-object-wildcard/InheritedObjectWildcard.negative.scala
```
