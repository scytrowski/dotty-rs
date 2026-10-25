# Universal equality oracle

Compiled with the pinned Scala 3.9.0 compiler at revision
`777528f19a58e794c9954a42f433373472ec57f8` and JDK 21.0.8:

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/universal-equality-oracle \
  -Vprint:typer crates/dotty-typer/tests/fixtures/universal-equality/UniversalEquality.scala
```

The positive fixture compiles for primitive-to-literal equality, `Any`,
reference inequality, custom references, and Boolean equality. The pinned
compiler rejects `Int == String` in
`UniversalEqualityIncompatible.scala` with:

```text
Values of types Int and String cannot be compared with == or !=
```

Scala 3.9 defines universal `Any.==` and `Any.!=` as final compiler-synthesized
methods on `Any`; the pinned source is
`target/dotty-scala39-1525/compiler/src/dotty/tools/dotc/core/Definitions.scala`
(lines 289 and 297–300 in that checkout).
