# Bounded `Int.&` oracle

Compiled with the pinned Scala 3.9.0 compiler at revision
`777528f19a58e794c9954a42f433373472ec57f8` and JDK 21.0.8:

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/int-bitwise-and-oracle \
  -Vprint:typer crates/dotty-typer/tests/fixtures/int-bitwise-and/IntBitwiseAnd.scala
```

The positive fixture accepts both `Int & Int` and the real `flags & 1`
expression. The former has type `Int`; `(flags & 1) != 0` has type `Boolean`.
The pinned Scala library declares `Int.&(x: Int): Int` in
`library/src/scala/Int.scala`. This bounded typer increment supports only that
canonical `Int` parameter shape; it does not claim the adjacent Byte, Short,
Char, Long, OR, XOR, or shift overloads.

The negative fixture `IntBitwiseAndIncompatible.scala` is rejected because
`IncompatibleReference` has no `&` member. The typer regression keeps that
case unresolved rather than accepting an unrelated same-spelled operator.
