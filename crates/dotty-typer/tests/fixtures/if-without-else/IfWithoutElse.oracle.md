# `if` Without `else` Oracle

This fixture is checked against Scala 3.9.0 at pinned revision
`777528f19a58e794c9954a42f433373472ec57f8`.

The compiler accepts the file. `inferred` has result type `Unit`; Scala adapts
the non-Unit `String` expression in its then branch to `Unit` and emits the
discarded-value warning. `statement` also compiles as `Unit`. An explicit
`else ()` remains an ordinary two-branch conditional and is not treated as a
parser-synthesized missing branch.

Reproduce with the pinned runtime jars and JDK 21:

```sh
mkdir -p target/if-without-else-oracle
java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Vprint:typer \
  -d target/if-without-else-oracle \
  crates/dotty-typer/tests/fixtures/if-without-else/IfWithoutElse.scala
```
